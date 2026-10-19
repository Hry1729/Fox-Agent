//! The `agent_backend` migration and immutability trigger against a real
//! SQLite (plan 6.6, Host wiring first batch). The schema below is a minimal
//! stand-in for the desktop `conversations` table (only the columns the
//! migration and the old-row checks need); the SQL under test is the exact
//! constant the desktop migration runner applies as version 95.
//!
//! Opt-in: `cargo test -p fox-agent-backend --features sqlite-tests`.
#![cfg(feature = "sqlite-tests")]

use fox_agent_backend::host_routing::{
    route_stored, BackendSwitch, HostEntry, Route, AGENT_BACKEND_MIGRATION_SQL,
};
use fox_agent_backend::port::AgentBackend;
use rusqlite::Connection;

fn old_database() -> Connection {
    let connection = Connection::open_in_memory().expect("open");
    connection
        .execute_batch(
            "CREATE TABLE conversations (
                 id TEXT PRIMARY KEY,
                 agent_id TEXT NOT NULL,
                 title TEXT NOT NULL,
                 status TEXT NOT NULL,
                 created_at INTEGER NOT NULL,
                 updated_at INTEGER NOT NULL,
                 conversation_kind TEXT NOT NULL DEFAULT 'primary'
             );
             CREATE TABLE kernel_runs (
                 run_id TEXT PRIMARY KEY,
                 engine_id TEXT NOT NULL,
                 kernel_mode TEXT NOT NULL
             );
             INSERT INTO conversations(id,agent_id,title,status,created_at,updated_at)
                 VALUES ('old-pi','a','Old pi','active',1,1),
                        ('old-codex','a','Old codex','active',1,1),
                        ('old-deepseek','a','Old deepseek','active',1,1),
                        ('old-legacy','a','Old legacy','active',1,1);
             INSERT INTO kernel_runs VALUES ('r-pi','pi','authoritative'),
                                            ('r-codex','codex','authoritative'),
                                            ('r-ds','deepseek_harness','authoritative'),
                                            ('r-legacy','pi','legacy');",
        )
        .expect("old schema");
    connection
}

fn migrate(connection: &Connection) {
    connection.execute_batch(AGENT_BACKEND_MIGRATION_SQL).expect("migration");
}

fn stored(connection: &Connection, id: &str) -> String {
    connection
        .query_row("SELECT agent_backend FROM conversations WHERE id=?1", [id], |row| row.get(0))
        .expect("read agent_backend")
}

#[test]
fn every_old_conversation_maps_to_fox_and_old_columns_are_untouched() {
    let connection = old_database();
    migrate(&connection);
    for id in ["old-pi", "old-codex", "old-deepseek", "old-legacy"] {
        assert_eq!(stored(&connection, id), "fox");
        let (bound, route) =
            route_stored(HostEntry::Resume, Some(&stored(&connection, id)), None, &BackendSwitch::all_off())
                .expect("old conversations stay routable through Fox");
        assert_eq!((bound, route), (AgentBackend::Fox, Route::Fox));
    }
    // engine_id and the legacy marker keep their historical values.
    let engines: Vec<(String, String)> = connection
        .prepare("SELECT engine_id, kernel_mode FROM kernel_runs ORDER BY run_id")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        engines,
        vec![
            ("codex".into(), "authoritative".into()),
            ("deepseek_harness".into(), "authoritative".into()),
            ("pi".into(), "legacy".into()),
            ("pi".into(), "authoritative".into()),
        ]
    );
    let titles: i64 = connection
        .query_row("SELECT COUNT(*) FROM conversations WHERE title LIKE 'Old %' AND updated_at=1", [], |row| row.get(0))
        .unwrap();
    assert_eq!(titles, 4, "no historical row is rewritten");
}

#[test]
fn new_rows_write_the_backend_and_it_is_immutable() {
    let connection = old_database();
    migrate(&connection);
    connection
        .execute(
            "INSERT INTO conversations(id,agent_id,title,status,created_at,updated_at,agent_backend)
             VALUES ('new-pi','a','New','active',2,2,'pi')",
            [],
        )
        .expect("insert pi conversation");
    assert_eq!(stored(&connection, "new-pi"), "pi");

    for (id, target) in [("new-pi", "fox"), ("old-codex", "codex"), ("old-pi", "pi")] {
        let error = connection
            .execute("UPDATE conversations SET agent_backend=?2 WHERE id=?1", [id, target])
            .expect_err("agent_backend must not change");
        assert!(error.to_string().contains("agent_backend_immutable"), "{error}");
    }
    assert_eq!(stored(&connection, "new-pi"), "pi");
    assert_eq!(stored(&connection, "old-codex"), "fox");

    // Unrelated updates and same-value writes still work.
    connection.execute("UPDATE conversations SET title='Renamed' WHERE id='new-pi'", []).expect("rename");
    connection
        .execute("UPDATE conversations SET agent_backend='pi', updated_at=3 WHERE id='new-pi'", [])
        .expect("same value is not a change");
}

#[test]
fn the_check_constraint_rejects_engine_ids_and_unknown_values() {
    let connection = old_database();
    migrate(&connection);
    for value in ["deepseek_harness", "claude", "", "FOX"] {
        let error = connection
            .execute(
                "INSERT INTO conversations(id,agent_id,title,status,created_at,updated_at,agent_backend)
                 VALUES ('bad','a','Bad','active',2,2,?1)",
                [value],
            )
            .expect_err("only the four backend ids are accepted");
        assert!(error.to_string().to_ascii_lowercase().contains("check"), "{error}");
    }
    let error = connection
        .execute(
            "INSERT INTO conversations(id,agent_id,title,status,created_at,updated_at,agent_backend)
             VALUES ('null','a','Null','active',2,2,NULL)",
            [],
        )
        .expect_err("NULL is rejected");
    assert!(error.to_string().to_ascii_lowercase().contains("not null"), "{error}");
}

#[test]
fn migrating_a_copy_leaves_the_original_readable_by_the_old_schema() {
    // Rehearsal shape for the engineer checklist: migrate a backup copy, keep
    // the original, and read the original with the old column list.
    let dir = std::env::temp_dir().join(format!("fox-host-routing-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let original = dir.join("fox.db");
    let copy = dir.join("fox-copy.db");
    let _ = std::fs::remove_file(&original);
    let _ = std::fs::remove_file(&copy);
    {
        let connection = Connection::open(&original).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE conversations (id TEXT PRIMARY KEY, agent_id TEXT NOT NULL, title TEXT NOT NULL,
                     status TEXT NOT NULL, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL);
                 INSERT INTO conversations VALUES ('c1','a','Old','active',1,1);",
            )
            .unwrap();
    }
    std::fs::copy(&original, &copy).unwrap();
    let migrated = Connection::open(&copy).unwrap();
    migrate(&migrated);
    assert_eq!(stored(&migrated, "c1"), "fox");
    // Old column list still reads on the migrated copy (additive only).
    let title: String = migrated
        .query_row("SELECT id, agent_id, title, status, created_at, updated_at FROM conversations WHERE id='c1'", [], |row| row.get(2))
        .unwrap();
    assert_eq!(title, "Old");
    // A second application fails loudly (the runner guards with schema_migrations).
    assert!(migrated.execute_batch(AGENT_BACKEND_MIGRATION_SQL).is_err());
    drop(migrated);
    let untouched = Connection::open(&original).unwrap();
    let has_column: bool = untouched
        .query_row("SELECT EXISTS(SELECT 1 FROM pragma_table_info('conversations') WHERE name='agent_backend')", [], |row| row.get(0))
        .unwrap();
    assert!(!has_column, "the original is not modified by migrating the copy");
    drop(untouched);
    let _ = std::fs::remove_dir_all(&dir);
}
