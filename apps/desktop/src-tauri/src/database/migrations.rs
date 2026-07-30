use rusqlite::{Connection, Result};

const MIGRATION_1: &str = r#"
CREATE TABLE IF NOT EXISTS schema_migrations (
    version INTEGER PRIMARY KEY,
    applied_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS agents (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    description TEXT NOT NULL,
    runtime_type TEXT NOT NULL,
    system_prompt TEXT NOT NULL,
    default_model TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS conversations (
    id TEXT PRIMARY KEY,
    agent_id TEXT NOT NULL REFERENCES agents(id),
    title TEXT NOT NULL,
    project_root TEXT,
    status TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    last_message_at INTEGER
);

CREATE TABLE IF NOT EXISTS messages (
    id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    run_id TEXT,
    role TEXT NOT NULL,
    kind TEXT NOT NULL,
    content TEXT NOT NULL,
    status TEXT NOT NULL,
    ordinal INTEGER NOT NULL,
    runtime_message_id TEXT,
    metadata_json TEXT NOT NULL DEFAULT '{}',
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_messages_conversation_ordinal
    ON messages(conversation_id, ordinal);

CREATE TABLE IF NOT EXISTS runs (
    id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    runtime_session_id TEXT,
    status TEXT NOT NULL,
    model TEXT NOT NULL,
    started_at INTEGER,
    finished_at INTEGER,
    error_code TEXT,
    error_message TEXT,
    last_seq INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS runtime_sessions (
    id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    runtime_type TEXT NOT NULL,
    runtime_session_id TEXT NOT NULL,
    runtime_version TEXT,
    session_path TEXT,
    status TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    UNIQUE(conversation_id, runtime_type)
);

CREATE TABLE IF NOT EXISTS run_events (
    id TEXT PRIMARY KEY,
    run_id TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    seq INTEGER NOT NULL,
    event_type TEXT NOT NULL,
    event_json TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    UNIQUE(run_id, seq)
);
"#;

const MIGRATION_2: &str = r#"
CREATE TABLE IF NOT EXISTS yuxi_service (
    singleton_id INTEGER PRIMARY KEY CHECK(singleton_id = 1),
    name TEXT NOT NULL,
    base_url TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1,
    last_status TEXT NOT NULL DEFAULT 'unknown',
    last_version TEXT,
    last_latency_ms INTEGER,
    last_checked_at INTEGER,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
"#;

const MIGRATION_3: &str = r#"
CREATE TABLE IF NOT EXISTS model_service (
    singleton_id INTEGER PRIMARY KEY CHECK(singleton_id = 1),
    name TEXT NOT NULL,
    base_url TEXT NOT NULL,
    model_id TEXT NOT NULL,
    api_type TEXT NOT NULL DEFAULT 'openai-completions',
    context_window INTEGER NOT NULL DEFAULT 128000,
    max_output_tokens INTEGER NOT NULL DEFAULT 8192,
    enabled INTEGER NOT NULL DEFAULT 1,
    last_status TEXT NOT NULL DEFAULT 'unknown',
    last_latency_ms INTEGER,
    last_checked_at INTEGER,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
"#;

const MIGRATION_4: &str = r#"
CREATE TABLE IF NOT EXISTS projects (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    root_path TEXT NOT NULL UNIQUE,
    permission_mode TEXT NOT NULL DEFAULT 'read_only'
        CHECK(permission_mode IN ('read_only', 'ask', 'allow')),
    status TEXT NOT NULL DEFAULT 'active'
        CHECK(status IN ('active', 'missing', 'revoked')),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    last_opened_at INTEGER
);

ALTER TABLE conversations ADD COLUMN project_id TEXT REFERENCES projects(id);

CREATE INDEX IF NOT EXISTS idx_conversations_project_id
    ON conversations(project_id);

INSERT OR IGNORE INTO projects(
    id, name, root_path, permission_mode, status, created_at, updated_at, last_opened_at
)
SELECT
    'legacy-' || lower(hex(randomblob(16))),
    project_root,
    project_root,
    'read_only',
    'active',
    created_at,
    updated_at,
    updated_at
FROM conversations
WHERE project_root IS NOT NULL AND trim(project_root) <> '';

UPDATE conversations
SET project_id = (
    SELECT projects.id FROM projects WHERE projects.root_path = conversations.project_root
)
WHERE project_id IS NULL
  AND project_root IS NOT NULL
  AND trim(project_root) <> '';

CREATE TABLE IF NOT EXISTS tool_calls (
    id TEXT PRIMARY KEY,
    runtime_tool_call_id TEXT NOT NULL,
    run_id TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    tool_name TEXT NOT NULL,
    input_json TEXT NOT NULL DEFAULT '{}',
    status TEXT NOT NULL,
    result_json TEXT,
    error_message TEXT,
    execution_location TEXT NOT NULL DEFAULT 'runtime',
    requires_approval INTEGER NOT NULL DEFAULT 0,
    started_at INTEGER NOT NULL,
    completed_at INTEGER,
    updated_at INTEGER NOT NULL,
    UNIQUE(run_id, runtime_tool_call_id)
);

CREATE INDEX IF NOT EXISTS idx_tool_calls_conversation_started
    ON tool_calls(conversation_id, started_at);

CREATE TABLE IF NOT EXISTS approvals (
    id TEXT PRIMARY KEY,
    tool_call_id TEXT NOT NULL UNIQUE REFERENCES tool_calls(id) ON DELETE CASCADE,
    status TEXT NOT NULL DEFAULT 'pending'
        CHECK(status IN ('pending', 'approved', 'denied', 'cancelled', 'expired')),
    requested_action TEXT NOT NULL,
    request_json TEXT NOT NULL DEFAULT '{}',
    decision_json TEXT,
    requested_at INTEGER NOT NULL,
    resolved_at INTEGER
);

CREATE INDEX IF NOT EXISTS idx_approvals_status_requested
    ON approvals(status, requested_at);

CREATE TABLE IF NOT EXISTS attachments (
    id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    message_id TEXT REFERENCES messages(id) ON DELETE SET NULL,
    display_name TEXT NOT NULL,
    storage_path TEXT NOT NULL,
    media_type TEXT,
    byte_size INTEGER NOT NULL DEFAULT 0,
    sha256 TEXT,
    status TEXT NOT NULL DEFAULT 'ready',
    created_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_attachments_conversation_created
    ON attachments(conversation_id, created_at);

CREATE TABLE IF NOT EXISTS artifacts (
    id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    run_id TEXT REFERENCES runs(id) ON DELETE SET NULL,
    display_name TEXT NOT NULL,
    artifact_type TEXT NOT NULL,
    storage_path TEXT NOT NULL,
    media_type TEXT,
    byte_size INTEGER NOT NULL DEFAULT 0,
    sha256 TEXT,
    status TEXT NOT NULL DEFAULT 'ready',
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_artifacts_conversation_created
    ON artifacts(conversation_id, created_at);

CREATE TABLE IF NOT EXISTS service_connections (
    id TEXT PRIMARY KEY,
    service_type TEXT NOT NULL,
    name TEXT NOT NULL,
    base_url TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1,
    credential_ref TEXT,
    last_status TEXT NOT NULL DEFAULT 'unknown',
    last_version TEXT,
    last_latency_ms INTEGER,
    last_checked_at INTEGER,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    UNIQUE(service_type, base_url)
);

INSERT OR IGNORE INTO service_connections(
    id, service_type, name, base_url, enabled, last_status, last_version,
    last_latency_ms, last_checked_at, created_at, updated_at
)
SELECT
    'yuxi-primary', 'yuxi', name, base_url, enabled, last_status, last_version,
    last_latency_ms, last_checked_at, created_at, updated_at
FROM yuxi_service
WHERE singleton_id = 1;

CREATE TABLE IF NOT EXISTS knowledge_bindings (
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    service_connection_id TEXT NOT NULL REFERENCES service_connections(id) ON DELETE CASCADE,
    knowledge_base_id TEXT NOT NULL,
    knowledge_base_name TEXT,
    enabled INTEGER NOT NULL DEFAULT 1,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY(conversation_id, service_connection_id, knowledge_base_id)
);

CREATE TABLE IF NOT EXISTS agent_runtime_config (
    agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    scope_type TEXT NOT NULL CHECK(scope_type IN ('agent', 'conversation')),
    scope_id TEXT NOT NULL,
    config_key TEXT NOT NULL,
    value_json TEXT NOT NULL,
    source TEXT NOT NULL DEFAULT 'user',
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY(agent_id, scope_type, scope_id, config_key)
);
"#;

const MIGRATION_5: &str = r#"
ALTER TABLE runs ADD COLUMN external_run_id TEXT;
ALTER TABLE runs ADD COLUMN external_cursor TEXT;

CREATE INDEX IF NOT EXISTS idx_runs_external_run_id
    ON runs(external_run_id);
"#;

const MIGRATION_6: &str = r#"
ALTER TABLE model_service ADD COLUMN supports_image_input INTEGER NOT NULL DEFAULT 0;
"#;

const MIGRATION_7: &str = r#"
CREATE TABLE IF NOT EXISTS mcp_servers (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    command TEXT NOT NULL,
    args_json TEXT NOT NULL DEFAULT '[]',
    enabled INTEGER NOT NULL DEFAULT 1,
    status TEXT NOT NULL DEFAULT 'unknown',
    last_error TEXT,
    last_checked_at INTEGER,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
"#;

const MIGRATION_8: &str = r#"
CREATE INDEX IF NOT EXISTS idx_runs_conversation_created
    ON runs(conversation_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_run_events_run_seq
    ON run_events(run_id, seq);
CREATE INDEX IF NOT EXISTS idx_attachments_message_id
    ON attachments(message_id);
CREATE INDEX IF NOT EXISTS idx_artifacts_run_id
    ON artifacts(run_id);
"#;

const MIGRATION_9: &str = r#"
CREATE VIRTUAL TABLE IF NOT EXISTS conversation_fts USING fts5(
    conversation_id UNINDEXED,
    title,
    project_root,
    agent_name,
    tokenize = 'unicode61'
);

CREATE VIRTUAL TABLE IF NOT EXISTS message_fts USING fts5(
    message_id UNINDEXED,
    conversation_id UNINDEXED,
    content,
    tokenize = 'unicode61'
);

INSERT INTO conversation_fts(conversation_id, title, project_root, agent_name)
SELECT c.id, c.title, COALESCE(p.root_path, c.project_root, ''), a.name
FROM conversations c
JOIN agents a ON a.id = c.agent_id
LEFT JOIN projects p ON p.id = c.project_id;

INSERT INTO message_fts(message_id, conversation_id, content)
SELECT id, conversation_id, content FROM messages;

CREATE TRIGGER IF NOT EXISTS conversation_fts_insert AFTER INSERT ON conversations BEGIN
    INSERT INTO conversation_fts(conversation_id, title, project_root, agent_name)
    SELECT new.id, new.title, COALESCE(p.root_path, new.project_root, ''), a.name
    FROM agents a LEFT JOIN projects p ON p.id = new.project_id
    WHERE a.id = new.agent_id;
END;

CREATE TRIGGER IF NOT EXISTS conversation_fts_update AFTER UPDATE ON conversations BEGIN
    DELETE FROM conversation_fts WHERE conversation_id = old.id;
    INSERT INTO conversation_fts(conversation_id, title, project_root, agent_name)
    SELECT new.id, new.title, COALESCE(p.root_path, new.project_root, ''), a.name
    FROM agents a LEFT JOIN projects p ON p.id = new.project_id
    WHERE a.id = new.agent_id;
END;

CREATE TRIGGER IF NOT EXISTS conversation_fts_delete AFTER DELETE ON conversations BEGIN
    DELETE FROM conversation_fts WHERE conversation_id = old.id;
END;

CREATE TRIGGER IF NOT EXISTS message_fts_insert AFTER INSERT ON messages BEGIN
    INSERT INTO message_fts(message_id, conversation_id, content)
    VALUES (new.id, new.conversation_id, new.content);
END;

CREATE TRIGGER IF NOT EXISTS message_fts_update AFTER UPDATE ON messages BEGIN
    DELETE FROM message_fts WHERE message_id = old.id;
    INSERT INTO message_fts(message_id, conversation_id, content)
    VALUES (new.id, new.conversation_id, new.content);
END;

CREATE TRIGGER IF NOT EXISTS message_fts_delete AFTER DELETE ON messages BEGIN
    DELETE FROM message_fts WHERE message_id = old.id;
END;
"#;

const MIGRATION_10: &str = r#"
CREATE TABLE IF NOT EXISTS model_providers (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    base_url TEXT NOT NULL UNIQUE,
    api_type TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1,
    is_default INTEGER NOT NULL DEFAULT 0,
    last_status TEXT NOT NULL DEFAULT 'unknown',
    last_latency_ms INTEGER,
    last_checked_at INTEGER,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS provider_models (
    id TEXT PRIMARY KEY,
    provider_id TEXT NOT NULL REFERENCES model_providers(id) ON DELETE CASCADE,
    model_id TEXT NOT NULL,
    display_name TEXT NOT NULL,
    context_window INTEGER NOT NULL DEFAULT 128000,
    max_output_tokens INTEGER NOT NULL DEFAULT 8192,
    supports_image_input INTEGER NOT NULL DEFAULT 0,
    is_default INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    UNIQUE(provider_id, model_id)
);

INSERT OR IGNORE INTO model_providers(
    id, name, base_url, api_type, enabled, is_default, last_status,
    last_latency_ms, last_checked_at, created_at, updated_at
)
SELECT 'legacy-primary', name, base_url, api_type, enabled, 1, last_status,
       last_latency_ms, last_checked_at, created_at, updated_at
FROM model_service WHERE singleton_id = 1;

INSERT OR IGNORE INTO provider_models(
    id, provider_id, model_id, display_name, context_window, max_output_tokens,
    supports_image_input, is_default, created_at, updated_at
)
SELECT 'legacy-primary-model', 'legacy-primary', model_id, model_id, context_window,
       max_output_tokens, supports_image_input, 1, created_at, updated_at
FROM model_service WHERE singleton_id = 1;

CREATE INDEX IF NOT EXISTS idx_provider_models_provider
    ON provider_models(provider_id);
"#;

const MIGRATION_11: &str = r#"
CREATE TABLE IF NOT EXISTS knowledge_preview_cache (
    cache_key TEXT PRIMARY KEY,
    knowledge_base_id TEXT NOT NULL,
    document_id TEXT NOT NULL,
    source_revision TEXT NOT NULL,
    variant TEXT NOT NULL,
    storage_path TEXT NOT NULL UNIQUE,
    media_type TEXT,
    byte_size INTEGER NOT NULL DEFAULT 0,
    lease_count INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    last_accessed_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_knowledge_preview_cache_lru
    ON knowledge_preview_cache(lease_count, last_accessed_at);
"#;

const MIGRATION_12: &str = r#"
CREATE TABLE IF NOT EXISTS app_settings (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL,
    updated_at INTEGER NOT NULL
);
"#;

const MIGRATION_13: &str = r#"
CREATE TABLE IF NOT EXISTS knowledge_document_reading_state (
    knowledge_base_id TEXT NOT NULL,
    document_id TEXT NOT NULL,
    source_revision TEXT,
    page INTEGER NOT NULL DEFAULT 1 CHECK(page >= 1),
    scroll_offset REAL NOT NULL DEFAULT 0 CHECK(scroll_offset >= 0),
    zoom REAL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY(knowledge_base_id, document_id)
);

CREATE TABLE IF NOT EXISTS knowledge_document_bookmarks (
    id TEXT PRIMARY KEY,
    knowledge_base_id TEXT NOT NULL,
    document_id TEXT NOT NULL,
    source_revision TEXT,
    page INTEGER NOT NULL DEFAULT 1 CHECK(page >= 1),
    anchor TEXT NOT NULL DEFAULT '',
    excerpt TEXT NOT NULL DEFAULT '',
    label TEXT NOT NULL DEFAULT '',
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_knowledge_document_bookmarks_document
    ON knowledge_document_bookmarks(knowledge_base_id, document_id, page, created_at);

CREATE TABLE IF NOT EXISTS knowledge_document_annotations (
    id TEXT PRIMARY KEY,
    knowledge_base_id TEXT NOT NULL,
    document_id TEXT NOT NULL,
    source_revision TEXT,
    annotation_type TEXT NOT NULL DEFAULT 'note'
        CHECK(annotation_type IN ('note', 'highlight')),
    page INTEGER NOT NULL DEFAULT 1 CHECK(page >= 1),
    anchor TEXT NOT NULL DEFAULT '',
    excerpt TEXT NOT NULL DEFAULT '',
    note TEXT NOT NULL DEFAULT '',
    color TEXT NOT NULL DEFAULT 'blue',
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_knowledge_document_annotations_document
    ON knowledge_document_annotations(knowledge_base_id, document_id, page, created_at);
"#;

const MIGRATION_14: &str = r#"
CREATE TABLE goals (
    id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    title TEXT NOT NULL,
    objective TEXT NOT NULL,
    acceptance_summary TEXT,
    status TEXT NOT NULL
        CHECK(status IN ('proposed', 'active', 'blocked', 'completed', 'cancelled')),
    version INTEGER NOT NULL DEFAULT 1 CHECK(version >= 1),
    created_by TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    completed_at TEXT,
    blocked_reason TEXT
);

CREATE INDEX idx_goals_conversation_updated
    ON goals(conversation_id, updated_at DESC);
CREATE UNIQUE INDEX idx_goals_active_per_conversation
    ON goals(conversation_id)
    WHERE status IN ('active', 'blocked');

CREATE TABLE work_tasks (
    id TEXT PRIMARY KEY,
    goal_id TEXT NOT NULL REFERENCES goals(id) ON DELETE CASCADE,
    parent_task_id TEXT REFERENCES work_tasks(id) ON DELETE SET NULL,
    ordinal INTEGER NOT NULL CHECK(ordinal >= 0),
    title TEXT NOT NULL,
    detail TEXT,
    status TEXT NOT NULL
        CHECK(status IN ('queued', 'in_progress', 'completed', 'blocked', 'interrupted', 'skipped')),
    owner_run_id TEXT REFERENCES runs(id) ON DELETE SET NULL,
    attempt INTEGER NOT NULL DEFAULT 1 CHECK(attempt >= 1),
    blocked_reason TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    started_at TEXT,
    finished_at TEXT
);

CREATE INDEX idx_work_tasks_goal ON work_tasks(goal_id);
CREATE INDEX idx_work_tasks_status ON work_tasks(status);
CREATE UNIQUE INDEX idx_work_tasks_in_progress_per_goal
    ON work_tasks(goal_id)
    WHERE status = 'in_progress';

CREATE TABLE task_evidence (
    id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL REFERENCES work_tasks(id) ON DELETE CASCADE,
    source_run_id TEXT REFERENCES runs(id) ON DELETE SET NULL,
    evidence_type TEXT NOT NULL
        CHECK(evidence_type IN ('tool_call', 'trace_span', 'test_result', 'file_diff', 'artifact', 'user_confirmation', 'external_reference')),
    ref_kind TEXT NOT NULL
        CHECK(ref_kind IN ('tool_call', 'artifact', 'run_event', 'message', 'source')),
    ref_id TEXT NOT NULL,
    summary TEXT NOT NULL,
    metadata_json TEXT NOT NULL DEFAULT '{}',
    validity_status TEXT NOT NULL DEFAULT 'unverified'
        CHECK(validity_status IN ('unverified', 'valid', 'stale', 'missing', 'invalid')),
    trace_id TEXT,
    span_id TEXT,
    checked_at TEXT,
    invalid_reason TEXT,
    created_at TEXT NOT NULL
);

CREATE INDEX idx_task_evidence_task ON task_evidence(task_id);
CREATE INDEX idx_task_evidence_validity ON task_evidence(validity_status);

ALTER TABLE runs ADD COLUMN trace_id TEXT;
ALTER TABLE runs ADD COLUMN root_span_id TEXT;
ALTER TABLE run_events ADD COLUMN trace_id TEXT;
ALTER TABLE run_events ADD COLUMN span_id TEXT;
ALTER TABLE tool_calls ADD COLUMN trace_id TEXT;
ALTER TABLE tool_calls ADD COLUMN span_id TEXT;
"#;

const MIGRATION_15: &str = r#"
ALTER TABLE work_tasks ADD COLUMN version INTEGER NOT NULL DEFAULT 1 CHECK(version >= 1);

CREATE TABLE work_events (
    id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    sequence INTEGER NOT NULL CHECK(sequence >= 0),
    event_type TEXT NOT NULL,
    schema_version INTEGER NOT NULL CHECK(schema_version >= 1),
    event_json TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    UNIQUE(conversation_id, sequence)
);

CREATE INDEX idx_work_events_conversation_sequence
    ON work_events(conversation_id, sequence);
"#;

pub fn run(connection: &mut Connection, now: i64) -> Result<()> {
    connection.pragma_update(None, "foreign_keys", "ON")?;
    connection.pragma_update(None, "journal_mode", "WAL")?;
    connection.pragma_update(None, "busy_timeout", 5_000)?;

    let transaction = connection.transaction()?;
    transaction.execute_batch(MIGRATION_1)?;
    transaction.execute(
        "INSERT OR IGNORE INTO schema_migrations(version, applied_at) VALUES (1, ?1)",
        [now],
    )?;
    apply_migration(&transaction, 2, MIGRATION_2, now)?;
    apply_migration(&transaction, 3, MIGRATION_3, now)?;
    apply_migration(&transaction, 4, MIGRATION_4, now)?;
    apply_migration(&transaction, 5, MIGRATION_5, now)?;
    apply_migration(&transaction, 6, MIGRATION_6, now)?;
    apply_migration(&transaction, 7, MIGRATION_7, now)?;
    apply_migration(&transaction, 8, MIGRATION_8, now)?;
    apply_migration(&transaction, 9, MIGRATION_9, now)?;
    apply_migration(&transaction, 10, MIGRATION_10, now)?;
    apply_migration(&transaction, 11, MIGRATION_11, now)?;
    apply_migration(&transaction, 12, MIGRATION_12, now)?;
    apply_migration(&transaction, 13, MIGRATION_13, now)?;
    apply_migration(&transaction, 14, MIGRATION_14, now)?;
    apply_migration(&transaction, 15, MIGRATION_15, now)?;
    transaction.commit()
}

fn apply_migration(
    transaction: &rusqlite::Transaction<'_>,
    version: i64,
    sql: &str,
    now: i64,
) -> Result<()> {
    let applied = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM schema_migrations WHERE version = ?1)",
        [version],
        |row| row.get::<_, bool>(0),
    )?;
    if applied {
        return Ok(());
    }
    transaction.execute_batch(sql)?;
    transaction.execute(
        "INSERT INTO schema_migrations(version, applied_at) VALUES (?1, ?2)",
        rusqlite::params![version, now],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_pre_a0(connection: &mut Connection, now: i64) {
        connection
            .pragma_update(None, "foreign_keys", "ON")
            .expect("enable foreign keys");
        let transaction = connection.transaction().expect("begin migration");
        transaction.execute_batch(MIGRATION_1).expect("migration 1");
        transaction
            .execute(
                "INSERT OR IGNORE INTO schema_migrations(version, applied_at) VALUES (1, ?1)",
                [now],
            )
            .expect("record migration 1");
        for (version, sql) in [
            (2, MIGRATION_2),
            (3, MIGRATION_3),
            (4, MIGRATION_4),
            (5, MIGRATION_5),
            (6, MIGRATION_6),
            (7, MIGRATION_7),
            (8, MIGRATION_8),
            (9, MIGRATION_9),
            (10, MIGRATION_10),
            (11, MIGRATION_11),
            (12, MIGRATION_12),
            (13, MIGRATION_13),
        ] {
            apply_migration(&transaction, version, sql, now).expect("apply pre-A0 migration");
        }
        transaction.commit().expect("commit pre-A0 schema");
    }

    #[test]
    fn creates_knowledge_preview_cache_settings_and_document_activity_schema() {
        let mut connection = Connection::open_in_memory().expect("open database");
        run(&mut connection, 1).expect("apply migrations");

        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master
                     WHERE type = 'table' AND name = 'knowledge_preview_cache'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1
        );
        for table in [
            "knowledge_document_reading_state",
            "knowledge_document_bookmarks",
            "knowledge_document_annotations",
        ] {
            assert_eq!(
                connection
                    .query_row(
                        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
                        [table],
                        |row| row.get::<_, i64>(0),
                    )
                    .unwrap(),
                1,
                "missing migration table {table}"
            );
        }
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM schema_migrations WHERE version = 13",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master
                     WHERE type = 'table' AND name = 'app_settings'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM schema_migrations WHERE version = 12",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM schema_migrations WHERE version = 11",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1
        );
    }

    #[test]
    fn upgrades_pre_a0_data_and_is_idempotent() {
        let mut connection = Connection::open_in_memory().expect("open database");
        run_pre_a0(&mut connection, 1);
        connection
            .execute_batch(
                "INSERT INTO agents(
                    id, name, description, runtime_type, system_prompt, default_model,
                    created_at, updated_at
                 ) VALUES ('agent-a0', 'A0', '', 'pi', '', 'model', 1, 1);
                 INSERT INTO conversations(
                    id, agent_id, title, status, created_at, updated_at
                 ) VALUES ('conversation-a0', 'agent-a0', 'A0', 'active', 1, 1);
                 INSERT INTO runs(
                    id, conversation_id, status, model, started_at, created_at
                 ) VALUES ('run-a0', 'conversation-a0', 'running', 'model', 2, 2);
                 INSERT INTO messages(
                    id, conversation_id, run_id, role, kind, content, status, ordinal,
                    created_at, updated_at
                 ) VALUES ('message-a0', 'conversation-a0', 'run-a0', 'user', 'text',
                           'keep me', 'completed', 1, 2, 2);
                 INSERT INTO run_events(
                    id, run_id, seq, event_type, event_json, created_at
                 ) VALUES ('event-a0', 'run-a0', 1, 'run.failed',
                           '{\"type\":\"run.failed\"}', 3);",
            )
            .expect("seed pre-A0 data");

        run(&mut connection, 4).expect("upgrade to A0");
        run(&mut connection, 5).expect("repeat A0 migration");

        for (table, count) in [
            ("conversations", 1_i64),
            ("messages", 1),
            ("runs", 1),
            ("run_events", 1),
            ("goals", 0),
            ("work_tasks", 0),
            ("task_evidence", 0),
        ] {
            let actual: i64 = connection
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .expect("count migrated rows");
            assert_eq!(actual, count, "unexpected row count for {table}");
        }
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM schema_migrations WHERE version = 14",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1
        );
        for (table, column) in [
            ("runs", "trace_id"),
            ("runs", "root_span_id"),
            ("run_events", "trace_id"),
            ("run_events", "span_id"),
            ("tool_calls", "trace_id"),
            ("tool_calls", "span_id"),
        ] {
            let count = connection
                .query_row(
                    &format!("SELECT COUNT(*) FROM pragma_table_info('{table}') WHERE name = ?1"),
                    [column],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap();
            assert_eq!(count, 1, "missing {table}.{column}");
        }
    }

    #[test]
    fn enforces_a0_status_and_single_active_work_constraints() {
        let mut connection = Connection::open_in_memory().expect("open database");
        run(&mut connection, 1).expect("apply migrations");
        connection
            .execute_batch(
                "INSERT INTO agents(
                    id, name, description, runtime_type, system_prompt, default_model,
                    created_at, updated_at
                 ) VALUES ('agent-a0', 'A0', '', 'pi', '', 'model', 1, 1);
                 INSERT INTO conversations(
                    id, agent_id, title, status, created_at, updated_at
                 ) VALUES ('conversation-a0', 'agent-a0', 'A0', 'active', 1, 1);
                 INSERT INTO goals(
                    id, conversation_id, title, objective, status, created_by, created_at, updated_at
                 ) VALUES ('goal-a', 'conversation-a0', 'A', 'A', 'active', 'user', '1', '1');",
            )
            .expect("seed active goal");

        assert!(connection
            .execute(
                "INSERT INTO goals(
                    id, conversation_id, title, objective, status, created_by, created_at, updated_at
                 ) VALUES ('goal-b', 'conversation-a0', 'B', 'B', 'blocked', 'user', '1', '1')",
                [],
            )
            .is_err());
        assert!(connection
            .execute(
                "INSERT INTO goals(
                    id, conversation_id, title, objective, status, created_by, created_at, updated_at
                 ) VALUES ('goal-invalid', 'conversation-a0', 'X', 'X', 'running', 'user', '1', '1')",
                [],
            )
            .is_err());

        connection
            .execute(
                "INSERT INTO work_tasks(
                    id, goal_id, ordinal, title, status, created_at, updated_at
                 ) VALUES ('task-a', 'goal-a', 0, 'A', 'in_progress', '1', '1')",
                [],
            )
            .expect("insert active task");
        assert!(connection
            .execute(
                "INSERT INTO work_tasks(
                    id, goal_id, ordinal, title, status, created_at, updated_at
                 ) VALUES ('task-b', 'goal-a', 1, 'B', 'in_progress', '1', '1')",
                [],
            )
            .is_err());
    }

    #[test]
    fn failed_upgrade_can_restore_and_migrate_the_pre_a0_backup() {
        let root = std::env::temp_dir().join(format!(
            "fox-a0-migration-backup-test-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).expect("create test directory");
        let original = root.join("fox.db");
        let backup = root.join("fox-pre-a0.db");
        let restored = root.join("fox-restored.db");

        {
            let mut connection = Connection::open(&original).expect("open original database");
            run_pre_a0(&mut connection, 1);
            connection
                .execute_batch(
                    "INSERT INTO agents(
                        id, name, description, runtime_type, system_prompt, default_model,
                        created_at, updated_at
                     ) VALUES ('agent-backup', 'Backup', '', 'pi', '', 'model', 1, 1);
                     INSERT INTO conversations(
                        id, agent_id, title, status, created_at, updated_at
                     ) VALUES ('conversation-backup', 'agent-backup', 'Backup', 'active', 1, 1);",
                )
                .expect("seed original database");
            connection
                .execute("VACUUM INTO ?1", [backup.to_string_lossy().as_ref()])
                .expect("create pre-A0 backup");
            connection
                .execute("CREATE TABLE goals(id TEXT PRIMARY KEY)", [])
                .expect("inject incompatible partial schema");
            assert!(run(&mut connection, 2).is_err());
            assert_eq!(
                connection
                    .query_row(
                        "SELECT COUNT(*) FROM schema_migrations WHERE version = 14",
                        [],
                        |row| row.get::<_, i64>(0),
                    )
                    .unwrap(),
                0
            );
        }

        std::fs::copy(&backup, &restored).expect("restore backup");
        {
            let mut connection = Connection::open(&restored).expect("open restored database");
            run(&mut connection, 3).expect("migrate restored database");
            assert_eq!(
                connection
                    .query_row(
                        "SELECT COUNT(*) FROM conversations WHERE id = 'conversation-backup'",
                        [],
                        |row| row.get::<_, i64>(0),
                    )
                    .unwrap(),
                1
            );
            assert_eq!(
                connection
                    .query_row(
                        "SELECT COUNT(*) FROM schema_migrations WHERE version = 14",
                        [],
                        |row| row.get::<_, i64>(0),
                    )
                    .unwrap(),
                1
            );
        }

        std::fs::remove_dir_all(root).expect("remove test directory");
    }

    #[test]
    fn upgrades_phase_one_project_roots_without_losing_conversations() {
        let mut connection = Connection::open_in_memory().expect("open database");
        connection
            .execute_batch(MIGRATION_1)
            .expect("phase one schema");
        connection.execute_batch(MIGRATION_2).expect("Yuxi schema");
        connection.execute_batch(MIGRATION_3).expect("model schema");
        for version in 1..=3 {
            connection
                .execute(
                    "INSERT INTO schema_migrations(version, applied_at) VALUES (?1, 1)",
                    [version],
                )
                .expect("record old migration");
        }
        connection
            .execute(
                "INSERT INTO agents(
                    id, name, description, runtime_type, system_prompt, default_model,
                    created_at, updated_at
                 ) VALUES ('fox-general', 'Fox', '', 'pi', '', 'model', 1, 1)",
                [],
            )
            .expect("insert agent");
        connection
            .execute(
                "INSERT INTO conversations(
                    id, agent_id, title, project_root, status, created_at, updated_at
                 ) VALUES ('conversation-1', 'fox-general', 'Legacy', 'D:\\projects\\legacy',
                           'active', 1, 2)",
                [],
            )
            .expect("insert conversation");

        run(&mut connection, 3).expect("upgrade database");
        run(&mut connection, 4).expect("migration remains idempotent");

        let (project_id, project_root): (String, String) = connection
            .query_row(
                "SELECT c.project_id, p.root_path
                 FROM conversations c JOIN projects p ON p.id = c.project_id
                 WHERE c.id = 'conversation-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("load migrated project");
        assert!(!project_id.is_empty());
        assert_eq!(project_root, "D:\\projects\\legacy");
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM schema_migrations WHERE version = 4",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1
        );
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM tool_calls", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
}
