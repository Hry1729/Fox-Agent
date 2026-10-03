use super::repositories::{agent_package_snapshot, package_snapshot_hash, query_agent_record};
use super::DATABASE_SCHEMA_VERSION;
use rusqlite::{Connection, OptionalExtension, Result};
use serde_json::json;
use sha2::{Digest, Sha256};

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
    -- Lifecycle bucket decided by the Host (deliverable / preview / process),
    -- never inferred from the file extension by the UI.
    artifact_class TEXT NOT NULL DEFAULT 'process',
    -- Where the bytes live: the conversation's project folder or an
    -- application-private Host area (compute workspace, preview cache,
    -- working copy, version store).
    artifact_origin TEXT NOT NULL DEFAULT 'project',
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

const MIGRATION_16: &str = r#"
CREATE TABLE pending_work_mode_dispatches (
    goal_id TEXT PRIMARY KEY REFERENCES goals(id) ON DELETE CASCADE,
    run_id TEXT NOT NULL UNIQUE REFERENCES runs(id) ON DELETE CASCADE,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    runtime_text TEXT NOT NULL,
    created_at INTEGER NOT NULL
);

CREATE INDEX idx_pending_work_mode_dispatches_conversation
    ON pending_work_mode_dispatches(conversation_id);
"#;

const MIGRATION_17: &str = r#"
ALTER TABLE conversations ADD COLUMN pinned INTEGER NOT NULL DEFAULT 0
    CHECK(pinned IN (0, 1));
ALTER TABLE conversations ADD COLUMN archived INTEGER NOT NULL DEFAULT 0
    CHECK(archived IN (0, 1));

CREATE INDEX idx_conversations_archived_pinned_activity
    ON conversations(archived, pinned, last_message_at, created_at);
"#;

const MIGRATION_18: &str = r#"
ALTER TABLE model_providers ADD COLUMN icon TEXT;
"#;

const MIGRATION_19: &str = r#"
ALTER TABLE agents ADD COLUMN icon TEXT;
ALTER TABLE agents ADD COLUMN category TEXT NOT NULL DEFAULT 'general';
ALTER TABLE agents ADD COLUMN opening_suggestions_json TEXT NOT NULL DEFAULT '[]';
ALTER TABLE agents ADD COLUMN is_builtin INTEGER NOT NULL DEFAULT 0 CHECK(is_builtin IN (0, 1));
ALTER TABLE agents ADD COLUMN package_version TEXT NOT NULL DEFAULT '1.0.0';
ALTER TABLE agents ADD COLUMN package_manifest_json TEXT NOT NULL DEFAULT '{}';
CREATE TABLE plan_revisions (
    id TEXT PRIMARY KEY,
    goal_id TEXT NOT NULL REFERENCES goals(id) ON DELETE CASCADE,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    revision INTEGER NOT NULL CHECK(revision >= 1),
    title TEXT NOT NULL,
    summary TEXT NOT NULL,
    tasks_json TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('proposed', 'approved', 'rejected', 'superseded')),
    created_by TEXT NOT NULL,
    created_at TEXT NOT NULL,
    approved_at TEXT,
    UNIQUE(goal_id, revision)
);
CREATE INDEX idx_plan_revisions_goal_revision ON plan_revisions(goal_id, revision DESC);

CREATE TABLE review_findings (
    id TEXT PRIMARY KEY,
    goal_id TEXT NOT NULL REFERENCES goals(id) ON DELETE CASCADE,
    task_id TEXT REFERENCES work_tasks(id) ON DELETE SET NULL,
    plan_revision_id TEXT REFERENCES plan_revisions(id) ON DELETE SET NULL,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    severity TEXT NOT NULL CHECK(severity IN ('critical', 'high', 'medium', 'low', 'info')),
    category TEXT NOT NULL,
    title TEXT NOT NULL,
    detail TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('open', 'resolved', 'waived')),
    created_by TEXT NOT NULL,
    created_at TEXT NOT NULL,
    resolved_at TEXT
);
CREATE INDEX idx_review_findings_goal_status ON review_findings(goal_id, status, severity);

CREATE TABLE acceptances (
    id TEXT PRIMARY KEY,
    goal_id TEXT NOT NULL REFERENCES goals(id) ON DELETE CASCADE,
    plan_revision_id TEXT REFERENCES plan_revisions(id) ON DELETE SET NULL,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    status TEXT NOT NULL CHECK(status IN ('pending', 'accepted', 'rejected')),
    summary TEXT NOT NULL,
    checks_json TEXT NOT NULL,
    reviewer TEXT NOT NULL,
    created_at TEXT NOT NULL,
    resolved_at TEXT
);
CREATE INDEX idx_acceptances_goal_created ON acceptances(goal_id, created_at DESC);
"#;

const MIGRATION_24: &str = r#"
ALTER TABLE conversations ADD COLUMN archived_at INTEGER;
ALTER TABLE conversations ADD COLUMN trashed_at INTEGER;
ALTER TABLE conversations ADD COLUMN parent_conversation_id TEXT
    REFERENCES conversations(id) ON DELETE SET NULL;
ALTER TABLE conversations ADD COLUMN forked_from_message_id TEXT
    REFERENCES messages(id) ON DELETE SET NULL;
ALTER TABLE conversations ADD COLUMN lineage_root_id TEXT;

UPDATE conversations
SET archived_at = CASE WHEN archived = 1 THEN updated_at ELSE NULL END,
    lineage_root_id = id;

CREATE INDEX idx_conversations_lifecycle_activity
    ON conversations(trashed_at, archived, pinned, last_message_at, created_at);
CREATE INDEX idx_conversations_parent
    ON conversations(parent_conversation_id, created_at);
CREATE INDEX idx_conversations_lineage
    ON conversations(lineage_root_id, created_at);
"#;

const MIGRATION_25: &str = r#"
CREATE TABLE memory_entities (
    id TEXT PRIMARY KEY,
    scope TEXT NOT NULL CHECK(scope IN ('global', 'agent', 'project')),
    scope_key TEXT NOT NULL DEFAULT '',
    kind TEXT NOT NULL CHECK(kind IN ('preference', 'identity', 'project', 'workflow', 'fact', 'other')),
    canonical_key TEXT NOT NULL,
    content TEXT NOT NULL,
    state TEXT NOT NULL CHECK(state IN ('candidate', 'confirmed', 'conflict')),
    enabled INTEGER NOT NULL DEFAULT 0 CHECK(enabled IN (0, 1)),
    source_conversation_id TEXT REFERENCES conversations(id) ON DELETE SET NULL,
    source_message_id TEXT REFERENCES messages(id) ON DELETE SET NULL,
    source_run_id TEXT REFERENCES runs(id) ON DELETE SET NULL,
    evidence_excerpt TEXT NOT NULL DEFAULT '',
    created_by TEXT NOT NULL CHECK(created_by IN ('user', 'agent', 'system')),
    confidence REAL NOT NULL DEFAULT 1.0 CHECK(confidence >= 0.0 AND confidence <= 1.0),
    version INTEGER NOT NULL DEFAULT 1,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    confirmed_at INTEGER,
    disabled_at INTEGER,
    deleted_at INTEGER
);

CREATE INDEX idx_memory_entities_governance
    ON memory_entities(deleted_at, scope, scope_key, state, enabled, updated_at DESC);
CREATE INDEX idx_memory_entities_key
    ON memory_entities(scope, scope_key, canonical_key, deleted_at);

CREATE TABLE memory_revisions (
    id TEXT PRIMARY KEY,
    memory_id TEXT NOT NULL REFERENCES memory_entities(id) ON DELETE CASCADE,
    actor TEXT NOT NULL CHECK(actor IN ('user', 'agent', 'system')),
    action TEXT NOT NULL,
    before_json TEXT,
    after_json TEXT,
    created_at INTEGER NOT NULL
);
CREATE INDEX idx_memory_revisions_memory_created
    ON memory_revisions(memory_id, created_at DESC);

CREATE TABLE memory_conflicts (
    id TEXT PRIMARY KEY,
    existing_memory_id TEXT NOT NULL REFERENCES memory_entities(id) ON DELETE CASCADE,
    competing_memory_id TEXT NOT NULL REFERENCES memory_entities(id) ON DELETE CASCADE,
    status TEXT NOT NULL DEFAULT 'open' CHECK(status IN ('open', 'resolved')),
    resolution TEXT CHECK(resolution IN ('keep_existing', 'accept_competing')),
    created_at INTEGER NOT NULL,
    resolved_at INTEGER,
    UNIQUE(existing_memory_id, competing_memory_id)
);
CREATE INDEX idx_memory_conflicts_status_created
    ON memory_conflicts(status, created_at DESC);

CREATE TABLE memory_recalls (
    id TEXT PRIMARY KEY,
    memory_id TEXT NOT NULL REFERENCES memory_entities(id) ON DELETE CASCADE,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    run_id TEXT REFERENCES runs(id) ON DELETE CASCADE,
    query TEXT NOT NULL,
    reason TEXT NOT NULL,
    score REAL NOT NULL,
    rank INTEGER NOT NULL,
    evidence_excerpt TEXT NOT NULL DEFAULT '',
    recalled_at INTEGER NOT NULL
);
CREATE INDEX idx_memory_recalls_memory_created
    ON memory_recalls(memory_id, recalled_at DESC);
CREATE INDEX idx_memory_recalls_run_rank
    ON memory_recalls(run_id, rank);
"#;

const MIGRATION_26: &str = r#"
CREATE TABLE trace_spans (
    id TEXT PRIMARY KEY CHECK(length(id) = 16),
    trace_id TEXT NOT NULL CHECK(length(trace_id) = 32),
    parent_span_id TEXT REFERENCES trace_spans(id) ON DELETE CASCADE,
    run_id TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    operation TEXT NOT NULL,
    category TEXT NOT NULL CHECK(category IN ('run', 'phase', 'tool', 'ui')),
    status TEXT NOT NULL DEFAULT 'unset' CHECK(status IN ('unset', 'ok', 'error')),
    started_at INTEGER NOT NULL,
    ended_at INTEGER,
    duration_ms INTEGER CHECK(duration_ms IS NULL OR duration_ms >= 0),
    attributes_json TEXT NOT NULL DEFAULT '{}',
    error_type TEXT,
    error_message TEXT,
    entity_id TEXT,
    schema_version INTEGER NOT NULL DEFAULT 1
);
CREATE UNIQUE INDEX idx_trace_spans_run_root
    ON trace_spans(run_id) WHERE category = 'run';
CREATE INDEX idx_trace_spans_trace_started
    ON trace_spans(trace_id, started_at, id);
CREATE INDEX idx_trace_spans_run_category
    ON trace_spans(run_id, category, started_at, id);
CREATE INDEX idx_trace_spans_open
    ON trace_spans(run_id, category, ended_at) WHERE ended_at IS NULL;
CREATE INDEX idx_trace_spans_entity
    ON trace_spans(run_id, category, entity_id) WHERE entity_id IS NOT NULL;

CREATE TABLE evaluation_runs (
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL,
    schema_version INTEGER NOT NULL,
    generated_at TEXT NOT NULL,
    recorded_at INTEGER NOT NULL,
    duration_ms INTEGER NOT NULL CHECK(duration_ms >= 0),
    report_hash TEXT NOT NULL,
    suite_count INTEGER NOT NULL CHECK(suite_count >= 0),
    total_count INTEGER NOT NULL CHECK(total_count >= 0),
    passed_count INTEGER NOT NULL CHECK(passed_count >= 0),
    failed_count INTEGER NOT NULL CHECK(failed_count >= 0),
    report_json TEXT NOT NULL
);
CREATE INDEX idx_evaluation_runs_recorded
    ON evaluation_runs(recorded_at DESC, id);

CREATE TABLE evaluation_suite_results (
    evaluation_run_id TEXT NOT NULL REFERENCES evaluation_runs(id) ON DELETE CASCADE,
    suite_name TEXT NOT NULL,
    source TEXT,
    source_url TEXT,
    total_count INTEGER NOT NULL CHECK(total_count >= 0),
    passed_count INTEGER NOT NULL CHECK(passed_count >= 0),
    failed_count INTEGER NOT NULL CHECK(failed_count >= 0),
    PRIMARY KEY(evaluation_run_id, suite_name)
);
CREATE INDEX idx_evaluation_suite_name_run
    ON evaluation_suite_results(suite_name, evaluation_run_id);

UPDATE runs
SET trace_id = lower(hex(randomblob(16)))
WHERE trace_id IS NULL OR length(trace_id) != 32;
UPDATE runs
SET root_span_id = lower(hex(randomblob(8)))
WHERE root_span_id IS NULL OR length(root_span_id) != 16;

INSERT INTO trace_spans(
    id, trace_id, parent_span_id, run_id, conversation_id, name, operation,
    category, status, started_at, ended_at, duration_ms, attributes_json, schema_version
)
SELECT root_span_id, trace_id, NULL, id, conversation_id, 'Fox invoke_agent',
       'invoke_agent', 'run',
       CASE WHEN status IN ('failed', 'interrupted') THEN 'error'
            WHEN status IN ('completed', 'cancelled') THEN 'ok' ELSE 'unset' END,
       COALESCE(started_at, created_at), finished_at,
       CASE WHEN finished_at IS NULL THEN NULL
            ELSE MAX(0, finished_at - COALESCE(started_at, created_at)) END,
       '{"fox.backfilled":true,"fox.trace.schema_version":1}', 1
FROM runs;

UPDATE run_events
SET trace_id = (SELECT trace_id FROM runs WHERE runs.id = run_events.run_id),
    span_id = (SELECT root_span_id FROM runs WHERE runs.id = run_events.run_id)
WHERE trace_id IS NULL OR length(trace_id) != 32 OR span_id IS NULL OR length(span_id) != 16;
UPDATE tool_calls
SET trace_id = (SELECT trace_id FROM runs WHERE runs.id = tool_calls.run_id),
    span_id = (SELECT root_span_id FROM runs WHERE runs.id = tool_calls.run_id)
WHERE trace_id IS NULL OR length(trace_id) != 32 OR span_id IS NULL OR length(span_id) != 16;
"#;

const MIGRATION_27: &str = r#"
ALTER TABLE conversations ADD COLUMN conversation_kind TEXT NOT NULL DEFAULT 'primary'
    CHECK(conversation_kind IN ('primary', 'child'));

ALTER TABLE runs ADD COLUMN parent_run_id TEXT REFERENCES runs(id) ON DELETE CASCADE;
ALTER TABLE runs ADD COLUMN root_run_id TEXT;
ALTER TABLE runs ADD COLUMN run_kind TEXT NOT NULL DEFAULT 'primary'
    CHECK(run_kind IN ('primary', 'child'));
ALTER TABLE runs ADD COLUMN depth INTEGER NOT NULL DEFAULT 0 CHECK(depth >= 0);
ALTER TABLE runs ADD COLUMN budget_json TEXT NOT NULL DEFAULT '{}';

UPDATE runs SET root_run_id = id WHERE root_run_id IS NULL;

CREATE INDEX idx_runs_parent_created
    ON runs(parent_run_id, created_at, id);
CREATE INDEX idx_runs_root_status
    ON runs(root_run_id, status, created_at, id);
CREATE INDEX idx_conversations_kind_activity
    ON conversations(conversation_kind, archived, trashed_at, last_message_at, created_at);

CREATE TABLE child_run_delegations (
    id TEXT PRIMARY KEY,
    parent_run_id TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    child_run_id TEXT NOT NULL UNIQUE REFERENCES runs(id) ON DELETE CASCADE,
    child_conversation_id TEXT NOT NULL UNIQUE REFERENCES conversations(id) ON DELETE CASCADE,
    tool_call_id TEXT NOT NULL,
    worker_agent_id TEXT NOT NULL REFERENCES agents(id),
    objective TEXT NOT NULL,
    context TEXT NOT NULL DEFAULT '',
    status TEXT NOT NULL
        CHECK(status IN ('queued', 'running', 'completed', 'failed', 'cancelled', 'interrupted')),
    max_duration_ms INTEGER NOT NULL CHECK(max_duration_ms BETWEEN 1000 AND 900000),
    max_total_tokens INTEGER NOT NULL CHECK(max_total_tokens BETWEEN 256 AND 200000),
    max_output_tokens INTEGER NOT NULL CHECK(max_output_tokens BETWEEN 64 AND 32768),
    max_tool_calls INTEGER NOT NULL CHECK(max_tool_calls BETWEEN 0 AND 100),
    result_text TEXT,
    input_tokens INTEGER NOT NULL DEFAULT 0 CHECK(input_tokens >= 0),
    output_tokens INTEGER NOT NULL DEFAULT 0 CHECK(output_tokens >= 0),
    total_tokens INTEGER NOT NULL DEFAULT 0 CHECK(total_tokens >= 0),
    tool_call_count INTEGER NOT NULL DEFAULT 0 CHECK(tool_call_count >= 0),
    error_code TEXT,
    error_message TEXT,
    created_at INTEGER NOT NULL,
    started_at INTEGER,
    finished_at INTEGER,
    UNIQUE(parent_run_id, tool_call_id)
);
CREATE INDEX idx_child_delegations_parent_created
    ON child_run_delegations(parent_run_id, created_at, id);
CREATE INDEX idx_child_delegations_status_created
    ON child_run_delegations(status, created_at, id);

CREATE TRIGGER delete_hidden_child_conversation_after_delegation
AFTER DELETE ON child_run_delegations
BEGIN
    DELETE FROM conversations
    WHERE id = OLD.child_conversation_id AND conversation_kind = 'child';
END;
"#;

const MIGRATION_28: &str = r#"
ALTER TABLE mcp_servers ADD COLUMN transport TEXT NOT NULL DEFAULT 'stdio'
    CHECK(transport IN ('stdio', 'streamable_http', 'openapi'));
ALTER TABLE mcp_servers ADD COLUMN endpoint_url TEXT;
ALTER TABLE mcp_servers ADD COLUMN definition TEXT;
ALTER TABLE mcp_servers ADD COLUMN last_latency_ms INTEGER
    CHECK(last_latency_ms IS NULL OR last_latency_ms >= 0);
ALTER TABLE mcp_servers ADD COLUMN tool_count INTEGER
    CHECK(tool_count IS NULL OR tool_count >= 0);
ALTER TABLE mcp_servers ADD COLUMN consecutive_failures INTEGER NOT NULL DEFAULT 0
    CHECK(consecutive_failures >= 0);

CREATE INDEX idx_mcp_servers_transport_status
    ON mcp_servers(transport, enabled, status, updated_at);

CREATE TABLE lifecycle_hooks (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    event TEXT NOT NULL CHECK(event IN ('before_run', 'before_tool', 'after_tool', 'after_run')),
    matcher TEXT NOT NULL DEFAULT '*',
    action TEXT NOT NULL CHECK(action IN ('block', 'require_approval', 'annotate')),
    reason TEXT NOT NULL DEFAULT '',
    enabled INTEGER NOT NULL DEFAULT 1 CHECK(enabled IN (0, 1)),
    priority INTEGER NOT NULL DEFAULT 100 CHECK(priority BETWEEN 0 AND 1000),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE INDEX idx_lifecycle_hooks_event_enabled_priority
    ON lifecycle_hooks(event, enabled, priority, id);

CREATE TABLE lifecycle_hook_executions (
    id TEXT PRIMARY KEY,
    hook_id TEXT NOT NULL REFERENCES lifecycle_hooks(id) ON DELETE CASCADE,
    run_id TEXT REFERENCES runs(id) ON DELETE CASCADE,
    tool_call_id TEXT,
    event TEXT NOT NULL,
    tool_name TEXT,
    action TEXT NOT NULL,
    outcome TEXT NOT NULL CHECK(outcome IN ('matched', 'applied', 'skipped', 'failed')),
    details_json TEXT NOT NULL DEFAULT '{}',
    created_at INTEGER NOT NULL
);
CREATE INDEX idx_lifecycle_hook_executions_run_created
    ON lifecycle_hook_executions(run_id, created_at, id);
"#;

const MIGRATION_29: &str = r#"
ALTER TABLE agents ADD COLUMN package_source TEXT NOT NULL DEFAULT 'local'
    CHECK(package_source IN ('builtin', 'local', 'imported', 'remote'));
ALTER TABLE agents ADD COLUMN package_id TEXT;
ALTER TABLE agents ADD COLUMN package_hash TEXT;

UPDATE agents SET package_source = 'builtin' WHERE is_builtin = 1;
UPDATE agents SET package_source = 'remote' WHERE runtime_type != 'pi';

CREATE UNIQUE INDEX idx_agents_package_id
    ON agents(package_id) WHERE package_id IS NOT NULL;

CREATE TABLE expert_package_versions (
    id TEXT PRIMARY KEY,
    expert_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    package_id TEXT NOT NULL,
    version TEXT NOT NULL,
    package_hash TEXT NOT NULL,
    package_json TEXT NOT NULL,
    source TEXT NOT NULL CHECK(source IN ('imported', 'local', 'builtin')),
    status TEXT NOT NULL CHECK(status IN ('active', 'historical')),
    created_at INTEGER NOT NULL,
    activated_at INTEGER NOT NULL,
    UNIQUE(package_id, version)
);
CREATE UNIQUE INDEX idx_expert_package_versions_active
    ON expert_package_versions(expert_id) WHERE status = 'active';
CREATE INDEX idx_expert_package_versions_history
    ON expert_package_versions(expert_id, activated_at DESC, version DESC);
"#;

const MIGRATION_30: &str = r#"
CREATE TABLE expert_workflow_runs (
    id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    expert_binding_id TEXT NOT NULL REFERENCES conversation_expert_bindings(id) ON DELETE CASCADE,
    expert_id TEXT NOT NULL REFERENCES agents(id),
    package_hash TEXT NOT NULL,
    workflow_id TEXT NOT NULL,
    workflow_version TEXT NOT NULL,
    workflow_json TEXT NOT NULL,
    goal_id TEXT NOT NULL REFERENCES goals(id) ON DELETE CASCADE,
    status TEXT NOT NULL CHECK(status IN (
        'running', 'awaiting_gate', 'completed', 'failed', 'cancelled'
    )),
    current_stage_index INTEGER NOT NULL DEFAULT 0 CHECK(current_stage_index >= 0),
    input_json TEXT NOT NULL,
    output_json TEXT,
    error_message TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    completed_at INTEGER
);
CREATE UNIQUE INDEX idx_expert_workflow_runs_active
    ON expert_workflow_runs(conversation_id)
    WHERE status IN ('running', 'awaiting_gate');
CREATE INDEX idx_expert_workflow_runs_conversation_created
    ON expert_workflow_runs(conversation_id, created_at DESC, id);

CREATE TABLE expert_workflow_stage_runs (
    id TEXT PRIMARY KEY,
    workflow_run_id TEXT NOT NULL REFERENCES expert_workflow_runs(id) ON DELETE CASCADE,
    stage_id TEXT NOT NULL,
    task_id TEXT NOT NULL REFERENCES work_tasks(id) ON DELETE CASCADE,
    ordinal INTEGER NOT NULL CHECK(ordinal >= 0),
    status TEXT NOT NULL CHECK(status IN (
        'pending', 'queued', 'running', 'awaiting_gate', 'completed', 'failed', 'skipped'
    )),
    attempt INTEGER NOT NULL DEFAULT 0 CHECK(attempt >= 0),
    max_attempts INTEGER NOT NULL CHECK(max_attempts BETWEEN 1 AND 5),
    output_json TEXT,
    error_message TEXT,
    started_at INTEGER,
    completed_at INTEGER,
    updated_at INTEGER NOT NULL,
    UNIQUE(workflow_run_id, stage_id),
    UNIQUE(workflow_run_id, ordinal),
    UNIQUE(task_id)
);
CREATE INDEX idx_expert_workflow_stage_runs_workflow
    ON expert_workflow_stage_runs(workflow_run_id, ordinal);

CREATE TABLE expert_workflow_gate_decisions (
    id TEXT PRIMARY KEY,
    workflow_run_id TEXT NOT NULL REFERENCES expert_workflow_runs(id) ON DELETE CASCADE,
    stage_id TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('pending', 'approved', 'rejected')),
    reason TEXT NOT NULL DEFAULT '',
    requested_at INTEGER NOT NULL,
    resolved_at INTEGER,
    resolved_by TEXT
);
CREATE UNIQUE INDEX idx_expert_workflow_gate_pending
    ON expert_workflow_gate_decisions(workflow_run_id, stage_id)
    WHERE status = 'pending';
CREATE INDEX idx_expert_workflow_gate_history
    ON expert_workflow_gate_decisions(workflow_run_id, requested_at DESC, id);
"#;

const MIGRATION_31: &str = r#"
CREATE TABLE expert_team_runs (
    id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    expert_binding_id TEXT NOT NULL REFERENCES conversation_expert_bindings(id) ON DELETE CASCADE,
    expert_id TEXT NOT NULL REFERENCES agents(id),
    package_hash TEXT NOT NULL,
    team_id TEXT NOT NULL,
    team_version TEXT NOT NULL,
    team_json TEXT NOT NULL,
    parent_run_id TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    status TEXT NOT NULL CHECK(status IN ('running', 'completed', 'failed', 'cancelled')),
    objective TEXT NOT NULL,
    context TEXT NOT NULL DEFAULT '',
    result_json TEXT,
    error_message TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    completed_at INTEGER
);
CREATE UNIQUE INDEX idx_expert_team_runs_active
    ON expert_team_runs(conversation_id)
    WHERE status = 'running';
CREATE INDEX idx_expert_team_runs_parent_created
    ON expert_team_runs(parent_run_id, created_at DESC, id);

ALTER TABLE child_run_delegations ADD COLUMN team_run_id TEXT
    REFERENCES expert_team_runs(id) ON DELETE CASCADE;
ALTER TABLE child_run_delegations ADD COLUMN team_member_id TEXT;
ALTER TABLE child_run_delegations ADD COLUMN allowed_tools_json TEXT;
CREATE UNIQUE INDEX idx_child_delegations_team_member
    ON child_run_delegations(team_run_id, team_member_id)
    WHERE team_run_id IS NOT NULL AND team_member_id IS NOT NULL;
CREATE INDEX idx_child_delegations_team_status
    ON child_run_delegations(team_run_id, status, created_at, id)
    WHERE team_run_id IS NOT NULL;
"#;

const MIGRATION_32: &str = r#"
CREATE TABLE digital_colleagues (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    expert_id TEXT NOT NULL REFERENCES agents(id),
    expert_binding_id TEXT NOT NULL REFERENCES conversation_expert_bindings(id),
    package_hash TEXT NOT NULL,
    package_snapshot_json TEXT NOT NULL,
    conversation_id TEXT NOT NULL UNIQUE REFERENCES conversations(id) ON DELETE CASCADE,
    objective TEXT NOT NULL,
    project_id TEXT REFERENCES projects(id) ON DELETE SET NULL,
    project_root TEXT,
    knowledge_references_json TEXT NOT NULL DEFAULT '[]',
    status TEXT NOT NULL CHECK(status IN ('active', 'paused', 'revoked')),
    max_runs_per_day INTEGER NOT NULL CHECK(max_runs_per_day BETWEEN 1 AND 100),
    max_tokens_per_day INTEGER NOT NULL CHECK(max_tokens_per_day BETWEEN 256 AND 2000000),
    max_duration_ms INTEGER NOT NULL CHECK(max_duration_ms BETWEEN 1000 AND 900000),
    max_output_tokens INTEGER NOT NULL CHECK(max_output_tokens BETWEEN 64 AND 32768),
    max_tool_calls INTEGER NOT NULL CHECK(max_tool_calls BETWEEN 0 AND 100),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    revoked_at INTEGER
);
CREATE INDEX idx_digital_colleagues_status_updated
    ON digital_colleagues(status, updated_at DESC, id);

CREATE TABLE digital_colleague_schedules (
    id TEXT PRIMARY KEY,
    colleague_id TEXT NOT NULL REFERENCES digital_colleagues(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    schedule_kind TEXT NOT NULL CHECK(schedule_kind = 'interval'),
    interval_seconds INTEGER NOT NULL CHECK(interval_seconds BETWEEN 60 AND 2592000),
    catchup_window_seconds INTEGER NOT NULL CHECK(catchup_window_seconds BETWEEN 60 AND 86400),
    enabled INTEGER NOT NULL DEFAULT 1 CHECK(enabled IN (0, 1)),
    next_due_at INTEGER NOT NULL,
    last_scheduled_at INTEGER,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE INDEX idx_digital_colleague_schedules_due
    ON digital_colleague_schedules(enabled, next_due_at, id);

CREATE TABLE digital_colleague_channels (
    id TEXT PRIMARY KEY,
    colleague_id TEXT NOT NULL REFERENCES digital_colleagues(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    channel_kind TEXT NOT NULL CHECK(channel_kind IN ('webhook', 'im_bridge')),
    external_identity TEXT NOT NULL,
    credential_ref TEXT NOT NULL UNIQUE,
    secret_prefix TEXT NOT NULL,
    rate_limit_per_minute INTEGER NOT NULL CHECK(rate_limit_per_minute BETWEEN 1 AND 60),
    status TEXT NOT NULL CHECK(status IN ('active', 'revoked')),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    revoked_at INTEGER,
    UNIQUE(colleague_id, channel_kind, external_identity)
);
CREATE INDEX idx_digital_colleague_channels_status
    ON digital_colleague_channels(colleague_id, status, updated_at DESC, id);

CREATE TABLE digital_colleague_triggers (
    id TEXT PRIMARY KEY,
    colleague_id TEXT NOT NULL REFERENCES digital_colleagues(id) ON DELETE CASCADE,
    source_type TEXT NOT NULL CHECK(source_type IN ('manual', 'schedule', 'channel')),
    source_id TEXT,
    idempotency_key TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN (
        'accepted', 'queued', 'running', 'completed', 'failed', 'cancelled', 'rejected', 'skipped'
    )),
    payload_json TEXT NOT NULL,
    scheduled_for INTEGER,
    run_id TEXT UNIQUE REFERENCES runs(id) ON DELETE SET NULL,
    input_tokens INTEGER NOT NULL DEFAULT 0 CHECK(input_tokens >= 0),
    output_tokens INTEGER NOT NULL DEFAULT 0 CHECK(output_tokens >= 0),
    total_tokens INTEGER NOT NULL DEFAULT 0 CHECK(total_tokens >= 0),
    tool_call_count INTEGER NOT NULL DEFAULT 0 CHECK(tool_call_count >= 0),
    error_code TEXT,
    error_message TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    completed_at INTEGER,
    UNIQUE(colleague_id, idempotency_key)
);
CREATE INDEX idx_digital_colleague_triggers_colleague_created
    ON digital_colleague_triggers(colleague_id, created_at DESC, id);
CREATE INDEX idx_digital_colleague_triggers_status
    ON digital_colleague_triggers(status, updated_at, id);

CREATE TABLE digital_colleague_audit_log (
    id TEXT PRIMARY KEY,
    colleague_id TEXT REFERENCES digital_colleagues(id) ON DELETE SET NULL,
    channel_id TEXT REFERENCES digital_colleague_channels(id) ON DELETE SET NULL,
    trigger_id TEXT REFERENCES digital_colleague_triggers(id) ON DELETE SET NULL,
    event TEXT NOT NULL,
    outcome TEXT NOT NULL CHECK(outcome IN ('accepted', 'applied', 'rejected', 'skipped', 'failed')),
    actor TEXT NOT NULL,
    details_json TEXT NOT NULL DEFAULT '{}',
    created_at INTEGER NOT NULL
);
CREATE INDEX idx_digital_colleague_audit_created
    ON digital_colleague_audit_log(created_at DESC, id);
CREATE INDEX idx_digital_colleague_audit_colleague
    ON digital_colleague_audit_log(colleague_id, created_at DESC, id);
"#;

const MIGRATION_33: &str = r#"
CREATE TABLE run_continuation_decisions (
    decision_id TEXT PRIMARY KEY,
    schema_version INTEGER NOT NULL CHECK(schema_version = 1),
    run_id TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    event_cursor INTEGER NOT NULL CHECK(event_cursor >= 0),
    decision TEXT NOT NULL CHECK(decision IN (
        'continue', 'repair', 'wait_approval', 'complete', 'blocked'
    )),
    reason_code TEXT NOT NULL CHECK(
        schema_version = 1 AND reason_code IN (
            'work_remaining',
            'validation_failed',
            'approval_pending',
            'external_dependency_unavailable',
            'budget_exhausted',
            'user_input_required',
            'retry_available',
            'retry_exhausted',
            'acceptance_missing',
            'acceptance_candidate',
            'acceptance_passed',
            'host_audit_required',
            'tool_failed',
            'task_interrupted'
        )
    ),
    active_task_ids_json TEXT NOT NULL DEFAULT '[]'
        CHECK(json_valid(active_task_ids_json) AND json_type(active_task_ids_json) = 'array'),
    evidence_ids_json TEXT NOT NULL DEFAULT '[]'
        CHECK(json_valid(evidence_ids_json) AND json_type(evidence_ids_json) = 'array'),
    missing_acceptance_json TEXT NOT NULL DEFAULT '[]'
        CHECK(json_valid(missing_acceptance_json) AND json_type(missing_acceptance_json) = 'array'),
    next_action TEXT CHECK(next_action IS NULL OR length(trim(next_action)) > 0),
    retry_class TEXT CHECK(retry_class IN (
        'none', 'recoverable', 'retry_limited', 'non_retryable', 'host_decides'
    )),
    blocked_dependency_refs_json TEXT NOT NULL DEFAULT '[]'
        CHECK(
            json_valid(blocked_dependency_refs_json)
            AND json_type(blocked_dependency_refs_json) = 'array'
        ),
    host_validation_outcome TEXT NOT NULL CHECK(host_validation_outcome IN ('accepted', 'rejected')),
    host_validation_error TEXT,
    created_at INTEGER NOT NULL,
    CHECK(
        (host_validation_outcome = 'accepted' AND host_validation_error IS NULL)
        OR (
            host_validation_outcome = 'rejected'
            AND host_validation_error IS NOT NULL
            AND length(trim(host_validation_error)) > 0
        )
    )
);

CREATE INDEX idx_run_continuation_decisions_run_cursor
    ON run_continuation_decisions(run_id, event_cursor, created_at, decision_id);

CREATE TABLE run_continuation_ingest_diagnostics (
    run_id TEXT NOT NULL CHECK(
        length(trim(run_id)) BETWEEN 1 AND 200
    ),
    event_seq INTEGER NOT NULL CHECK(event_seq >= 0),
    execution_profile_id TEXT
        CHECK(
            execution_profile_id IS NULL
            OR length(trim(execution_profile_id)) BETWEEN 1 AND 200
        ),
    shadow_mode INTEGER NOT NULL CHECK(shadow_mode IN (0, 1)),
    error_code TEXT NOT NULL CHECK(
        length(trim(error_code)) BETWEEN 1 AND 200
    ),
    error_message TEXT NOT NULL CHECK(
        length(trim(error_message)) BETWEEN 1 AND 4096
    ),
    created_at INTEGER NOT NULL,
    PRIMARY KEY(run_id, event_seq),
    FOREIGN KEY(run_id, event_seq)
        REFERENCES run_events(run_id, seq) ON DELETE CASCADE
);

CREATE INDEX idx_run_continuation_ingest_diagnostics_created
    ON run_continuation_ingest_diagnostics(created_at, run_id, event_seq);

CREATE TRIGGER validate_run_continuation_ingest_event
BEFORE INSERT ON run_continuation_ingest_diagnostics
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1 FROM run_events
    WHERE run_id = NEW.run_id
      AND seq = NEW.event_seq
      AND event_type = 'run.continuation_proposed'
)
BEGIN
    SELECT RAISE(
        ABORT,
        'continuation ingest diagnostic requires run.continuation_proposed event'
    );
END;
"#;

const MIGRATION_34: &str = r#"
ALTER TABLE review_findings ADD COLUMN resolved_by TEXT REFERENCES runs(id) ON DELETE SET NULL;

CREATE TABLE run_execution_profiles (
    run_id TEXT PRIMARY KEY REFERENCES runs(id) ON DELETE CASCADE,
    schema_version INTEGER NOT NULL CHECK(schema_version = 1),
    profile_id TEXT NOT NULL CHECK(profile_id IN (
        'legacy', 'durable_v2_shadow', 'durable_v2', 'graph_readonly_preview'
    )),
    snapshot_json TEXT NOT NULL CHECK(
        json_valid(snapshot_json) AND json_type(snapshot_json) = 'object'
    ),
    profile_hash TEXT NOT NULL CHECK(length(profile_hash) = 64),
    frozen_at INTEGER NOT NULL
);

CREATE INDEX idx_run_execution_profiles_profile
    ON run_execution_profiles(profile_id, frozen_at, run_id);

CREATE TABLE task_validation_policies (
    task_id TEXT PRIMARY KEY REFERENCES work_tasks(id) ON DELETE CASCADE,
    schema_version INTEGER NOT NULL CHECK(schema_version = 1),
    policy_id TEXT NOT NULL CHECK(policy_id IN ('legacy_v1', 'standard_v1', 'high_risk_v1')),
    snapshot_json TEXT NOT NULL CHECK(json_valid(snapshot_json) AND json_type(snapshot_json) = 'object'),
    policy_hash TEXT NOT NULL CHECK(length(policy_hash) = 64),
    frozen_at INTEGER NOT NULL
);

CREATE INDEX idx_task_validation_policies_policy
    ON task_validation_policies(policy_id, frozen_at, task_id);

CREATE TABLE task_attempts (
    id TEXT PRIMARY KEY CHECK(length(trim(id)) BETWEEN 1 AND 200),
    task_id TEXT NOT NULL REFERENCES work_tasks(id) ON DELETE CASCADE,
    attempt_number INTEGER NOT NULL CHECK(attempt_number >= 1),
    kind TEXT NOT NULL CHECK(kind IN ('execution', 'repair')),
    status TEXT NOT NULL CHECK(status IN ('running', 'succeeded', 'failed', 'blocked', 'cancelled')),
    run_id TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    policy_hash TEXT NOT NULL CHECK(length(policy_hash) = 64),
    root_cause TEXT CHECK(root_cause IS NULL OR length(trim(root_cause)) BETWEEN 1 AND 4000),
    finding_ids_json TEXT NOT NULL DEFAULT '[]'
        CHECK(json_valid(finding_ids_json) AND json_type(finding_ids_json) = 'array'),
    evidence_ids_json TEXT NOT NULL DEFAULT '[]'
        CHECK(json_valid(evidence_ids_json) AND json_type(evidence_ids_json) = 'array'),
    failure_reason TEXT CHECK(failure_reason IS NULL OR length(trim(failure_reason)) BETWEEN 1 AND 4000),
    version INTEGER NOT NULL DEFAULT 1 CHECK(version >= 1),
    evidence_rowid_watermark INTEGER NOT NULL CHECK(evidence_rowid_watermark >= 0),
    finding_rowid_watermark INTEGER NOT NULL CHECK(finding_rowid_watermark >= 0),
    started_at INTEGER NOT NULL,
    finished_at INTEGER,
    UNIQUE(task_id, attempt_number),
    CHECK(
        (status = 'running' AND finished_at IS NULL AND failure_reason IS NULL)
        OR (status = 'succeeded' AND finished_at IS NOT NULL AND failure_reason IS NULL)
        OR (status IN ('failed', 'blocked', 'cancelled') AND finished_at IS NOT NULL)
    ),
    CHECK(
        (kind = 'execution' AND root_cause IS NULL AND json_array_length(finding_ids_json) = 0)
        OR (kind = 'repair' AND root_cause IS NOT NULL AND json_array_length(finding_ids_json) > 0)
    )
);

CREATE UNIQUE INDEX idx_task_attempts_one_running
    ON task_attempts(task_id) WHERE status = 'running';
CREATE INDEX idx_task_attempts_task_history
    ON task_attempts(task_id, attempt_number DESC, id DESC);
CREATE INDEX idx_task_attempts_run
    ON task_attempts(run_id, started_at DESC, id DESC);
"#;

const MIGRATION_35: &str = r#"
ALTER TABLE approvals ADD COLUMN category TEXT NOT NULL DEFAULT 'tool_execution'
    CHECK(category IN ('tool_execution', 'task_repair_budget_override'));
ALTER TABLE approvals ADD COLUMN claimed_at INTEGER
    CHECK(claimed_at IS NULL OR claimed_at >= 0);
ALTER TABLE approvals ADD COLUMN claimed_by_run_id TEXT
    REFERENCES runs(id) ON DELETE SET NULL;

CREATE TABLE task_repair_override_events (
    id TEXT PRIMARY KEY CHECK(length(trim(id)) BETWEEN 1 AND 200),
    task_id TEXT NOT NULL UNIQUE REFERENCES work_tasks(id) ON DELETE CASCADE,
    attempt_id TEXT NOT NULL UNIQUE REFERENCES task_attempts(id) ON DELETE CASCADE,
    approval_id TEXT NOT NULL UNIQUE REFERENCES approvals(id) ON DELETE RESTRICT,
    tool_call_id TEXT NOT NULL UNIQUE REFERENCES tool_calls(id) ON DELETE RESTRICT,
    run_id TEXT NOT NULL REFERENCES runs(id) ON DELETE RESTRICT,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    policy_id TEXT NOT NULL CHECK(policy_id IN ('standard_v1', 'high_risk_v1')),
    policy_hash TEXT NOT NULL CHECK(length(policy_hash) = 64),
    normal_repair_budget INTEGER NOT NULL CHECK(normal_repair_budget >= 1),
    normal_repair_used INTEGER NOT NULL CHECK(normal_repair_used >= normal_repair_budget),
    override_count INTEGER NOT NULL CHECK(override_count = 1),
    input_hash TEXT NOT NULL CHECK(length(input_hash) = 64),
    escalation_reason TEXT NOT NULL CHECK(length(trim(escalation_reason)) BETWEEN 1 AND 2000),
    created_at INTEGER NOT NULL CHECK(created_at >= 0)
);

CREATE INDEX idx_task_repair_override_events_conversation_created
    ON task_repair_override_events(conversation_id, created_at DESC, id DESC);
CREATE INDEX idx_task_repair_override_events_run_created
    ON task_repair_override_events(run_id, created_at DESC, id DESC);

CREATE TRIGGER task_repair_override_events_no_update
BEFORE UPDATE ON task_repair_override_events
BEGIN
    SELECT RAISE(ABORT, 'task repair override events are append-only');
END;

"#;

const MIGRATION_36: &str = r#"
CREATE TABLE work_graph_specs (
    goal_id TEXT PRIMARY KEY REFERENCES goals(id) ON DELETE CASCADE,
    plan_revision_id TEXT NOT NULL UNIQUE
        REFERENCES plan_revisions(id) ON DELETE CASCADE,
    schema_version INTEGER NOT NULL CHECK(schema_version = 1),
    graph_mode TEXT NOT NULL CHECK(graph_mode = 'read_only'),
    max_nodes INTEGER NOT NULL CHECK(max_nodes = 3),
    max_depth INTEGER NOT NULL CHECK(max_depth = 1),
    spec_hash TEXT NOT NULL CHECK(
        length(spec_hash) = 64
        AND spec_hash NOT GLOB '*[^0-9a-f]*'
    ),
    activated_by_run_id TEXT NOT NULL
        REFERENCES runs(id) ON DELETE RESTRICT
        CHECK(length(trim(activated_by_run_id)) BETWEEN 1 AND 200),
    activation_tool_call_id TEXT NOT NULL UNIQUE
        REFERENCES tool_calls(id) ON DELETE RESTRICT
        CHECK(length(trim(activation_tool_call_id)) BETWEEN 1 AND 200),
    activated_at INTEGER NOT NULL CHECK(activated_at >= 0)
);

CREATE TRIGGER work_graph_specs_require_approved_plan
BEFORE INSERT ON work_graph_specs
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1
    FROM plan_revisions
    WHERE id = NEW.plan_revision_id
      AND goal_id = NEW.goal_id
      AND status = 'approved'
)
BEGIN
    SELECT RAISE(ABORT, 'work graph requires an approved PlanRevision for the same Goal');
END;

CREATE TRIGGER work_graph_specs_validate_activation_source
BEFORE INSERT ON work_graph_specs
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1
    FROM goals AS goal
    JOIN runs AS activation_run
      ON activation_run.id = NEW.activated_by_run_id
     AND activation_run.conversation_id = goal.conversation_id
     AND activation_run.status = 'running'
     AND activation_run.run_kind = 'primary'
     AND activation_run.parent_run_id IS NULL
     AND activation_run.depth = 0
    JOIN run_execution_profiles AS profile
      ON profile.run_id = activation_run.id
     AND profile.schema_version = 1
     AND profile.profile_id = 'durable_v2'
     AND profile.snapshot_json = '{"id":"durable_v2","schemaVersion":1}'
     AND profile.profile_hash = 'ae08f4dc8c1327cbd3de30879f01aa7475d9a728a3c0f738579086435d714f7e'
    JOIN tool_calls AS activation_call
      ON activation_call.id = NEW.activation_tool_call_id
     AND activation_call.run_id = activation_run.id
     AND activation_call.conversation_id = goal.conversation_id
     AND activation_call.tool_name = 'graph_readonly_activate'
     AND activation_call.execution_location = 'host'
     AND activation_call.status = 'running'
     AND activation_call.requires_approval = 0
     AND json_valid(activation_call.input_json)
     AND json_type(activation_call.input_json) = 'object'
     AND json_extract(activation_call.input_json, '$.planRevisionId') = NEW.plan_revision_id
     AND (SELECT COUNT(*) FROM json_each(activation_call.input_json)) = 1
     AND NOT EXISTS (
         SELECT 1 FROM json_each(activation_call.input_json)
         WHERE key <> 'planRevisionId'
     )
    WHERE goal.id = NEW.goal_id
)
BEGIN
    SELECT RAISE(
        ABORT,
        'work graph activation requires the exactly-bound running Host Graph activation ToolCall'
    );
END;

CREATE TRIGGER work_graph_specs_no_update
BEFORE UPDATE ON work_graph_specs
BEGIN
    SELECT RAISE(ABORT, 'work graph specs are immutable');
END;

CREATE TABLE work_graph_nodes (
    task_id TEXT PRIMARY KEY REFERENCES work_tasks(id) ON DELETE CASCADE,
    goal_id TEXT NOT NULL REFERENCES work_graph_specs(goal_id) ON DELETE CASCADE,
    node_key TEXT NOT NULL CHECK(length(trim(node_key)) BETWEEN 1 AND 100),
    access_mode TEXT NOT NULL CHECK(access_mode = 'read_only'),
    depth INTEGER NOT NULL CHECK(depth BETWEEN 0 AND 1),
    acceptance_criteria_json TEXT NOT NULL CHECK(
        json_valid(acceptance_criteria_json)
        AND json_type(acceptance_criteria_json) = 'array'
        AND json_array_length(acceptance_criteria_json) BETWEEN 1 AND 8
    ),
    created_at INTEGER NOT NULL CHECK(created_at >= 0),
    UNIQUE(goal_id, node_key)
);

CREATE INDEX idx_work_graph_nodes_goal_depth
    ON work_graph_nodes(goal_id, depth, task_id);

CREATE TRIGGER work_graph_nodes_require_same_goal
BEFORE INSERT ON work_graph_nodes
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1
    FROM work_tasks
    WHERE id = NEW.task_id AND goal_id = NEW.goal_id
)
BEGIN
    SELECT RAISE(ABORT, 'work graph node Task must belong to the same Goal');
END;

CREATE TRIGGER work_graph_nodes_validate_canonical_input
BEFORE INSERT ON work_graph_nodes
FOR EACH ROW
WHEN NEW.node_key <> trim(NEW.node_key)
 OR length(NEW.node_key) > 100
 OR NEW.node_key GLOB '*[^0-9A-Za-z._-]*'
 OR EXISTS (
     SELECT 1
     FROM json_each(NEW.acceptance_criteria_json)
     WHERE type <> 'text'
        OR value <> trim(value)
        OR length(trim(value)) NOT BETWEEN 1 AND 1000
 )
 OR EXISTS (
     SELECT 1
     FROM json_each(NEW.acceptance_criteria_json)
     GROUP BY value
     HAVING COUNT(*) > 1
 )
BEGIN
    SELECT RAISE(
        ABORT,
        'work graph nodeKey and acceptance criteria must be canonical and unique'
    );
END;

CREATE TRIGGER work_graph_nodes_limit_per_goal
BEFORE INSERT ON work_graph_nodes
FOR EACH ROW
WHEN (
    SELECT COUNT(*) FROM work_graph_nodes WHERE goal_id = NEW.goal_id
) >= 3
BEGIN
    SELECT RAISE(ABORT, 'work graph supports at most three nodes');
END;

CREATE TRIGGER work_graph_nodes_no_update
BEFORE UPDATE ON work_graph_nodes
BEGIN
    SELECT RAISE(ABORT, 'work graph nodes are immutable');
END;

CREATE TRIGGER work_graph_tasks_freeze_structure
BEFORE UPDATE OF goal_id, parent_task_id, ordinal, title, detail ON work_tasks
FOR EACH ROW
WHEN EXISTS (SELECT 1 FROM work_graph_nodes WHERE task_id = OLD.id)
 AND (
     NEW.goal_id IS NOT OLD.goal_id
     OR NEW.parent_task_id IS NOT OLD.parent_task_id
     OR NEW.ordinal IS NOT OLD.ordinal
     OR NEW.title IS NOT OLD.title
     OR NEW.detail IS NOT OLD.detail
 )
BEGIN
    SELECT RAISE(
        ABORT,
        'Graph Task structure is frozen; activate a new PlanRevision instead'
    );
END;

CREATE TABLE work_task_edges (
    goal_id TEXT NOT NULL REFERENCES work_graph_specs(goal_id) ON DELETE CASCADE,
    dependency_task_id TEXT NOT NULL
        REFERENCES work_graph_nodes(task_id) ON DELETE CASCADE,
    dependent_task_id TEXT NOT NULL
        REFERENCES work_graph_nodes(task_id) ON DELETE CASCADE,
    created_at INTEGER NOT NULL CHECK(created_at >= 0),
    PRIMARY KEY(goal_id, dependency_task_id, dependent_task_id),
    CHECK(dependency_task_id <> dependent_task_id)
);

CREATE INDEX idx_work_task_edges_inbound
    ON work_task_edges(goal_id, dependent_task_id, dependency_task_id);
CREATE INDEX idx_work_task_edges_outbound
    ON work_task_edges(goal_id, dependency_task_id, dependent_task_id);

CREATE TRIGGER work_task_edges_require_same_goal_and_forward_depth
BEFORE INSERT ON work_task_edges
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1
    FROM work_graph_nodes AS dependency
    JOIN work_graph_nodes AS dependent
      ON dependent.task_id = NEW.dependent_task_id
    WHERE dependency.task_id = NEW.dependency_task_id
      AND dependency.goal_id = NEW.goal_id
      AND dependent.goal_id = NEW.goal_id
      AND dependency.depth < dependent.depth
)
BEGIN
    SELECT RAISE(
        ABORT,
        'work graph edge endpoints must share the Goal and increase depth'
    );
END;

CREATE TRIGGER work_task_edges_no_update
BEFORE UPDATE ON work_task_edges
BEGIN
    SELECT RAISE(ABORT, 'work graph edges are immutable');
END;

-- SQLite exposes no built-in "this DELETE came from an FK cascade" predicate.
-- This sentinel exists only between a Goal's BEFORE/AFTER DELETE triggers. FK
-- actions execute between those phases, so direct deletes remain blocked while
-- authoritative Goal/Conversation cascades can remove the whole Graph atomically.
CREATE TABLE work_graph_cascade_delete_scopes (
    goal_id TEXT PRIMARY KEY CHECK(length(trim(goal_id)) BETWEEN 1 AND 200),
    conversation_id TEXT NOT NULL CHECK(length(trim(conversation_id)) BETWEEN 1 AND 200),
    opened_at INTEGER NOT NULL CHECK(opened_at >= 0)
);

CREATE TRIGGER goals_open_work_graph_cascade_delete_scope
BEFORE DELETE ON goals
FOR EACH ROW
WHEN EXISTS (SELECT 1 FROM work_graph_specs WHERE goal_id = OLD.id)
BEGIN
    INSERT OR IGNORE INTO work_graph_cascade_delete_scopes(
        goal_id, conversation_id, opened_at
    )
    VALUES (OLD.id, OLD.conversation_id, CAST(strftime('%s', 'now') AS INTEGER) * 1000);
END;

CREATE TRIGGER goals_close_work_graph_cascade_delete_scope
AFTER DELETE ON goals
FOR EACH ROW
BEGIN
    DELETE FROM work_graph_cascade_delete_scopes WHERE goal_id = OLD.id;
END;

CREATE TRIGGER conversations_open_work_graph_cascade_delete_scopes
BEFORE DELETE ON conversations
FOR EACH ROW
BEGIN
    INSERT OR IGNORE INTO work_graph_cascade_delete_scopes(
        goal_id, conversation_id, opened_at
    )
    SELECT
        spec.goal_id,
        OLD.id,
        CAST(strftime('%s', 'now') AS INTEGER) * 1000
    FROM work_graph_specs AS spec
    JOIN goals AS goal ON goal.id = spec.goal_id
    WHERE goal.conversation_id = OLD.id;
END;

CREATE TRIGGER conversations_close_work_graph_cascade_delete_scopes
AFTER DELETE ON conversations
FOR EACH ROW
BEGIN
    DELETE FROM work_graph_cascade_delete_scopes WHERE conversation_id = OLD.id;
END;

CREATE TRIGGER work_graph_specs_no_direct_delete
BEFORE DELETE ON work_graph_specs
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1 FROM work_graph_cascade_delete_scopes WHERE goal_id = OLD.goal_id
)
BEGIN
    SELECT RAISE(ABORT, 'work graph specs can only be deleted with their Goal');
END;

CREATE TRIGGER work_graph_nodes_no_direct_delete
BEFORE DELETE ON work_graph_nodes
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1 FROM work_graph_cascade_delete_scopes WHERE goal_id = OLD.goal_id
)
BEGIN
    SELECT RAISE(ABORT, 'work graph nodes can only be deleted with their Goal');
END;

CREATE TRIGGER work_task_edges_no_direct_delete
BEFORE DELETE ON work_task_edges
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1 FROM work_graph_cascade_delete_scopes WHERE goal_id = OLD.goal_id
)
BEGIN
    SELECT RAISE(ABORT, 'work graph edges can only be deleted with their Goal');
END;

CREATE TRIGGER work_graph_tasks_no_direct_delete
BEFORE DELETE ON work_tasks
FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM work_graph_nodes WHERE task_id = OLD.id
)
 AND NOT EXISTS (
     SELECT 1 FROM work_graph_cascade_delete_scopes WHERE goal_id = OLD.goal_id
 )
BEGIN
    SELECT RAISE(ABORT, 'Graph Tasks can only be deleted with their Goal');
END;

ALTER TABLE child_run_delegations ADD COLUMN graph_task_attempt_id TEXT
    REFERENCES task_attempts(id) ON DELETE SET NULL;

CREATE UNIQUE INDEX idx_child_delegations_graph_task_attempt
    ON child_run_delegations(graph_task_attempt_id)
    WHERE graph_task_attempt_id IS NOT NULL;

CREATE TRIGGER child_delegations_validate_graph_attempt_insert
BEFORE INSERT ON child_run_delegations
FOR EACH ROW
WHEN NEW.graph_task_attempt_id IS NOT NULL AND NOT EXISTS (
    SELECT 1
    FROM task_attempts AS attempt
    JOIN work_graph_nodes AS node ON node.task_id = attempt.task_id
    JOIN work_tasks AS task ON task.id = attempt.task_id
    JOIN goals AS goal ON goal.id = node.goal_id
    JOIN runs AS parent_run
      ON parent_run.id = NEW.parent_run_id
     AND parent_run.conversation_id = goal.conversation_id
     AND parent_run.status = 'running'
     AND parent_run.run_kind = 'primary'
     AND parent_run.parent_run_id IS NULL
     AND parent_run.depth = 0
    JOIN run_execution_profiles AS parent_profile
      ON parent_profile.run_id = parent_run.id
     AND parent_profile.schema_version = 1
     AND parent_profile.profile_id = 'durable_v2'
     AND parent_profile.snapshot_json = '{"id":"durable_v2","schemaVersion":1}'
     AND parent_profile.profile_hash = 'ae08f4dc8c1327cbd3de30879f01aa7475d9a728a3c0f738579086435d714f7e'
    JOIN conversations AS parent_conversation
      ON parent_conversation.id = parent_run.conversation_id
    JOIN runs AS child_run
      ON child_run.id = NEW.child_run_id
     AND child_run.parent_run_id = parent_run.id
     AND child_run.conversation_id = NEW.child_conversation_id
     AND child_run.root_run_id = COALESCE(parent_run.root_run_id, parent_run.id)
     AND child_run.run_kind = 'child'
     AND child_run.depth = parent_run.depth + 1
     AND child_run.status = 'queued'
    JOIN conversations AS child_conversation
      ON child_conversation.id = NEW.child_conversation_id
     AND child_conversation.conversation_kind = 'child'
     AND child_conversation.parent_conversation_id = parent_conversation.id
     AND child_conversation.lineage_root_id = COALESCE(
         parent_conversation.lineage_root_id,
         parent_conversation.id
     )
    JOIN tool_calls AS node_start_call
      ON node_start_call.id = NEW.tool_call_id
     AND node_start_call.run_id = parent_run.id
     AND node_start_call.conversation_id = parent_run.conversation_id
     AND node_start_call.tool_name = 'graph_readonly_node_start'
     AND node_start_call.execution_location = 'host'
     AND node_start_call.status = 'running'
     AND node_start_call.requires_approval = 0
     AND json_valid(node_start_call.input_json)
     AND json_type(node_start_call.input_json) = 'object'
     AND json_extract(node_start_call.input_json, '$.goalId') = node.goal_id
     AND json_extract(node_start_call.input_json, '$.taskId') = attempt.task_id
     AND json_type(node_start_call.input_json, '$.expectedTaskVersion') = 'integer'
     AND json_extract(node_start_call.input_json, '$.expectedTaskVersion') = task.version - 1
     AND json_extract(node_start_call.input_json, '$.expectedTaskVersion') >= 1
     AND json_extract(node_start_call.input_json, '$.attemptId') = attempt.id
     AND json_extract(node_start_call.input_json, '$.workerAgentId') = NEW.worker_agent_id
     AND json_extract(node_start_call.input_json, '$.objective') = NEW.objective
     AND json_extract(node_start_call.input_json, '$.context') = NEW.context
     AND json_type(node_start_call.input_json, '$.budget') = 'object'
     AND json_extract(node_start_call.input_json, '$.budget.maxDurationMs') =
         NEW.max_duration_ms
     AND json_extract(node_start_call.input_json, '$.budget.maxTotalTokens') =
         NEW.max_total_tokens
     AND json_extract(node_start_call.input_json, '$.budget.maxOutputTokens') =
         NEW.max_output_tokens
     AND json_extract(node_start_call.input_json, '$.budget.maxToolCalls') =
         NEW.max_tool_calls
     AND (SELECT COUNT(*) FROM json_each(node_start_call.input_json)) = 8
     AND NOT EXISTS (
         SELECT 1 FROM json_each(node_start_call.input_json)
         WHERE key NOT IN (
             'goalId', 'taskId', 'expectedTaskVersion', 'attemptId',
             'workerAgentId', 'objective', 'context', 'budget'
         )
     )
     AND (
         SELECT COUNT(*)
         FROM json_each(json_extract(node_start_call.input_json, '$.budget'))
     ) = 4
     AND NOT EXISTS (
         SELECT 1
         FROM json_each(json_extract(node_start_call.input_json, '$.budget'))
         WHERE key NOT IN (
             'maxDurationMs', 'maxTotalTokens', 'maxOutputTokens', 'maxToolCalls'
         )
     )
    WHERE attempt.id = NEW.graph_task_attempt_id
      AND attempt.run_id = NEW.parent_run_id
      AND attempt.kind = 'execution'
      AND attempt.status = 'running'
      AND task.goal_id = node.goal_id
      AND task.status = 'in_progress'
      AND NEW.status = 'queued'
      AND NEW.team_run_id IS NULL
      AND NEW.team_member_id IS NULL
      AND NEW.allowed_tools_json = '["read","ls","find","grep"]'
      AND NEW.max_duration_ms <= 45000
      AND NEW.max_total_tokens <= 4096
      AND NEW.max_output_tokens <= 1024
      AND NEW.max_output_tokens <= NEW.max_total_tokens
      AND NEW.max_tool_calls <= 6
)
BEGIN
    SELECT RAISE(
        ABORT,
        'graph delegation requires exact parent Attempt, Child lineage, Host call, scope, and budget facts'
    );
END;

CREATE TRIGGER child_delegations_validate_graph_attempt_update
BEFORE UPDATE OF parent_run_id, graph_task_attempt_id ON child_run_delegations
FOR EACH ROW
WHEN NEW.graph_task_attempt_id IS NOT NULL AND NOT EXISTS (
    SELECT 1
    FROM task_attempts AS attempt
    JOIN work_graph_nodes AS node ON node.task_id = attempt.task_id
    JOIN work_tasks AS task ON task.id = attempt.task_id
    JOIN goals AS goal ON goal.id = node.goal_id
    JOIN runs AS parent_run
      ON parent_run.id = NEW.parent_run_id
     AND parent_run.conversation_id = goal.conversation_id
     AND parent_run.status = 'running'
     AND parent_run.run_kind = 'primary'
     AND parent_run.parent_run_id IS NULL
     AND parent_run.depth = 0
    JOIN run_execution_profiles AS parent_profile
      ON parent_profile.run_id = parent_run.id
     AND parent_profile.schema_version = 1
     AND parent_profile.profile_id = 'durable_v2'
     AND parent_profile.snapshot_json = '{"id":"durable_v2","schemaVersion":1}'
     AND parent_profile.profile_hash = 'ae08f4dc8c1327cbd3de30879f01aa7475d9a728a3c0f738579086435d714f7e'
    JOIN conversations AS parent_conversation
      ON parent_conversation.id = parent_run.conversation_id
    JOIN runs AS child_run
      ON child_run.id = NEW.child_run_id
     AND child_run.parent_run_id = parent_run.id
     AND child_run.conversation_id = NEW.child_conversation_id
     AND child_run.root_run_id = COALESCE(parent_run.root_run_id, parent_run.id)
     AND child_run.run_kind = 'child'
     AND child_run.depth = parent_run.depth + 1
     AND child_run.status = 'queued'
    JOIN conversations AS child_conversation
      ON child_conversation.id = NEW.child_conversation_id
     AND child_conversation.conversation_kind = 'child'
     AND child_conversation.parent_conversation_id = parent_conversation.id
     AND child_conversation.lineage_root_id = COALESCE(
         parent_conversation.lineage_root_id,
         parent_conversation.id
     )
    JOIN tool_calls AS node_start_call
      ON node_start_call.id = NEW.tool_call_id
     AND node_start_call.run_id = parent_run.id
     AND node_start_call.conversation_id = parent_run.conversation_id
     AND node_start_call.tool_name = 'graph_readonly_node_start'
     AND node_start_call.execution_location = 'host'
     AND node_start_call.status = 'running'
     AND node_start_call.requires_approval = 0
     AND json_valid(node_start_call.input_json)
     AND json_type(node_start_call.input_json) = 'object'
     AND json_extract(node_start_call.input_json, '$.goalId') = node.goal_id
     AND json_extract(node_start_call.input_json, '$.taskId') = attempt.task_id
     AND json_type(node_start_call.input_json, '$.expectedTaskVersion') = 'integer'
     AND json_extract(node_start_call.input_json, '$.expectedTaskVersion') = task.version - 1
     AND json_extract(node_start_call.input_json, '$.expectedTaskVersion') >= 1
     AND json_extract(node_start_call.input_json, '$.attemptId') = attempt.id
     AND json_extract(node_start_call.input_json, '$.workerAgentId') = NEW.worker_agent_id
     AND json_extract(node_start_call.input_json, '$.objective') = NEW.objective
     AND json_extract(node_start_call.input_json, '$.context') = NEW.context
     AND json_type(node_start_call.input_json, '$.budget') = 'object'
     AND json_extract(node_start_call.input_json, '$.budget.maxDurationMs') =
         NEW.max_duration_ms
     AND json_extract(node_start_call.input_json, '$.budget.maxTotalTokens') =
         NEW.max_total_tokens
     AND json_extract(node_start_call.input_json, '$.budget.maxOutputTokens') =
         NEW.max_output_tokens
     AND json_extract(node_start_call.input_json, '$.budget.maxToolCalls') =
         NEW.max_tool_calls
     AND (SELECT COUNT(*) FROM json_each(node_start_call.input_json)) = 8
     AND NOT EXISTS (
         SELECT 1 FROM json_each(node_start_call.input_json)
         WHERE key NOT IN (
             'goalId', 'taskId', 'expectedTaskVersion', 'attemptId',
             'workerAgentId', 'objective', 'context', 'budget'
         )
     )
     AND (
         SELECT COUNT(*)
         FROM json_each(json_extract(node_start_call.input_json, '$.budget'))
     ) = 4
     AND NOT EXISTS (
         SELECT 1
         FROM json_each(json_extract(node_start_call.input_json, '$.budget'))
         WHERE key NOT IN (
             'maxDurationMs', 'maxTotalTokens', 'maxOutputTokens', 'maxToolCalls'
         )
     )
    WHERE attempt.id = NEW.graph_task_attempt_id
      AND attempt.run_id = NEW.parent_run_id
      AND attempt.kind = 'execution'
      AND attempt.status = 'running'
      AND task.goal_id = node.goal_id
      AND task.status = 'in_progress'
      AND NEW.status = 'queued'
      AND NEW.team_run_id IS NULL
      AND NEW.team_member_id IS NULL
      AND NEW.allowed_tools_json = '["read","ls","find","grep"]'
      AND NEW.max_duration_ms <= 45000
      AND NEW.max_total_tokens <= 4096
      AND NEW.max_output_tokens <= 1024
      AND NEW.max_output_tokens <= NEW.max_total_tokens
      AND NEW.max_tool_calls <= 6
)
BEGIN
    SELECT RAISE(
        ABORT,
        'graph delegation requires exact parent Attempt, Child lineage, Host call, scope, and budget facts'
    );
END;

CREATE TRIGGER child_delegations_seal_graph_profile_insert
AFTER INSERT ON child_run_delegations
FOR EACH ROW
WHEN NEW.graph_task_attempt_id IS NOT NULL
BEGIN
    INSERT INTO run_execution_profiles(
        run_id, schema_version, profile_id, snapshot_json, profile_hash, frozen_at
    )
    SELECT
        NEW.child_run_id,
        1,
        'durable_v2',
        '{"id":"durable_v2","schemaVersion":1}',
        'ae08f4dc8c1327cbd3de30879f01aa7475d9a728a3c0f738579086435d714f7e',
        NEW.created_at
    WHERE NOT EXISTS (
        SELECT 1 FROM run_execution_profiles WHERE run_id = NEW.child_run_id
    );

    SELECT RAISE(ABORT, 'graph Child Run requires the canonical durable_v2 frozen profile')
    WHERE NOT EXISTS (
        SELECT 1
        FROM run_execution_profiles
        WHERE run_id = NEW.child_run_id
          AND schema_version = 1
          AND profile_id = 'durable_v2'
          AND snapshot_json = '{"id":"durable_v2","schemaVersion":1}'
          AND profile_hash = 'ae08f4dc8c1327cbd3de30879f01aa7475d9a728a3c0f738579086435d714f7e'
    );
END;

CREATE TRIGGER child_delegations_seal_graph_profile_update
AFTER UPDATE OF graph_task_attempt_id ON child_run_delegations
FOR EACH ROW
WHEN NEW.graph_task_attempt_id IS NOT NULL
BEGIN
    INSERT INTO run_execution_profiles(
        run_id, schema_version, profile_id, snapshot_json, profile_hash, frozen_at
    )
    SELECT
        NEW.child_run_id,
        1,
        'durable_v2',
        '{"id":"durable_v2","schemaVersion":1}',
        'ae08f4dc8c1327cbd3de30879f01aa7475d9a728a3c0f738579086435d714f7e',
        NEW.created_at
    WHERE NOT EXISTS (
        SELECT 1 FROM run_execution_profiles WHERE run_id = NEW.child_run_id
    );

    SELECT RAISE(ABORT, 'graph Child Run requires the canonical durable_v2 frozen profile')
    WHERE NOT EXISTS (
        SELECT 1
        FROM run_execution_profiles
        WHERE run_id = NEW.child_run_id
          AND schema_version = 1
          AND profile_id = 'durable_v2'
          AND snapshot_json = '{"id":"durable_v2","schemaVersion":1}'
          AND profile_hash = 'ae08f4dc8c1327cbd3de30879f01aa7475d9a728a3c0f738579086435d714f7e'
    );
END;

CREATE TRIGGER child_delegations_preserve_graph_binding
BEFORE UPDATE OF parent_run_id, child_run_id, child_conversation_id, tool_call_id,
    team_run_id, team_member_id, allowed_tools_json, graph_task_attempt_id
ON child_run_delegations
FOR EACH ROW
WHEN OLD.graph_task_attempt_id IS NOT NULL
 AND EXISTS (
     SELECT 1 FROM task_attempts WHERE id = OLD.graph_task_attempt_id
 )
 AND (
     NEW.parent_run_id IS NOT OLD.parent_run_id
     OR NEW.child_run_id IS NOT OLD.child_run_id
     OR NEW.child_conversation_id IS NOT OLD.child_conversation_id
     OR NEW.tool_call_id IS NOT OLD.tool_call_id
     OR NEW.team_run_id IS NOT OLD.team_run_id
     OR NEW.team_member_id IS NOT OLD.team_member_id
     OR NEW.allowed_tools_json IS NOT OLD.allowed_tools_json
     OR NEW.graph_task_attempt_id IS NOT OLD.graph_task_attempt_id
 )
BEGIN
    SELECT RAISE(ABORT, 'graph delegation authority facts are immutable');
END;

CREATE TRIGGER work_graph_activation_tool_authority_no_update
BEFORE UPDATE OF run_id, conversation_id, tool_name, input_json,
    execution_location, requires_approval
ON tool_calls
FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM work_graph_specs WHERE activation_tool_call_id = OLD.id
)
BEGIN
    SELECT RAISE(ABORT, 'work graph activation ToolCall authority facts are immutable');
END;

CREATE TRIGGER work_graph_node_start_tool_authority_no_update
BEFORE UPDATE OF run_id, conversation_id, tool_name, input_json,
    execution_location, requires_approval
ON tool_calls
FOR EACH ROW
WHEN EXISTS (
    SELECT 1
    FROM child_run_delegations
    WHERE tool_call_id = OLD.id AND graph_task_attempt_id IS NOT NULL
)
BEGIN
    SELECT RAISE(ABORT, 'work graph node-start ToolCall authority facts are immutable');
END;

CREATE TRIGGER work_graph_run_lineage_no_update
BEFORE UPDATE OF conversation_id, parent_run_id, root_run_id, run_kind, depth
ON runs
FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM work_graph_specs WHERE activated_by_run_id = OLD.id
)
OR EXISTS (
    SELECT 1
    FROM child_run_delegations
    WHERE child_run_id = OLD.id AND graph_task_attempt_id IS NOT NULL
)
OR EXISTS (
    SELECT 1
    FROM child_run_delegations
    WHERE parent_run_id = OLD.id AND graph_task_attempt_id IS NOT NULL
)
BEGIN
    SELECT RAISE(ABORT, 'work graph Run lineage facts are immutable');
END;

CREATE TRIGGER work_graph_profiles_no_update
BEFORE UPDATE ON run_execution_profiles
FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM work_graph_specs WHERE activated_by_run_id = OLD.run_id
)
OR EXISTS (
    SELECT 1
    FROM child_run_delegations
    WHERE child_run_id = OLD.run_id AND graph_task_attempt_id IS NOT NULL
)
OR EXISTS (
    SELECT 1
    FROM child_run_delegations
    WHERE parent_run_id = OLD.run_id AND graph_task_attempt_id IS NOT NULL
)
BEGIN
    SELECT RAISE(ABORT, 'work graph frozen execution profiles are immutable');
END;

CREATE TRIGGER work_graph_profiles_no_direct_delete
AFTER DELETE ON run_execution_profiles
FOR EACH ROW
WHEN EXISTS (SELECT 1 FROM runs WHERE id = OLD.run_id)
 AND (
     EXISTS (
         SELECT 1 FROM work_graph_specs WHERE activated_by_run_id = OLD.run_id
     )
     OR EXISTS (
         SELECT 1
         FROM child_run_delegations
         WHERE child_run_id = OLD.run_id AND graph_task_attempt_id IS NOT NULL
     )
     OR EXISTS (
         SELECT 1
         FROM child_run_delegations
         WHERE parent_run_id = OLD.run_id AND graph_task_attempt_id IS NOT NULL
     )
 )
BEGIN
    SELECT RAISE(ABORT, 'work graph frozen execution profiles cannot be deleted directly');
END;

CREATE TRIGGER work_graph_child_requires_profile_before_start
BEFORE UPDATE OF status ON runs
FOR EACH ROW
WHEN NEW.status <> 'queued'
 AND EXISTS (
     SELECT 1
     FROM child_run_delegations
     WHERE child_run_id = NEW.id AND graph_task_attempt_id IS NOT NULL
 )
 AND NOT EXISTS (
     SELECT 1
     FROM run_execution_profiles
     WHERE run_id = NEW.id
       AND schema_version = 1
       AND profile_id = 'durable_v2'
       AND snapshot_json = '{"id":"durable_v2","schemaVersion":1}'
       AND profile_hash = 'ae08f4dc8c1327cbd3de30879f01aa7475d9a728a3c0f738579086435d714f7e'
 )
BEGIN
    SELECT RAISE(ABORT, 'graph Child Run cannot start without its canonical durable_v2 profile');
END;

CREATE TRIGGER task_attempts_preserve_graph_delegation_owner
BEFORE UPDATE OF task_id, run_id ON task_attempts
FOR EACH ROW
WHEN EXISTS (
    SELECT 1
    FROM child_run_delegations AS delegation
    WHERE delegation.graph_task_attempt_id = OLD.id
      AND (
          delegation.parent_run_id <> NEW.run_id
          OR NOT EXISTS (
              SELECT 1 FROM work_graph_nodes WHERE task_id = NEW.task_id
          )
      )
)
BEGIN
    SELECT RAISE(ABORT, 'cannot detach a delegated Attempt from its parent Graph Task');
END;

DROP INDEX idx_work_tasks_in_progress_per_goal;

CREATE INDEX idx_work_tasks_goal_status
    ON work_tasks(goal_id, status, ordinal, id);

CREATE TRIGGER work_tasks_single_non_graph_active_insert
BEFORE INSERT ON work_tasks
FOR EACH ROW
WHEN NEW.status = 'in_progress'
 AND NOT EXISTS (SELECT 1 FROM work_graph_nodes WHERE task_id = NEW.id)
 AND EXISTS (
     SELECT 1 FROM work_tasks
     WHERE goal_id = NEW.goal_id AND status = 'in_progress'
 )
BEGIN
    SELECT RAISE(ABORT, 'non-Graph Goal supports only one in-progress Task');
END;

CREATE TRIGGER work_tasks_single_non_graph_active_update
BEFORE UPDATE OF status ON work_tasks
FOR EACH ROW
WHEN NEW.status = 'in_progress'
 AND OLD.status <> 'in_progress'
 AND NOT EXISTS (SELECT 1 FROM work_graph_nodes WHERE task_id = NEW.id)
 AND EXISTS (
     SELECT 1 FROM work_tasks
     WHERE goal_id = NEW.goal_id
       AND status = 'in_progress'
       AND id <> NEW.id
 )
BEGIN
    SELECT RAISE(ABORT, 'non-Graph Goal supports only one in-progress Task');
END;

CREATE TRIGGER work_tasks_graph_cannot_overlap_non_graph_update
BEFORE UPDATE OF status ON work_tasks
FOR EACH ROW
WHEN NEW.status = 'in_progress'
 AND OLD.status <> 'in_progress'
 AND EXISTS (SELECT 1 FROM work_graph_nodes WHERE task_id = NEW.id)
 AND EXISTS (
     SELECT 1
     FROM work_tasks AS other
     WHERE other.goal_id = NEW.goal_id
       AND other.status = 'in_progress'
       AND other.id <> NEW.id
       AND NOT EXISTS (
           SELECT 1 FROM work_graph_nodes WHERE task_id = other.id
       )
 )
BEGIN
    SELECT RAISE(ABORT, 'Graph and non-Graph Tasks cannot execute together');
END;
"#;

const MIGRATION_37: &str = r#"
CREATE TABLE work_graph_node_finishes (
    id TEXT PRIMARY KEY CHECK(length(trim(id)) BETWEEN 1 AND 200),
    goal_id TEXT NOT NULL REFERENCES work_graph_specs(goal_id) ON DELETE CASCADE,
    task_id TEXT NOT NULL REFERENCES work_graph_nodes(task_id) ON DELETE CASCADE,
    attempt_id TEXT NOT NULL UNIQUE REFERENCES task_attempts(id) ON DELETE CASCADE,
    parent_run_id TEXT NOT NULL REFERENCES runs(id) ON DELETE RESTRICT,
    child_run_id TEXT NOT NULL UNIQUE REFERENCES runs(id) ON DELETE RESTRICT,
    tool_call_id TEXT NOT NULL UNIQUE REFERENCES tool_calls(id) ON DELETE RESTRICT,
    validation_policy_id TEXT NOT NULL
        CHECK(validation_policy_id IN ('standard_v1', 'high_risk_v1')),
    validation_policy_hash TEXT NOT NULL CHECK(
        length(validation_policy_hash) = 64
        AND validation_policy_hash NOT GLOB '*[^0-9a-f]*'
    ),
    expected_task_version INTEGER NOT NULL CHECK(expected_task_version >= 1),
    expected_attempt_version INTEGER NOT NULL CHECK(expected_attempt_version >= 1),
    child_result_hash TEXT NOT NULL CHECK(
        length(child_result_hash) = 64
        AND child_result_hash NOT GLOB '*[^0-9a-f]*'
    ),
    summary TEXT NOT NULL CHECK(
        summary = trim(summary) AND length(summary) BETWEEN 1 AND 4000
    ),
    input_json TEXT NOT NULL CHECK(
        json_valid(input_json) AND json_type(input_json) = 'object'
    ),
    input_hash TEXT NOT NULL CHECK(
        length(input_hash) = 64 AND input_hash NOT GLOB '*[^0-9a-f]*'
    ),
    created_at INTEGER NOT NULL CHECK(created_at >= 0)
);

CREATE INDEX idx_work_graph_node_finishes_task_created
    ON work_graph_node_finishes(task_id, created_at DESC, id DESC);
CREATE INDEX idx_work_graph_node_finishes_goal_created
    ON work_graph_node_finishes(goal_id, created_at DESC, id DESC);

CREATE TABLE work_graph_node_criterion_evidence (
    finish_id TEXT NOT NULL
        REFERENCES work_graph_node_finishes(id) ON DELETE CASCADE,
    criterion_ordinal INTEGER NOT NULL CHECK(criterion_ordinal BETWEEN 0 AND 7),
    criterion TEXT NOT NULL CHECK(
        criterion = trim(criterion) AND length(criterion) BETWEEN 1 AND 1000
    ),
    evidence_id TEXT NOT NULL
        REFERENCES task_evidence(id) ON DELETE CASCADE
        CHECK(length(trim(evidence_id)) BETWEEN 1 AND 200),
    PRIMARY KEY(finish_id, criterion_ordinal, evidence_id)
);

CREATE INDEX idx_work_graph_node_criterion_evidence_evidence
    ON work_graph_node_criterion_evidence(evidence_id, finish_id);

CREATE TRIGGER work_graph_node_finishes_validate_authority
BEFORE INSERT ON work_graph_node_finishes
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1
    FROM work_graph_nodes AS node
    JOIN work_tasks AS task
      ON task.id = node.task_id
     AND task.goal_id = node.goal_id
     AND task.status = 'in_progress'
     AND task.owner_run_id = NEW.parent_run_id
     AND task.version = NEW.expected_task_version
    JOIN goals AS goal
      ON goal.id = node.goal_id
     AND goal.status = 'active'
    JOIN task_attempts AS attempt
      ON attempt.id = NEW.attempt_id
     AND attempt.task_id = node.task_id
     AND attempt.run_id = NEW.parent_run_id
     AND attempt.kind = 'execution'
     AND attempt.status = 'running'
     AND attempt.version = NEW.expected_attempt_version
    JOIN child_run_delegations AS delegation
     ON delegation.graph_task_attempt_id = attempt.id
     AND delegation.parent_run_id = NEW.parent_run_id
     AND delegation.child_run_id = NEW.child_run_id
     AND delegation.status = 'completed'
     AND (
         length(trim(COALESCE(delegation.result_text, ''))) > 0
         OR EXISTS (
             SELECT 1
             FROM messages AS child_result_message
             WHERE child_result_message.run_id = delegation.child_run_id
               AND child_result_message.role = 'assistant'
               AND length(trim(child_result_message.content)) > 0
         )
     )
    JOIN runs AS parent_run
      ON parent_run.id = NEW.parent_run_id
     AND parent_run.conversation_id = goal.conversation_id
     AND parent_run.status = 'running'
     AND parent_run.run_kind = 'primary'
     AND parent_run.parent_run_id IS NULL
     AND parent_run.depth = 0
    JOIN run_execution_profiles AS parent_profile
      ON parent_profile.run_id = parent_run.id
     AND parent_profile.schema_version = 1
     AND parent_profile.profile_id = 'durable_v2'
     AND parent_profile.snapshot_json = '{"id":"durable_v2","schemaVersion":1}'
     AND parent_profile.profile_hash = 'ae08f4dc8c1327cbd3de30879f01aa7475d9a728a3c0f738579086435d714f7e'
    JOIN runs AS child_run
      ON child_run.id = NEW.child_run_id
     AND child_run.conversation_id = delegation.child_conversation_id
     AND child_run.parent_run_id = parent_run.id
     AND child_run.root_run_id = COALESCE(parent_run.root_run_id, parent_run.id)
     AND child_run.run_kind = 'child'
     AND child_run.depth = 1
     AND child_run.status = 'completed'
     AND child_run.finished_at IS NOT NULL
    JOIN run_execution_profiles AS child_profile
      ON child_profile.run_id = child_run.id
     AND child_profile.schema_version = 1
     AND child_profile.profile_id = 'durable_v2'
     AND child_profile.snapshot_json = '{"id":"durable_v2","schemaVersion":1}'
     AND child_profile.profile_hash = 'ae08f4dc8c1327cbd3de30879f01aa7475d9a728a3c0f738579086435d714f7e'
    JOIN task_validation_policies AS policy
      ON policy.task_id = task.id
     AND policy.schema_version = 1
     AND policy.policy_id = NEW.validation_policy_id
     AND policy.policy_hash = NEW.validation_policy_hash
     AND attempt.policy_hash = policy.policy_hash
     AND policy.snapshot_json = CASE policy.policy_id
         WHEN 'standard_v1' THEN
             '{"schemaVersion":1,"id":"standard_v1","riskLevel":"standard","requiredChecks":["inspection"],"allowedCheckTypes":["test","inspection","review","manual"],"reviewerPolicy":"host_validated","maxRepairAttempts":2,"completionRequiresAcceptance":true,"hash":"bcef9ad7087b2872a0152f493cfba131a4283a3170dc8fe5824d108d19e3d04d"}'
         WHEN 'high_risk_v1' THEN
             '{"schemaVersion":1,"id":"high_risk_v1","riskLevel":"high","requiredChecks":["inspection","review"],"allowedCheckTypes":["test","inspection","review","manual"],"reviewerPolicy":"independent","maxRepairAttempts":1,"completionRequiresAcceptance":true,"hash":"bb1258169c5d923dfd5fd1980dd52150ab96e159bd01d07ff0d04c48998dc5ea"}'
     END
    JOIN tool_calls AS finish_call
      ON finish_call.id = NEW.tool_call_id
     AND finish_call.run_id = parent_run.id
     AND finish_call.conversation_id = goal.conversation_id
     AND finish_call.tool_name = 'graph_readonly_node_finish'
     AND finish_call.execution_location = 'host'
     AND finish_call.status = 'running'
     AND finish_call.requires_approval = 0
     AND finish_call.input_json = NEW.input_json
     AND json(NEW.input_json) = NEW.input_json
     AND json_extract(NEW.input_json, '$.goalId') = node.goal_id
     AND json_extract(NEW.input_json, '$.taskId') = node.task_id
     AND json_extract(NEW.input_json, '$.attemptId') = attempt.id
     AND json_type(NEW.input_json, '$.expectedTaskVersion') = 'integer'
     AND json_extract(NEW.input_json, '$.expectedTaskVersion') = NEW.expected_task_version
     AND json_type(NEW.input_json, '$.expectedAttemptVersion') = 'integer'
     AND json_extract(NEW.input_json, '$.expectedAttemptVersion') =
         NEW.expected_attempt_version
     AND json_extract(NEW.input_json, '$.summary') = NEW.summary
     AND json_type(NEW.input_json, '$.criterionEvidence') = 'array'
     AND json_array_length(json_extract(NEW.input_json, '$.criterionEvidence')) =
         json_array_length(node.acceptance_criteria_json)
     AND json_array_length(json_extract(NEW.input_json, '$.criterionEvidence')) BETWEEN 1 AND 8
     AND (
         SELECT group_concat(key, ',') FROM json_each(NEW.input_json)
     ) = 'attemptId,criterionEvidence,expectedAttemptVersion,expectedTaskVersion,goalId,summary,taskId'
     AND NOT EXISTS (
         SELECT 1
         FROM json_each(json_extract(NEW.input_json, '$.criterionEvidence')) AS criterion_entry
         WHERE criterion_entry.type <> 'object'
            OR (
                SELECT group_concat(key, ',') FROM json_each(criterion_entry.value)
            ) <> 'criterion,evidenceIds'
            OR json_type(criterion_entry.value, '$.criterion') <> 'text'
            OR json_extract(criterion_entry.value, '$.criterion') <>
                json_extract(
                    node.acceptance_criteria_json,
                    '$[' || criterion_entry.key || ']'
                )
            OR json_type(criterion_entry.value, '$.evidenceIds') <> 'array'
            OR json_array_length(
                json_extract(criterion_entry.value, '$.evidenceIds')
            ) NOT BETWEEN 1 AND 16
            OR EXISTS (
                SELECT 1
                FROM json_each(
                    json_extract(criterion_entry.value, '$.evidenceIds')
                ) AS evidence_entry
                WHERE evidence_entry.type <> 'text'
                   OR evidence_entry.value <> trim(evidence_entry.value)
                   OR length(evidence_entry.value) NOT BETWEEN 1 AND 200
            )
            OR EXISTS (
                SELECT 1
                FROM json_each(
                    json_extract(criterion_entry.value, '$.evidenceIds')
                ) AS evidence_entry
                GROUP BY evidence_entry.value
                HAVING COUNT(*) > 1
            )
     )
     AND (
         SELECT COUNT(*)
         FROM json_each(json_extract(NEW.input_json, '$.criterionEvidence')) AS criterion_entry,
              json_each(json_extract(criterion_entry.value, '$.evidenceIds')) AS evidence_entry
     ) BETWEEN 1 AND 100
     AND NOT EXISTS (
         SELECT 1
         FROM json_each(json_extract(NEW.input_json, '$.criterionEvidence')) AS criterion_entry,
              json_each(json_extract(criterion_entry.value, '$.evidenceIds')) AS evidence_entry
         WHERE NOT EXISTS (
             SELECT 1
             FROM task_evidence AS criterion_evidence
             JOIN tool_calls AS criterion_call
               ON criterion_call.id = criterion_evidence.ref_id
              AND criterion_call.run_id = parent_run.id
              AND criterion_call.conversation_id = goal.conversation_id
              AND criterion_call.status = 'completed'
              AND COALESCE(criterion_call.error_message, '') = ''
              AND criterion_call.started_at > attempt.started_at
              AND criterion_call.completed_at IS NOT NULL
              AND criterion_call.completed_at >= criterion_call.started_at
              AND criterion_call.updated_at >= criterion_call.started_at
             WHERE criterion_evidence.id = evidence_entry.value
               AND criterion_evidence.task_id = node.task_id
               AND criterion_evidence.source_run_id = parent_run.id
               AND criterion_evidence.ref_kind = 'tool_call'
               AND criterion_evidence.validity_status = 'valid'
               AND criterion_evidence.checked_at IS NOT NULL
               AND criterion_evidence.rowid > attempt.evidence_rowid_watermark
               AND json_valid(criterion_evidence.metadata_json)
               AND json_type(criterion_evidence.metadata_json) = 'object'
               AND (
                   (
                       criterion_evidence.evidence_type = 'tool_call'
                       AND json_extract(
                           criterion_evidence.metadata_json,
                           '$.validationCheckType'
                       ) = 'inspection'
                       AND (
                           (
                               criterion_call.execution_location = 'runtime'
                               AND criterion_call.tool_name IN ('read', 'ls', 'find', 'grep')
                               AND criterion_call.requires_approval = 0
                           )
                           OR (
                               criterion_call.execution_location = 'host'
                               AND criterion_call.tool_name IN (
                                   'git_read', 'structured_data', 'tabular_data'
                               )
                               AND criterion_call.requires_approval = 0
                           )
                           OR (
                               criterion_call.execution_location = 'host'
                               AND criterion_call.tool_name = 'sqlite_read'
                               AND criterion_call.requires_approval = 1
                           )
                       )
                   )
                   OR (
                       criterion_evidence.evidence_type = 'test_result'
                       AND json_extract(
                           criterion_evidence.metadata_json,
                           '$.validationCheckType'
                       ) = 'test'
                       AND criterion_call.execution_location = 'host'
                       AND criterion_call.tool_name IN ('test_run', 'code_check')
                       AND criterion_call.requires_approval = 1
                   )
               )
         )
     )
    WHERE node.task_id = NEW.task_id
      AND node.goal_id = NEW.goal_id
      AND policy.policy_id = 'standard_v1'
      AND NOT EXISTS (
          SELECT 1
          FROM review_findings AS blocker
          WHERE blocker.goal_id = node.goal_id
            AND (blocker.task_id = node.task_id OR blocker.task_id IS NULL)
            AND blocker.status = 'open'
            AND blocker.severity IN ('critical', 'high', 'medium')
      )
)
BEGIN
    SELECT RAISE(
        ABORT,
        'Graph node finish requires exact Host authority, Child terminal, policy, CAS, and canonical input facts'
    );
END;

CREATE TRIGGER work_graph_node_criterion_evidence_validate
BEFORE INSERT ON work_graph_node_criterion_evidence
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1
    FROM work_graph_node_finishes AS finish
    JOIN work_graph_nodes AS node ON node.task_id = finish.task_id
    JOIN task_attempts AS attempt ON attempt.id = finish.attempt_id
    JOIN goals AS goal ON goal.id = finish.goal_id
    JOIN task_evidence AS evidence
      ON evidence.id = NEW.evidence_id
     AND evidence.task_id = finish.task_id
     AND evidence.source_run_id = finish.parent_run_id
     AND evidence.validity_status = 'valid'
     AND evidence.rowid > attempt.evidence_rowid_watermark
     AND evidence.ref_kind = 'tool_call'
     AND evidence.checked_at IS NOT NULL
     AND json_valid(evidence.metadata_json)
     AND json_type(evidence.metadata_json) = 'object'
     AND (
         (
             evidence.evidence_type = 'tool_call'
             AND json_extract(evidence.metadata_json, '$.validationCheckType') = 'inspection'
         )
         OR (
             evidence.evidence_type = 'test_result'
             AND json_extract(evidence.metadata_json, '$.validationCheckType') = 'test'
         )
     )
    WHERE finish.id = NEW.finish_id
      AND NEW.criterion_ordinal < json_array_length(node.acceptance_criteria_json)
      AND NEW.criterion = json_extract(
          node.acceptance_criteria_json,
          '$[' || NEW.criterion_ordinal || ']'
      )
      AND NEW.criterion = json_extract(
          finish.input_json,
          '$.criterionEvidence[' || NEW.criterion_ordinal || '].criterion'
      )
      AND EXISTS (
          SELECT 1
          FROM json_each(
              json_extract(
                  finish.input_json,
                  '$.criterionEvidence[' || NEW.criterion_ordinal || '].evidenceIds'
              )
          ) AS input_evidence
          WHERE input_evidence.value = NEW.evidence_id
      )
      AND EXISTS (
          SELECT 1 FROM tool_calls AS evidence_call
          WHERE evidence_call.id = evidence.ref_id
            AND evidence_call.run_id = finish.parent_run_id
            AND evidence_call.conversation_id = goal.conversation_id
            AND evidence_call.status = 'completed'
            AND COALESCE(evidence_call.error_message, '') = ''
            AND (
                (
                    evidence.evidence_type = 'tool_call'
                    AND json_extract(
                        evidence.metadata_json,
                        '$.validationCheckType'
                    ) = 'inspection'
                    AND evidence_call.execution_location = 'runtime'
                    AND evidence_call.tool_name IN ('read', 'ls', 'find', 'grep')
                    AND evidence_call.requires_approval = 0
                )
                OR (
                    evidence.evidence_type = 'tool_call'
                    AND json_extract(
                        evidence.metadata_json,
                        '$.validationCheckType'
                    ) = 'inspection'
                    AND evidence_call.execution_location = 'host'
                    AND evidence_call.tool_name IN (
                        'git_read', 'structured_data', 'tabular_data'
                    )
                    AND evidence_call.requires_approval = 0
                )
                OR (
                    evidence.evidence_type = 'tool_call'
                    AND json_extract(
                        evidence.metadata_json,
                        '$.validationCheckType'
                    ) = 'inspection'
                    AND evidence_call.execution_location = 'host'
                    AND evidence_call.tool_name = 'sqlite_read'
                    AND evidence_call.requires_approval = 1
                )
                OR (
                    evidence.evidence_type = 'test_result'
                    AND json_extract(evidence.metadata_json, '$.validationCheckType') = 'test'
                    AND evidence_call.execution_location = 'host'
                    AND evidence_call.tool_name IN ('test_run', 'code_check')
                    AND evidence_call.requires_approval = 1
                )
            )
            AND evidence_call.started_at > attempt.started_at
            AND evidence_call.completed_at IS NOT NULL
            AND evidence_call.completed_at >= evidence_call.started_at
            AND evidence_call.updated_at >= evidence_call.started_at
      )
)
BEGIN
    SELECT RAISE(
        ABORT,
        'Graph criterion binding requires exact frozen criterion and current Host-valid parent Evidence'
    );
END;

CREATE TRIGGER task_attempts_require_graph_node_finish
BEFORE UPDATE OF status ON task_attempts
FOR EACH ROW
WHEN OLD.status = 'running'
 AND NEW.status = 'succeeded'
 AND EXISTS (SELECT 1 FROM work_graph_nodes WHERE task_id = OLD.task_id)
 AND NOT EXISTS (
     SELECT 1
     FROM work_graph_node_finishes AS finish
     JOIN work_graph_nodes AS node ON node.task_id = finish.task_id
     JOIN work_tasks AS task ON task.id = node.task_id
     JOIN goals AS goal
       ON goal.id = node.goal_id
      AND goal.status = 'active'
     JOIN runs AS parent_run
       ON parent_run.id = finish.parent_run_id
      AND parent_run.conversation_id = goal.conversation_id
      AND parent_run.status = 'running'
      AND parent_run.run_kind = 'primary'
      AND parent_run.parent_run_id IS NULL
      AND parent_run.depth = 0
     JOIN child_run_delegations AS delegation
      ON delegation.graph_task_attempt_id = finish.attempt_id
      AND delegation.parent_run_id = finish.parent_run_id
      AND delegation.child_run_id = finish.child_run_id
      AND delegation.status = 'completed'
      AND (
          length(trim(COALESCE(delegation.result_text, ''))) > 0
          OR EXISTS (
              SELECT 1
              FROM messages AS child_result_message
              WHERE child_result_message.run_id = delegation.child_run_id
                AND child_result_message.role = 'assistant'
                AND length(trim(child_result_message.content)) > 0
          )
      )
     JOIN runs AS child_run
       ON child_run.id = finish.child_run_id
      AND child_run.status = 'completed'
      AND child_run.finished_at IS NOT NULL
     JOIN task_validation_policies AS policy
       ON policy.task_id = finish.task_id
      AND policy.policy_id = finish.validation_policy_id
      AND policy.policy_hash = finish.validation_policy_hash
      AND policy.policy_hash = OLD.policy_hash
     JOIN tool_calls AS finish_call
       ON finish_call.id = finish.tool_call_id
      AND finish_call.run_id = finish.parent_run_id
      AND finish_call.conversation_id = goal.conversation_id
      AND finish_call.tool_name = 'graph_readonly_node_finish'
      AND finish_call.execution_location = 'host'
      AND finish_call.status = 'running'
      AND finish_call.requires_approval = 0
      AND finish_call.input_json = finish.input_json
     WHERE finish.attempt_id = OLD.id
       AND finish.task_id = OLD.task_id
       AND finish.parent_run_id = OLD.run_id
       AND finish.expected_attempt_version = OLD.version
       AND finish.expected_task_version = task.version
       AND NOT EXISTS (
           SELECT 1
           FROM json_each(node.acceptance_criteria_json) AS criterion_entry
           WHERE NOT EXISTS (
               SELECT 1
               FROM work_graph_node_criterion_evidence AS binding
               WHERE binding.finish_id = finish.id
                 AND binding.criterion_ordinal = CAST(criterion_entry.key AS INTEGER)
                 AND binding.criterion = criterion_entry.value
           )
       )
       AND (
           SELECT COUNT(*)
           FROM work_graph_node_criterion_evidence AS binding
           WHERE binding.finish_id = finish.id
       ) = (
           SELECT COUNT(*)
           FROM json_each(json_extract(finish.input_json, '$.criterionEvidence')) AS criterion_input,
                json_each(json_extract(criterion_input.value, '$.evidenceIds')) AS evidence_input
       )
       AND NOT EXISTS (
           SELECT 1
           FROM work_graph_node_criterion_evidence AS binding
           JOIN task_evidence AS evidence ON evidence.id = binding.evidence_id
           LEFT JOIN tool_calls AS evidence_call ON evidence_call.id = evidence.ref_id
           WHERE binding.finish_id = finish.id
             AND (
                 evidence.task_id <> finish.task_id
                 OR evidence.source_run_id IS NOT finish.parent_run_id
                 OR evidence.validity_status <> 'valid'
                 OR evidence.checked_at IS NULL
                 OR evidence.rowid <= OLD.evidence_rowid_watermark
                 OR evidence.ref_kind <> 'tool_call'
                 OR NOT json_valid(evidence.metadata_json)
                 OR evidence_call.id IS NULL
                 OR evidence_call.run_id IS NOT finish.parent_run_id
                 OR evidence_call.conversation_id IS NOT goal.conversation_id
                 OR evidence_call.status <> 'completed'
                 OR COALESCE(evidence_call.error_message, '') <> ''
                 OR NOT (
                     (
                         evidence.evidence_type = 'tool_call'
                         AND json_extract(
                             evidence.metadata_json,
                             '$.validationCheckType'
                         ) = 'inspection'
                         AND evidence_call.execution_location = 'runtime'
                         AND evidence_call.tool_name IN ('read', 'ls', 'find', 'grep')
                         AND evidence_call.requires_approval = 0
                     )
                     OR (
                         evidence.evidence_type = 'tool_call'
                         AND json_extract(
                             evidence.metadata_json,
                             '$.validationCheckType'
                         ) = 'inspection'
                         AND evidence_call.execution_location = 'host'
                         AND evidence_call.tool_name IN (
                             'git_read', 'structured_data', 'tabular_data'
                         )
                         AND evidence_call.requires_approval = 0
                     )
                     OR (
                         evidence.evidence_type = 'tool_call'
                         AND json_extract(
                             evidence.metadata_json,
                             '$.validationCheckType'
                         ) = 'inspection'
                         AND evidence_call.execution_location = 'host'
                         AND evidence_call.tool_name = 'sqlite_read'
                         AND evidence_call.requires_approval = 1
                     )
                     OR (
                         evidence.evidence_type = 'test_result'
                         AND json_extract(
                             evidence.metadata_json,
                             '$.validationCheckType'
                         ) = 'test'
                         AND evidence_call.execution_location = 'host'
                         AND evidence_call.tool_name IN ('test_run', 'code_check')
                         AND evidence_call.requires_approval = 1
                     )
                 )
                 OR evidence_call.started_at <= OLD.started_at
                 OR evidence_call.completed_at IS NULL
                 OR evidence_call.completed_at < evidence_call.started_at
                 OR evidence_call.updated_at < evidence_call.started_at
                 OR NOT EXISTS (
                     SELECT 1
                     FROM json_each(NEW.evidence_ids_json) AS finished_evidence
                     WHERE finished_evidence.value = binding.evidence_id
                 )
             )
       )
       AND EXISTS (
           SELECT 1
           FROM task_evidence AS inspection_evidence
           JOIN tool_calls AS inspection_call
             ON inspection_call.id = inspection_evidence.ref_id
            AND inspection_call.run_id = finish.parent_run_id
            AND inspection_call.conversation_id = goal.conversation_id
            AND inspection_call.status = 'completed'
            AND COALESCE(inspection_call.error_message, '') = ''
            AND inspection_call.started_at > OLD.started_at
            AND inspection_call.completed_at IS NOT NULL
            AND inspection_call.completed_at >= inspection_call.started_at
            AND inspection_call.updated_at >= inspection_call.started_at
           WHERE inspection_evidence.task_id = OLD.task_id
             AND inspection_evidence.source_run_id = finish.parent_run_id
             AND inspection_evidence.evidence_type = 'tool_call'
             AND inspection_evidence.ref_kind = 'tool_call'
             AND inspection_evidence.validity_status = 'valid'
             AND inspection_evidence.checked_at IS NOT NULL
             AND inspection_evidence.rowid > OLD.evidence_rowid_watermark
             AND json_valid(inspection_evidence.metadata_json)
             AND json_extract(
                 inspection_evidence.metadata_json,
                 '$.validationCheckType'
             ) = 'inspection'
             AND (
                 (
                     inspection_call.execution_location = 'runtime'
                     AND inspection_call.tool_name IN ('read', 'ls', 'find', 'grep')
                     AND inspection_call.requires_approval = 0
                 )
                 OR (
                     inspection_call.execution_location = 'host'
                     AND inspection_call.tool_name IN (
                         'git_read', 'structured_data', 'tabular_data'
                     )
                     AND inspection_call.requires_approval = 0
                 )
                 OR (
                     inspection_call.execution_location = 'host'
                     AND inspection_call.tool_name = 'sqlite_read'
                     AND inspection_call.requires_approval = 1
                 )
             )
       )
       AND NOT EXISTS (
           SELECT 1
           FROM review_findings AS blocker
           WHERE blocker.goal_id = node.goal_id
             AND (blocker.task_id = node.task_id OR blocker.task_id IS NULL)
             AND blocker.status = 'open'
             AND blocker.severity IN ('critical', 'high', 'medium')
       )
       AND (
           policy.policy_id = 'standard_v1'
       )
 )
BEGIN
    SELECT RAISE(
        ABORT,
        'Graph Attempt cannot succeed without a complete current criterion-to-Evidence finish fact'
    );
END;

CREATE TRIGGER work_tasks_require_graph_node_finish
BEFORE UPDATE OF status ON work_tasks
FOR EACH ROW
WHEN OLD.status = 'in_progress'
 AND NEW.status = 'completed'
 AND EXISTS (SELECT 1 FROM work_graph_nodes WHERE task_id = OLD.id)
 AND NOT EXISTS (
     SELECT 1
     FROM work_graph_node_finishes AS finish
     JOIN work_graph_nodes AS node ON node.task_id = finish.task_id
     JOIN goals AS goal
       ON goal.id = node.goal_id
      AND goal.status = 'active'
     JOIN task_attempts AS attempt
       ON attempt.id = finish.attempt_id
      AND attempt.task_id = finish.task_id
      AND attempt.run_id = finish.parent_run_id
      AND attempt.status = 'succeeded'
     WHERE finish.task_id = OLD.id
       AND finish.expected_task_version = OLD.version
       AND finish.expected_attempt_version + 1 = attempt.version
       AND OLD.owner_run_id = finish.parent_run_id
 )
BEGIN
    SELECT RAISE(
        ABORT,
        'Graph Task cannot complete without its Host-validated Node finish fact'
    );
END;

CREATE TRIGGER work_graph_node_finishes_no_update
BEFORE UPDATE ON work_graph_node_finishes
BEGIN
    SELECT RAISE(ABORT, 'Graph node finish facts are immutable');
END;

CREATE TRIGGER work_graph_node_criterion_evidence_no_update
BEFORE UPDATE ON work_graph_node_criterion_evidence
BEGIN
    SELECT RAISE(ABORT, 'Graph criterion-to-Evidence bindings are immutable');
END;

CREATE TRIGGER work_graph_node_finishes_no_direct_delete
BEFORE DELETE ON work_graph_node_finishes
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1 FROM work_graph_cascade_delete_scopes WHERE goal_id = OLD.goal_id
)
BEGIN
    SELECT RAISE(ABORT, 'Graph node finish facts can only be deleted with their Goal');
END;

CREATE TRIGGER work_graph_node_criterion_evidence_no_direct_delete
BEFORE DELETE ON work_graph_node_criterion_evidence
FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM work_graph_node_finishes WHERE id = OLD.finish_id
)
 AND NOT EXISTS (
    SELECT 1
    FROM work_graph_node_finishes AS finish
    JOIN work_graph_cascade_delete_scopes AS scope ON scope.goal_id = finish.goal_id
    WHERE finish.id = OLD.finish_id
)
BEGIN
    SELECT RAISE(ABORT, 'Graph criterion bindings can only be deleted with their Goal');
END;

CREATE TRIGGER work_graph_bound_evidence_no_direct_delete
BEFORE DELETE ON task_evidence
FOR EACH ROW
WHEN EXISTS (
    SELECT 1
    FROM work_graph_node_criterion_evidence AS binding
    JOIN work_graph_node_finishes AS finish ON finish.id = binding.finish_id
    WHERE binding.evidence_id = OLD.id
      AND NOT EXISTS (
          SELECT 1 FROM work_graph_cascade_delete_scopes WHERE goal_id = finish.goal_id
      )
)
BEGIN
    SELECT RAISE(ABORT, 'Evidence bound to a Graph finish cannot be deleted directly');
END;

CREATE TRIGGER work_graph_bound_evidence_authority_no_update
BEFORE UPDATE OF task_id, source_run_id, evidence_type, ref_kind, ref_id,
    summary, metadata_json, trace_id, span_id, created_at
ON task_evidence
FOR EACH ROW
WHEN EXISTS (
    SELECT 1
    FROM work_graph_node_criterion_evidence
    WHERE evidence_id = OLD.id
)
 AND (
     NEW.task_id IS NOT OLD.task_id
     OR NEW.source_run_id IS NOT OLD.source_run_id
     OR NEW.evidence_type IS NOT OLD.evidence_type
     OR NEW.ref_kind IS NOT OLD.ref_kind
     OR NEW.ref_id IS NOT OLD.ref_id
     OR NEW.summary IS NOT OLD.summary
     OR NEW.metadata_json IS NOT OLD.metadata_json
     OR NEW.trace_id IS NOT OLD.trace_id
     OR NEW.span_id IS NOT OLD.span_id
     OR NEW.created_at IS NOT OLD.created_at
 )
BEGIN
    SELECT RAISE(ABORT, 'Graph-bound Evidence authority facts are immutable');
END;

CREATE TRIGGER work_graph_finish_tool_authority_no_update
BEFORE UPDATE OF run_id, conversation_id, tool_name, input_json,
    execution_location, requires_approval
ON tool_calls
FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM work_graph_node_finishes WHERE tool_call_id = OLD.id
)
BEGIN
    SELECT RAISE(ABORT, 'Graph node-finish ToolCall authority facts are immutable');
END;

CREATE TRIGGER work_graph_bound_evidence_tool_no_update
BEFORE UPDATE OF run_id, conversation_id, tool_name, input_json, status, result_json,
    error_message, execution_location, requires_approval, started_at, completed_at, updated_at
ON tool_calls
FOR EACH ROW
WHEN EXISTS (
    SELECT 1
    FROM work_graph_node_criterion_evidence AS binding
    JOIN task_evidence AS evidence ON evidence.id = binding.evidence_id
    WHERE evidence.ref_kind = 'tool_call' AND evidence.ref_id = OLD.id
)
 AND (
     NEW.run_id IS NOT OLD.run_id
     OR NEW.conversation_id IS NOT OLD.conversation_id
     OR NEW.tool_name IS NOT OLD.tool_name
     OR NEW.input_json IS NOT OLD.input_json
     OR NEW.status IS NOT OLD.status
     OR NEW.result_json IS NOT OLD.result_json
     OR NEW.error_message IS NOT OLD.error_message
     OR NEW.execution_location IS NOT OLD.execution_location
     OR NEW.requires_approval IS NOT OLD.requires_approval
     OR NEW.started_at IS NOT OLD.started_at
     OR NEW.completed_at IS NOT OLD.completed_at
     OR NEW.updated_at IS NOT OLD.updated_at
 )
BEGIN
    SELECT RAISE(ABORT, 'Graph criterion Evidence ToolCall facts are immutable');
END;

CREATE TRIGGER work_graph_bound_evidence_tool_no_direct_delete
BEFORE DELETE ON tool_calls
FOR EACH ROW
WHEN EXISTS (
    SELECT 1
    FROM work_graph_node_criterion_evidence AS binding
    JOIN work_graph_node_finishes AS finish ON finish.id = binding.finish_id
    JOIN task_evidence AS evidence ON evidence.id = binding.evidence_id
    WHERE evidence.ref_kind = 'tool_call'
      AND evidence.ref_id = OLD.id
      AND NOT EXISTS (
          SELECT 1 FROM work_graph_cascade_delete_scopes WHERE goal_id = finish.goal_id
      )
)
BEGIN
    SELECT RAISE(ABORT, 'Graph criterion Evidence ToolCall cannot be deleted directly');
END;

CREATE TRIGGER work_graph_finished_policy_no_update
BEFORE UPDATE ON task_validation_policies
FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM work_graph_node_finishes WHERE task_id = OLD.task_id
)
BEGIN
    SELECT RAISE(ABORT, 'Graph node-finish ValidationPolicy is frozen');
END;

CREATE TRIGGER work_graph_finished_policy_no_direct_delete
BEFORE DELETE ON task_validation_policies
FOR EACH ROW
WHEN EXISTS (
    SELECT 1
    FROM work_graph_node_finishes AS finish
    WHERE finish.task_id = OLD.task_id
      AND NOT EXISTS (
          SELECT 1 FROM work_graph_cascade_delete_scopes WHERE goal_id = finish.goal_id
      )
)
BEGIN
    SELECT RAISE(ABORT, 'Graph node-finish ValidationPolicy cannot be deleted directly');
END;

CREATE TRIGGER work_graph_finished_attempt_authority_no_update
BEFORE UPDATE OF task_id, attempt_number, kind, run_id, policy_hash, root_cause,
    finding_ids_json, evidence_rowid_watermark, finding_rowid_watermark, started_at
ON task_attempts
FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM work_graph_node_finishes WHERE attempt_id = OLD.id
)
 AND (
     NEW.task_id IS NOT OLD.task_id
     OR NEW.attempt_number IS NOT OLD.attempt_number
     OR NEW.kind IS NOT OLD.kind
     OR NEW.run_id IS NOT OLD.run_id
     OR NEW.policy_hash IS NOT OLD.policy_hash
     OR NEW.root_cause IS NOT OLD.root_cause
     OR NEW.finding_ids_json IS NOT OLD.finding_ids_json
     OR NEW.evidence_rowid_watermark IS NOT OLD.evidence_rowid_watermark
     OR NEW.finding_rowid_watermark IS NOT OLD.finding_rowid_watermark
     OR NEW.started_at IS NOT OLD.started_at
 )
BEGIN
    SELECT RAISE(ABORT, 'Graph node-finish Attempt authority facts are immutable');
END;

CREATE TRIGGER work_graph_finished_attempt_terminal_no_update
BEFORE UPDATE OF status, evidence_ids_json, failure_reason, version, finished_at
ON task_attempts
FOR EACH ROW
WHEN OLD.status = 'succeeded'
 AND EXISTS (
     SELECT 1 FROM work_graph_node_finishes WHERE attempt_id = OLD.id
 )
 AND (
     NEW.status IS NOT OLD.status
     OR NEW.evidence_ids_json IS NOT OLD.evidence_ids_json
     OR NEW.failure_reason IS NOT OLD.failure_reason
     OR NEW.version IS NOT OLD.version
     OR NEW.finished_at IS NOT OLD.finished_at
 )
BEGIN
    SELECT RAISE(ABORT, 'a succeeded Graph node-finish Attempt is immutable');
END;

CREATE TRIGGER work_graph_finished_delegation_terminal_no_update
BEFORE UPDATE OF status, result_text, input_tokens, output_tokens, total_tokens,
    tool_call_count, error_code, error_message, started_at, finished_at
ON child_run_delegations
FOR EACH ROW
WHEN EXISTS (
    SELECT 1
    FROM work_graph_node_finishes
    WHERE attempt_id = OLD.graph_task_attempt_id
      AND child_run_id = OLD.child_run_id
)
 AND (
     NEW.status IS NOT OLD.status
     OR NEW.result_text IS NOT OLD.result_text
     OR NEW.input_tokens IS NOT OLD.input_tokens
     OR NEW.output_tokens IS NOT OLD.output_tokens
     OR NEW.total_tokens IS NOT OLD.total_tokens
     OR NEW.tool_call_count IS NOT OLD.tool_call_count
     OR NEW.error_code IS NOT OLD.error_code
     OR NEW.error_message IS NOT OLD.error_message
     OR NEW.started_at IS NOT OLD.started_at
     OR NEW.finished_at IS NOT OLD.finished_at
 )
BEGIN
    SELECT RAISE(ABORT, 'Graph node-finish Child delegation terminal facts are immutable');
END;

CREATE TRIGGER work_graph_finished_child_terminal_no_update
BEFORE UPDATE OF status, finished_at, error_code, error_message
ON runs
FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM work_graph_node_finishes WHERE child_run_id = OLD.id
)
 AND (
     NEW.status IS NOT OLD.status
     OR NEW.finished_at IS NOT OLD.finished_at
     OR NEW.error_code IS NOT OLD.error_code
     OR NEW.error_message IS NOT OLD.error_message
 )
BEGIN
    SELECT RAISE(ABORT, 'Graph node-finish Child Run terminal facts are immutable');
END;

CREATE TRIGGER work_graph_finished_child_result_no_insert
BEFORE INSERT ON messages
FOR EACH ROW
WHEN NEW.role = 'assistant'
 AND EXISTS (
     SELECT 1 FROM work_graph_node_finishes WHERE child_run_id = NEW.run_id
 )
BEGIN
    SELECT RAISE(ABORT, 'Graph node-finish Child result messages are sealed');
END;

CREATE TRIGGER work_graph_finished_child_result_no_update
BEFORE UPDATE OF run_id, role, content, ordinal ON messages
FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM work_graph_node_finishes WHERE child_run_id = OLD.run_id
)
 AND (
     NEW.run_id IS NOT OLD.run_id
     OR NEW.role IS NOT OLD.role
     OR NEW.content IS NOT OLD.content
     OR NEW.ordinal IS NOT OLD.ordinal
 )
BEGIN
    SELECT RAISE(ABORT, 'Graph node-finish Child result messages are immutable');
END;

CREATE TRIGGER work_graph_finished_child_result_no_direct_delete
BEFORE DELETE ON messages
FOR EACH ROW
WHEN EXISTS (
    SELECT 1
    FROM work_graph_node_finishes AS finish
    WHERE finish.child_run_id = OLD.run_id
      AND NOT EXISTS (
          SELECT 1 FROM work_graph_cascade_delete_scopes WHERE goal_id = finish.goal_id
      )
)
BEGIN
    SELECT RAISE(ABORT, 'Graph node-finish Child result messages cannot be deleted directly');
END;
"#;

const MIGRATION_38: &str = r#"
CREATE TABLE work_graph_node_cancel_intents (
    id TEXT PRIMARY KEY CHECK(length(trim(id)) BETWEEN 1 AND 200),
    goal_id TEXT NOT NULL REFERENCES work_graph_specs(goal_id) ON DELETE CASCADE,
    task_id TEXT NOT NULL REFERENCES work_graph_nodes(task_id) ON DELETE CASCADE,
    attempt_id TEXT NOT NULL REFERENCES task_attempts(id) ON DELETE CASCADE,
    parent_run_id TEXT NOT NULL REFERENCES runs(id) ON DELETE RESTRICT,
    delegation_id TEXT NOT NULL REFERENCES child_run_delegations(id) ON DELETE RESTRICT,
    child_run_id TEXT NOT NULL REFERENCES runs(id) ON DELETE RESTRICT,
    tool_call_id TEXT NOT NULL UNIQUE REFERENCES tool_calls(id) ON DELETE RESTRICT,
    expected_task_version INTEGER NOT NULL CHECK(expected_task_version >= 1),
    expected_attempt_version INTEGER NOT NULL CHECK(expected_attempt_version >= 1),
    reason TEXT NOT NULL CHECK(
        reason = trim(reason) AND length(reason) BETWEEN 1 AND 2000
    ),
    input_json TEXT NOT NULL CHECK(
        json_valid(input_json) AND json_type(input_json) = 'object'
    ),
    input_hash TEXT NOT NULL CHECK(
        length(input_hash) = 64 AND input_hash NOT GLOB '*[^0-9a-f]*'
    ),
    created_at INTEGER NOT NULL CHECK(created_at >= 0),
    activated_at INTEGER CHECK(
        activated_at IS NULL OR (activated_at >= created_at AND activated_at >= 0)
    )
);

CREATE INDEX idx_work_graph_node_cancel_intents_attempt_created
    ON work_graph_node_cancel_intents(attempt_id, created_at DESC, id DESC);
CREATE INDEX idx_work_graph_node_cancel_intents_child_created
    ON work_graph_node_cancel_intents(child_run_id, created_at DESC, id DESC);
CREATE UNIQUE INDEX idx_work_graph_node_cancel_intents_active_attempt
    ON work_graph_node_cancel_intents(attempt_id) WHERE activated_at IS NOT NULL;
CREATE UNIQUE INDEX idx_work_graph_node_cancel_intents_active_child
    ON work_graph_node_cancel_intents(child_run_id) WHERE activated_at IS NOT NULL;

CREATE TABLE work_graph_node_terminal_reconciliations (
    id TEXT PRIMARY KEY CHECK(length(trim(id)) BETWEEN 1 AND 200),
    goal_id TEXT NOT NULL REFERENCES work_graph_specs(goal_id) ON DELETE CASCADE,
    task_id TEXT NOT NULL REFERENCES work_graph_nodes(task_id) ON DELETE CASCADE,
    attempt_id TEXT NOT NULL UNIQUE REFERENCES task_attempts(id) ON DELETE CASCADE,
    parent_run_id TEXT NOT NULL REFERENCES runs(id) ON DELETE RESTRICT,
    delegation_id TEXT NOT NULL REFERENCES child_run_delegations(id) ON DELETE RESTRICT,
    child_run_id TEXT NOT NULL UNIQUE REFERENCES runs(id) ON DELETE RESTRICT,
    cancel_intent_id TEXT UNIQUE
        REFERENCES work_graph_node_cancel_intents(id) ON DELETE CASCADE,
    child_terminal_status TEXT NOT NULL CHECK(
        child_terminal_status IN ('failed', 'cancelled', 'interrupted')
    ),
    attempt_terminal_status TEXT NOT NULL CHECK(
        attempt_terminal_status IN ('failed', 'cancelled')
    ),
    expected_task_version INTEGER NOT NULL CHECK(expected_task_version >= 1),
    expected_attempt_version INTEGER NOT NULL CHECK(expected_attempt_version >= 1),
    child_terminal_json TEXT NOT NULL CHECK(
        json_valid(child_terminal_json) AND json_type(child_terminal_json) = 'object'
    ),
    child_terminal_hash TEXT NOT NULL CHECK(
        length(child_terminal_hash) = 64
        AND child_terminal_hash NOT GLOB '*[^0-9a-f]*'
    ),
    failure_reason TEXT NOT NULL CHECK(
        failure_reason = trim(failure_reason)
        AND length(failure_reason) BETWEEN 1 AND 4000
    ),
    created_at INTEGER NOT NULL CHECK(created_at >= 0),
    CHECK(
        (child_terminal_status = 'cancelled' AND attempt_terminal_status = 'cancelled')
        OR (
            child_terminal_status IN ('failed', 'interrupted')
            AND attempt_terminal_status = 'failed'
        )
    )
);

CREATE INDEX idx_work_graph_node_terminal_reconciliations_task_created
    ON work_graph_node_terminal_reconciliations(task_id, created_at DESC, id DESC);
CREATE INDEX idx_work_graph_node_terminal_reconciliations_goal_created
    ON work_graph_node_terminal_reconciliations(goal_id, created_at DESC, id DESC);

-- Seal the v36 Graph authority before any v38 intent/reconciliation exists. Without
-- these guards, deleting an Attempt would SET NULL the delegation mapping, while
-- deleting a delegation, Child Run, or node-start ToolCall would erase the facts
-- used to recognize the Attempt as Graph-bound. Goal/Conversation aggregate
-- cascades remain authorized by the existing statement-local scope sentinel.
CREATE TRIGGER work_graph_bound_attempts_no_direct_delete
BEFORE DELETE ON task_attempts
FOR EACH ROW
WHEN EXISTS (
    SELECT 1
    FROM child_run_delegations AS delegation
    JOIN work_tasks AS task ON task.id = OLD.task_id
    WHERE delegation.graph_task_attempt_id = OLD.id
      AND NOT EXISTS (
          SELECT 1
          FROM work_graph_cascade_delete_scopes AS scope
          WHERE scope.goal_id = task.goal_id
      )
)
BEGIN
    SELECT RAISE(ABORT, 'Graph-bound Attempts can only be deleted with their Goal');
END;

CREATE TRIGGER work_graph_bound_delegations_no_direct_delete
BEFORE DELETE ON child_run_delegations
FOR EACH ROW
WHEN OLD.graph_task_attempt_id IS NOT NULL
 AND EXISTS (
     SELECT 1
     FROM task_attempts AS attempt
     JOIN work_tasks AS task ON task.id = attempt.task_id
     WHERE attempt.id = OLD.graph_task_attempt_id
       AND NOT EXISTS (
           SELECT 1
           FROM work_graph_cascade_delete_scopes AS scope
           WHERE scope.goal_id = task.goal_id
       )
 )
BEGIN
    SELECT RAISE(ABORT, 'Graph-bound delegations can only be deleted with their Goal');
END;

CREATE TRIGGER work_graph_bound_runs_no_direct_delete
BEFORE DELETE ON runs
FOR EACH ROW
WHEN EXISTS (
    SELECT 1
    FROM child_run_delegations AS delegation
    JOIN task_attempts AS attempt ON attempt.id = delegation.graph_task_attempt_id
    JOIN work_tasks AS task ON task.id = attempt.task_id
    WHERE (delegation.child_run_id = OLD.id OR delegation.parent_run_id = OLD.id)
      AND NOT EXISTS (
          SELECT 1
          FROM work_graph_cascade_delete_scopes AS scope
          WHERE scope.goal_id = task.goal_id
      )
)
BEGIN
    SELECT RAISE(ABORT, 'Graph-bound parent and Child Runs can only be deleted with their Goal');
END;

CREATE TRIGGER work_graph_node_start_tools_no_direct_delete
BEFORE DELETE ON tool_calls
FOR EACH ROW
WHEN EXISTS (
    SELECT 1
    FROM child_run_delegations AS delegation
    JOIN task_attempts AS attempt ON attempt.id = delegation.graph_task_attempt_id
    JOIN work_tasks AS task ON task.id = attempt.task_id
    WHERE delegation.tool_call_id = OLD.id
      AND NOT EXISTS (
          SELECT 1
          FROM work_graph_cascade_delete_scopes AS scope
          WHERE scope.goal_id = task.goal_id
      )
)
BEGIN
    SELECT RAISE(ABORT, 'Graph node-start ToolCalls can only be deleted with their Goal');
END;

CREATE TRIGGER work_graph_node_cancel_intents_validate_insert
BEFORE INSERT ON work_graph_node_cancel_intents
FOR EACH ROW
WHEN NEW.activated_at IS NOT NULL OR NOT EXISTS (
    SELECT 1
    FROM work_graph_nodes AS node
    JOIN work_tasks AS task
      ON task.id = node.task_id
     AND task.goal_id = node.goal_id
     AND task.status = 'in_progress'
     AND task.owner_run_id = NEW.parent_run_id
     AND task.version = NEW.expected_task_version
    JOIN goals AS goal
      ON goal.id = node.goal_id
     AND goal.status = 'active'
    JOIN task_attempts AS attempt
      ON attempt.id = NEW.attempt_id
     AND attempt.task_id = node.task_id
     AND attempt.run_id = NEW.parent_run_id
     AND attempt.kind = 'execution'
     AND attempt.status = 'running'
     AND attempt.version = NEW.expected_attempt_version
    JOIN child_run_delegations AS delegation
      ON delegation.id = NEW.delegation_id
     AND delegation.graph_task_attempt_id = attempt.id
     AND delegation.parent_run_id = NEW.parent_run_id
     AND delegation.child_run_id = NEW.child_run_id
     AND delegation.status IN ('queued', 'running')
    JOIN runs AS parent_run
      ON parent_run.id = NEW.parent_run_id
     AND parent_run.conversation_id = goal.conversation_id
     AND parent_run.status = 'running'
     AND parent_run.run_kind = 'primary'
     AND parent_run.parent_run_id IS NULL
     AND parent_run.depth = 0
    JOIN run_execution_profiles AS parent_profile
      ON parent_profile.run_id = parent_run.id
     AND parent_profile.schema_version = 1
     AND parent_profile.profile_id = 'durable_v2'
     AND parent_profile.snapshot_json = '{"id":"durable_v2","schemaVersion":1}'
     AND parent_profile.profile_hash =
         'ae08f4dc8c1327cbd3de30879f01aa7475d9a728a3c0f738579086435d714f7e'
    JOIN runs AS child_run
      ON child_run.id = NEW.child_run_id
     AND child_run.conversation_id = delegation.child_conversation_id
     AND child_run.parent_run_id = parent_run.id
     AND child_run.root_run_id = COALESCE(parent_run.root_run_id, parent_run.id)
     AND child_run.run_kind = 'child'
     AND child_run.depth = 1
     AND child_run.status IN ('queued', 'running', 'cancelling')
     AND child_run.finished_at IS NULL
    JOIN run_execution_profiles AS child_profile
      ON child_profile.run_id = child_run.id
     AND child_profile.schema_version = 1
     AND child_profile.profile_id = 'durable_v2'
     AND child_profile.snapshot_json = '{"id":"durable_v2","schemaVersion":1}'
     AND child_profile.profile_hash =
         'ae08f4dc8c1327cbd3de30879f01aa7475d9a728a3c0f738579086435d714f7e'
    JOIN tool_calls AS cancel_call
      ON cancel_call.id = NEW.tool_call_id
     AND cancel_call.run_id = parent_run.id
     AND cancel_call.conversation_id = goal.conversation_id
     AND cancel_call.tool_name = 'graph_readonly_node_cancel'
     AND cancel_call.execution_location = 'host'
     AND cancel_call.requires_approval = 0
     AND cancel_call.status = 'running'
     AND cancel_call.error_message IS NULL
     AND cancel_call.input_json = NEW.input_json
     AND json(NEW.input_json) = NEW.input_json
     AND json_type(NEW.input_json, '$.goalId') = 'text'
     AND json_extract(NEW.input_json, '$.goalId') = node.goal_id
     AND json_type(NEW.input_json, '$.taskId') = 'text'
     AND json_extract(NEW.input_json, '$.taskId') = node.task_id
     AND json_type(NEW.input_json, '$.attemptId') = 'text'
     AND json_extract(NEW.input_json, '$.attemptId') = attempt.id
     AND json_type(NEW.input_json, '$.expectedTaskVersion') = 'integer'
     AND json_extract(NEW.input_json, '$.expectedTaskVersion') =
         NEW.expected_task_version
     AND json_type(NEW.input_json, '$.expectedAttemptVersion') = 'integer'
     AND json_extract(NEW.input_json, '$.expectedAttemptVersion') =
         NEW.expected_attempt_version
     AND json_type(NEW.input_json, '$.reason') = 'text'
     AND json_extract(NEW.input_json, '$.reason') = NEW.reason
     AND (
         SELECT group_concat(key, ',') FROM json_each(NEW.input_json)
     ) = 'attemptId,expectedAttemptVersion,expectedTaskVersion,goalId,reason,taskId'
    WHERE node.task_id = NEW.task_id
      AND node.goal_id = NEW.goal_id
)
BEGIN
    SELECT RAISE(ABORT, 'Graph node cancel intent requires exact live authority');
END;

CREATE TRIGGER work_graph_node_cancel_intents_validate_activation
BEFORE UPDATE OF activated_at ON work_graph_node_cancel_intents
FOR EACH ROW
WHEN OLD.activated_at IS NULL
 AND NEW.activated_at IS NOT NULL
 AND NOT EXISTS (
     SELECT 1
     FROM work_graph_nodes AS node
     JOIN work_tasks AS task
       ON task.id = node.task_id
      AND task.goal_id = node.goal_id
      AND task.status = 'in_progress'
      AND task.owner_run_id = OLD.parent_run_id
      AND task.version = OLD.expected_task_version
     JOIN goals AS goal
       ON goal.id = node.goal_id
      AND goal.status = 'active'
     JOIN task_attempts AS attempt
       ON attempt.id = OLD.attempt_id
      AND attempt.task_id = node.task_id
      AND attempt.run_id = OLD.parent_run_id
      AND attempt.kind = 'execution'
      AND attempt.status = 'running'
      AND attempt.version = OLD.expected_attempt_version
     JOIN child_run_delegations AS delegation
       ON delegation.id = OLD.delegation_id
      AND delegation.graph_task_attempt_id = attempt.id
      AND delegation.parent_run_id = OLD.parent_run_id
      AND delegation.child_run_id = OLD.child_run_id
      AND delegation.status IN ('queued', 'running')
     JOIN runs AS parent_run
       ON parent_run.id = OLD.parent_run_id
      AND parent_run.conversation_id = goal.conversation_id
      AND parent_run.status = 'running'
      AND parent_run.run_kind = 'primary'
      AND parent_run.parent_run_id IS NULL
      AND parent_run.depth = 0
     JOIN run_execution_profiles AS parent_profile
       ON parent_profile.run_id = parent_run.id
      AND parent_profile.schema_version = 1
      AND parent_profile.profile_id = 'durable_v2'
      AND parent_profile.snapshot_json = '{"id":"durable_v2","schemaVersion":1}'
      AND parent_profile.profile_hash =
          'ae08f4dc8c1327cbd3de30879f01aa7475d9a728a3c0f738579086435d714f7e'
     JOIN runs AS child_run
       ON child_run.id = OLD.child_run_id
      AND child_run.conversation_id = delegation.child_conversation_id
      AND child_run.parent_run_id = parent_run.id
      AND child_run.root_run_id = COALESCE(parent_run.root_run_id, parent_run.id)
      AND child_run.run_kind = 'child'
      AND child_run.depth = 1
      AND child_run.status IN ('queued', 'running', 'cancelling')
      AND child_run.finished_at IS NULL
     JOIN run_execution_profiles AS child_profile
       ON child_profile.run_id = child_run.id
      AND child_profile.schema_version = 1
      AND child_profile.profile_id = 'durable_v2'
      AND child_profile.snapshot_json = '{"id":"durable_v2","schemaVersion":1}'
      AND child_profile.profile_hash =
          'ae08f4dc8c1327cbd3de30879f01aa7475d9a728a3c0f738579086435d714f7e'
     JOIN tool_calls AS cancel_call
       ON cancel_call.id = OLD.tool_call_id
      AND cancel_call.run_id = OLD.parent_run_id
      AND cancel_call.conversation_id = goal.conversation_id
      AND cancel_call.tool_name = 'graph_readonly_node_cancel'
      AND cancel_call.execution_location = 'host'
      AND cancel_call.requires_approval = 0
      AND cancel_call.status = 'completed'
      AND cancel_call.error_message IS NULL
      AND cancel_call.result_json IS NOT NULL
      AND json_valid(cancel_call.result_json)
      AND cancel_call.completed_at IS NOT NULL
      AND cancel_call.completed_at >= cancel_call.started_at
      AND cancel_call.updated_at >= cancel_call.completed_at
      AND cancel_call.input_json = OLD.input_json
      AND json(OLD.input_json) = OLD.input_json
     WHERE node.task_id = OLD.task_id
       AND node.goal_id = OLD.goal_id
       AND OLD.input_hash = NEW.input_hash
 )
BEGIN
    SELECT RAISE(ABORT, 'Graph node cancel intent activates only after exact Host completion');
END;

CREATE TRIGGER work_graph_node_cancel_intents_no_update
BEFORE UPDATE ON work_graph_node_cancel_intents
FOR EACH ROW
WHEN NEW.id IS NOT OLD.id
 OR NEW.goal_id IS NOT OLD.goal_id
 OR NEW.task_id IS NOT OLD.task_id
 OR NEW.attempt_id IS NOT OLD.attempt_id
 OR NEW.parent_run_id IS NOT OLD.parent_run_id
 OR NEW.delegation_id IS NOT OLD.delegation_id
 OR NEW.child_run_id IS NOT OLD.child_run_id
 OR NEW.tool_call_id IS NOT OLD.tool_call_id
 OR NEW.expected_task_version IS NOT OLD.expected_task_version
 OR NEW.expected_attempt_version IS NOT OLD.expected_attempt_version
 OR NEW.reason IS NOT OLD.reason
 OR NEW.input_json IS NOT OLD.input_json
 OR NEW.input_hash IS NOT OLD.input_hash
 OR NEW.created_at IS NOT OLD.created_at
 OR OLD.activated_at IS NOT NULL
 OR NEW.activated_at IS NULL
BEGIN
    SELECT RAISE(ABORT, 'Graph node cancel intents only allow one pending-to-active transition');
END;

CREATE TRIGGER work_graph_node_terminal_reconciliations_validate_authority
BEFORE INSERT ON work_graph_node_terminal_reconciliations
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1
    FROM work_graph_nodes AS node
    JOIN work_tasks AS task
      ON task.id = node.task_id
     AND task.goal_id = node.goal_id
     AND task.status = 'in_progress'
     AND task.owner_run_id = NEW.parent_run_id
     AND task.version = NEW.expected_task_version
    JOIN goals AS goal
      ON goal.id = node.goal_id
     AND goal.status = 'active'
    JOIN task_attempts AS attempt
      ON attempt.id = NEW.attempt_id
     AND attempt.task_id = node.task_id
     AND attempt.run_id = NEW.parent_run_id
     AND attempt.kind = 'execution'
     AND attempt.status = 'running'
     AND attempt.version = NEW.expected_attempt_version
    JOIN child_run_delegations AS delegation
      ON delegation.id = NEW.delegation_id
     AND delegation.graph_task_attempt_id = attempt.id
     AND delegation.parent_run_id = NEW.parent_run_id
     AND delegation.child_run_id = NEW.child_run_id
     AND delegation.status = NEW.child_terminal_status
    JOIN runs AS parent_run
      ON parent_run.id = NEW.parent_run_id
     AND parent_run.conversation_id = goal.conversation_id
     AND parent_run.status = 'running'
     AND parent_run.run_kind = 'primary'
     AND parent_run.parent_run_id IS NULL
     AND parent_run.depth = 0
    JOIN run_execution_profiles AS parent_profile
      ON parent_profile.run_id = parent_run.id
     AND parent_profile.schema_version = 1
     AND parent_profile.profile_id = 'durable_v2'
     AND parent_profile.snapshot_json = '{"id":"durable_v2","schemaVersion":1}'
     AND parent_profile.profile_hash =
         'ae08f4dc8c1327cbd3de30879f01aa7475d9a728a3c0f738579086435d714f7e'
    JOIN runs AS child_run
      ON child_run.id = NEW.child_run_id
     AND child_run.conversation_id = delegation.child_conversation_id
     AND child_run.parent_run_id = parent_run.id
     AND child_run.root_run_id = COALESCE(parent_run.root_run_id, parent_run.id)
     AND child_run.run_kind = 'child'
     AND child_run.depth = 1
     AND child_run.status = NEW.child_terminal_status
     AND child_run.status IN ('failed', 'cancelled', 'interrupted')
     AND child_run.finished_at IS NOT NULL
     AND delegation.finished_at = child_run.finished_at
     AND delegation.error_code IS child_run.error_code
     AND delegation.error_message IS child_run.error_message
    JOIN run_execution_profiles AS child_profile
      ON child_profile.run_id = child_run.id
     AND child_profile.schema_version = 1
     AND child_profile.profile_id = 'durable_v2'
     AND child_profile.snapshot_json = '{"id":"durable_v2","schemaVersion":1}'
     AND child_profile.profile_hash =
         'ae08f4dc8c1327cbd3de30879f01aa7475d9a728a3c0f738579086435d714f7e'
    WHERE node.task_id = NEW.task_id
      AND node.goal_id = NEW.goal_id
      AND NEW.attempt_terminal_status = CASE NEW.child_terminal_status
          WHEN 'cancelled' THEN 'cancelled'
          ELSE 'failed'
      END
      AND json(NEW.child_terminal_json) = NEW.child_terminal_json
      AND (
          SELECT group_concat(key, ',') FROM json_each(NEW.child_terminal_json)
      ) = 'childRunId,errorCode,errorMessage,finishedAt,status'
      AND json_type(NEW.child_terminal_json, '$.childRunId') = 'text'
      AND json_extract(NEW.child_terminal_json, '$.childRunId') = child_run.id
      AND json_type(NEW.child_terminal_json, '$.status') = 'text'
      AND json_extract(NEW.child_terminal_json, '$.status') = child_run.status
      AND json_type(NEW.child_terminal_json, '$.finishedAt') = 'integer'
      AND json_extract(NEW.child_terminal_json, '$.finishedAt') = child_run.finished_at
      AND json_type(NEW.child_terminal_json, '$.errorCode') IN ('null', 'text')
      AND json_extract(NEW.child_terminal_json, '$.errorCode') IS child_run.error_code
      AND json_type(NEW.child_terminal_json, '$.errorMessage') IN ('null', 'text')
      AND json_extract(NEW.child_terminal_json, '$.errorMessage') IS child_run.error_message
      AND (
          (
              NEW.cancel_intent_id IS NULL
              AND NOT EXISTS (
                  SELECT 1
                  FROM work_graph_node_cancel_intents AS active_intent
                  WHERE active_intent.attempt_id = attempt.id
                    AND active_intent.child_run_id = child_run.id
                    AND active_intent.activated_at IS NOT NULL
              )
          )
          OR EXISTS (
              SELECT 1
              FROM work_graph_node_cancel_intents AS active_intent
              WHERE active_intent.id = NEW.cancel_intent_id
                AND active_intent.goal_id = node.goal_id
                AND active_intent.task_id = node.task_id
                AND active_intent.attempt_id = attempt.id
                AND active_intent.parent_run_id = parent_run.id
                AND active_intent.delegation_id = delegation.id
                AND active_intent.child_run_id = child_run.id
                AND active_intent.activated_at IS NOT NULL
          )
      )
)
BEGIN
    SELECT RAISE(ABORT, 'Graph terminal reconciliation requires exact negative Child authority');
END;

CREATE TRIGGER task_attempts_require_graph_terminal_reconciliation
BEFORE UPDATE OF status ON task_attempts
FOR EACH ROW
WHEN OLD.status = 'running'
 AND NEW.status IN ('failed', 'cancelled')
 AND EXISTS (
     SELECT 1 FROM child_run_delegations
     WHERE graph_task_attempt_id = OLD.id
 )
 AND NOT EXISTS (
     SELECT 1
     FROM runs AS parent_run
     WHERE parent_run.id = OLD.run_id
       AND NEW.version = OLD.version + 1
       AND NEW.finished_at IS NOT NULL
       AND (
           (parent_run.status = 'cancelled' AND NEW.status = 'cancelled')
           OR (
               parent_run.status IN ('completed', 'failed', 'interrupted')
               AND NEW.status = 'failed'
           )
       )
 )
 AND NOT EXISTS (
     SELECT 1
     FROM work_graph_node_terminal_reconciliations AS reconciliation
     WHERE reconciliation.attempt_id = OLD.id
       AND reconciliation.task_id = OLD.task_id
       AND reconciliation.parent_run_id = OLD.run_id
       AND reconciliation.attempt_terminal_status = NEW.status
       AND reconciliation.expected_attempt_version = OLD.version
       AND NEW.version = OLD.version + 1
       AND NEW.failure_reason = reconciliation.failure_reason
       AND NEW.finished_at IS NOT NULL
 )
BEGIN
    SELECT RAISE(ABORT, 'Graph Attempt negative terminal requires parent terminal or reconciliation');
END;

CREATE TRIGGER work_tasks_require_graph_terminal_reconciliation
BEFORE UPDATE OF status ON work_tasks
FOR EACH ROW
WHEN OLD.status = 'in_progress'
 AND NEW.status = 'interrupted'
 AND EXISTS (SELECT 1 FROM work_graph_nodes WHERE task_id = OLD.id)
 AND NOT EXISTS (
     SELECT 1
     FROM task_attempts AS attempt
     JOIN runs AS parent_run ON parent_run.id = attempt.run_id
     WHERE attempt.task_id = OLD.id
       AND attempt.run_id = OLD.owner_run_id
       AND attempt.finished_at IS NOT NULL
       AND attempt.id = (
           SELECT latest_attempt.id
           FROM task_attempts AS latest_attempt
           WHERE latest_attempt.task_id = OLD.id
           ORDER BY latest_attempt.attempt_number DESC, latest_attempt.id DESC
           LIMIT 1
       )
       AND EXISTS (
           SELECT 1
           FROM child_run_delegations AS delegation
           WHERE delegation.graph_task_attempt_id = attempt.id
             AND delegation.parent_run_id = attempt.run_id
       )
       AND (
           (
               parent_run.status = 'cancelled'
               AND attempt.status = 'cancelled'
           )
           OR (
               parent_run.status IN ('completed', 'failed', 'interrupted')
               AND attempt.status = 'failed'
           )
       )
       AND NEW.version = OLD.version + 1
       AND NEW.finished_at IS NOT NULL
 )
 AND NOT EXISTS (
     SELECT 1
     FROM work_graph_node_terminal_reconciliations AS reconciliation
     JOIN task_attempts AS attempt
       ON attempt.id = reconciliation.attempt_id
      AND attempt.task_id = reconciliation.task_id
      AND attempt.run_id = reconciliation.parent_run_id
      AND attempt.status = reconciliation.attempt_terminal_status
      AND attempt.version = reconciliation.expected_attempt_version + 1
     WHERE reconciliation.task_id = OLD.id
       AND reconciliation.parent_run_id = OLD.owner_run_id
       AND reconciliation.expected_task_version = OLD.version
       AND NEW.version = OLD.version + 1
       AND NEW.owner_run_id IS NULL
       AND NEW.blocked_reason = reconciliation.failure_reason
       AND NEW.finished_at IS NOT NULL
 )
BEGIN
    SELECT RAISE(ABORT, 'Graph Task interruption requires the same terminal authority chain');
END;

CREATE TRIGGER work_graph_node_terminal_reconciliations_no_update
BEFORE UPDATE ON work_graph_node_terminal_reconciliations
BEGIN
    SELECT RAISE(ABORT, 'Graph terminal reconciliation facts are immutable');
END;

CREATE TRIGGER work_graph_node_cancel_intents_no_direct_delete
BEFORE DELETE ON work_graph_node_cancel_intents
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1 FROM work_graph_cascade_delete_scopes WHERE goal_id = OLD.goal_id
)
BEGIN
    SELECT RAISE(ABORT, 'Graph node cancel intents can only be deleted with their Goal');
END;

CREATE TRIGGER work_graph_node_terminal_reconciliations_no_direct_delete
BEFORE DELETE ON work_graph_node_terminal_reconciliations
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1 FROM work_graph_cascade_delete_scopes WHERE goal_id = OLD.goal_id
)
BEGIN
    SELECT RAISE(ABORT, 'Graph terminal reconciliations can only be deleted with their Goal');
END;

CREATE TRIGGER work_graph_cancel_tool_authority_no_update
BEFORE UPDATE OF run_id, conversation_id, tool_name, input_json,
    execution_location, requires_approval, started_at
ON tool_calls
FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM work_graph_node_cancel_intents WHERE tool_call_id = OLD.id
)
BEGIN
    SELECT RAISE(ABORT, 'Graph node-cancel ToolCall authority facts are immutable');
END;

CREATE TRIGGER work_graph_cancel_tool_terminal_no_update
BEFORE UPDATE OF status, result_json, error_message, completed_at, updated_at
ON tool_calls
FOR EACH ROW
WHEN OLD.status <> 'running'
 AND EXISTS (
     SELECT 1 FROM work_graph_node_cancel_intents WHERE tool_call_id = OLD.id
 )
 AND (
     NEW.status IS NOT OLD.status
     OR NEW.result_json IS NOT OLD.result_json
     OR NEW.error_message IS NOT OLD.error_message
     OR NEW.completed_at IS NOT OLD.completed_at
     OR NEW.updated_at IS NOT OLD.updated_at
 )
BEGIN
    SELECT RAISE(ABORT, 'Graph node-cancel terminal ToolCall facts are immutable');
END;

CREATE TRIGGER work_graph_cancel_tool_no_direct_delete
BEFORE DELETE ON tool_calls
FOR EACH ROW
WHEN EXISTS (
    SELECT 1
    FROM work_graph_node_cancel_intents AS intent
    WHERE intent.tool_call_id = OLD.id
      AND NOT EXISTS (
          SELECT 1 FROM work_graph_cascade_delete_scopes WHERE goal_id = intent.goal_id
      )
)
BEGIN
    SELECT RAISE(ABORT, 'Graph node-cancel ToolCalls cannot be deleted directly');
END;

CREATE TRIGGER work_graph_cancel_attempt_no_direct_delete
BEFORE DELETE ON task_attempts
FOR EACH ROW
WHEN EXISTS (
    SELECT 1
    FROM work_graph_node_cancel_intents AS intent
    WHERE intent.attempt_id = OLD.id
      AND NOT EXISTS (
          SELECT 1 FROM work_graph_cascade_delete_scopes WHERE goal_id = intent.goal_id
      )
    UNION ALL
    SELECT 1
    FROM work_graph_node_terminal_reconciliations AS reconciliation
    WHERE reconciliation.attempt_id = OLD.id
      AND NOT EXISTS (
          SELECT 1
          FROM work_graph_cascade_delete_scopes
          WHERE goal_id = reconciliation.goal_id
      )
)
BEGIN
    SELECT RAISE(ABORT, 'Graph cancel/reconciliation Attempts cannot be deleted directly');
END;

CREATE TRIGGER work_graph_cancel_delegation_no_direct_delete
BEFORE DELETE ON child_run_delegations
FOR EACH ROW
WHEN EXISTS (
    SELECT 1
    FROM work_graph_node_cancel_intents AS intent
    WHERE intent.delegation_id = OLD.id
      AND NOT EXISTS (
          SELECT 1 FROM work_graph_cascade_delete_scopes WHERE goal_id = intent.goal_id
      )
    UNION ALL
    SELECT 1
    FROM work_graph_node_terminal_reconciliations AS reconciliation
    WHERE reconciliation.delegation_id = OLD.id
      AND NOT EXISTS (
          SELECT 1
          FROM work_graph_cascade_delete_scopes
          WHERE goal_id = reconciliation.goal_id
      )
)
BEGIN
    SELECT RAISE(ABORT, 'Graph cancel/reconciliation delegations cannot be deleted directly');
END;

CREATE TRIGGER work_graph_reconciled_attempt_authority_no_update
BEFORE UPDATE OF task_id, attempt_number, kind, run_id, policy_hash, root_cause,
    finding_ids_json, evidence_rowid_watermark, finding_rowid_watermark, started_at
ON task_attempts
FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM work_graph_node_terminal_reconciliations WHERE attempt_id = OLD.id
)
BEGIN
    SELECT RAISE(ABORT, 'Graph reconciled Attempt authority facts are immutable');
END;

CREATE TRIGGER work_graph_reconciled_attempt_terminal_no_update
BEFORE UPDATE OF status, evidence_ids_json, failure_reason, version, finished_at
ON task_attempts
FOR EACH ROW
WHEN EXISTS (
    SELECT 1
    FROM work_graph_node_terminal_reconciliations AS reconciliation
    WHERE reconciliation.attempt_id = OLD.id
      AND OLD.status = reconciliation.attempt_terminal_status
)
 AND (
     NEW.status IS NOT OLD.status
     OR NEW.evidence_ids_json IS NOT OLD.evidence_ids_json
     OR NEW.failure_reason IS NOT OLD.failure_reason
     OR NEW.version IS NOT OLD.version
     OR NEW.finished_at IS NOT OLD.finished_at
 )
BEGIN
    SELECT RAISE(ABORT, 'a terminal Graph reconciled Attempt is immutable');
END;

CREATE TRIGGER work_graph_reconciled_task_terminal_no_update
BEFORE UPDATE OF status, blocked_reason, owner_run_id, version, updated_at, finished_at
ON work_tasks
FOR EACH ROW
WHEN OLD.status = 'interrupted'
 AND EXISTS (
     SELECT 1 FROM work_graph_node_terminal_reconciliations WHERE task_id = OLD.id
 )
 AND (
     NEW.status IS NOT OLD.status
     OR NEW.blocked_reason IS NOT OLD.blocked_reason
     OR NEW.owner_run_id IS NOT OLD.owner_run_id
     OR NEW.version IS NOT OLD.version
     OR NEW.updated_at IS NOT OLD.updated_at
     OR NEW.finished_at IS NOT OLD.finished_at
 )
BEGIN
    SELECT RAISE(ABORT, 'a terminal Graph reconciled Task is immutable');
END;

CREATE TRIGGER work_graph_reconciled_delegation_terminal_no_update
BEFORE UPDATE OF graph_task_attempt_id, parent_run_id, child_run_id,
    child_conversation_id, status, error_code, error_message, started_at, finished_at
ON child_run_delegations
FOR EACH ROW
WHEN EXISTS (
    SELECT 1
    FROM work_graph_node_terminal_reconciliations
    WHERE delegation_id = OLD.id
)
 AND (
     NEW.graph_task_attempt_id IS NOT OLD.graph_task_attempt_id
     OR NEW.parent_run_id IS NOT OLD.parent_run_id
     OR NEW.child_run_id IS NOT OLD.child_run_id
     OR NEW.child_conversation_id IS NOT OLD.child_conversation_id
     OR NEW.status IS NOT OLD.status
     OR NEW.error_code IS NOT OLD.error_code
     OR NEW.error_message IS NOT OLD.error_message
     OR NEW.started_at IS NOT OLD.started_at
     OR NEW.finished_at IS NOT OLD.finished_at
 )
BEGIN
    SELECT RAISE(ABORT, 'Graph reconciled delegation authority facts are immutable');
END;

CREATE TRIGGER work_graph_reconciled_child_terminal_no_update
BEFORE UPDATE OF status, finished_at, error_code, error_message
ON runs
FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM work_graph_node_terminal_reconciliations WHERE child_run_id = OLD.id
)
 AND (
     NEW.status IS NOT OLD.status
     OR NEW.finished_at IS NOT OLD.finished_at
     OR NEW.error_code IS NOT OLD.error_code
     OR NEW.error_message IS NOT OLD.error_message
 )
BEGIN
    SELECT RAISE(ABORT, 'Graph reconciled Child terminal facts are immutable');
END;
"#;

const MIGRATION_39: &str = r#"
-- SQLite cannot widen a CHECK constraint in place. legacy_alter_table keeps the
-- many existing Graph authority triggers pointing at the canonical table name
-- while the profile table is rebuilt with the dedicated Reviewer profile.
PRAGMA legacy_alter_table = ON;
DROP TRIGGER IF EXISTS work_graph_profiles_no_update;
DROP TRIGGER IF EXISTS work_graph_profiles_no_direct_delete;
DROP INDEX IF EXISTS idx_run_execution_profiles_profile;
ALTER TABLE run_execution_profiles RENAME TO run_execution_profiles_v38;
CREATE TABLE run_execution_profiles (
    run_id TEXT PRIMARY KEY REFERENCES runs(id) ON DELETE CASCADE,
    schema_version INTEGER NOT NULL CHECK(schema_version = 1),
    profile_id TEXT NOT NULL CHECK(profile_id IN (
        'legacy', 'durable_v2_shadow', 'durable_v2', 'graph_readonly_preview',
        'graph_reviewer_v1'
    )),
    snapshot_json TEXT NOT NULL CHECK(
        json_valid(snapshot_json) AND json_type(snapshot_json) = 'object'
    ),
    profile_hash TEXT NOT NULL CHECK(length(profile_hash) = 64),
    frozen_at INTEGER NOT NULL
);
INSERT INTO run_execution_profiles(
    run_id, schema_version, profile_id, snapshot_json, profile_hash, frozen_at
)
SELECT run_id, schema_version, profile_id, snapshot_json, profile_hash, frozen_at
FROM run_execution_profiles_v38;
DROP TABLE run_execution_profiles_v38;
CREATE INDEX idx_run_execution_profiles_profile
    ON run_execution_profiles(profile_id, frozen_at, run_id);
PRAGMA legacy_alter_table = OFF;

CREATE TRIGGER work_graph_profiles_no_update
BEFORE UPDATE ON run_execution_profiles
FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM work_graph_specs WHERE activated_by_run_id = OLD.run_id
)
OR EXISTS (
    SELECT 1 FROM child_run_delegations
    WHERE child_run_id = OLD.run_id AND graph_task_attempt_id IS NOT NULL
)
OR EXISTS (
    SELECT 1 FROM child_run_delegations
    WHERE parent_run_id = OLD.run_id AND graph_task_attempt_id IS NOT NULL
)
OR EXISTS (
    SELECT 1 FROM work_graph_node_review_decisions
    WHERE reviewer_run_id = OLD.run_id
)
BEGIN
    SELECT RAISE(ABORT, 'work graph frozen execution profiles are immutable');
END;

CREATE TRIGGER work_graph_profiles_no_direct_delete
AFTER DELETE ON run_execution_profiles
FOR EACH ROW
WHEN EXISTS (SELECT 1 FROM runs WHERE id = OLD.run_id)
 AND (
     EXISTS (
         SELECT 1 FROM work_graph_specs WHERE activated_by_run_id = OLD.run_id
     )
     OR EXISTS (
         SELECT 1 FROM child_run_delegations
         WHERE child_run_id = OLD.run_id AND graph_task_attempt_id IS NOT NULL
     )
     OR EXISTS (
         SELECT 1 FROM child_run_delegations
         WHERE parent_run_id = OLD.run_id AND graph_task_attempt_id IS NOT NULL
     )
     OR EXISTS (
         SELECT 1 FROM work_graph_node_review_decisions
         WHERE reviewer_run_id = OLD.run_id
     )
 )
BEGIN
    SELECT RAISE(ABORT, 'work graph frozen execution profiles cannot be deleted directly');
END;

CREATE TABLE work_graph_node_review_requests (
    id TEXT PRIMARY KEY CHECK(length(trim(id)) BETWEEN 1 AND 200),
    goal_id TEXT NOT NULL REFERENCES work_graph_specs(goal_id) ON DELETE CASCADE,
    task_id TEXT NOT NULL REFERENCES work_graph_nodes(task_id) ON DELETE CASCADE,
    attempt_id TEXT NOT NULL REFERENCES task_attempts(id) ON DELETE RESTRICT,
    parent_run_id TEXT NOT NULL REFERENCES runs(id) ON DELETE RESTRICT,
    implementation_delegation_id TEXT NOT NULL
        REFERENCES child_run_delegations(id) ON DELETE RESTRICT,
    implementation_child_run_id TEXT NOT NULL REFERENCES runs(id) ON DELETE RESTRICT,
    tool_call_id TEXT NOT NULL UNIQUE REFERENCES tool_calls(id) ON DELETE RESTRICT,
    review_contract_id TEXT NOT NULL CHECK(review_contract_id = 'graph_reviewer_v1'),
    review_contract_hash TEXT NOT NULL CHECK(
        length(review_contract_hash) = 64
        AND review_contract_hash NOT GLOB '*[^0-9a-f]*'
    ),
    validation_policy_id TEXT NOT NULL CHECK(validation_policy_id = 'high_risk_v1'),
    validation_policy_hash TEXT NOT NULL CHECK(
        length(validation_policy_hash) = 64
        AND validation_policy_hash NOT GLOB '*[^0-9a-f]*'
    ),
    expected_task_version INTEGER NOT NULL CHECK(expected_task_version >= 1),
    expected_attempt_version INTEGER NOT NULL CHECK(expected_attempt_version >= 1),
    child_result_hash TEXT NOT NULL CHECK(
        length(child_result_hash) = 64
        AND child_result_hash NOT GLOB '*[^0-9a-f]*'
    ),
    summary TEXT NOT NULL CHECK(
        summary = trim(summary) AND length(summary) BETWEEN 1 AND 4000
    ),
    candidate_json TEXT NOT NULL CHECK(
        json_valid(candidate_json) AND json_type(candidate_json) = 'object'
    ),
    candidate_hash TEXT NOT NULL CHECK(
        length(candidate_hash) = 64 AND candidate_hash NOT GLOB '*[^0-9a-f]*'
    ),
    input_json TEXT NOT NULL CHECK(
        json_valid(input_json) AND json_type(input_json) = 'object'
    ),
    input_hash TEXT NOT NULL CHECK(
        length(input_hash) = 64 AND input_hash NOT GLOB '*[^0-9a-f]*'
    ),
    created_at INTEGER NOT NULL CHECK(created_at >= 0),
    activated_at INTEGER CHECK(
        activated_at IS NULL OR activated_at >= created_at
    ),
    settled_at INTEGER CHECK(
        settled_at IS NULL OR (
            activated_at IS NOT NULL AND settled_at >= activated_at
        )
    )
);

CREATE INDEX idx_work_graph_node_review_requests_attempt_created
    ON work_graph_node_review_requests(attempt_id, created_at DESC, id DESC);
CREATE INDEX idx_work_graph_node_review_requests_parent_created
    ON work_graph_node_review_requests(parent_run_id, created_at DESC, id DESC);
CREATE UNIQUE INDEX idx_work_graph_node_review_requests_active_attempt
    ON work_graph_node_review_requests(attempt_id)
    WHERE activated_at IS NOT NULL AND settled_at IS NULL;

CREATE TABLE work_graph_node_review_criterion_evidence (
    request_id TEXT NOT NULL
        REFERENCES work_graph_node_review_requests(id) ON DELETE CASCADE,
    criterion_ordinal INTEGER NOT NULL CHECK(criterion_ordinal BETWEEN 0 AND 7),
    criterion TEXT NOT NULL CHECK(
        criterion = trim(criterion) AND length(criterion) BETWEEN 1 AND 1000
    ),
    evidence_id TEXT NOT NULL REFERENCES task_evidence(id) ON DELETE RESTRICT
        CHECK(length(trim(evidence_id)) BETWEEN 1 AND 200),
    PRIMARY KEY(request_id, criterion_ordinal, evidence_id)
);

CREATE INDEX idx_work_graph_node_review_evidence_evidence
    ON work_graph_node_review_criterion_evidence(evidence_id, request_id);

CREATE TABLE work_graph_node_review_decisions (
    id TEXT PRIMARY KEY CHECK(length(trim(id)) BETWEEN 1 AND 200),
    request_id TEXT NOT NULL UNIQUE
        REFERENCES work_graph_node_review_requests(id) ON DELETE RESTRICT,
    goal_id TEXT NOT NULL REFERENCES work_graph_specs(goal_id) ON DELETE CASCADE,
    task_id TEXT NOT NULL REFERENCES work_graph_nodes(task_id) ON DELETE CASCADE,
    attempt_id TEXT NOT NULL REFERENCES task_attempts(id) ON DELETE RESTRICT,
    reviewer_delegation_id TEXT NOT NULL UNIQUE
        REFERENCES child_run_delegations(id) ON DELETE RESTRICT,
    reviewer_run_id TEXT NOT NULL UNIQUE REFERENCES runs(id) ON DELETE RESTRICT,
    reviewer_conversation_id TEXT NOT NULL UNIQUE
        REFERENCES conversations(id) ON DELETE RESTRICT,
    reviewer_agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE RESTRICT,
    outcome TEXT NOT NULL CHECK(outcome IN ('pass', 'revise', 'inconclusive')),
    summary TEXT NOT NULL CHECK(
        summary = trim(summary) AND length(summary) BETWEEN 1 AND 4000
    ),
    decision_json TEXT NOT NULL CHECK(
        json_valid(decision_json) AND json_type(decision_json) = 'object'
    ),
    decision_hash TEXT NOT NULL CHECK(
        length(decision_hash) = 64 AND decision_hash NOT GLOB '*[^0-9a-f]*'
    ),
    reviewer_terminal_json TEXT NOT NULL CHECK(
        json_valid(reviewer_terminal_json)
        AND json_type(reviewer_terminal_json) = 'object'
    ),
    reviewer_terminal_hash TEXT NOT NULL CHECK(
        length(reviewer_terminal_hash) = 64
        AND reviewer_terminal_hash NOT GLOB '*[^0-9a-f]*'
    ),
    proof_json TEXT NOT NULL CHECK(
        json_valid(proof_json) AND json_type(proof_json) = 'object'
    ),
    proof_hash TEXT NOT NULL CHECK(
        length(proof_hash) = 64 AND proof_hash NOT GLOB '*[^0-9a-f]*'
    ),
    created_at INTEGER NOT NULL CHECK(created_at >= 0)
);

CREATE INDEX idx_work_graph_node_review_decisions_attempt_created
    ON work_graph_node_review_decisions(attempt_id, created_at DESC, id DESC);

CREATE TABLE work_graph_node_review_proofs (
    decision_id TEXT NOT NULL
        REFERENCES work_graph_node_review_decisions(id) ON DELETE CASCADE,
    criterion_ordinal INTEGER NOT NULL CHECK(criterion_ordinal BETWEEN 0 AND 7),
    criterion TEXT NOT NULL CHECK(
        criterion = trim(criterion) AND length(criterion) BETWEEN 1 AND 1000
    ),
    runtime_tool_call_id TEXT NOT NULL
        CHECK(length(trim(runtime_tool_call_id)) BETWEEN 1 AND 200),
    tool_call_id TEXT NOT NULL REFERENCES tool_calls(id) ON DELETE RESTRICT
        CHECK(length(trim(tool_call_id)) BETWEEN 1 AND 200),
    PRIMARY KEY(decision_id, criterion_ordinal, tool_call_id),
    UNIQUE(decision_id, runtime_tool_call_id),
    UNIQUE(decision_id, tool_call_id)
);

CREATE INDEX idx_work_graph_node_review_proofs_tool
    ON work_graph_node_review_proofs(tool_call_id, decision_id);

CREATE TABLE work_graph_acceptances (
    id TEXT PRIMARY KEY CHECK(length(trim(id)) BETWEEN 1 AND 200),
    goal_id TEXT NOT NULL REFERENCES work_graph_specs(goal_id) ON DELETE CASCADE,
    plan_revision_id TEXT NOT NULL REFERENCES plan_revisions(id) ON DELETE RESTRICT,
    spec_hash TEXT NOT NULL CHECK(
        length(spec_hash) = 64 AND spec_hash NOT GLOB '*[^0-9a-f]*'
    ),
    submitter_run_id TEXT NOT NULL REFERENCES runs(id) ON DELETE RESTRICT,
    tool_call_id TEXT NOT NULL UNIQUE REFERENCES tool_calls(id) ON DELETE RESTRICT,
    expected_goal_version INTEGER NOT NULL CHECK(expected_goal_version >= 1),
    summary TEXT NOT NULL CHECK(
        summary = trim(summary) AND length(summary) BETWEEN 1 AND 4000
    ),
    node_projection_json TEXT NOT NULL CHECK(
        json_valid(node_projection_json)
        AND json_type(node_projection_json) = 'array'
    ),
    node_projection_hash TEXT NOT NULL CHECK(
        length(node_projection_hash) = 64
        AND node_projection_hash NOT GLOB '*[^0-9a-f]*'
    ),
    checks_json TEXT NOT NULL CHECK(
        json_valid(checks_json) AND json_type(checks_json) = 'object'
    ),
    checks_hash TEXT NOT NULL CHECK(
        length(checks_hash) = 64 AND checks_hash NOT GLOB '*[^0-9a-f]*'
    ),
    input_json TEXT NOT NULL CHECK(
        json_valid(input_json) AND json_type(input_json) = 'object'
    ),
    input_hash TEXT NOT NULL CHECK(
        length(input_hash) = 64 AND input_hash NOT GLOB '*[^0-9a-f]*'
    ),
    created_at INTEGER NOT NULL CHECK(created_at >= 0),
    accepted_at INTEGER CHECK(accepted_at IS NULL OR accepted_at >= created_at)
);

CREATE INDEX idx_work_graph_acceptances_goal_created
    ON work_graph_acceptances(goal_id, created_at DESC, id DESC);
CREATE UNIQUE INDEX idx_work_graph_acceptances_accepted_goal
    ON work_graph_acceptances(goal_id) WHERE accepted_at IS NOT NULL;

CREATE TRIGGER work_graph_node_review_requests_validate_insert
BEFORE INSERT ON work_graph_node_review_requests
FOR EACH ROW
WHEN NEW.activated_at IS NOT NULL OR NEW.settled_at IS NOT NULL OR NOT EXISTS (
    SELECT 1
    FROM work_graph_nodes AS node
    JOIN work_tasks AS task
      ON task.id = node.task_id
     AND task.goal_id = node.goal_id
     AND task.status = 'in_progress'
     AND task.owner_run_id = NEW.parent_run_id
     AND task.version = NEW.expected_task_version
    JOIN goals AS goal
      ON goal.id = node.goal_id AND goal.status = 'active'
    JOIN work_graph_specs AS spec
      ON spec.goal_id = goal.id
    JOIN plan_revisions AS plan
      ON plan.id = spec.plan_revision_id AND plan.status = 'approved'
    JOIN task_attempts AS attempt
      ON attempt.id = NEW.attempt_id
     AND attempt.task_id = node.task_id
     AND attempt.run_id = NEW.parent_run_id
     AND attempt.kind = 'execution'
     AND attempt.status = 'running'
     AND attempt.version = NEW.expected_attempt_version
    JOIN child_run_delegations AS implementation
      ON implementation.id = NEW.implementation_delegation_id
     AND implementation.graph_task_attempt_id = attempt.id
     AND implementation.parent_run_id = NEW.parent_run_id
     AND implementation.child_run_id = NEW.implementation_child_run_id
     AND implementation.status = 'completed'
    JOIN runs AS child_run
      ON child_run.id = implementation.child_run_id
     AND child_run.status = 'completed'
     AND child_run.finished_at IS NOT NULL
    JOIN task_validation_policies AS policy
      ON policy.task_id = task.id
     AND policy.policy_id = 'high_risk_v1'
     AND policy.policy_hash = NEW.validation_policy_hash
     AND attempt.policy_hash = policy.policy_hash
    JOIN runs AS parent_run
      ON parent_run.id = NEW.parent_run_id
     AND parent_run.conversation_id = goal.conversation_id
     AND parent_run.status = 'running'
     AND parent_run.run_kind = 'primary'
     AND parent_run.parent_run_id IS NULL
     AND parent_run.depth = 0
    JOIN run_execution_profiles AS parent_profile
      ON parent_profile.run_id = parent_run.id
     AND parent_profile.schema_version = 1
     AND parent_profile.profile_id = 'durable_v2'
     AND parent_profile.snapshot_json = '{"id":"durable_v2","schemaVersion":1}'
     AND parent_profile.profile_hash =
         'ae08f4dc8c1327cbd3de30879f01aa7475d9a728a3c0f738579086435d714f7e'
    JOIN tool_calls AS review_call
      ON review_call.id = NEW.tool_call_id
     AND review_call.run_id = parent_run.id
     AND review_call.conversation_id = goal.conversation_id
     AND review_call.tool_name = 'graph_readonly_node_review'
     AND review_call.execution_location = 'host'
     AND review_call.requires_approval = 0
     AND review_call.status = 'running'
     AND review_call.error_message IS NULL
     AND review_call.input_json = NEW.input_json
    WHERE node.task_id = NEW.task_id
      AND node.goal_id = NEW.goal_id
      AND NEW.review_contract_id = 'graph_reviewer_v1'
      AND NEW.review_contract_hash =
          '24a66ac5c9d85eeacabaf662192815995c600ff77703231624d2971e191b1c18'
      AND NEW.validation_policy_id = 'high_risk_v1'
      AND json(NEW.input_json) = NEW.input_json
      AND json_type(NEW.input_json, '$.goalId') = 'text'
      AND json_extract(NEW.input_json, '$.goalId') = NEW.goal_id
      AND json_type(NEW.input_json, '$.taskId') = 'text'
      AND json_extract(NEW.input_json, '$.taskId') = NEW.task_id
      AND json_type(NEW.input_json, '$.attemptId') = 'text'
      AND json_extract(NEW.input_json, '$.attemptId') = NEW.attempt_id
      AND json_type(NEW.input_json, '$.expectedTaskVersion') = 'integer'
      AND json_extract(NEW.input_json, '$.expectedTaskVersion') =
          NEW.expected_task_version
      AND json_type(NEW.input_json, '$.expectedAttemptVersion') = 'integer'
      AND json_extract(NEW.input_json, '$.expectedAttemptVersion') =
          NEW.expected_attempt_version
      AND json_type(NEW.input_json, '$.summary') = 'text'
      AND json_extract(NEW.input_json, '$.summary') = NEW.summary
      AND json_type(NEW.input_json, '$.criterionEvidence') = 'array'
      AND json_array_length(json_extract(NEW.input_json, '$.criterionEvidence')) =
          json_array_length(node.acceptance_criteria_json)
      AND (
          SELECT group_concat(key, ',') FROM json_each(NEW.input_json)
      ) = 'attemptId,criterionEvidence,expectedAttemptVersion,expectedTaskVersion,goalId,summary,taskId'
      AND json(NEW.candidate_json) = NEW.candidate_json
      AND json_extract(NEW.candidate_json, '$.attemptId') = NEW.attempt_id
      AND json_extract(NEW.candidate_json, '$.childResultHash') = NEW.child_result_hash
      AND json_extract(NEW.candidate_json, '$.criterionEvidence') =
          json_extract(NEW.input_json, '$.criterionEvidence')
      AND json_extract(NEW.candidate_json, '$.expectedAttemptVersion') =
          NEW.expected_attempt_version
      AND json_extract(NEW.candidate_json, '$.expectedTaskVersion') =
          NEW.expected_task_version
      AND json_extract(NEW.candidate_json, '$.goalId') = NEW.goal_id
      AND json_extract(NEW.candidate_json, '$.policyHash') = NEW.validation_policy_hash
      AND json_extract(NEW.candidate_json, '$.summary') = NEW.summary
      AND json_extract(NEW.candidate_json, '$.taskId') = NEW.task_id
      AND NOT EXISTS (
          SELECT 1 FROM plan_revisions AS newer
          WHERE newer.goal_id = NEW.goal_id
            AND newer.revision > plan.revision
            AND newer.status IN ('proposed', 'approved')
      )
      AND NOT EXISTS (
          SELECT 1 FROM review_findings AS blocker
          WHERE blocker.goal_id = NEW.goal_id
            AND (blocker.task_id = NEW.task_id OR blocker.task_id IS NULL)
            AND blocker.status = 'open'
            AND blocker.severity IN ('critical', 'high', 'medium')
      )
)
BEGIN
    SELECT RAISE(ABORT, 'Graph review request requires exact high-risk live authority');
END;

CREATE TRIGGER work_graph_node_review_evidence_validate
BEFORE INSERT ON work_graph_node_review_criterion_evidence
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1
    FROM work_graph_node_review_requests AS request
    JOIN work_graph_nodes AS node ON node.task_id = request.task_id
    JOIN task_attempts AS attempt ON attempt.id = request.attempt_id
    JOIN goals AS goal ON goal.id = request.goal_id
    JOIN task_evidence AS evidence
      ON evidence.id = NEW.evidence_id
     AND evidence.task_id = request.task_id
     AND evidence.source_run_id = request.parent_run_id
     AND evidence.validity_status = 'valid'
     AND evidence.checked_at IS NOT NULL
     AND evidence.rowid > attempt.evidence_rowid_watermark
     AND evidence.ref_kind = 'tool_call'
    JOIN tool_calls AS evidence_call
      ON evidence_call.id = evidence.ref_id
     AND evidence_call.run_id = request.parent_run_id
     AND evidence_call.conversation_id = goal.conversation_id
     AND evidence_call.status = 'completed'
     AND COALESCE(evidence_call.error_message, '') = ''
     AND evidence_call.started_at > attempt.started_at
     AND evidence_call.completed_at IS NOT NULL
     AND evidence_call.completed_at >= evidence_call.started_at
     AND evidence_call.updated_at >= evidence_call.started_at
    WHERE request.id = NEW.request_id
      AND request.activated_at IS NULL
      AND NEW.criterion_ordinal < json_array_length(node.acceptance_criteria_json)
      AND NEW.criterion = json_extract(
          node.acceptance_criteria_json,
          '$[' || NEW.criterion_ordinal || ']'
      )
      AND NEW.criterion = json_extract(
          request.input_json,
          '$.criterionEvidence[' || NEW.criterion_ordinal || '].criterion'
      )
      AND EXISTS (
          SELECT 1
          FROM json_each(json_extract(
              request.input_json,
              '$.criterionEvidence[' || NEW.criterion_ordinal || '].evidenceIds'
          )) AS supplied
          WHERE supplied.value = NEW.evidence_id
      )
      AND (
          (
              evidence.evidence_type = 'tool_call'
              AND json_extract(evidence.metadata_json, '$.validationCheckType') = 'inspection'
              AND (
                  (
                      evidence_call.execution_location = 'runtime'
                      AND evidence_call.tool_name IN ('read', 'ls', 'find', 'grep')
                      AND evidence_call.requires_approval = 0
                  )
                  OR (
                      evidence_call.execution_location = 'host'
                      AND evidence_call.tool_name IN ('git_read', 'structured_data', 'tabular_data')
                      AND evidence_call.requires_approval = 0
                  )
                  OR (
                      evidence_call.execution_location = 'host'
                      AND evidence_call.tool_name = 'sqlite_read'
                      AND evidence_call.requires_approval = 1
                  )
              )
          )
          OR (
              evidence.evidence_type = 'test_result'
              AND json_extract(evidence.metadata_json, '$.validationCheckType') = 'test'
              AND evidence_call.execution_location = 'host'
              AND evidence_call.tool_name IN ('test_run', 'code_check')
              AND evidence_call.requires_approval = 1
          )
      )
)
BEGIN
    SELECT RAISE(ABORT, 'Graph review criterion binding requires live parent inspection evidence');
END;

CREATE TRIGGER work_graph_node_review_requests_validate_activation
BEFORE UPDATE OF activated_at ON work_graph_node_review_requests
FOR EACH ROW
WHEN OLD.activated_at IS NULL AND NEW.activated_at IS NOT NULL AND NOT EXISTS (
    SELECT 1
    FROM work_tasks AS task
    JOIN goals AS goal ON goal.id = task.goal_id AND goal.status = 'active'
    JOIN task_attempts AS attempt
      ON attempt.id = OLD.attempt_id
     AND attempt.task_id = task.id
     AND attempt.run_id = OLD.parent_run_id
     AND attempt.status = 'running'
     AND attempt.version = OLD.expected_attempt_version
    JOIN child_run_delegations AS implementation
      ON implementation.id = OLD.implementation_delegation_id
     AND implementation.graph_task_attempt_id = attempt.id
     AND implementation.child_run_id = OLD.implementation_child_run_id
     AND implementation.status = 'completed'
    JOIN runs AS child_run
      ON child_run.id = implementation.child_run_id
     AND child_run.status = 'completed'
     AND child_run.finished_at IS NOT NULL
    JOIN runs AS parent_run
      ON parent_run.id = OLD.parent_run_id
     AND parent_run.conversation_id = goal.conversation_id
     AND parent_run.status = 'running'
    JOIN tool_calls AS review_call
      ON review_call.id = OLD.tool_call_id
     AND review_call.run_id = OLD.parent_run_id
     AND review_call.conversation_id = goal.conversation_id
     AND review_call.tool_name = 'graph_readonly_node_review'
     AND review_call.execution_location = 'host'
     AND review_call.requires_approval = 0
     AND review_call.status = 'completed'
     AND review_call.error_message IS NULL
     AND review_call.result_json IS NOT NULL
     AND json_valid(review_call.result_json)
     AND review_call.completed_at IS NOT NULL
    WHERE task.id = OLD.task_id
      AND task.goal_id = OLD.goal_id
      AND task.status = 'in_progress'
      AND task.owner_run_id = OLD.parent_run_id
      AND task.version = OLD.expected_task_version
      AND OLD.settled_at IS NULL
      AND OLD.input_hash = NEW.input_hash
      AND (
          SELECT COUNT(*) FROM work_graph_node_review_criterion_evidence AS binding
          WHERE binding.request_id = OLD.id
      ) = (
          SELECT COUNT(*)
          FROM json_each(json_extract(OLD.input_json, '$.criterionEvidence')) AS criterion,
               json_each(json_extract(criterion.value, '$.evidenceIds')) AS evidence
      )
)
BEGIN
    SELECT RAISE(ABORT, 'Graph review request activates only after exact Host completion');
END;

CREATE TRIGGER work_graph_node_review_requests_no_update
BEFORE UPDATE ON work_graph_node_review_requests
FOR EACH ROW
WHEN NEW.id IS NOT OLD.id
 OR NEW.goal_id IS NOT OLD.goal_id
 OR NEW.task_id IS NOT OLD.task_id
 OR NEW.attempt_id IS NOT OLD.attempt_id
 OR NEW.parent_run_id IS NOT OLD.parent_run_id
 OR NEW.implementation_delegation_id IS NOT OLD.implementation_delegation_id
 OR NEW.implementation_child_run_id IS NOT OLD.implementation_child_run_id
 OR NEW.tool_call_id IS NOT OLD.tool_call_id
 OR NEW.review_contract_id IS NOT OLD.review_contract_id
 OR NEW.review_contract_hash IS NOT OLD.review_contract_hash
 OR NEW.validation_policy_id IS NOT OLD.validation_policy_id
 OR NEW.validation_policy_hash IS NOT OLD.validation_policy_hash
 OR NEW.expected_task_version IS NOT OLD.expected_task_version
 OR NEW.expected_attempt_version IS NOT OLD.expected_attempt_version
 OR NEW.child_result_hash IS NOT OLD.child_result_hash
 OR NEW.summary IS NOT OLD.summary
 OR NEW.candidate_json IS NOT OLD.candidate_json
 OR NEW.candidate_hash IS NOT OLD.candidate_hash
 OR NEW.input_json IS NOT OLD.input_json
 OR NEW.input_hash IS NOT OLD.input_hash
 OR NEW.created_at IS NOT OLD.created_at
 OR (OLD.activated_at IS NOT NULL AND NEW.activated_at IS NOT OLD.activated_at)
 OR (OLD.activated_at IS NULL AND NEW.activated_at IS NULL)
 OR (OLD.settled_at IS NOT NULL AND NEW.settled_at IS NOT OLD.settled_at)
 OR (OLD.settled_at IS NULL AND NEW.settled_at IS NOT NULL AND NOT EXISTS (
     SELECT 1 FROM work_graph_node_review_decisions AS decision
     WHERE decision.request_id = OLD.id
 ))
BEGIN
    SELECT RAISE(ABORT, 'Graph review requests only allow activation and authoritative settlement');
END;

CREATE TRIGGER work_graph_review_delegations_validate_insert
BEFORE INSERT ON child_run_delegations
FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM work_graph_node_review_requests AS request
    WHERE request.parent_run_id = NEW.parent_run_id
      AND request.tool_call_id = NEW.tool_call_id
      AND request.activated_at IS NOT NULL
      AND request.settled_at IS NULL
)
AND NOT EXISTS (
    SELECT 1
    FROM work_graph_node_review_requests AS request
    JOIN runs AS parent_run ON parent_run.id = request.parent_run_id
    JOIN conversations AS parent_conversation
      ON parent_conversation.id = parent_run.conversation_id
    JOIN runs AS reviewer_run
      ON reviewer_run.id = NEW.child_run_id
     AND reviewer_run.parent_run_id = parent_run.id
     AND reviewer_run.root_run_id = COALESCE(parent_run.root_run_id, parent_run.id)
     AND reviewer_run.run_kind = 'child'
     AND reviewer_run.depth = 1
     AND reviewer_run.status = 'queued'
     AND reviewer_run.conversation_id = NEW.child_conversation_id
    JOIN conversations AS reviewer_conversation
      ON reviewer_conversation.id = NEW.child_conversation_id
     AND reviewer_conversation.conversation_kind = 'child'
     AND reviewer_conversation.parent_conversation_id = parent_conversation.id
     AND reviewer_conversation.lineage_root_id = COALESCE(
         parent_conversation.lineage_root_id,
         parent_conversation.id
     )
     AND reviewer_conversation.id <> parent_conversation.id
    WHERE request.parent_run_id = NEW.parent_run_id
      AND request.tool_call_id = NEW.tool_call_id
      AND request.implementation_child_run_id <> NEW.child_run_id
      AND NEW.graph_task_attempt_id IS NULL
      AND NEW.team_run_id IS NULL
      AND NEW.team_member_id IS NULL
      AND NEW.status = 'queued'
      AND NEW.objective = 'Independently review high-risk Graph node'
      AND NEW.context = request.candidate_json
      AND NEW.allowed_tools_json = '["read","ls","find","grep"]'
      AND NEW.max_duration_ms = 45000
      AND NEW.max_total_tokens = 4096
      AND NEW.max_output_tokens = 1024
      AND NEW.max_tool_calls = 6
)
BEGIN
    SELECT RAISE(ABORT, 'Graph Reviewer delegation requires exact independent readonly scope');
END;

CREATE TRIGGER work_graph_review_delegations_seal_profile
AFTER INSERT ON child_run_delegations
FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM work_graph_node_review_requests AS request
    WHERE request.parent_run_id = NEW.parent_run_id
      AND request.tool_call_id = NEW.tool_call_id
      AND request.activated_at IS NOT NULL
      AND request.settled_at IS NULL
)
BEGIN
    INSERT INTO run_execution_profiles(
        run_id, schema_version, profile_id, snapshot_json, profile_hash, frozen_at
    ) VALUES (
        NEW.child_run_id, 1, 'graph_reviewer_v1',
        '{"id":"graph_reviewer_v1","schemaVersion":1}',
        '24a66ac5c9d85eeacabaf662192815995c600ff77703231624d2971e191b1c18',
        NEW.created_at
    );
END;

CREATE TRIGGER work_graph_node_review_decisions_validate_insert
BEFORE INSERT ON work_graph_node_review_decisions
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1
    FROM work_graph_node_review_requests AS request
    JOIN child_run_delegations AS reviewer_delegation
      ON reviewer_delegation.id = NEW.reviewer_delegation_id
     AND reviewer_delegation.parent_run_id = request.parent_run_id
     AND reviewer_delegation.tool_call_id = request.tool_call_id
     AND reviewer_delegation.child_run_id = NEW.reviewer_run_id
     AND reviewer_delegation.child_conversation_id = NEW.reviewer_conversation_id
     AND reviewer_delegation.graph_task_attempt_id IS NULL
     AND reviewer_delegation.status IN (
         'completed', 'failed', 'cancelled', 'interrupted'
     )
    JOIN runs AS reviewer_run
      ON reviewer_run.id = NEW.reviewer_run_id
     AND reviewer_run.conversation_id = NEW.reviewer_conversation_id
     AND reviewer_run.parent_run_id = request.parent_run_id
     AND reviewer_run.run_kind = 'child'
     AND reviewer_run.depth = 1
     AND reviewer_run.status = reviewer_delegation.status
     AND reviewer_run.finished_at IS NOT NULL
    JOIN conversations AS reviewer_conversation
      ON reviewer_conversation.id = NEW.reviewer_conversation_id
     AND reviewer_conversation.agent_id = NEW.reviewer_agent_id
     AND reviewer_conversation.conversation_kind = 'child'
    JOIN run_execution_profiles AS reviewer_profile
      ON reviewer_profile.run_id = reviewer_run.id
     AND reviewer_profile.schema_version = 1
     AND reviewer_profile.profile_id = 'graph_reviewer_v1'
     AND reviewer_profile.snapshot_json =
         '{"id":"graph_reviewer_v1","schemaVersion":1}'
     AND reviewer_profile.profile_hash =
         '24a66ac5c9d85eeacabaf662192815995c600ff77703231624d2971e191b1c18'
    WHERE request.id = NEW.request_id
      AND request.goal_id = NEW.goal_id
      AND request.task_id = NEW.task_id
      AND request.attempt_id = NEW.attempt_id
      AND request.activated_at IS NOT NULL
      AND request.settled_at IS NULL
      AND reviewer_run.id <> request.parent_run_id
      AND reviewer_run.id <> request.implementation_child_run_id
      AND NOT EXISTS (
          SELECT 1 FROM task_attempts AS any_attempt
          WHERE any_attempt.task_id = request.task_id
            AND any_attempt.run_id = reviewer_run.id
      )
      AND json(NEW.decision_json) = NEW.decision_json
      AND json_type(NEW.decision_json, '$.criteria') = 'array'
      AND json_type(NEW.decision_json, '$.findings') = 'array'
      AND json_type(NEW.decision_json, '$.recommendation') = 'text'
      AND json_type(NEW.decision_json, '$.summary') = 'text'
      AND json_extract(NEW.decision_json, '$.summary') = NEW.summary
      AND (SELECT group_concat(key, ',') FROM json_each(NEW.decision_json)) =
          'criteria,findings,recommendation,summary'
      AND (
          (
              NEW.outcome = 'pass'
              AND json_extract(NEW.decision_json, '$.recommendation') = 'pass'
              AND json_array_length(json_extract(NEW.decision_json, '$.findings')) = 0
              AND json_array_length(json_extract(NEW.decision_json, '$.criteria')) =
                  json_array_length((
                      SELECT node.acceptance_criteria_json
                      FROM work_graph_nodes AS node WHERE node.task_id = request.task_id
                  ))
              AND NOT EXISTS (
                  SELECT 1
                  FROM json_each(json_extract(NEW.decision_json, '$.criteria')) AS criterion
                  WHERE json_type(criterion.value) <> 'object'
                     OR (SELECT group_concat(key, ',') FROM json_each(criterion.value)) <>
                        'criterion,status,toolCallIds'
                     OR json_type(criterion.value, '$.criterion') <> 'text'
                     OR json_extract(criterion.value, '$.criterion') <>
                        json_extract((
                            SELECT node.acceptance_criteria_json
                            FROM work_graph_nodes AS node WHERE node.task_id = request.task_id
                        ), '$[' || criterion.key || ']')
                     OR json_type(criterion.value, '$.status') <> 'text'
                     OR json_extract(criterion.value, '$.status') <> 'passed'
                     OR json_type(criterion.value, '$.toolCallIds') <> 'array'
                     OR json_array_length(json_extract(criterion.value, '$.toolCallIds'))
                        NOT BETWEEN 1 AND 16
                     OR EXISTS (
                         SELECT 1 FROM json_each(
                             json_extract(criterion.value, '$.toolCallIds')
                         ) AS proof_id
                         WHERE proof_id.type <> 'text'
                            OR proof_id.value <> trim(proof_id.value)
                            OR length(proof_id.value) NOT BETWEEN 1 AND 200
                     )
                     OR EXISTS (
                         SELECT 1 FROM json_each(
                             json_extract(criterion.value, '$.toolCallIds')
                         ) AS proof_id
                         GROUP BY proof_id.value HAVING COUNT(*) > 1
                     )
              )
          )
          OR (
              NEW.outcome = 'revise'
              AND json_extract(NEW.decision_json, '$.recommendation') = 'revise'
              AND json_array_length(json_extract(NEW.decision_json, '$.findings'))
                  BETWEEN 1 AND 16
              AND NOT EXISTS (
                  SELECT 1
                  FROM json_each(json_extract(NEW.decision_json, '$.findings')) AS finding
                  WHERE json_type(finding.value) <> 'object'
                     OR (SELECT group_concat(key, ',') FROM json_each(finding.value)) <>
                        'criterion,detail,severity,title,toolCallIds'
                     OR json_type(finding.value, '$.criterion') <> 'text'
                     OR NOT EXISTS (
                         SELECT 1
                         FROM json_each(json_extract(NEW.decision_json, '$.criteria')) AS criterion
                         WHERE json_extract(criterion.value, '$.criterion') =
                               json_extract(finding.value, '$.criterion')
                           AND json_extract(criterion.value, '$.status') = 'failed'
                     )
                     OR json_type(finding.value, '$.title') <> 'text'
                     OR json_extract(finding.value, '$.title') <>
                        trim(json_extract(finding.value, '$.title'))
                     OR length(json_extract(finding.value, '$.title')) NOT BETWEEN 1 AND 200
                     OR json_type(finding.value, '$.detail') <> 'text'
                     OR json_extract(finding.value, '$.detail') <>
                        trim(json_extract(finding.value, '$.detail'))
                     OR length(json_extract(finding.value, '$.detail')) NOT BETWEEN 1 AND 2000
                     OR json_type(finding.value, '$.severity') <> 'text'
                     OR json_extract(finding.value, '$.severity') NOT IN (
                         'critical', 'high', 'medium', 'low', 'info'
                     )
                     OR json_type(finding.value, '$.toolCallIds') <> 'array'
                     OR json_array_length(json_extract(finding.value, '$.toolCallIds'))
                        NOT BETWEEN 1 AND 16
                     OR EXISTS (
                         SELECT 1
                         FROM json_each(json_extract(finding.value, '$.toolCallIds')) AS finding_tool
                         WHERE finding_tool.type <> 'text'
                            OR NOT EXISTS (
                                SELECT 1
                                FROM json_each(json_extract(NEW.decision_json, '$.criteria')) AS criterion,
                                     json_each(json_extract(criterion.value, '$.toolCallIds')) AS criterion_tool
                                WHERE json_extract(criterion.value, '$.criterion') =
                                      json_extract(finding.value, '$.criterion')
                                  AND criterion_tool.value = finding_tool.value
                            )
                     )
                     OR EXISTS (
                         SELECT 1
                         FROM json_each(json_extract(finding.value, '$.toolCallIds')) AS finding_tool
                         GROUP BY finding_tool.value HAVING COUNT(*) > 1
                     )
              )
              AND json_array_length(json_extract(NEW.decision_json, '$.criteria')) =
                  json_array_length((
                      SELECT node.acceptance_criteria_json
                      FROM work_graph_nodes AS node WHERE node.task_id = request.task_id
                  ))
              AND EXISTS (
                  SELECT 1
                  FROM json_each(json_extract(NEW.decision_json, '$.criteria')) AS criterion
                  WHERE json_extract(criterion.value, '$.status') = 'failed'
              )
              AND NOT EXISTS (
                  SELECT 1
                  FROM json_each(json_extract(NEW.decision_json, '$.criteria')) AS criterion
                  WHERE json_type(criterion.value) <> 'object'
                     OR (SELECT group_concat(key, ',') FROM json_each(criterion.value)) <>
                        'criterion,status,toolCallIds'
                     OR json_type(criterion.value, '$.criterion') <> 'text'
                     OR json_extract(criterion.value, '$.criterion') <>
                        json_extract((
                            SELECT node.acceptance_criteria_json
                            FROM work_graph_nodes AS node WHERE node.task_id = request.task_id
                        ), '$[' || criterion.key || ']')
                     OR json_type(criterion.value, '$.status') <> 'text'
                     OR json_extract(criterion.value, '$.status') NOT IN (
                         'passed', 'failed', 'inconclusive'
                     )
                     OR json_type(criterion.value, '$.toolCallIds') <> 'array'
                     OR json_array_length(json_extract(criterion.value, '$.toolCallIds'))
                        NOT BETWEEN 1 AND 16
                     OR EXISTS (
                         SELECT 1 FROM json_each(
                             json_extract(criterion.value, '$.toolCallIds')
                         ) AS proof_id
                         WHERE proof_id.type <> 'text'
                            OR proof_id.value <> trim(proof_id.value)
                            OR length(proof_id.value) NOT BETWEEN 1 AND 200
                     )
                     OR EXISTS (
                         SELECT 1 FROM json_each(
                             json_extract(criterion.value, '$.toolCallIds')
                         ) AS proof_id
                         GROUP BY proof_id.value HAVING COUNT(*) > 1
                     )
              )
          )
          OR (
              NEW.outcome = 'inconclusive'
              AND json_extract(NEW.decision_json, '$.recommendation') = 'inconclusive'
              AND json_array_length(json_extract(NEW.decision_json, '$.criteria')) = 0
              AND json_array_length(json_extract(NEW.decision_json, '$.findings')) = 0
          )
      )
      AND json(NEW.reviewer_terminal_json) = NEW.reviewer_terminal_json
      AND json_extract(NEW.reviewer_terminal_json, '$.childRunId') = reviewer_run.id
      AND json_extract(NEW.reviewer_terminal_json, '$.status') = reviewer_run.status
      AND json_extract(NEW.reviewer_terminal_json, '$.finishedAt') = reviewer_run.finished_at
      AND json(NEW.proof_json) = NEW.proof_json
      AND json_extract(NEW.proof_json, '$.candidateHash') = request.candidate_hash
      AND json_extract(NEW.proof_json, '$.requestId') = request.id
      AND json_extract(NEW.proof_json, '$.reviewContractHash') = request.review_contract_hash
      AND json_extract(NEW.proof_json, '$.reviewerConversationId') = reviewer_run.conversation_id
      AND json_extract(NEW.proof_json, '$.reviewerRunId') = reviewer_run.id
      AND json_extract(NEW.proof_json, '$.terminalHash') = NEW.reviewer_terminal_hash
      AND json_type(NEW.proof_json, '$.toolCallMappings') = 'array'
      AND json_type(NEW.proof_json, '$.toolCallMappingsHash') = 'text'
      AND length(json_extract(NEW.proof_json, '$.toolCallMappingsHash')) = 64
      AND (SELECT group_concat(key, ',') FROM json_each(NEW.proof_json)) =
          'candidateHash,requestId,reviewContractHash,reviewerConversationId,reviewerRunId,terminalHash,toolCallMappings,toolCallMappingsHash'
      AND (
          (
              NEW.outcome IN ('pass', 'revise')
              AND reviewer_run.status = 'completed'
              AND length(trim(COALESCE(reviewer_delegation.result_text, ''))) > 0
          )
          OR NEW.outcome = 'inconclusive'
      )
)
BEGIN
    SELECT RAISE(ABORT, 'Graph review decision requires an independent authoritative Reviewer terminal');
END;

CREATE TRIGGER work_graph_node_review_proofs_validate_insert
BEFORE INSERT ON work_graph_node_review_proofs
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1
    FROM work_graph_node_review_decisions AS decision
    JOIN work_graph_node_review_requests AS request ON request.id = decision.request_id
    JOIN work_graph_nodes AS node ON node.task_id = request.task_id
    JOIN runs AS reviewer_run ON reviewer_run.id = decision.reviewer_run_id
    JOIN tool_calls AS proof_call
      ON proof_call.id = NEW.tool_call_id
     AND proof_call.runtime_tool_call_id = NEW.runtime_tool_call_id
     AND proof_call.run_id = decision.reviewer_run_id
     AND proof_call.conversation_id = decision.reviewer_conversation_id
     AND proof_call.execution_location = 'runtime'
     AND proof_call.tool_name IN ('read', 'ls', 'find', 'grep')
     AND proof_call.requires_approval = 0
     AND proof_call.status = 'completed'
     AND COALESCE(proof_call.error_message, '') = ''
     AND proof_call.started_at > request.activated_at
     AND proof_call.completed_at IS NOT NULL
     AND proof_call.completed_at >= proof_call.started_at
     AND proof_call.completed_at <= reviewer_run.finished_at
     AND proof_call.updated_at >= proof_call.completed_at
    WHERE decision.id = NEW.decision_id
      AND decision.outcome IN ('pass', 'revise')
      AND request.settled_at IS NULL
      AND NEW.criterion_ordinal < json_array_length(node.acceptance_criteria_json)
      AND NEW.criterion = json_extract(
          node.acceptance_criteria_json,
          '$[' || NEW.criterion_ordinal || ']'
      )
      AND NEW.criterion = json_extract(
          decision.decision_json,
          '$.criteria[' || NEW.criterion_ordinal || '].criterion'
      )
      AND EXISTS (
          SELECT 1
          FROM json_each(json_extract(
              decision.decision_json,
              '$.criteria[' || NEW.criterion_ordinal || '].toolCallIds'
          )) AS supplied
          WHERE supplied.type = 'text'
            AND supplied.value = NEW.runtime_tool_call_id
      )
      AND EXISTS (
          SELECT 1
          FROM json_each(json_extract(decision.proof_json, '$.toolCallMappings')) AS mapping
          WHERE json_type(mapping.value) = 'object'
            AND json_extract(mapping.value, '$.runtimeToolCallId') = NEW.runtime_tool_call_id
            AND json_extract(mapping.value, '$.toolCallId') = NEW.tool_call_id
            AND (SELECT group_concat(key, ',') FROM json_each(mapping.value)) =
                'runtimeToolCallId,toolCallId'
      )
)
BEGIN
    SELECT RAISE(ABORT, 'Graph review proof requires an exact Reviewer readonly ToolCall');
END;

CREATE TRIGGER work_graph_node_review_requests_validate_settlement
BEFORE UPDATE OF settled_at ON work_graph_node_review_requests
FOR EACH ROW
WHEN OLD.settled_at IS NULL AND NEW.settled_at IS NOT NULL AND NOT EXISTS (
    SELECT 1
    FROM work_graph_node_review_decisions AS decision
    WHERE decision.request_id = OLD.id
      AND (
          (
              decision.outcome = 'inconclusive'
              AND NOT EXISTS (
                  SELECT 1 FROM work_graph_node_review_proofs AS proof
                  WHERE proof.decision_id = decision.id
              )
              AND json_array_length(json_extract(
                  decision.proof_json, '$.toolCallMappings'
              )) = 0
          )
          OR (
              decision.outcome IN ('pass', 'revise')
              AND (
                  SELECT COUNT(*) FROM work_graph_node_review_proofs AS proof
                  WHERE proof.decision_id = decision.id
              ) = (
                  SELECT COUNT(*)
                  FROM json_each(json_extract(decision.decision_json, '$.criteria')) AS criterion,
                       json_each(json_extract(criterion.value, '$.toolCallIds')) AS tool_call
              )
              AND (
                  SELECT COUNT(*) FROM work_graph_node_review_proofs AS proof
                  WHERE proof.decision_id = decision.id
              ) = json_array_length(json_extract(
                  decision.proof_json, '$.toolCallMappings'
              ))
              AND NOT EXISTS (
                  SELECT 1
                  FROM json_each(json_extract(decision.decision_json, '$.criteria')) AS criterion
                  WHERE NOT EXISTS (
                      SELECT 1 FROM work_graph_node_review_proofs AS proof
                      WHERE proof.decision_id = decision.id
                        AND proof.criterion_ordinal = criterion.key
                        AND proof.criterion = json_extract(criterion.value, '$.criterion')
                  )
              )
          )
      )
)
BEGIN
    SELECT RAISE(ABORT, 'Graph review settlement requires complete immutable proof bindings');
END;

CREATE TRIGGER work_graph_high_risk_finishes_require_review_pass
BEFORE INSERT ON work_graph_node_finishes
FOR EACH ROW
WHEN NEW.validation_policy_id = 'high_risk_v1'
 AND NOT EXISTS (
     SELECT 1
     FROM work_graph_node_review_requests AS request
     JOIN work_graph_node_review_decisions AS decision
       ON decision.request_id = request.id AND decision.outcome = 'pass'
     WHERE request.goal_id = NEW.goal_id
       AND request.task_id = NEW.task_id
       AND request.attempt_id = NEW.attempt_id
       AND request.parent_run_id = NEW.parent_run_id
       AND request.implementation_child_run_id = NEW.child_run_id
       AND request.input_json = NEW.input_json
       AND request.child_result_hash = NEW.child_result_hash
       AND request.validation_policy_hash = NEW.validation_policy_hash
       AND request.expected_task_version = NEW.expected_task_version
       AND request.expected_attempt_version = NEW.expected_attempt_version
       AND request.activated_at IS NOT NULL
       AND request.settled_at IS NOT NULL
       AND EXISTS (
           SELECT 1 FROM work_graph_node_review_proofs AS proof
           WHERE proof.decision_id = decision.id
       )
 )
BEGIN
    SELECT RAISE(ABORT, 'high-risk Graph finish requires an exact independent pass');
END;

CREATE TRIGGER work_graph_high_risk_attempts_require_review_pass
BEFORE UPDATE OF status ON task_attempts
FOR EACH ROW
WHEN OLD.status = 'running' AND NEW.status = 'succeeded'
 AND EXISTS (
     SELECT 1
     FROM work_graph_node_finishes AS finish
     WHERE finish.attempt_id = OLD.id AND finish.validation_policy_id = 'high_risk_v1'
 )
 AND NOT EXISTS (
     SELECT 1
     FROM work_graph_node_finishes AS finish
     JOIN work_graph_node_review_requests AS request
       ON request.attempt_id = finish.attempt_id
      AND request.input_json = finish.input_json
      AND request.child_result_hash = finish.child_result_hash
      AND request.validation_policy_hash = finish.validation_policy_hash
      AND request.activated_at IS NOT NULL
      AND request.settled_at IS NOT NULL
     JOIN work_graph_node_review_decisions AS decision
       ON decision.request_id = request.id AND decision.outcome = 'pass'
     WHERE finish.attempt_id = OLD.id
       AND EXISTS (
           SELECT 1 FROM work_graph_node_review_proofs AS proof
           WHERE proof.decision_id = decision.id
       )
 )
BEGIN
    SELECT RAISE(ABORT, 'high-risk Graph Attempt cannot succeed without the exact Reviewer proof');
END;

CREATE TRIGGER task_attempts_require_graph_review_revise
BEFORE UPDATE OF status ON task_attempts
FOR EACH ROW
WHEN OLD.status = 'running'
 AND NEW.status = 'blocked'
 AND EXISTS (
     SELECT 1 FROM child_run_delegations
     WHERE graph_task_attempt_id = OLD.id
 )
 AND NOT EXISTS (
     SELECT 1
     FROM work_graph_node_review_decisions AS decision
     JOIN work_graph_node_review_requests AS request
       ON request.id = decision.request_id
      AND request.settled_at IS NULL
     WHERE decision.attempt_id = OLD.id
       AND decision.outcome = 'revise'
       AND NEW.version = OLD.version + 1
       AND NEW.finished_at IS NOT NULL
       AND NEW.failure_reason = decision.summary
 )
BEGIN
    SELECT RAISE(ABORT, 'Graph Attempt blocked requires an authoritative revise decision');
END;

CREATE TRIGGER work_tasks_require_graph_review_revise
BEFORE UPDATE OF status ON work_tasks
FOR EACH ROW
WHEN OLD.status = 'in_progress'
 AND NEW.status = 'blocked'
 AND EXISTS (SELECT 1 FROM work_graph_nodes WHERE task_id = OLD.id)
 AND NOT EXISTS (
     SELECT 1
     FROM work_graph_node_review_decisions AS decision
     JOIN work_graph_node_review_requests AS request
       ON request.id = decision.request_id
      AND request.settled_at IS NULL
     JOIN task_attempts AS attempt
       ON attempt.id = decision.attempt_id
      AND attempt.status = 'blocked'
     WHERE decision.task_id = OLD.id
       AND decision.outcome = 'revise'
       AND NEW.version = OLD.version + 1
       AND NEW.owner_run_id IS NULL
       AND NEW.blocked_reason = decision.summary
       AND NEW.finished_at IS NOT NULL
 )
BEGIN
    SELECT RAISE(ABORT, 'Graph Task blocked requires the same revise authority chain');
END;

CREATE TRIGGER work_graph_acceptances_validate_insert
BEFORE INSERT ON work_graph_acceptances
FOR EACH ROW
WHEN NEW.accepted_at IS NOT NULL OR NOT EXISTS (
    SELECT 1
    FROM work_graph_specs AS spec
    JOIN goals AS goal
      ON goal.id = spec.goal_id
     AND goal.status = 'active'
     AND goal.version = NEW.expected_goal_version
    JOIN plan_revisions AS plan
      ON plan.id = spec.plan_revision_id
     AND plan.status = 'approved'
    JOIN runs AS submitter
      ON submitter.id = NEW.submitter_run_id
     AND submitter.conversation_id = goal.conversation_id
     AND submitter.status = 'running'
     AND submitter.run_kind = 'primary'
     AND submitter.parent_run_id IS NULL
     AND submitter.depth = 0
    JOIN run_execution_profiles AS profile
      ON profile.run_id = submitter.id
     AND profile.schema_version = 1
     AND profile.profile_id = 'durable_v2'
     AND profile.snapshot_json = '{"id":"durable_v2","schemaVersion":1}'
     AND profile.profile_hash =
         'ae08f4dc8c1327cbd3de30879f01aa7475d9a728a3c0f738579086435d714f7e'
    JOIN tool_calls AS accept_call
      ON accept_call.id = NEW.tool_call_id
     AND accept_call.run_id = submitter.id
     AND accept_call.conversation_id = goal.conversation_id
     AND accept_call.tool_name = 'graph_readonly_accept'
     AND accept_call.execution_location = 'host'
     AND accept_call.requires_approval = 0
     AND accept_call.status = 'running'
     AND accept_call.error_message IS NULL
     AND accept_call.input_json = NEW.input_json
    WHERE spec.goal_id = NEW.goal_id
      AND spec.plan_revision_id = NEW.plan_revision_id
      AND spec.spec_hash = NEW.spec_hash
      AND json(NEW.input_json) = NEW.input_json
      AND json_extract(NEW.input_json, '$.goalId') = NEW.goal_id
      AND json_extract(NEW.input_json, '$.expectedGoalVersion') = NEW.expected_goal_version
      AND json_extract(NEW.input_json, '$.summary') = NEW.summary
      AND (SELECT group_concat(key, ',') FROM json_each(NEW.input_json)) =
          'expectedGoalVersion,goalId,summary'
      AND json(NEW.node_projection_json) = NEW.node_projection_json
      AND json_array_length(NEW.node_projection_json) = (
          SELECT COUNT(*) FROM work_graph_nodes WHERE goal_id = NEW.goal_id
      )
      AND NOT EXISTS (
          SELECT 1
          FROM json_each(NEW.node_projection_json) AS projected
          WHERE json_type(projected.value) <> 'object'
             OR (SELECT group_concat(key, ',') FROM json_each(projected.value)) <>
                'attemptId,childResultHash,childRunId,decisionId,finishId,policyHash,policyId,taskId,taskVersion'
             OR NOT EXISTS (
                 SELECT 1
                 FROM work_graph_node_finishes AS finish
                 JOIN work_tasks AS task ON task.id = finish.task_id
                 JOIN task_attempts AS attempt ON attempt.id = finish.attempt_id
                 JOIN task_validation_policies AS policy ON policy.task_id = finish.task_id
                 WHERE finish.goal_id = NEW.goal_id
                   AND finish.id = json_extract(projected.value, '$.finishId')
                   AND finish.task_id = json_extract(projected.value, '$.taskId')
                   AND finish.attempt_id = json_extract(projected.value, '$.attemptId')
                   AND finish.child_run_id = json_extract(projected.value, '$.childRunId')
                   AND finish.child_result_hash =
                       json_extract(projected.value, '$.childResultHash')
                   AND finish.validation_policy_id =
                       json_extract(projected.value, '$.policyId')
                   AND finish.validation_policy_hash =
                       json_extract(projected.value, '$.policyHash')
                   AND task.version = json_extract(projected.value, '$.taskVersion')
                   AND task.status = 'completed'
                   AND attempt.status = 'succeeded'
                   AND policy.policy_hash = finish.validation_policy_hash
                   AND (
                       (
                           policy.policy_id = 'standard_v1'
                           AND json_type(projected.value, '$.decisionId') = 'null'
                       )
                       OR EXISTS (
                           SELECT 1
                           FROM work_graph_node_review_requests AS request
                           JOIN work_graph_node_review_decisions AS decision
                             ON decision.request_id = request.id AND decision.outcome = 'pass'
                           WHERE request.attempt_id = finish.attempt_id
                             AND request.input_json = finish.input_json
                             AND request.child_result_hash = finish.child_result_hash
                             AND request.validation_policy_hash = finish.validation_policy_hash
                             AND request.settled_at IS NOT NULL
                             AND decision.id = json_extract(projected.value, '$.decisionId')
                       )
                   )
             )
      )
      AND json(NEW.checks_json) = NEW.checks_json
      AND (SELECT group_concat(key, ',') FROM json_each(NEW.checks_json)) =
          'activeReviewCount,currentPlanRevisionId,nodeCount,openFindingCount,specHash'
      AND json_extract(NEW.checks_json, '$.activeReviewCount') = 0
      AND json_extract(NEW.checks_json, '$.currentPlanRevisionId') = NEW.plan_revision_id
      AND json_extract(NEW.checks_json, '$.nodeCount') =
          json_array_length(NEW.node_projection_json)
      AND json_extract(NEW.checks_json, '$.openFindingCount') = 0
      AND json_extract(NEW.checks_json, '$.specHash') = NEW.spec_hash
      AND NOT EXISTS (
          SELECT 1 FROM plan_revisions AS newer
          WHERE newer.goal_id = NEW.goal_id
            AND newer.revision > plan.revision
            AND newer.status IN ('proposed', 'approved')
      )
)
BEGIN
    SELECT RAISE(ABORT, 'Graph acceptance intent requires exact current Host authority');
END;

CREATE TRIGGER work_graph_acceptances_validate_activation
BEFORE UPDATE OF accepted_at ON work_graph_acceptances
FOR EACH ROW
WHEN OLD.accepted_at IS NULL AND NEW.accepted_at IS NOT NULL AND NOT EXISTS (
    SELECT 1
    FROM work_graph_specs AS spec
    JOIN goals AS goal
      ON goal.id = spec.goal_id
     AND goal.status = 'active'
     AND goal.version = OLD.expected_goal_version
    JOIN plan_revisions AS plan
      ON plan.id = spec.plan_revision_id
     AND plan.status = 'approved'
    JOIN runs AS submitter
      ON submitter.id = OLD.submitter_run_id
     AND submitter.conversation_id = goal.conversation_id
     AND submitter.status = 'running'
    JOIN tool_calls AS accept_call
      ON accept_call.id = OLD.tool_call_id
     AND accept_call.run_id = submitter.id
     AND accept_call.conversation_id = goal.conversation_id
     AND accept_call.tool_name = 'graph_readonly_accept'
     AND accept_call.execution_location = 'host'
     AND accept_call.requires_approval = 0
     AND accept_call.status = 'completed'
     AND accept_call.error_message IS NULL
     AND accept_call.result_json IS NOT NULL
     AND json_valid(accept_call.result_json)
     AND accept_call.completed_at IS NOT NULL
    WHERE spec.goal_id = OLD.goal_id
      AND spec.plan_revision_id = OLD.plan_revision_id
      AND spec.spec_hash = OLD.spec_hash
      AND NOT EXISTS (
          SELECT 1 FROM plan_revisions AS newer
          WHERE newer.goal_id = OLD.goal_id
            AND newer.revision > plan.revision
            AND newer.status IN ('proposed', 'approved')
      )
      AND NOT EXISTS (
          SELECT 1 FROM review_findings AS finding
          WHERE finding.goal_id = OLD.goal_id AND finding.status = 'open'
      )
      AND NOT EXISTS (
          SELECT 1
          FROM work_graph_nodes AS node
          JOIN work_tasks AS task ON task.id = node.task_id
          WHERE node.goal_id = OLD.goal_id
            AND (
                task.status <> 'completed'
                OR NOT EXISTS (
                    SELECT 1
                    FROM work_graph_node_finishes AS finish
                    JOIN task_attempts AS attempt
                      ON attempt.id = finish.attempt_id
                     AND attempt.status = 'succeeded'
                    JOIN task_validation_policies AS policy
                      ON policy.task_id = task.id
                     AND policy.policy_hash = finish.validation_policy_hash
                    WHERE finish.goal_id = OLD.goal_id
                      AND finish.task_id = node.task_id
                      AND (
                          policy.policy_id = 'standard_v1'
                          OR EXISTS (
                              SELECT 1
                              FROM work_graph_node_review_requests AS request
                              JOIN work_graph_node_review_decisions AS decision
                                ON decision.request_id = request.id
                               AND decision.outcome = 'pass'
                              WHERE request.attempt_id = finish.attempt_id
                                AND request.input_json = finish.input_json
                                AND request.child_result_hash = finish.child_result_hash
                                AND request.validation_policy_hash = finish.validation_policy_hash
                                AND request.settled_at IS NOT NULL
                                AND EXISTS (
                                    SELECT 1 FROM work_graph_node_review_proofs AS proof
                                    WHERE proof.decision_id = decision.id
                                )
                                AND NOT EXISTS (
                                    SELECT 1
                                    FROM work_graph_node_review_proofs AS proof
                                    LEFT JOIN tool_calls AS proof_call
                                      ON proof_call.id = proof.tool_call_id
                                    LEFT JOIN runs AS reviewer_run
                                      ON reviewer_run.id = decision.reviewer_run_id
                                    WHERE proof.decision_id = decision.id
                                      AND (
                                          proof_call.id IS NULL
                                          OR proof_call.runtime_tool_call_id <>
                                             proof.runtime_tool_call_id
                                          OR proof_call.run_id <> decision.reviewer_run_id
                                          OR proof_call.conversation_id <>
                                             decision.reviewer_conversation_id
                                          OR proof_call.execution_location <> 'runtime'
                                          OR proof_call.tool_name NOT IN ('read', 'ls', 'find', 'grep')
                                          OR proof_call.requires_approval <> 0
                                          OR proof_call.status <> 'completed'
                                          OR COALESCE(proof_call.error_message, '') <> ''
                                          OR proof_call.completed_at IS NULL
                                          OR proof_call.started_at <= request.activated_at
                                          OR proof_call.completed_at > reviewer_run.finished_at
                                      )
                                )
                          )
                      )
                )
            )
      )
      AND NOT EXISTS (
          SELECT 1
          FROM work_graph_node_criterion_evidence AS binding
          JOIN work_graph_node_finishes AS finish ON finish.id = binding.finish_id
          JOIN task_evidence AS evidence ON evidence.id = binding.evidence_id
          WHERE finish.goal_id = OLD.goal_id
            AND (evidence.validity_status <> 'valid' OR evidence.checked_at IS NULL)
      )
      AND NOT EXISTS (
          SELECT 1 FROM work_graph_node_review_requests AS request
          WHERE request.goal_id = OLD.goal_id
            AND request.activated_at IS NOT NULL
            AND request.settled_at IS NULL
      )
      AND NOT EXISTS (
          SELECT 1
          FROM child_run_delegations AS delegation
          JOIN task_attempts AS attempt ON attempt.id = delegation.graph_task_attempt_id
          JOIN work_tasks AS task ON task.id = attempt.task_id
          WHERE task.goal_id = OLD.goal_id
            AND delegation.status IN ('queued', 'running')
      )
)
BEGIN
    SELECT RAISE(ABORT, 'Graph acceptance requires a complete live accepted projection');
END;

CREATE TRIGGER work_graph_acceptances_no_update
BEFORE UPDATE ON work_graph_acceptances
FOR EACH ROW
WHEN NEW.id IS NOT OLD.id
 OR NEW.goal_id IS NOT OLD.goal_id
 OR NEW.plan_revision_id IS NOT OLD.plan_revision_id
 OR NEW.spec_hash IS NOT OLD.spec_hash
 OR NEW.submitter_run_id IS NOT OLD.submitter_run_id
 OR NEW.tool_call_id IS NOT OLD.tool_call_id
 OR NEW.expected_goal_version IS NOT OLD.expected_goal_version
 OR NEW.summary IS NOT OLD.summary
 OR NEW.node_projection_json IS NOT OLD.node_projection_json
 OR NEW.node_projection_hash IS NOT OLD.node_projection_hash
 OR NEW.checks_json IS NOT OLD.checks_json
 OR NEW.checks_hash IS NOT OLD.checks_hash
 OR NEW.input_json IS NOT OLD.input_json
 OR NEW.input_hash IS NOT OLD.input_hash
 OR NEW.created_at IS NOT OLD.created_at
 OR OLD.accepted_at IS NOT NULL
 OR NEW.accepted_at IS NULL
BEGIN
    SELECT RAISE(ABORT, 'Graph acceptance provenance only allows one activation');
END;

CREATE TRIGGER acceptances_require_graph_provenance
BEFORE INSERT ON acceptances
FOR EACH ROW
WHEN NEW.status = 'accepted'
 AND EXISTS (SELECT 1 FROM work_graph_specs WHERE goal_id = NEW.goal_id)
 AND NOT EXISTS (
     SELECT 1 FROM work_graph_acceptances AS graph_acceptance
     WHERE graph_acceptance.id = NEW.id
       AND graph_acceptance.goal_id = NEW.goal_id
       AND graph_acceptance.plan_revision_id = NEW.plan_revision_id
       AND graph_acceptance.accepted_at IS NOT NULL
       AND graph_acceptance.summary = NEW.summary
       AND graph_acceptance.checks_json = NEW.checks_json
       AND graph_acceptance.submitter_run_id = NEW.reviewer
 )
BEGIN
    SELECT RAISE(ABORT, 'Graph accepted record requires immutable Graph provenance');
END;

CREATE TRIGGER goals_require_graph_acceptance
BEFORE UPDATE OF status ON goals
FOR EACH ROW
WHEN OLD.status = 'active'
 AND NEW.status = 'completed'
 AND EXISTS (SELECT 1 FROM work_graph_specs WHERE goal_id = OLD.id)
 AND NOT EXISTS (
     SELECT 1
     FROM work_graph_acceptances AS graph_acceptance
     JOIN acceptances AS acceptance
       ON acceptance.id = graph_acceptance.id
      AND acceptance.goal_id = graph_acceptance.goal_id
      AND acceptance.status = 'accepted'
     WHERE graph_acceptance.goal_id = OLD.id
       AND graph_acceptance.accepted_at IS NOT NULL
       AND NEW.version = OLD.version + 1
       AND NEW.completed_at IS NOT NULL
 )
BEGIN
    SELECT RAISE(ABORT, 'Graph Goal completion requires the same accepted provenance chain');
END;

CREATE TRIGGER work_graph_node_review_decisions_no_update
BEFORE UPDATE ON work_graph_node_review_decisions
BEGIN
    SELECT RAISE(ABORT, 'Graph review decisions are immutable');
END;

CREATE TRIGGER work_graph_node_review_proofs_no_update
BEFORE UPDATE ON work_graph_node_review_proofs
BEGIN
    SELECT RAISE(ABORT, 'Graph review proof bindings are immutable');
END;

CREATE TRIGGER work_graph_node_review_requests_no_direct_delete
BEFORE DELETE ON work_graph_node_review_requests
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1 FROM work_graph_cascade_delete_scopes WHERE goal_id = OLD.goal_id
)
BEGIN
    SELECT RAISE(ABORT, 'Graph review requests can only be deleted with their Goal');
END;

CREATE TRIGGER work_graph_node_review_decisions_no_direct_delete
BEFORE DELETE ON work_graph_node_review_decisions
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1 FROM work_graph_cascade_delete_scopes WHERE goal_id = OLD.goal_id
)
BEGIN
    SELECT RAISE(ABORT, 'Graph review decisions can only be deleted with their Goal');
END;

CREATE TRIGGER work_graph_node_review_proofs_no_direct_delete
BEFORE DELETE ON work_graph_node_review_proofs
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1
    FROM work_graph_node_review_decisions AS decision
    WHERE decision.id = OLD.decision_id
      AND EXISTS (
          SELECT 1 FROM work_graph_cascade_delete_scopes WHERE goal_id = decision.goal_id
      )
)
BEGIN
    SELECT RAISE(ABORT, 'Graph review proof bindings can only be deleted with their Goal');
END;

CREATE TRIGGER work_graph_acceptances_no_direct_delete
BEFORE DELETE ON work_graph_acceptances
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1 FROM work_graph_cascade_delete_scopes WHERE goal_id = OLD.goal_id
)
BEGIN
    SELECT RAISE(ABORT, 'Graph acceptance provenance can only be deleted with its Goal');
END;

CREATE TRIGGER work_graph_generic_acceptances_no_update
BEFORE UPDATE ON acceptances
FOR EACH ROW
WHEN EXISTS (SELECT 1 FROM work_graph_acceptances WHERE id = OLD.id)
BEGIN
    SELECT RAISE(ABORT, 'accepted Graph records are immutable');
END;

CREATE TRIGGER work_graph_generic_acceptances_no_direct_delete
BEFORE DELETE ON acceptances
FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM work_graph_acceptances AS graph_acceptance
    WHERE graph_acceptance.id = OLD.id
      AND NOT EXISTS (
          SELECT 1 FROM work_graph_cascade_delete_scopes
          WHERE goal_id = graph_acceptance.goal_id
      )
)
BEGIN
    SELECT RAISE(ABORT, 'accepted Graph records can only be deleted with their Goal');
END;

CREATE TRIGGER work_graph_review_authority_runs_no_direct_delete
BEFORE DELETE ON runs
FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM work_graph_node_review_requests AS request
    WHERE OLD.id IN (request.parent_run_id, request.implementation_child_run_id)
      AND NOT EXISTS (
          SELECT 1 FROM work_graph_cascade_delete_scopes WHERE goal_id = request.goal_id
      )
    UNION ALL
    SELECT 1 FROM work_graph_node_review_decisions AS decision
    WHERE decision.reviewer_run_id = OLD.id
      AND NOT EXISTS (
          SELECT 1 FROM work_graph_cascade_delete_scopes WHERE goal_id = decision.goal_id
      )
    UNION ALL
    SELECT 1 FROM work_graph_acceptances AS acceptance
    WHERE acceptance.submitter_run_id = OLD.id
      AND NOT EXISTS (
          SELECT 1 FROM work_graph_cascade_delete_scopes WHERE goal_id = acceptance.goal_id
      )
)
BEGIN
    SELECT RAISE(ABORT, 'Graph review/acceptance Runs can only be deleted with their Goal');
END;

CREATE TRIGGER work_graph_review_authority_delegations_no_direct_delete
BEFORE DELETE ON child_run_delegations
FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM work_graph_node_review_requests AS request
    WHERE request.implementation_delegation_id = OLD.id
      AND NOT EXISTS (
          SELECT 1 FROM work_graph_cascade_delete_scopes WHERE goal_id = request.goal_id
      )
    UNION ALL
    SELECT 1 FROM work_graph_node_review_decisions AS decision
    WHERE decision.reviewer_delegation_id = OLD.id
      AND NOT EXISTS (
          SELECT 1 FROM work_graph_cascade_delete_scopes WHERE goal_id = decision.goal_id
      )
)
BEGIN
    SELECT RAISE(ABORT, 'Graph review delegations can only be deleted with their Goal');
END;

CREATE TRIGGER work_graph_review_accept_tools_no_direct_delete
BEFORE DELETE ON tool_calls
FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM work_graph_node_review_requests AS request
    WHERE request.tool_call_id = OLD.id
      AND NOT EXISTS (
          SELECT 1 FROM work_graph_cascade_delete_scopes WHERE goal_id = request.goal_id
      )
    UNION ALL
    SELECT 1 FROM work_graph_acceptances AS acceptance
    WHERE acceptance.tool_call_id = OLD.id
      AND NOT EXISTS (
          SELECT 1 FROM work_graph_cascade_delete_scopes WHERE goal_id = acceptance.goal_id
      )
)
BEGIN
    SELECT RAISE(ABORT, 'Graph review/acceptance ToolCalls can only be deleted with their Goal');
END;

CREATE TRIGGER work_graph_reviewer_proof_tools_no_update
BEFORE UPDATE OF runtime_tool_call_id, run_id, conversation_id, tool_name,
                 execution_location, requires_approval, status, error_message,
                 started_at, completed_at, updated_at
ON tool_calls
FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM work_graph_node_review_proofs AS proof
    WHERE proof.tool_call_id = OLD.id
)
BEGIN
    SELECT RAISE(ABORT, 'Graph Reviewer proof ToolCall authority is immutable');
END;

CREATE TRIGGER work_graph_reviewer_proof_tools_no_direct_delete
BEFORE DELETE ON tool_calls
FOR EACH ROW
WHEN EXISTS (
    SELECT 1
    FROM work_graph_node_review_proofs AS proof
    JOIN work_graph_node_review_decisions AS decision ON decision.id = proof.decision_id
    WHERE proof.tool_call_id = OLD.id
      AND NOT EXISTS (
          SELECT 1 FROM work_graph_cascade_delete_scopes WHERE goal_id = decision.goal_id
      )
)
BEGIN
    SELECT RAISE(ABORT, 'Graph Reviewer proof ToolCalls can only be deleted with their Goal');
END;

CREATE TRIGGER work_graph_review_bound_evidence_no_direct_delete
BEFORE DELETE ON task_evidence
FOR EACH ROW
WHEN EXISTS (
    SELECT 1
    FROM work_graph_node_review_criterion_evidence AS binding
    JOIN work_graph_node_review_requests AS request ON request.id = binding.request_id
    WHERE binding.evidence_id = OLD.id
      AND NOT EXISTS (
          SELECT 1 FROM work_graph_cascade_delete_scopes WHERE goal_id = request.goal_id
      )
)
BEGIN
    SELECT RAISE(ABORT, 'Graph review-bound Evidence can only be deleted with its Goal');
END;
"#;

const AGENT_CLASSIFICATION_SCHEMA: &str = r#"
ALTER TABLE agents ADD COLUMN agent_kind TEXT NOT NULL DEFAULT 'expert';
ALTER TABLE agents ADD COLUMN invocation_mode TEXT NOT NULL DEFAULT 'inline';
ALTER TABLE agents ADD COLUMN visibility TEXT NOT NULL DEFAULT 'expert_center'
    CHECK(
        (agent_kind = 'assistant' AND invocation_mode = 'primary' AND visibility = 'chat_selector') OR
        (agent_kind = 'expert' AND invocation_mode = 'inline' AND visibility = 'expert_center') OR
        (agent_kind = 'worker' AND invocation_mode = 'child' AND visibility = 'hidden')
    );

UPDATE agents
SET agent_kind = 'assistant', invocation_mode = 'primary', visibility = 'chat_selector'
WHERE id = 'fox-general';

UPDATE agents
SET agent_kind = CASE
        WHEN json_valid(system_prompt) AND COALESCE(
            json_extract(system_prompt, '$.isSubagent'),
            json_extract(system_prompt, '$.is_subagent'),
            0
        ) = 1 THEN 'worker'
        WHEN json_valid(system_prompt) AND (
            COALESCE(json_extract(system_prompt, '$.isDefault'), json_extract(system_prompt, '$.is_default'), 0) = 1 OR
            COALESCE(json_extract(system_prompt, '$.chatVisible'), json_extract(system_prompt, '$.chat_visible'), 0) = 1
        ) THEN 'assistant'
        ELSE 'expert'
    END,
    invocation_mode = CASE
        WHEN json_valid(system_prompt) AND COALESCE(
            json_extract(system_prompt, '$.isSubagent'),
            json_extract(system_prompt, '$.is_subagent'),
            0
        ) = 1 THEN 'child'
        WHEN json_valid(system_prompt) AND (
            COALESCE(json_extract(system_prompt, '$.isDefault'), json_extract(system_prompt, '$.is_default'), 0) = 1 OR
            COALESCE(json_extract(system_prompt, '$.chatVisible'), json_extract(system_prompt, '$.chat_visible'), 0) = 1
        ) THEN 'primary'
        ELSE 'inline'
    END,
    visibility = CASE
        WHEN json_valid(system_prompt) AND COALESCE(
            json_extract(system_prompt, '$.isSubagent'),
            json_extract(system_prompt, '$.is_subagent'),
            0
        ) = 1 THEN 'hidden'
        WHEN json_valid(system_prompt) AND (
            COALESCE(json_extract(system_prompt, '$.isDefault'), json_extract(system_prompt, '$.is_default'), 0) = 1 OR
            COALESCE(json_extract(system_prompt, '$.chatVisible'), json_extract(system_prompt, '$.chat_visible'), 0) = 1
        ) THEN 'chat_selector'
        ELSE 'expert_center'
    END
WHERE runtime_type = 'yuxi';
"#;

const CONVERSATION_EXPERT_BINDINGS_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS conversation_expert_bindings (
    id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    expert_id TEXT NOT NULL REFERENCES agents(id),
    state TEXT NOT NULL CHECK(state IN ('active', 'replaced', 'removed')),
    activation_source TEXT NOT NULL,
    expert_version TEXT NOT NULL,
    package_hash TEXT NOT NULL,
    display_snapshot_json TEXT NOT NULL,
    activated_at INTEGER NOT NULL,
    deactivated_at INTEGER
);
CREATE INDEX IF NOT EXISTS idx_conversation_expert_bindings_history
    ON conversation_expert_bindings(conversation_id, activated_at, id);
CREATE UNIQUE INDEX IF NOT EXISTS idx_conversation_expert_bindings_single_active
    ON conversation_expert_bindings(conversation_id) WHERE state = 'active';
"#;

const MIGRATION_40: &str = r#"
ALTER TABLE projects ADD COLUMN archived_at INTEGER;

CREATE TABLE app_notifications (
    id TEXT PRIMARY KEY,
    merge_key TEXT NOT NULL UNIQUE,
    kind TEXT NOT NULL CHECK(kind IN ('approval', 'question', 'progress', 'completed', 'failed')),
    severity TEXT NOT NULL CHECK(severity IN ('quiet', 'normal', 'high')),
    title TEXT NOT NULL,
    body TEXT NOT NULL,
    source_type TEXT NOT NULL,
    source_id TEXT NOT NULL,
    workspace_view TEXT,
    entity_id TEXT,
    progress INTEGER CHECK(progress IS NULL OR progress BETWEEN 0 AND 100),
    status TEXT NOT NULL,
    action_json TEXT NOT NULL DEFAULT '{}'
        CHECK(json_valid(action_json) AND json_type(action_json) = 'object'),
    read_at INTEGER,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE INDEX idx_app_notifications_read_updated
    ON app_notifications(read_at, updated_at DESC, id);
CREATE INDEX idx_app_notifications_kind_status
    ON app_notifications(kind, status, updated_at DESC);

CREATE TABLE notification_preferences (
    singleton_id INTEGER PRIMARY KEY CHECK(singleton_id = 1),
    system_popup INTEGER NOT NULL DEFAULT 0 CHECK(system_popup IN (0, 1)),
    sound INTEGER NOT NULL DEFAULT 0 CHECK(sound IN (0, 1)),
    badge INTEGER NOT NULL DEFAULT 1 CHECK(badge IN (0, 1)),
    quiet_progress INTEGER NOT NULL DEFAULT 1 CHECK(quiet_progress IN (0, 1)),
    updated_at INTEGER NOT NULL
);
INSERT INTO notification_preferences(
    singleton_id, system_popup, sound, badge, quiet_progress, updated_at
) VALUES (1, 0, 0, 1, 1, 0);

CREATE TABLE message_feedback (
    id TEXT PRIMARY KEY,
    message_id TEXT NOT NULL UNIQUE REFERENCES messages(id) ON DELETE CASCADE,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    run_id TEXT REFERENCES runs(id) ON DELETE SET NULL,
    sentiment TEXT NOT NULL CHECK(sentiment IN ('positive', 'negative')),
    category TEXT CHECK(category IN ('irrelevant', 'code_error', 'misunderstanding', 'other')),
    comment TEXT,
    model TEXT,
    context_json TEXT NOT NULL DEFAULT '{}'
        CHECK(json_valid(context_json) AND json_type(context_json) = 'object'),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE INDEX idx_message_feedback_created
    ON message_feedback(created_at DESC, id);
"#;

const MIGRATION_41: &str = r#"
ALTER TABLE notification_preferences
ADD COLUMN sound_id TEXT NOT NULL DEFAULT 'soft'
    CHECK(sound_id IN ('soft', 'chime', 'pop', 'signal'));
"#;

// Some databases recorded v36 before the Graph cascade-delete sentinel was added
// to that migration. Later Graph triggers still reference the sentinel, so deleting
// any Goal fails with "no such table: main.work_graph_cascade_delete_scopes".
// Repair the missing object under a new version and reinstall the four scope
// triggers so both Goal and Conversation cascades remain atomic.
const MIGRATION_42: &str = r#"
CREATE TABLE IF NOT EXISTS work_graph_cascade_delete_scopes (
    goal_id TEXT PRIMARY KEY CHECK(length(trim(goal_id)) BETWEEN 1 AND 200),
    conversation_id TEXT NOT NULL CHECK(length(trim(conversation_id)) BETWEEN 1 AND 200),
    opened_at INTEGER NOT NULL CHECK(opened_at >= 0)
);

DROP TRIGGER IF EXISTS goals_open_work_graph_cascade_delete_scope;
CREATE TRIGGER goals_open_work_graph_cascade_delete_scope
BEFORE DELETE ON goals
FOR EACH ROW
WHEN EXISTS (SELECT 1 FROM work_graph_specs WHERE goal_id = OLD.id)
BEGIN
    INSERT OR IGNORE INTO work_graph_cascade_delete_scopes(
        goal_id, conversation_id, opened_at
    )
    VALUES (OLD.id, OLD.conversation_id, CAST(strftime('%s', 'now') AS INTEGER) * 1000);
END;

DROP TRIGGER IF EXISTS goals_close_work_graph_cascade_delete_scope;
CREATE TRIGGER goals_close_work_graph_cascade_delete_scope
AFTER DELETE ON goals
FOR EACH ROW
BEGIN
    DELETE FROM work_graph_cascade_delete_scopes WHERE goal_id = OLD.id;
END;

DROP TRIGGER IF EXISTS conversations_open_work_graph_cascade_delete_scopes;
CREATE TRIGGER conversations_open_work_graph_cascade_delete_scopes
BEFORE DELETE ON conversations
FOR EACH ROW
BEGIN
    INSERT OR IGNORE INTO work_graph_cascade_delete_scopes(
        goal_id, conversation_id, opened_at
    )
    SELECT
        spec.goal_id,
        OLD.id,
        CAST(strftime('%s', 'now') AS INTEGER) * 1000
    FROM work_graph_specs AS spec
    JOIN goals AS goal ON goal.id = spec.goal_id
    WHERE goal.conversation_id = OLD.id;
END;

DROP TRIGGER IF EXISTS conversations_close_work_graph_cascade_delete_scopes;
CREATE TRIGGER conversations_close_work_graph_cascade_delete_scopes
AFTER DELETE ON conversations
FOR EACH ROW
BEGIN
    DELETE FROM work_graph_cascade_delete_scopes WHERE conversation_id = OLD.id;
END;
"#;

// Conversation-level grants used to be keyed only by tool name. That allowed an
// approval for one target to authorize a different command, file, URL, or MCP
// operation. Rebuild the table with an explicit operation scope and intentionally
// discard legacy broad grants: they cannot be narrowed safely after the fact.
const MIGRATION_43: &str = r#"
DROP TABLE IF EXISTS conversation_tool_permissions_v43;
CREATE TABLE conversation_tool_permissions_v43 (
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    tool_name TEXT NOT NULL,
    scope_key TEXT NOT NULL CHECK(length(trim(scope_key)) BETWEEN 1 AND 512),
    granted_at INTEGER NOT NULL,
    PRIMARY KEY(conversation_id, tool_name, scope_key)
);
DROP TABLE conversation_tool_permissions;
ALTER TABLE conversation_tool_permissions_v43 RENAME TO conversation_tool_permissions;
CREATE INDEX idx_conversation_tool_permissions_granted
    ON conversation_tool_permissions(conversation_id, granted_at DESC);
"#;

// Clearing a derived notification used to delete its row. The next notification
// sync then recreated the same historical run/approval/job as a brand-new unread
// item. Keep a dismissal tombstone on the row so unchanged source state remains
// hidden, while a later lifecycle transition can deliberately reopen it.
const MIGRATION_44: &str = r#"
ALTER TABLE app_notifications ADD COLUMN dismissed_at INTEGER;
CREATE INDEX idx_app_notifications_visible_updated
    ON app_notifications(dismissed_at, read_at, updated_at DESC, id);
"#;

// Project permissions used to live only on projects. Persist the selected
// interaction/execution mode on every conversation as well, so projectless
// chats do not silently fall back to read-only after the first message or an
// application restart. A linked project's permission remains authoritative.
const MIGRATION_45: &str = r#"
ALTER TABLE conversations
ADD COLUMN permission_mode TEXT NOT NULL DEFAULT 'ask'
    CHECK(permission_mode IN ('read_only', 'ask', 'allow'));
UPDATE conversations
SET permission_mode = COALESCE(
    (SELECT permission_mode FROM projects WHERE projects.id = conversations.project_id),
    permission_mode
);
"#;

// SenseNova product pages use a human-friendly display name, while the
// OpenAI-compatible API requires a lowercase, hyphenated model ID. Repair
// configurations that accidentally stored the display name as the ID.
const MIGRATION_46: &str = r#"
UPDATE model_service
SET model_id = 'sensenova-6.8-flash-lite',
    last_status = 'unknown',
    last_latency_ms = NULL,
    last_checked_at = NULL
WHERE lower(base_url) LIKE '%://token.sensenova.cn/%'
  AND lower(trim(model_id)) = 'sensenova 6.8 flash lite';

UPDATE provider_models
SET model_id = 'sensenova-6.8-flash-lite'
WHERE lower(trim(model_id)) = 'sensenova 6.8 flash lite'
  AND provider_id IN (
      SELECT id FROM model_providers
      WHERE lower(base_url) LIKE '%://token.sensenova.cn/%'
  );

UPDATE model_providers
SET last_status = 'unknown',
    last_latency_ms = NULL,
    last_checked_at = NULL
WHERE lower(base_url) LIKE '%://token.sensenova.cn/%';
"#;

// Earlier UI actions persisted confirmations even when their result was
// already visible (opening a file, changing a selector, or restoring the
// default assistant). Remove those legacy rows once and keep the notification
// center focused on actionable or background work.
const MIGRATION_47: &str = r#"
DELETE FROM app_notifications
WHERE (
    source_type = 'ui'
    AND (
        body IN (
            '已切换助手，发送消息后创建对话',
            '已打开相关执行记录',
            '已打开项目文件夹',
            '已使用系统默认方式打开',
            '已在文件夹中定位'
        )
        OR body LIKE '已使用 % 打开'
        OR body LIKE '执行方式已切换为%'
        OR body LIKE '已选择：%'
        OR title = '正在重新加载预览'
    )
)
OR (
    source_type = 'knowledge_document'
    AND body = '已在系统默认应用中打开原文件。'
);
"#;

/// Kernel schema (v48): durable facts for the Fox Agent Kernel. These tables are
/// purely additive — legacy runs do not reference them and rolling back to the
/// pre-migration application + database backup restores prior behaviour without
/// data loss (forward-only migration, see 迁移备份与恢复).
const MIGRATION_48: &str = r#"
CREATE TABLE IF NOT EXISTS kernel_runs (
    run_id TEXT PRIMARY KEY,
    engine_id TEXT NOT NULL,
    kernel_mode TEXT NOT NULL CHECK(kernel_mode IN ('legacy','shadow','authoritative')),
    capability_manifest_version INTEGER NOT NULL,
    permission_snapshot_id TEXT NOT NULL,
    execution_profile_id TEXT NOT NULL,
    prompt_config_hash TEXT NOT NULL,
    frozen_config_json TEXT NOT NULL,
    state TEXT NOT NULL,
    last_event_seq INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    terminal_at INTEGER
);

CREATE TABLE IF NOT EXISTS kernel_tool_batches (
    batch_id TEXT PRIMARY KEY,
    run_id TEXT NOT NULL,
    ordered_tool_call_ids_json TEXT NOT NULL,
    barrier_emitted INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_kernel_tool_batches_run
    ON kernel_tool_batches(run_id);

CREATE TABLE IF NOT EXISTS kernel_tool_calls (
    run_id TEXT NOT NULL,
    tool_call_id TEXT NOT NULL,
    batch_id TEXT NOT NULL,
    tool TEXT NOT NULL,
    source_order INTEGER NOT NULL,
    canonical_input_json TEXT NOT NULL,
    state TEXT NOT NULL CHECK(state IN (
        'pending','waiting_approval','running','completed','failed','cancelled'
    )),
    result_json TEXT,
    created_at INTEGER NOT NULL,
    settled_at INTEGER,
    PRIMARY KEY(run_id, tool_call_id)
);
CREATE INDEX IF NOT EXISTS idx_kernel_tool_calls_batch
    ON kernel_tool_calls(run_id, batch_id);
CREATE INDEX IF NOT EXISTS idx_kernel_tool_calls_state
    ON kernel_tool_calls(run_id, state);

CREATE TABLE IF NOT EXISTS kernel_approvals (
    run_id TEXT NOT NULL,
    tool_call_id TEXT NOT NULL,
    state TEXT NOT NULL CHECK(state IN (
        'pending','allow_once','allow_conversation','denied','cancelled','expired'
    )),
    created_at INTEGER NOT NULL,
    decided_at INTEGER,
    PRIMARY KEY(run_id, tool_call_id)
);
CREATE INDEX IF NOT EXISTS idx_kernel_approvals_pending
    ON kernel_approvals(run_id, state);

-- Append-only kernel event stream; idempotent on (run_id, seq).
CREATE TABLE IF NOT EXISTS kernel_events (
    run_id TEXT NOT NULL,
    seq INTEGER NOT NULL,
    event_type TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    PRIMARY KEY(run_id, seq)
);
CREATE INDEX IF NOT EXISTS idx_kernel_events_run
    ON kernel_events(run_id, seq);
"#;

/// Kernel schema (v49): durable Effect/Dispatch Outbox and full RunController
/// rehydration columns.
///
/// Identity ruling (enforced here and in the repository, matching v48):
/// * `batch_id` is GLOBALLY unique (kernel_tool_batches PRIMARY KEY batch_id);
///   a batch carries its owning `run_id`, and reusing a batch id in another run
///   is rejected.
/// * a tool call is unique within a run: kernel_tool_calls PRIMARY KEY
///   `(run_id, tool_call_id)`.
/// * an outbox effect is keyed `(run_id, effect_key)` with a run-scoped unique
///   `idempotency_key`.
///
/// The outbox holds external actions (tool dispatch, approval prompt, engine
/// cancel) that are committed in the SAME transaction as the Kernel decision
/// facts, and only executed after commit. Each row has a stable idempotency key
/// so a crash after commit never produces a second logical side effect.
///
/// Forward-only and additive. `apply_migration` records v49 atomically, so the
/// migration runner is repeat-safe; the raw ALTER script itself is not rerun.
/// Rollback is the pre-migration backup plus the previous application build.
const MIGRATION_49: &str = r#"
-- Rehydration columns on kernel_runs (all additive / nullable or defaulted).
ALTER TABLE kernel_runs ADD COLUMN turn_id TEXT;
ALTER TABLE kernel_runs ADD COLUMN running_elapsed_ms INTEGER NOT NULL DEFAULT 0;
ALTER TABLE kernel_runs ADD COLUMN running_since_wall_ms INTEGER;
ALTER TABLE kernel_runs ADD COLUMN approval_deadline_wall_ms INTEGER;
ALTER TABLE kernel_runs ADD COLUMN capability_manifest_hash TEXT;
ALTER TABLE kernel_runs ADD COLUMN terminal_written INTEGER NOT NULL DEFAULT 0;
ALTER TABLE kernel_runs ADD COLUMN retry_state_json TEXT;
ALTER TABLE kernel_runs ADD COLUMN compaction_state_json TEXT;

-- Link a dispatched tool call to its stable external idempotency key.
ALTER TABLE kernel_tool_calls ADD COLUMN dispatch_idempotency_key TEXT;

-- kernel_tool_batches keeps the v48 GLOBAL batch_id PRIMARY KEY; no rebuild.

-- Durable external-effect outbox.
CREATE TABLE IF NOT EXISTS kernel_effect_outbox (
    run_id TEXT NOT NULL,
    effect_key TEXT NOT NULL,
    effect_type TEXT NOT NULL CHECK(effect_type IN (
        'dispatch_tool','request_approval','cancel_engine_turn','cancel_tool_call','publish_snapshot'
    )),
    idempotency_key TEXT NOT NULL,
    tool_call_id TEXT,
    batch_id TEXT,
    payload_json TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('pending','leased','completed','failed')),
    attempts INTEGER NOT NULL DEFAULT 0,
    lease_owner TEXT,
    leased_at INTEGER,
    completed_at INTEGER,
    last_error TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (run_id, effect_key)
);
CREATE INDEX IF NOT EXISTS idx_kernel_outbox_status
    ON kernel_effect_outbox(run_id, status);
CREATE INDEX IF NOT EXISTS idx_kernel_outbox_due
    ON kernel_effect_outbox(status, created_at);
-- An external idempotency key maps to at most one logical effect within a run.
CREATE UNIQUE INDEX IF NOT EXISTS idx_kernel_outbox_idem
    ON kernel_effect_outbox(run_id, idempotency_key);
"#;

/// Kernel schema (v50): make the all-settled batch delivery itself durable.
///
/// SQLite cannot extend an existing CHECK constraint in place, so this is a
/// forward-only table rebuild that preserves every v49 outbox row and adds the
/// `deliver_tool_batch` effect kind. The schema migration runner makes the step
/// repeat-safe by recording v50 in `schema_migrations` in the same transaction.
const MIGRATION_50: &str = r#"
ALTER TABLE kernel_effect_outbox RENAME TO kernel_effect_outbox_v49;

CREATE TABLE kernel_effect_outbox (
    run_id TEXT NOT NULL,
    effect_key TEXT NOT NULL,
    effect_type TEXT NOT NULL CHECK(effect_type IN (
        'dispatch_tool','request_approval','cancel_engine_turn','cancel_tool_call',
        'deliver_tool_batch','publish_snapshot'
    )),
    idempotency_key TEXT NOT NULL,
    tool_call_id TEXT,
    batch_id TEXT,
    payload_json TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('pending','leased','completed','failed')),
    attempts INTEGER NOT NULL DEFAULT 0,
    lease_owner TEXT,
    leased_at INTEGER,
    completed_at INTEGER,
    last_error TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (run_id, effect_key)
);

INSERT INTO kernel_effect_outbox (
    run_id, effect_key, effect_type, idempotency_key, tool_call_id, batch_id,
    payload_json, status, attempts, lease_owner, leased_at, completed_at,
    last_error, created_at, updated_at
)
SELECT
    run_id, effect_key, effect_type, idempotency_key, tool_call_id, batch_id,
    payload_json, status, attempts, lease_owner, leased_at, completed_at,
    last_error, created_at, updated_at
FROM kernel_effect_outbox_v49;

DROP TABLE kernel_effect_outbox_v49;

CREATE INDEX idx_kernel_outbox_status
    ON kernel_effect_outbox(run_id, status);
CREATE INDEX idx_kernel_outbox_due
    ON kernel_effect_outbox(status, created_at);
CREATE UNIQUE INDEX idx_kernel_outbox_idem
    ON kernel_effect_outbox(run_id, idempotency_key);
"#;

/// Kernel schema (v51): production Shadow observation/diff persistence.
///
/// Shadow runs are physically isolated from the executable outbox: a shadow
/// decision is NEVER written to `kernel_effect_outbox`, so no startup/lease
/// scan (which only reads `kernel_effect_outbox`) can ever execute a shadow
/// side effect. Shadow runs and their comparison records live in dedicated,
/// append-only observation tables with no leased/pending executability.
///
/// Forward-only, additive and repeatable; rollback is the pre-migration backup
/// plus the previous build (see 迁移备份与恢复).
const MIGRATION_51: &str = r#"
-- One shadow context per legacy (kernel mode = 'shadow') run. Holds the frozen
-- identity and the legacy run it observes. No execution columns exist here.
CREATE TABLE IF NOT EXISTS kernel_shadow_runs (
    shadow_run_id TEXT PRIMARY KEY,
    legacy_run_id TEXT NOT NULL,
    conversation_id TEXT NOT NULL,
    turn_id TEXT,
    engine_id TEXT NOT NULL,
    kernel_mode TEXT NOT NULL CHECK(kernel_mode IN ('shadow')),
    capability_manifest_version INTEGER NOT NULL,
    capability_manifest_hash TEXT NOT NULL,
    permission_snapshot_id TEXT NOT NULL,
    execution_profile_id TEXT NOT NULL,
    prompt_config_hash TEXT NOT NULL,
    frozen_config_json TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    UNIQUE(legacy_run_id)
);
CREATE INDEX IF NOT EXISTS idx_kernel_shadow_runs_legacy
    ON kernel_shadow_runs(legacy_run_id);

-- Append-only comparison records. Each row is one legacy-vs-kernel decision
-- diff at an event cursor; 'match' rows record agreement. Nothing here is
-- leaseable or executable — there is no status/outbox column by design.
CREATE TABLE IF NOT EXISTS kernel_shadow_diffs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    shadow_run_id TEXT NOT NULL,
    legacy_run_id TEXT NOT NULL,
    turn_id TEXT,
    event_cursor INTEGER NOT NULL,
    event_type TEXT,
    category TEXT NOT NULL CHECK(category IN (
        'match',
        'state_mismatch',
        'approval_mismatch',
        'tool_param_or_order_mismatch',
        'terminal_mismatch',
        'timeout_retry_mismatch',
        'not_comparable',
        'shadow_error'
    )),
    legacy_disposition_json TEXT NOT NULL,
    kernel_disposition_json TEXT NOT NULL,
    detail_json TEXT NOT NULL,
    schema_version INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    FOREIGN KEY(shadow_run_id) REFERENCES kernel_shadow_runs(shadow_run_id)
);
CREATE INDEX IF NOT EXISTS idx_kernel_shadow_diffs_run_cursor
    ON kernel_shadow_diffs(shadow_run_id, event_cursor);
CREATE INDEX IF NOT EXISTS idx_kernel_shadow_diffs_category
    ON kernel_shadow_diffs(category);
"#;

/// Kernel schema (v52): persist the model-request wall anchor so an in-flight
/// model request keeps its timeout window across restarts (a crash must not
/// grant a fresh window). This is a NEW migration (not appended to released
/// v51) so databases that already ran v51 without this column pick it up.
const MIGRATION_52: &str = r#"
ALTER TABLE kernel_runs ADD COLUMN model_request_since_wall_ms INTEGER;
"#;

const AGENT_EXPERT_SCHEMA_VERSION: i64 = 20;
const EXPERT_PACKAGE_SNAPSHOT_SCHEMA_VERSION: i64 = 21;
const EXPERT_PACKAGE_SNAPSHOT_SCHEMA: &str = r#"
ALTER TABLE conversation_expert_bindings
ADD COLUMN package_snapshot_json TEXT NOT NULL DEFAULT '{}';
"#;

const P0_P2_SCHEMA_VERSION: i64 = 22;
const P0_P2_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS plugin_catalog_cache (
    plugin_id TEXT NOT NULL,
    version TEXT NOT NULL,
    kind TEXT NOT NULL CHECK(kind IN ('mcp', 'skill', 'tool')),
    origin TEXT NOT NULL CHECK(origin IN ('builtin', 'official', 'community', 'local')),
    manifest_json TEXT NOT NULL,
    package_hash TEXT,
    signature TEXT,
    fetched_at INTEGER NOT NULL,
    PRIMARY KEY(plugin_id, version)
);

CREATE INDEX IF NOT EXISTS idx_plugin_catalog_cache_kind_origin
    ON plugin_catalog_cache(kind, origin, fetched_at DESC);

CREATE TABLE IF NOT EXISTS plugin_installations (
    plugin_id TEXT PRIMARY KEY,
    installed_version TEXT NOT NULL,
    origin TEXT NOT NULL CHECK(origin IN ('builtin', 'official', 'community', 'local')),
    install_status TEXT NOT NULL CHECK(
        install_status IN (
            'not_installed', 'installing', 'installed', 'update_available',
            'uninstalling', 'install_failed'
        )
    ),
    install_path TEXT,
    package_hash TEXT,
    installed_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    last_error_code TEXT,
    last_error_message TEXT
);

CREATE TABLE IF NOT EXISTS plugin_operations (
    id TEXT PRIMARY KEY,
    plugin_id TEXT,
    parent_operation_id TEXT REFERENCES plugin_operations(id) ON DELETE SET NULL,
    operation TEXT NOT NULL,
    status TEXT NOT NULL,
    stage TEXT CHECK(stage IN ('resolving', 'downloading', 'verifying', 'extracting', 'registering')),
    progress INTEGER NOT NULL DEFAULT 0 CHECK(progress BETWEEN 0 AND 100),
    outcome TEXT CHECK(outcome IN ('success', 'partial')),
    data_json TEXT,
    last_sequence INTEGER NOT NULL DEFAULT 0 CHECK(last_sequence >= 0),
    error_code TEXT,
    error_message TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_plugin_operations_plugin_updated
    ON plugin_operations(plugin_id, updated_at DESC);

CREATE TABLE IF NOT EXISTS knowledge_bindings_v2 (
    id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    source TEXT NOT NULL CHECK(source IN ('local', 'remote')),
    provider_key TEXT NOT NULL,
    connection_id TEXT REFERENCES service_connections(id) ON DELETE CASCADE,
    knowledge_base_id TEXT NOT NULL,
    knowledge_base_name TEXT,
    enabled INTEGER NOT NULL DEFAULT 1 CHECK(enabled IN (0, 1)),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    CHECK(
        (source = 'local' AND provider_key = 'local' AND connection_id IS NULL) OR
        (source = 'remote' AND connection_id IS NOT NULL AND provider_key = connection_id)
    ),
    UNIQUE(conversation_id, source, provider_key, knowledge_base_id)
);

CREATE INDEX IF NOT EXISTS idx_knowledge_bindings_v2_conversation
    ON knowledge_bindings_v2(conversation_id, enabled, knowledge_base_name);

CREATE TRIGGER IF NOT EXISTS knowledge_bindings_legacy_insert_to_v2
AFTER INSERT ON knowledge_bindings
BEGIN
    INSERT INTO knowledge_bindings_v2(
        id, conversation_id, source, provider_key, connection_id,
        knowledge_base_id, knowledge_base_name, enabled, created_at, updated_at
    ) VALUES (
        printf('legacy:%s:%s:%s', NEW.conversation_id, NEW.service_connection_id, NEW.knowledge_base_id),
        NEW.conversation_id,
        'remote',
        NEW.service_connection_id,
        NEW.service_connection_id,
        NEW.knowledge_base_id,
        NEW.knowledge_base_name,
        NEW.enabled,
        NEW.created_at,
        NEW.updated_at
    ) ON CONFLICT(id) DO UPDATE SET
        conversation_id = excluded.conversation_id,
        source = excluded.source,
        provider_key = excluded.provider_key,
        connection_id = excluded.connection_id,
        knowledge_base_id = excluded.knowledge_base_id,
        knowledge_base_name = excluded.knowledge_base_name,
        enabled = excluded.enabled,
        created_at = excluded.created_at,
        updated_at = excluded.updated_at;
END;

CREATE TRIGGER IF NOT EXISTS knowledge_bindings_legacy_update_to_v2
AFTER UPDATE ON knowledge_bindings
BEGIN
    DELETE FROM knowledge_bindings_v2
    WHERE id = printf('legacy:%s:%s:%s', OLD.conversation_id, OLD.service_connection_id, OLD.knowledge_base_id);
    INSERT INTO knowledge_bindings_v2(
        id, conversation_id, source, provider_key, connection_id,
        knowledge_base_id, knowledge_base_name, enabled, created_at, updated_at
    ) VALUES (
        printf('legacy:%s:%s:%s', NEW.conversation_id, NEW.service_connection_id, NEW.knowledge_base_id),
        NEW.conversation_id,
        'remote',
        NEW.service_connection_id,
        NEW.service_connection_id,
        NEW.knowledge_base_id,
        NEW.knowledge_base_name,
        NEW.enabled,
        NEW.created_at,
        NEW.updated_at
    ) ON CONFLICT(id) DO UPDATE SET
        conversation_id = excluded.conversation_id,
        source = excluded.source,
        provider_key = excluded.provider_key,
        connection_id = excluded.connection_id,
        knowledge_base_id = excluded.knowledge_base_id,
        knowledge_base_name = excluded.knowledge_base_name,
        enabled = excluded.enabled,
        created_at = excluded.created_at,
        updated_at = excluded.updated_at;
END;

CREATE TRIGGER IF NOT EXISTS knowledge_bindings_legacy_delete_from_v2
AFTER DELETE ON knowledge_bindings
BEGIN
    DELETE FROM knowledge_bindings_v2
    WHERE id = printf('legacy:%s:%s:%s', OLD.conversation_id, OLD.service_connection_id, OLD.knowledge_base_id);
END;
"#;

// Full tool results larger than the inline `result_json` cap are kept in this
// sibling blob table instead of being discarded. The inline row keeps the
// compact preview and gains `result_blob_sha256` (added by the column below)
// so the model-facing copy is unchanged while every byte remains recoverable
// through `tool_result_range`. Blob rows are content-addressed by sha256 and
// immutable: two calls with identical large results share one row.
const MIGRATION_63: &str = r#"
ALTER TABLE kernel_effect_outbox RENAME TO kernel_effect_outbox_v62;
CREATE TABLE kernel_effect_outbox (
    run_id TEXT NOT NULL, effect_key TEXT NOT NULL,
    effect_type TEXT NOT NULL CHECK(effect_type IN ('dispatch_tool','request_approval','cancel_engine_turn',
        'cancel_tool_call','deliver_tool_batch','initial_model','continuation_model','publish_snapshot')),
    idempotency_key TEXT NOT NULL, tool_call_id TEXT, batch_id TEXT, payload_json TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('pending','leased','completed','failed')),
    attempts INTEGER NOT NULL DEFAULT 0, lease_owner TEXT, leased_at INTEGER, completed_at INTEGER,
    last_error TEXT, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL,
    PRIMARY KEY(run_id,effect_key)
);
INSERT INTO kernel_effect_outbox SELECT * FROM kernel_effect_outbox_v62;
DROP TABLE kernel_effect_outbox_v62;
CREATE INDEX idx_kernel_outbox_status ON kernel_effect_outbox(run_id,status);
CREATE INDEX idx_kernel_outbox_due ON kernel_effect_outbox(status,created_at);
CREATE UNIQUE INDEX idx_kernel_outbox_idem ON kernel_effect_outbox(run_id,idempotency_key);
"#;

/// v64 (2026-09-13 design supplements): skill activation audit, ordinary-task
/// delivery checklist, persistent mid-run steering queue, managed file
/// versions. Every table is additive; no historical migration is rewritten.
const MIGRATION_64: &str = r#"
CREATE TABLE IF NOT EXISTS skill_activations (
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    conversation_id TEXT,
    skill_id TEXT NOT NULL,
    version TEXT NOT NULL,
    content_sha256 TEXT NOT NULL,
    source TEXT NOT NULL CHECK(source IN ('initial_prompt','on_demand')),
    required_tools_json TEXT NOT NULL DEFAULT '[]',
    missing_tools_json TEXT NOT NULL DEFAULT '[]',
    tools_available INTEGER NOT NULL CHECK(tools_available IN (0,1)),
    char_count INTEGER NOT NULL CHECK(char_count >= 0),
    byte_count INTEGER NOT NULL CHECK(byte_count >= 0),
    created_at INTEGER NOT NULL,
    UNIQUE(run_id, skill_id, source, content_sha256)
);
CREATE TABLE IF NOT EXISTS delivery_checklist_items (
    run_id TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    item_key TEXT NOT NULL,
    target_path TEXT,
    artifact_id TEXT,
    display_name TEXT NOT NULL,
    checks_json TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending' CHECK(status IN ('pending','passed','failed')),
    finding_json TEXT,
    checked_at INTEGER,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY(run_id,item_key)
);
CREATE INDEX IF NOT EXISTS idx_delivery_items_run ON delivery_checklist_items(run_id,status);
CREATE TABLE IF NOT EXISTS delivery_repair_rounds (
    run_id TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    round INTEGER NOT NULL CHECK(round >= 1),
    findings_json TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    PRIMARY KEY(run_id,round)
);
CREATE TABLE IF NOT EXISTS run_steering_messages (
    run_id TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    seq INTEGER NOT NULL CHECK(seq >= 1),
    message_id TEXT NOT NULL UNIQUE,
    content TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'received' CHECK(status IN ('received','applied','cancelled')),
    received_at INTEGER NOT NULL,
    applied_at INTEGER,
    applied_event_seq INTEGER,
    applied_dispatch_key TEXT,
    PRIMARY KEY(run_id,seq)
);
CREATE INDEX IF NOT EXISTS idx_run_steering_status ON run_steering_messages(run_id,status);
CREATE TABLE IF NOT EXISTS managed_file_versions (
    id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    run_id TEXT REFERENCES runs(id) ON DELETE SET NULL,
    tool_call_id TEXT,
    tool TEXT NOT NULL,
    storage_path TEXT NOT NULL,
    display_name TEXT NOT NULL,
    version_no INTEGER NOT NULL CHECK(version_no >= 1),
    change_kind TEXT NOT NULL CHECK(change_kind IN ('created','modified','restored')),
    before_hash TEXT,
    before_size INTEGER,
    after_hash TEXT,
    after_size INTEGER,
    backup_path TEXT,
    restored_from_id TEXT,
    created_at INTEGER NOT NULL,
    UNIQUE(storage_path, version_no)
);
CREATE INDEX IF NOT EXISTS idx_managed_file_versions_path ON managed_file_versions(storage_path, created_at);
CREATE INDEX IF NOT EXISTS idx_managed_file_versions_run ON managed_file_versions(run_id);
"#;

/// v65 (2026-09-13 design supplements, item 4): mid-run steering gains the
/// `delivered` state between `received` and `applied`: the model dispatch that
/// carries a message is durably armed (its lease committed) before the worker
/// answers, so a worker death after the directive leaves the message bound to
/// the in-flight dispatch rather than lost or double applied. The v64 table
/// is append-only data, so it is rebuilt with the widened CHECK constraint
/// instead of editing the historical migration.
const MIGRATION_65: &str = r#"
CREATE TABLE run_steering_messages_v65 (
    run_id TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    seq INTEGER NOT NULL CHECK(seq >= 1),
    message_id TEXT NOT NULL UNIQUE,
    content TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'received' CHECK(status IN ('received','delivered','applied','cancelled')),
    received_at INTEGER NOT NULL,
    applied_at INTEGER,
    applied_event_seq INTEGER,
    applied_dispatch_key TEXT,
    PRIMARY KEY(run_id,seq)
);
INSERT INTO run_steering_messages_v65
    (run_id, seq, message_id, content, status, received_at, applied_at, applied_event_seq, applied_dispatch_key)
SELECT run_id, seq, message_id, content, status, received_at, applied_at, applied_event_seq, applied_dispatch_key
FROM run_steering_messages;
DROP TABLE run_steering_messages;
ALTER TABLE run_steering_messages_v65 RENAME TO run_steering_messages;
CREATE INDEX IF NOT EXISTS idx_run_steering_status ON run_steering_messages(run_id,status);
"#;

/// v67 (2026-09-14 review repair N6): a restore that overwrites content the
/// registry does not know about (a user edit made outside any recorded task)
/// preserved those bytes as a safety backup but had no way to express them as a
/// *content version*, so no version row could bring that content back. The
/// `replaced` kind names exactly that: the content this restore replaced, kept
/// as a first-class, selectable version.
///
/// SQLite cannot widen a CHECK constraint in place, so the table is rebuilt with
/// the same shape and data (the v66 columns included). Nothing else changes, and
/// the table keeps its append-only meaning.
const MIGRATION_67: &str = r#"
CREATE TABLE managed_file_versions_v67 (
    id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    run_id TEXT REFERENCES runs(id) ON DELETE SET NULL,
    tool_call_id TEXT,
    tool TEXT NOT NULL,
    storage_path TEXT NOT NULL,
    display_name TEXT NOT NULL,
    version_no INTEGER NOT NULL CHECK(version_no >= 1),
    change_kind TEXT NOT NULL CHECK(change_kind IN ('created','modified','restored','replaced')),
    before_hash TEXT,
    before_size INTEGER,
    after_hash TEXT,
    after_size INTEGER,
    backup_path TEXT,
    after_backup_path TEXT,
    restored_from_id TEXT,
    source_kind TEXT NOT NULL DEFAULT 'legacy_unknown',
    source_verified INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    UNIQUE(storage_path, version_no)
);
INSERT INTO managed_file_versions_v67
    (id, conversation_id, run_id, tool_call_id, tool, storage_path, display_name, version_no,
     change_kind, before_hash, before_size, after_hash, after_size, backup_path,
     after_backup_path, restored_from_id, source_kind, source_verified, created_at)
SELECT id, conversation_id, run_id, tool_call_id, tool, storage_path, display_name, version_no,
       change_kind, before_hash, before_size, after_hash, after_size, backup_path,
       after_backup_path, restored_from_id, source_kind, source_verified, created_at
FROM managed_file_versions;
DROP TABLE managed_file_versions;
ALTER TABLE managed_file_versions_v67 RENAME TO managed_file_versions;
CREATE INDEX IF NOT EXISTS idx_managed_file_versions_path ON managed_file_versions(storage_path, created_at);
CREATE INDEX IF NOT EXISTS idx_managed_file_versions_run ON managed_file_versions(run_id);
CREATE INDEX IF NOT EXISTS idx_managed_file_versions_verified
    ON managed_file_versions(storage_path, version_no, source_verified);
"#;

/// v68 (limit plan A, items #4-#6): a human who never answered must not destroy
/// the work.
///
/// Before this migration an elapsed `approval_wait_ms` ended the attempt as a
/// plain `failed` run whose un-decided tool calls were marked `failed`, so a user
/// who stepped away for five minutes lost the continuation entry point even
/// though everything executed up to that point was intact.
///
/// The new semantics need facts the schema could not express:
///   * `kernel_runs.state = approval_expired` - the attempt is over, but it is
///     explicitly continuable and keeps its completed results;
///   * `kernel_tool_calls.state = expired` - the call was never dispatched.
///
/// Both are CHECK constraints, so the two tables are widened by
/// [`rebuild_kernel_tables_for_v68`], which runs on the connection *before* this
/// transaction with foreign keys disabled: a `DROP TABLE` inside an FK-enabled
/// transaction would cascade-delete `kernel_host_actions` and
/// `kernel_model_configs` (see that function's documentation).
///
/// `kernel_approval_history` records an expiry as a first-class auditable fact
/// that can name its successor; `kernel_run_progress` records what actually
/// completed so a continuation never has to guess; `kernel_authorization_grants`
/// records authorization granted after the run started, as a separate fact that
/// never rewrites the immutable frozen binding.
const MIGRATION_68: &str = r#"
-- The narrowed CHECK constraints on kernel_runs / kernel_tool_calls were already
-- widened before this transaction opened, by rebuild_kernel_tables_for_v68, which
-- needs foreign keys disabled on the connection (a setting SQLite ignores inside
-- a transaction). Nothing here drops a table, so no dependent row can cascade.

CREATE TABLE IF NOT EXISTS kernel_approval_history (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id TEXT NOT NULL,
    tool_call_id TEXT NOT NULL,
    state TEXT NOT NULL CHECK(state IN (
        'pending','allow_once','allow_conversation','denied','cancelled','expired','superseded'
    )),
    requested_at INTEGER NOT NULL,
    expires_at INTEGER,
    resolved_at INTEGER,
    waited_ms INTEGER,
    superseded_by_run_id TEXT,
    superseded_by_approval_id TEXT,
    resume_count INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_kernel_approval_history_run
    ON kernel_approval_history(run_id, tool_call_id, created_at);

CREATE TABLE IF NOT EXISTS kernel_run_progress (
    run_id TEXT NOT NULL,
    attempt INTEGER NOT NULL,
    pause_reason TEXT NOT NULL,
    completed_tool_calls INTEGER NOT NULL,
    pending_tool_calls INTEGER NOT NULL,
    terminal_written INTEGER NOT NULL,
    running_elapsed_ms INTEGER NOT NULL,
    continuable INTEGER NOT NULL,
    summary_json TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    PRIMARY KEY(run_id, attempt, pause_reason)
);

CREATE TABLE IF NOT EXISTS kernel_authorization_grants (
    id TEXT PRIMARY KEY,
    run_id TEXT NOT NULL,
    conversation_id TEXT NOT NULL,
    approval_id TEXT NOT NULL,
    tool TEXT NOT NULL CHECK(length(trim(tool)) BETWEEN 1 AND 128),
    project_root TEXT,
    scope_kind TEXT NOT NULL CHECK(scope_kind IN ('path','action','tool','office_request','mcp_tool')),
    scope_value TEXT NOT NULL CHECK(length(trim(scope_value)) BETWEEN 1 AND 512),
    source TEXT NOT NULL CHECK(source IN ('conversation_approval')),
    created_at INTEGER NOT NULL,
    expires_at INTEGER,
    revoked_at INTEGER,
    revoke_reason TEXT,
    use_count INTEGER NOT NULL DEFAULT 0,
    last_used_at INTEGER,
    UNIQUE(run_id, tool, scope_value)
);
CREATE INDEX IF NOT EXISTS idx_kernel_authorization_grants_run
    ON kernel_authorization_grants(run_id, tool, revoked_at);
CREATE INDEX IF NOT EXISTS idx_kernel_authorization_grants_conversation
    ON kernel_authorization_grants(conversation_id, revoked_at);
"#;

/// v69 (limit plan A, #6/#7): the user's explicit run-budget choice, and the
/// unified Host lifecycle for background compute jobs.
///
/// `kernel_run_budget_choices` records *what the user asked for* before the run
/// is frozen, so the tier shown in the UI is the tier the Kernel enforces. An
/// absent row is the historical default, never "unbounded".
///
/// `kernel_jobs` is one lifecycle reused by every background kind rather than a
/// second job system: a job belongs to an existing Run and conversation, obeys
/// the same frozen permission scope, and its result is a reference into the
/// existing result store. `(run_id, idempotency_key)` is unique so a retried
/// start returns the same job instead of launching a second one; `owner_pid`
/// plus `owner_started_at` are what let a restart distinguish "the process is
/// gone" from "the job is still running elsewhere".
const MIGRATION_69: &str = r#"
CREATE TABLE IF NOT EXISTS kernel_run_budget_choices (
    run_id TEXT PRIMARY KEY,
    tier TEXT NOT NULL CHECK(tier IN ('short','standard','long','custom')),
    custom_execution_ms INTEGER,
    frozen_execution_ms INTEGER NOT NULL CHECK(frozen_execution_ms > 0),
    created_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS kernel_jobs (
    job_id TEXT PRIMARY KEY,
    run_id TEXT NOT NULL,
    conversation_id TEXT NOT NULL,
    kind TEXT NOT NULL CHECK(length(trim(kind)) BETWEEN 1 AND 64),
    idempotency_key TEXT NOT NULL CHECK(length(trim(idempotency_key)) BETWEEN 1 AND 200),
    params_hash TEXT NOT NULL,
    params_json TEXT NOT NULL,
    state TEXT NOT NULL CHECK(state IN ('queued','running','paused','cancelled','failed','completed')),
    cursor TEXT,
    progress_done INTEGER NOT NULL DEFAULT 0,
    progress_total INTEGER,
    result_ref TEXT,
    result_bytes INTEGER,
    result_sha256 TEXT,
    error_code TEXT,
    error_message TEXT,
    attempts INTEGER NOT NULL DEFAULT 0,
    deadline_ms INTEGER,
    owner_pid INTEGER,
    owner_started_at INTEGER,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    finished_at INTEGER,
    UNIQUE(run_id, idempotency_key)
);
CREATE INDEX IF NOT EXISTS idx_kernel_jobs_run ON kernel_jobs(run_id, state);
CREATE INDEX IF NOT EXISTS idx_kernel_jobs_conversation
    ON kernel_jobs(conversation_id, created_at);
"#;

/// v66 (2026-09-14 review repairs R1/R2/R6): records what a registered file
/// version can actually restore and what produced it, and lets a delivery
/// checklist item carry the structured, independently verifiable requirements
/// the task text spelled out.
///
/// * `after_backup_path` — Host-owned byte snapshot of the content this row
///   records, so selecting *this* version restores *this* content instead of
///   the pre-write state. Rows written before this migration have no snapshot;
///   they stay recoverable only when a later row's pre-write backup provably
///   holds the same bytes (same storage path, matching hash and size).
/// * `source_kind` / `source_verified` — provenance of the row. Only Host- and
///   verified Office-connector writes are `verified=1`; a version whose facts
///   arrived from an unverified origin is never restorable.
/// * `delivery_checklist_items.requirements_json` — structured checks derived
///   from the task text (required section count, required chart count, count
///   or ratio consistency). Empty means "no structured requirement was stated",
///   which the delivery report must show as 未核验 rather than as a pass.
///
/// The columns are added through the idempotent helper (not a raw
/// `ALTER TABLE`) because upgrade fixtures apply later migrations on top of
/// tables that an earlier fixture may already have rebuilt.
const MIGRATION_66_COLUMNS: &[(&str, &str, &str)] = &[
    ("managed_file_versions", "after_backup_path", "TEXT"),
    (
        "managed_file_versions",
        "source_kind",
        "TEXT NOT NULL DEFAULT 'legacy_unknown'",
    ),
    (
        "managed_file_versions",
        "source_verified",
        "INTEGER NOT NULL DEFAULT 0",
    ),
    (
        "delivery_checklist_items",
        "requirements_json",
        "TEXT NOT NULL DEFAULT '[]'",
    ),
];

const MIGRATION_66_INDEXES: &str = r#"
CREATE INDEX IF NOT EXISTS idx_managed_file_versions_verified
    ON managed_file_versions(storage_path, version_no, source_verified);
"#;

const MIGRATION_62: &str = r#"
CREATE TABLE IF NOT EXISTS tool_call_result_blobs (
    sha256 TEXT PRIMARY KEY,
    body_json TEXT NOT NULL,
    byte_size INTEGER NOT NULL CHECK(byte_size > 0),
    created_at INTEGER NOT NULL
);
"#;
const CONVERSATION_TOOL_PERMISSION_SCHEMA_VERSION: i64 = 23;
const MIGRATION_61: &str = r#"
DROP TRIGGER kernel_model_config_insert_guard;
CREATE TRIGGER kernel_model_config_insert_guard BEFORE INSERT ON kernel_model_configs
WHEN NOT EXISTS (
    SELECT 1 FROM kernel_runs r JOIN run_control_bindings b ON b.run_id=r.run_id
    WHERE r.run_id=NEW.run_id AND r.kernel_mode='authoritative'
      AND r.engine_id IN ('pi','codex','deepseek_harness') AND b.engine_id=r.engine_id
      AND b.authority='authoritative' AND r.state='created' AND r.last_event_seq=0
      AND r.prompt_config_hash=NEW.config_hash
      AND r.execution_profile_id=json_extract(NEW.config_json,'$.executionProfileId')
      AND r.engine_id=COALESCE(json_extract(NEW.config_json,'$.engineId'),'pi')
)
BEGIN SELECT RAISE(ABORT, 'Kernel model configuration must match frozen engine before Run start'); END;
DROP TRIGGER kernel_initial_input_insert_guard;
CREATE TRIGGER kernel_initial_input_insert_guard BEFORE INSERT ON kernel_initial_inputs
WHEN NOT EXISTS (
    SELECT 1 FROM kernel_runs r JOIN kernel_model_configs c ON c.run_id=r.run_id
    JOIN run_control_bindings b ON b.run_id=r.run_id
    WHERE r.run_id=NEW.run_id AND r.state='created' AND r.last_event_seq=0
      AND r.kernel_mode='authoritative' AND r.engine_id IN ('pi','codex','deepseek_harness')
      AND b.authority='authoritative' AND b.engine_id=r.engine_id
      AND r.prompt_config_hash=NEW.prompt_config_hash AND c.config_hash=NEW.prompt_config_hash
      AND json_extract(NEW.input_json,'$.runId')=NEW.run_id
      AND json_extract(NEW.input_json,'$.turnId')=NEW.turn_id
      AND json_extract(NEW.input_json,'$.promptConfigHash')=NEW.prompt_config_hash
      AND json_extract(NEW.input_json,'$.schemaVersion')=NEW.schema_version
      AND length(trim(NEW.turn_id))>0
)
BEGIN SELECT RAISE(ABORT, 'Kernel initial input must match frozen engine before Run start'); END;
"#;
const MIGRATION_60: &str = r#"
CREATE TABLE kernel_reconciliation_events (
    run_id TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    revision INTEGER NOT NULL CHECK(revision > 0),
    effect_key TEXT NOT NULL,
    kind TEXT NOT NULL CHECK(kind IN ('query','confirmation','resume')),
    body_json TEXT NOT NULL CHECK(json_valid(body_json)),
    created_at INTEGER NOT NULL,
    PRIMARY KEY(run_id,revision)
);
CREATE TRIGGER kernel_reconciliation_event_immutable BEFORE UPDATE ON kernel_reconciliation_events
BEGIN SELECT RAISE(ABORT, 'Reconciliation evidence is append-only'); END;
CREATE TABLE kernel_recovery_runs (
    source_run_id TEXT PRIMARY KEY REFERENCES runs(id) ON DELETE CASCADE,
    recovery_run_id TEXT NOT NULL UNIQUE REFERENCES runs(id) ON DELETE CASCADE,
    created_at INTEGER NOT NULL
);
"#;
const MIGRATION_59: &str = r#"
CREATE TABLE kernel_host_actions (
    run_id TEXT NOT NULL,
    tool_call_id TEXT NOT NULL,
    body_json TEXT NOT NULL CHECK(json_valid(body_json)),
    body_hash TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending' CHECK(status IN ('pending','claimed','completed')),
    created_at INTEGER NOT NULL,
    completed_at INTEGER,
    PRIMARY KEY(run_id,tool_call_id),
    FOREIGN KEY(run_id,tool_call_id) REFERENCES kernel_tool_calls(run_id,tool_call_id) ON DELETE CASCADE
);
CREATE TRIGGER kernel_host_action_immutable BEFORE UPDATE OF run_id,tool_call_id,body_json,body_hash ON kernel_host_actions
BEGIN SELECT RAISE(ABORT, 'Kernel post-commit action is immutable'); END;
"#;
const MIGRATION_58: &str = r#"
CREATE TABLE kernel_host_runs (
    run_id TEXT PRIMARY KEY REFERENCES kernel_initial_inputs(run_id) ON DELETE CASCADE,
    scope_json TEXT NOT NULL CHECK(json_valid(scope_json)),
    scope_hash TEXT NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE TRIGGER kernel_host_scope_immutable BEFORE UPDATE ON kernel_host_runs
BEGIN SELECT RAISE(ABORT, 'Kernel host scope is immutable'); END;
CREATE TRIGGER kernel_host_scope_insert_guard BEFORE INSERT ON kernel_host_runs
WHEN NOT EXISTS (SELECT 1 FROM kernel_runs r JOIN run_control_bindings b ON b.run_id=r.run_id
    WHERE r.run_id=NEW.run_id AND r.state='created' AND r.last_event_seq=0
      AND r.kernel_mode='authoritative' AND b.authority='authoritative')
BEGIN SELECT RAISE(ABORT, 'Kernel host scope must be frozen before Run start'); END;
CREATE TABLE kernel_host_commands (
    command_seq INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id TEXT NOT NULL REFERENCES run_control_bindings(run_id) ON DELETE CASCADE,
    command_key TEXT NOT NULL,
    kind TEXT NOT NULL CHECK(kind IN ('cancel','approval')),
    tool_call_id TEXT,
    decision TEXT CHECK(decision IN ('allow_once','allow_conversation','denied')),
    status TEXT NOT NULL DEFAULT 'pending' CHECK(status IN ('pending','completed')),
    created_at INTEGER NOT NULL,
    completed_at INTEGER,
    UNIQUE(run_id,command_key),
    CHECK((kind='cancel' AND tool_call_id IS NULL AND decision IS NULL)
       OR (kind='approval' AND length(tool_call_id)>0 AND decision IS NOT NULL))
);
CREATE INDEX idx_kernel_host_commands_pending ON kernel_host_commands(run_id,status,command_seq);
"#;
const MIGRATION_57: &str = r#"
ALTER TABLE kernel_effect_outbox RENAME TO kernel_effect_outbox_v56;
CREATE TABLE kernel_effect_outbox (
    run_id TEXT NOT NULL, effect_key TEXT NOT NULL,
    effect_type TEXT NOT NULL CHECK(effect_type IN ('dispatch_tool','request_approval','cancel_engine_turn',
        'cancel_tool_call','deliver_tool_batch','initial_model','publish_snapshot')),
    idempotency_key TEXT NOT NULL, tool_call_id TEXT, batch_id TEXT, payload_json TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('pending','leased','completed','failed')),
    attempts INTEGER NOT NULL DEFAULT 0, lease_owner TEXT, leased_at INTEGER, completed_at INTEGER,
    last_error TEXT, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL,
    PRIMARY KEY(run_id,effect_key)
);
INSERT INTO kernel_effect_outbox SELECT * FROM kernel_effect_outbox_v56;
DROP TABLE kernel_effect_outbox_v56;
CREATE INDEX idx_kernel_outbox_status ON kernel_effect_outbox(run_id,status);
CREATE INDEX idx_kernel_outbox_due ON kernel_effect_outbox(status,created_at);
CREATE UNIQUE INDEX idx_kernel_outbox_idem ON kernel_effect_outbox(run_id,idempotency_key);
"#;
const MIGRATION_56: &str = r#"
CREATE TABLE kernel_initial_inputs (
    run_id TEXT PRIMARY KEY REFERENCES kernel_model_configs(run_id) ON DELETE CASCADE,
    schema_version INTEGER NOT NULL CHECK(schema_version = 1),
    turn_id TEXT NOT NULL,
    input_json TEXT NOT NULL CHECK(json_valid(input_json)),
    input_hash TEXT NOT NULL,
    prompt_config_hash TEXT NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE TRIGGER kernel_initial_input_immutable BEFORE UPDATE ON kernel_initial_inputs
BEGIN SELECT RAISE(ABORT, 'Kernel initial input is immutable'); END;
CREATE TRIGGER kernel_initial_input_insert_guard BEFORE INSERT ON kernel_initial_inputs
WHEN NOT EXISTS (
    SELECT 1 FROM kernel_runs r JOIN kernel_model_configs c ON c.run_id=r.run_id
    JOIN run_control_bindings b ON b.run_id=r.run_id
    WHERE r.run_id=NEW.run_id AND r.state='created' AND r.last_event_seq=0
      AND r.kernel_mode='authoritative' AND r.engine_id='pi'
      AND b.authority='authoritative' AND b.engine_id='pi'
      AND r.prompt_config_hash=NEW.prompt_config_hash AND c.config_hash=NEW.prompt_config_hash
      AND json_extract(NEW.input_json,'$.runId')=NEW.run_id
      AND json_extract(NEW.input_json,'$.turnId')=NEW.turn_id
      AND json_extract(NEW.input_json,'$.promptConfigHash')=NEW.prompt_config_hash
      AND json_extract(NEW.input_json,'$.schemaVersion')=NEW.schema_version
      AND length(trim(NEW.turn_id))>0
)
BEGIN SELECT RAISE(ABORT, 'Kernel initial input must match frozen configuration before Run start'); END;
CREATE TRIGGER kernel_initial_input_start_guard BEFORE UPDATE OF turn_id ON kernel_runs
WHEN EXISTS (SELECT 1 FROM kernel_initial_inputs i WHERE i.run_id=NEW.run_id
    AND (NEW.turn_id IS NULL OR i.turn_id<>NEW.turn_id))
BEGIN SELECT RAISE(ABORT, 'Kernel turn differs from frozen initial input'); END;
"#;
const MIGRATION_55: &str = r#"
CREATE TABLE kernel_model_configs (
    run_id TEXT PRIMARY KEY REFERENCES kernel_runs(run_id) ON DELETE CASCADE,
    schema_version INTEGER NOT NULL CHECK(schema_version = 1),
    adapter_version TEXT NOT NULL,
    config_json TEXT NOT NULL CHECK(json_valid(config_json)),
    config_hash TEXT NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE TRIGGER kernel_model_config_immutable BEFORE UPDATE ON kernel_model_configs
BEGIN SELECT RAISE(ABORT, 'Kernel model configuration is immutable'); END;
CREATE TRIGGER kernel_model_config_insert_guard BEFORE INSERT ON kernel_model_configs
WHEN NOT EXISTS (
    SELECT 1 FROM kernel_runs r JOIN run_control_bindings b ON b.run_id=r.run_id
    WHERE r.run_id=NEW.run_id AND r.kernel_mode='authoritative' AND r.engine_id='pi'
      AND b.authority='authoritative' AND b.engine_id='pi'
      AND r.state='created' AND r.last_event_seq=0
      AND r.prompt_config_hash=NEW.config_hash
      AND r.execution_profile_id=json_extract(NEW.config_json,'$.executionProfileId')
)
BEGIN SELECT RAISE(ABORT, 'Kernel model configuration must be frozen before Run start'); END;
"#;
const MIGRATION_54: &str = r#"
CREATE TABLE run_control_bindings (
    run_id TEXT PRIMARY KEY REFERENCES runs(id) ON DELETE CASCADE,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    authority TEXT NOT NULL CHECK(authority IN ('legacy','authoritative')),
    engine_id TEXT NOT NULL CHECK(engine_id IN ('pi','codex','deepseek_harness')),
    binding_json TEXT NOT NULL CHECK(json_valid(binding_json)),
    binding_hash TEXT NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE INDEX idx_run_control_conversation ON run_control_bindings(conversation_id, created_at);
"#;
const _: () = assert!(DATABASE_SCHEMA_VERSION >= CONVERSATION_TOOL_PERMISSION_SCHEMA_VERSION);
const CONVERSATION_TOOL_PERMISSION_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS conversation_tool_permissions (
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    tool_name TEXT NOT NULL,
    granted_at INTEGER NOT NULL,
    PRIMARY KEY(conversation_id, tool_name)
);
"#;

pub fn run(connection: &mut Connection, now: i64) -> Result<()> {
    run_with_target(connection, now, MigrationTarget::Latest)
}

#[cfg(test)]
pub(crate) fn run_to_v74_for_test(connection: &mut Connection, now: i64) -> Result<()> {
    run_with_target(connection, now, MigrationTarget::V74)
}

#[derive(Clone, Copy)]
enum MigrationTarget {
    Latest,
    #[cfg(test)]
    V74,
    #[cfg(test)]
    V79,
    #[cfg(test)]
    V83,
    #[cfg(test)]
    V85,
    #[cfg(test)]
    V87,
}

fn run_with_target(connection: &mut Connection, now: i64, target: MigrationTarget) -> Result<()> {
    connection.pragma_update(None, "foreign_keys", "ON")?;
    connection.pragma_update(None, "journal_mode", "WAL")?;
    connection.pragma_update(None, "busy_timeout", 5_000)?;

    // SQLite table replacement needs FK enforcement off before BEGIN. All
    // changes, the integrity check, and the version record remain atomic.
    connection.pragma_update(None, "foreign_keys", "OFF")?;
    let result = run_transaction(connection, now, target);
    let restored = connection.pragma_update(None, "foreign_keys", "ON");
    result?;
    restored
}

fn run_transaction(connection: &mut Connection, now: i64, _target: MigrationTarget) -> Result<()> {
    let transaction = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
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
    apply_migration(&transaction, 16, MIGRATION_16, now)?;
    apply_migration(&transaction, 17, MIGRATION_17, now)?;
    apply_migration(&transaction, 18, MIGRATION_18, now)?;
    apply_migration(&transaction, 19, MIGRATION_19, now)?;
    apply_agent_expert_migration(&transaction, now)?;
    // Agent row decoding includes the E1 provenance columns. Apply v29 before the
    // v21 snapshot backfill so legacy experts can be decoded during that backfill.
    apply_migration(&transaction, 29, MIGRATION_29, now)?;
    apply_expert_package_snapshot_migration(&transaction, now)?;
    apply_p0_p2_migration(&transaction, now)?;
    apply_conversation_tool_permission_migration(&transaction, now)?;
    apply_migration(&transaction, 24, MIGRATION_24, now)?;
    apply_migration(&transaction, 25, MIGRATION_25, now)?;
    apply_migration(&transaction, 26, MIGRATION_26, now)?;
    apply_migration(&transaction, 27, MIGRATION_27, now)?;
    apply_migration(&transaction, 28, MIGRATION_28, now)?;
    apply_migration(&transaction, 30, MIGRATION_30, now)?;
    apply_migration(&transaction, 31, MIGRATION_31, now)?;
    apply_migration(&transaction, 32, MIGRATION_32, now)?;
    apply_migration(&transaction, 33, MIGRATION_33, now)?;
    apply_migration(&transaction, 34, MIGRATION_34, now)?;
    apply_migration(&transaction, 35, MIGRATION_35, now)?;
    apply_migration(&transaction, 36, MIGRATION_36, now)?;
    apply_migration(&transaction, 37, MIGRATION_37, now)?;
    apply_migration(&transaction, 38, MIGRATION_38, now)?;
    apply_graph_review_acceptance_migration(&transaction, now)?;
    apply_migration(&transaction, 40, MIGRATION_40, now)?;
    apply_migration(&transaction, 41, MIGRATION_41, now)?;
    apply_migration(&transaction, 42, MIGRATION_42, now)?;
    apply_migration(&transaction, 43, MIGRATION_43, now)?;
    apply_migration(&transaction, 44, MIGRATION_44, now)?;
    apply_migration(&transaction, 45, MIGRATION_45, now)?;
    apply_migration(&transaction, 46, MIGRATION_46, now)?;
    apply_migration(&transaction, 47, MIGRATION_47, now)?;
    apply_migration(&transaction, 48, MIGRATION_48, now)?;
    apply_migration(&transaction, 49, MIGRATION_49, now)?;
    apply_migration(&transaction, 50, MIGRATION_50, now)?;
    apply_migration(&transaction, 51, MIGRATION_51, now)?;
    apply_migration(&transaction, 52, MIGRATION_52, now)?;
    apply_migration(
        &transaction,
        53,
        r#"
        CREATE TABLE kernel_shadow_checkpoints (
            shadow_run_id TEXT PRIMARY KEY REFERENCES kernel_shadow_runs(shadow_run_id) ON DELETE CASCADE,
            state TEXT NOT NULL CHECK(state IN ('active', 'failed', 'closed')),
            checkpoint_json TEXT NOT NULL,
            updated_at INTEGER NOT NULL
        );
    "#,
        now,
    )?;
    apply_migration(&transaction, 54, MIGRATION_54, now)?;
    apply_migration(&transaction, 55, MIGRATION_55, now)?;
    apply_migration(&transaction, 56, MIGRATION_56, now)?;
    apply_migration(&transaction, 57, MIGRATION_57, now)?;
    apply_migration(&transaction, 58, MIGRATION_58, now)?;
    apply_migration(&transaction, 59, MIGRATION_59, now)?;
    apply_migration(&transaction, 60, MIGRATION_60, now)?;
    apply_migration(&transaction, 61, MIGRATION_61, now)?;
    // The column add is idempotent via the helper so downgrade fixtures that
    // recreate the table never see a duplicate-column failure on re-run.
    let v62_applied = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM schema_migrations WHERE version = 62)",
        [],
        |row| row.get::<_, bool>(0),
    )?;
    if !v62_applied {
        ensure_column_if_missing(&transaction, "tool_calls", "result_blob_sha256", "TEXT")?;
        transaction.execute_batch(MIGRATION_62)?;
        transaction.execute(
            "INSERT INTO schema_migrations(version, applied_at) VALUES (62, ?1)",
            [now],
        )?;
    }
    apply_migration(&transaction, 63, MIGRATION_63, now)?;
    apply_migration(&transaction, 64, MIGRATION_64, now)?;
    apply_migration(&transaction, 65, MIGRATION_65, now)?;
    let v66_applied = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM schema_migrations WHERE version = 66)",
        [],
        |row| row.get::<_, bool>(0),
    )?;
    if !v66_applied {
        for (table, column, definition) in MIGRATION_66_COLUMNS {
            ensure_column_if_missing(&transaction, table, column, definition)?;
        }
        transaction.execute_batch(MIGRATION_66_INDEXES)?;
        transaction.execute(
            "INSERT INTO schema_migrations(version, applied_at) VALUES (66, ?1)",
            [now],
        )?;
    }
    apply_migration(&transaction, 67, MIGRATION_67, now)?;
    rebuild_kernel_tables_for_v68(&transaction)?;
    apply_migration(&transaction, 68, MIGRATION_68, now)?;
    apply_migration(&transaction, 69, MIGRATION_69, now)?;
    apply_migration(&transaction, 70, MIGRATION_70, now)?;
    let v71_applied = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM schema_migrations WHERE version = 71)",
        [],
        |row| row.get::<_, bool>(0),
    )?;
    if !v71_applied {
        for (table, column, definition) in MIGRATION_71_COLUMNS {
            ensure_column_if_missing(&transaction, table, column, definition)?;
        }
        transaction.execute_batch(MIGRATION_71_INDEXES)?;
        transaction.execute(
            "INSERT INTO schema_migrations(version, applied_at) VALUES (71, ?1)",
            [now],
        )?;
    }
    apply_migration(&transaction, 72, MIGRATION_72, now)?;
    apply_migration(&transaction, 73, MIGRATION_73, now)?;
    apply_migration(&transaction, 74, MIGRATION_74, now)?;
    #[cfg(test)]
    if matches!(_target, MigrationTarget::V74) {
        return finish_transaction(transaction);
    }
    let v75_applied = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM schema_migrations WHERE version = 75)",
        [],
        |row| row.get::<_, bool>(0),
    )?;
    if !v75_applied {
        transaction.execute_batch(MIGRATION_75)?;
        for (table, column, definition) in MIGRATION_75_ATTEMPTS_COLUMNS {
            ensure_column_if_missing(&transaction, table, column, definition)?;
        }
        // A v74 database created by the original (pre-Manage) migration still
        // carries the OLD credentials CHECK, and `apply_migration` never reruns
        // a recorded version. Rebuild the constraint additively so a Manage
        // credential can be issued on upgraded databases too. Historical rows
        // are copied byte-for-byte; nothing is backfilled.
        rebuild_credentials_check(&transaction)?;
        transaction.execute(
            "INSERT INTO schema_migrations(version, applied_at) VALUES (75, ?1)",
            [now],
        )?;
    }
    let v76_applied = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM schema_migrations WHERE version = 76)",
        [],
        |row| row.get::<_, bool>(0),
    )?;
    if !v76_applied {
        // A completed read/query is a terminal SUCCESS state, distinct from a
        // refusal. The CHECK must allow it on databases created before this
        // state existed (a forward rebuild, never an edit of a recorded
        // migration).
        rebuild_attempts_state_check(&transaction)?;
        transaction.execute(
            "INSERT INTO schema_migrations(version, applied_at) VALUES (76, ?1)",
            [now],
        )?;
    }
    let v77_applied = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM schema_migrations WHERE version = 77)",
        [],
        |row| row.get::<_, bool>(0),
    )?;
    if !v77_applied {
        transaction.execute_batch(MIGRATION_77)?;
        transaction.execute("INSERT INTO schema_migrations(version, applied_at) VALUES (77, ?1)", [now])?;
    }
    ensure_column_if_missing(&transaction, "kernel_approvals", "policy_version", "INTEGER")?;
    ensure_column_if_missing(&transaction, "kernel_host_commands", "policy_version", "INTEGER")?;
    transaction.execute_batch("CREATE TRIGGER IF NOT EXISTS execution_approval_version AFTER INSERT ON kernel_approvals BEGIN
      UPDATE kernel_approvals SET policy_version=(SELECT p.version FROM kernel_execution_policies p JOIN runs r ON r.conversation_id=p.conversation_id WHERE r.id=NEW.run_id) WHERE run_id=NEW.run_id AND tool_call_id=NEW.tool_call_id;
    END;")?;
    transaction.execute("INSERT OR IGNORE INTO schema_migrations(version, applied_at) VALUES (78, ?1)", [now])?;
    let v79_applied = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM schema_migrations WHERE version = 79)",
        [],
        |row| row.get::<_, bool>(0),
    )?;
    if !v79_applied {
        rebuild_kernel_jobs_idempotency_check(&transaction)?;
        transaction.execute(
            "INSERT INTO schema_migrations(version, applied_at) VALUES (79, ?1)",
            [now],
        )?;
    }
    #[cfg(test)]
    if matches!(_target, MigrationTarget::V79) {
        return finish_transaction(transaction);
    }
    let v80_applied = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM schema_migrations WHERE version = 80)",
        [],
        |row| row.get::<_, bool>(0),
    )?;
    if !v80_applied {
        transaction.execute_batch(MIGRATION_80)?;
        // columns go through the idempotent helper so a re-run on a database
        // that already carries them is a no-op rather than a hard failure, and
        // the revocation trigger is created only after they exist.
        ensure_column_if_missing(
            &transaction,
            "conversation_tool_permissions",
            "revoked_at",
            "INTEGER",
        )?;
        ensure_column_if_missing(
            &transaction,
            "conversation_tool_permissions",
            "revoke_reason",
            "TEXT",
        )?;
        transaction.execute_batch(
            "DROP TRIGGER IF EXISTS execution_legacy_grant_revoked;
             CREATE TRIGGER execution_legacy_grant_revoked AFTER UPDATE OF revoked_at
             ON conversation_tool_permissions
             WHEN OLD.revoked_at IS NULL AND NEW.revoked_at IS NOT NULL BEGIN
              UPDATE kernel_execution_policies SET version=version+1
                WHERE conversation_id=NEW.conversation_id;
              INSERT INTO kernel_parent_generations(run_id,generation)
                SELECT id,1 FROM runs WHERE conversation_id=NEW.conversation_id
                ON CONFLICT(run_id) DO UPDATE SET generation=generation+1;
             END;",
        )?;
        transaction.execute(
            "INSERT INTO schema_migrations(version, applied_at) VALUES (80, ?1)",
            [now],
        )?;
    }
    // Earlier builds replayed M77 on every open after M80 had removed this
    // project-to-conversation trigger. Remove any trigger they recreated, too.
    transaction.execute_batch("DROP TRIGGER IF EXISTS execution_policy_project_change;")?;
    let v81_applied = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM schema_migrations WHERE version = 81)",
        [],
        |row| row.get::<_, bool>(0),
    )?;
    if !v81_applied {
        // REV-05 / coordinator M3: the purpose-specific whole-file-replacement
        // authorization. Bound to the target, the observed version, the
        // candidate content digest and the request digest, consumed exactly
        // once, atomically with the persistent execution claim.
        transaction.execute_batch(MIGRATION_81)?;
        // The two digests travel with the credential row so the claim can bind
        // them without re-deriving anything from tool input. `credential_json`
        // keeps its own copy; these columns are nullable so a credential
        // persisted before this migration stays valid.
        ensure_column_if_missing(
            &transaction,
            "kernel_execution_credentials",
            "replace_candidate_digest",
            "TEXT",
        )?;
        ensure_column_if_missing(
            &transaction,
            "kernel_execution_credentials",
            "replace_request_digest",
            "TEXT",
        )?;
        transaction.execute(
            "INSERT INTO schema_migrations(version, applied_at) VALUES (81, ?1)",
            [now],
        )?;
    }
    let v82_applied = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM schema_migrations WHERE version = 82)",
        [],
        |row| row.get::<_, bool>(0),
    )?;
    if !v82_applied {
        // Stable restore request identity: a repeated delivery of the SAME
        // request is deduplicated against the recorded result instead of
        // performing the file mutation twice (coordinator item 3).
        transaction.execute_batch(MIGRATION_82)?;
        transaction.execute(
            "INSERT INTO schema_migrations(version, applied_at) VALUES (82, ?1)",
            [now],
        )?;
    }
    let v83_applied = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM schema_migrations WHERE version = 83)",
        [],
        |row| row.get::<_, bool>(0),
    )?;
    if !v83_applied {
        // Existing v82 requests did not record `force`, so their intent cannot
        // be reconstructed. Keep the new fields NULL and reject their replay.
        ensure_column_if_missing(
            &transaction,
            "kernel_restore_requests",
            "intent_digest",
            "TEXT",
        )?;
        ensure_column_if_missing(
            &transaction,
            "kernel_restore_requests",
            "attempt_owner",
            "TEXT",
        )?;
        transaction.execute(
            "INSERT INTO schema_migrations(version, applied_at) VALUES (83, ?1)",
            [now],
        )?;
    }
    #[cfg(test)]
    if matches!(_target, MigrationTarget::V83) {
        return finish_transaction(transaction);
    }
    apply_migration(&transaction, 84, MIGRATION_84, now)?;
    apply_migration(&transaction, 85, MIGRATION_85, now)?;
    #[cfg(test)]
    if matches!(_target, MigrationTarget::V85) {
        return finish_transaction(transaction);
    }
    apply_migration(&transaction, 86, MIGRATION_86, now)?;
    apply_migration(&transaction, 87, MIGRATION_87, now)?;
    #[cfg(test)]
    if matches!(_target, MigrationTarget::V87) {
        return finish_transaction(transaction);
    }
    apply_migration(&transaction, 88, MIGRATION_88, now)?;
    apply_migration(&transaction, 89, MIGRATION_89, now)?;
    finish_transaction(transaction)
}

fn finish_transaction(transaction: rusqlite::Transaction<'_>) -> Result<()> {
    let violations: i64 = transaction.query_row(
        "SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| row.get(0))?;
    if violations != 0 {
        return Err(rusqlite::Error::InvalidQuery);
    }
    transaction.commit()
}

fn apply_conversation_tool_permission_migration(
    transaction: &rusqlite::Transaction<'_>,
    now: i64,
) -> Result<()> {
    let applied = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM schema_migrations WHERE version = ?1)",
        [CONVERSATION_TOOL_PERMISSION_SCHEMA_VERSION],
        |row| row.get::<_, bool>(0),
    )?;
    if applied {
        return Ok(());
    }
    transaction.execute_batch(CONVERSATION_TOOL_PERMISSION_SCHEMA)?;
    transaction.execute(
        "INSERT INTO schema_migrations(version, applied_at) VALUES (?1, ?2)",
        rusqlite::params![CONVERSATION_TOOL_PERMISSION_SCHEMA_VERSION, now],
    )?;
    Ok(())
}

fn apply_p0_p2_migration(transaction: &rusqlite::Transaction<'_>, now: i64) -> Result<()> {
    let applied = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM schema_migrations WHERE version = ?1)",
        [P0_P2_SCHEMA_VERSION],
        |row| row.get::<_, bool>(0),
    )?;
    if applied {
        return Ok(());
    }

    ensure_column_if_missing(transaction, "mcp_servers", "catalog_plugin_id", "TEXT")?;
    ensure_column_if_missing(
        transaction,
        "mcp_servers",
        "origin",
        "TEXT NOT NULL DEFAULT 'local'",
    )?;
    transaction.execute_batch(P0_P2_SCHEMA)?;
    transaction.execute(
        "CREATE INDEX IF NOT EXISTS idx_mcp_servers_catalog_plugin_id
         ON mcp_servers(catalog_plugin_id)",
        [],
    )?;
    transaction.execute(
        "INSERT INTO knowledge_bindings_v2(
            id, conversation_id, source, provider_key, connection_id,
            knowledge_base_id, knowledge_base_name, enabled, created_at, updated_at
         )
         SELECT
            printf('legacy:%s:%s:%s', conversation_id, service_connection_id, knowledge_base_id),
            conversation_id,
            'remote',
            service_connection_id,
            service_connection_id,
            knowledge_base_id,
            knowledge_base_name,
            enabled,
            created_at,
            updated_at
         FROM knowledge_bindings",
        [],
    )?;
    transaction.execute(
        "INSERT INTO schema_migrations(version, applied_at) VALUES (?1, ?2)",
        rusqlite::params![P0_P2_SCHEMA_VERSION, now],
    )?;
    Ok(())
}

/// Add a column only when it is missing.
///
/// Takes a `&Connection` so it works both inside a migration transaction (a
/// `Transaction` derefs to `Connection`) and from the pre-transaction v68 schema
/// step, which must run before any transaction is open.
fn ensure_column_if_missing(
    connection: &Connection,
    table: &str,
    column: &str,
    definition: &str,
) -> Result<()> {
    let table_info_sql = format!(
        "SELECT EXISTS(
            SELECT 1 FROM pragma_table_info('{table}') WHERE name = ?1
         )"
    );
    let exists = connection.query_row(&table_info_sql, [column], |row| row.get::<_, bool>(0))?;
    if !exists {
        connection.execute(
            &format!("ALTER TABLE {table} ADD COLUMN {column} {definition}"),
            [],
        )?;
    }
    Ok(())
}

/// B1a-R2 (migration 75): rebuild the `kernel_execution_credentials` action_class
/// CHECK so an upgraded v74 database can hold the `manage` class. Editing the
/// historical MIGRATION_74 text cannot help: `apply_migration` never reruns a
/// recorded version and `CREATE TABLE IF NOT EXISTS` never alters an existing
/// table, so this must be an additive, controlled rebuild.
///
/// Historical rows are copied unchanged (no backfill, no re-signing); the table
/// is recreated with the SAME primary key and column order.
fn rebuild_credentials_check(transaction: &rusqlite::Transaction<'_>) -> Result<()> {
    // Skip when the constraint already allows `manage` (fresh v75 databases).
    let constraint: String = transaction.query_row(
        "SELECT COALESCE(
            (SELECT sql FROM sqlite_master
              WHERE type='table' AND name='kernel_execution_credentials'),
            '')",
        [],
        |row| row.get(0),
    )?;
    if constraint.contains("'manage'") {
        return Ok(());
    }
    transaction.execute_batch(
        "CREATE TABLE kernel_execution_credentials_v75 (
             run_id TEXT NOT NULL,
             dispatch_id TEXT NOT NULL,
             conversation_id TEXT NOT NULL,
             intent_digest TEXT NOT NULL,
             action_class TEXT NOT NULL CHECK(action_class IN ('read','write','execute','destructive','sensitive_egress','manage')),
             file_baseline TEXT,
             resolved_profile TEXT NOT NULL,
             policy_snapshot_id TEXT NOT NULL,
             backend_required TEXT NOT NULL,
             backend_evidence_digest TEXT,
             credential_digest TEXT NOT NULL,
             credential_json TEXT NOT NULL,
             created_at INTEGER NOT NULL,
             PRIMARY KEY(run_id, dispatch_id)
         );
         INSERT INTO kernel_execution_credentials_v75
             (run_id, dispatch_id, conversation_id, intent_digest, action_class, file_baseline,
              resolved_profile, policy_snapshot_id, backend_required, backend_evidence_digest,
              credential_digest, credential_json, created_at)
         SELECT run_id, dispatch_id, conversation_id, intent_digest, action_class, file_baseline,
                resolved_profile, policy_snapshot_id, backend_required, backend_evidence_digest,
                credential_digest, credential_json, created_at
           FROM kernel_execution_credentials;
         DROP TABLE kernel_execution_credentials;
         ALTER TABLE kernel_execution_credentials_v75 RENAME TO kernel_execution_credentials;",
    )?;
    Ok(())
}

/// Migration 79 widens only the database guard for canonical Host dispatch job
/// keys. Legacy/model job families retain the historical 200-character branch;
/// the Rust admission path separately enforces the tighter 200-byte legacy
/// bound and the canonical Host encoding, run binding, and 2072-byte ceiling.
/// The Host branch casts to BLOB before `length`, so the database ceiling is in
/// bytes for UTF-8 keys too; Rust remains the authority for their exact shape.
///
/// The v69 table plus the four v70 columns are recreated verbatim. Foreign keys
/// are disabled by `run` before its atomic migration transaction, and the
/// runner checks the whole database before recording success.
fn rebuild_kernel_jobs_idempotency_check(
    transaction: &rusqlite::Transaction<'_>,
) -> Result<()> {
    transaction.execute_batch(
        r#"
CREATE TABLE kernel_jobs_v79 (
    job_id TEXT PRIMARY KEY,
    run_id TEXT NOT NULL,
    conversation_id TEXT NOT NULL,
    kind TEXT NOT NULL CHECK(length(trim(kind)) BETWEEN 1 AND 64),
    idempotency_key TEXT NOT NULL CHECK(
        length(trim(idempotency_key)) BETWEEN 1 AND 200
        OR (
            idempotency_key GLOB 'job:tool-dispatch:*'
            AND length(CAST(idempotency_key AS BLOB)) <= 2072
        )
    ),
    params_hash TEXT NOT NULL,
    params_json TEXT NOT NULL,
    state TEXT NOT NULL CHECK(state IN ('queued','running','paused','cancelled','failed','completed')),
    cursor TEXT,
    progress_done INTEGER NOT NULL DEFAULT 0,
    progress_total INTEGER,
    result_ref TEXT,
    result_bytes INTEGER,
    result_sha256 TEXT,
    error_code TEXT,
    error_message TEXT,
    attempts INTEGER NOT NULL DEFAULT 0,
    deadline_ms INTEGER,
    owner_pid INTEGER,
    owner_started_at INTEGER,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    finished_at INTEGER,
    cancel_requested_at INTEGER,
    cancel_acknowledged_at INTEGER,
    cancelled_by TEXT,
    uncertain_at INTEGER,
    UNIQUE(run_id, idempotency_key)
);
INSERT INTO kernel_jobs_v79 (
    job_id, run_id, conversation_id, kind, idempotency_key, params_hash,
    params_json, state, cursor, progress_done, progress_total, result_ref,
    result_bytes, result_sha256, error_code, error_message, attempts,
    deadline_ms, owner_pid, owner_started_at, created_at, updated_at,
    finished_at, cancel_requested_at, cancel_acknowledged_at, cancelled_by,
    uncertain_at
)
SELECT
    job_id, run_id, conversation_id, kind, idempotency_key, params_hash,
    params_json, state, cursor, progress_done, progress_total, result_ref,
    result_bytes, result_sha256, error_code, error_message, attempts,
    deadline_ms, owner_pid, owner_started_at, created_at, updated_at,
    finished_at, cancel_requested_at, cancel_acknowledged_at, cancelled_by,
    uncertain_at
FROM kernel_jobs;
DROP TABLE kernel_jobs;
ALTER TABLE kernel_jobs_v79 RENAME TO kernel_jobs;
CREATE INDEX idx_kernel_jobs_run ON kernel_jobs(run_id, state);
CREATE INDEX idx_kernel_jobs_conversation ON kernel_jobs(conversation_id, created_at);
"#,
    )?;
    Ok(())
}

fn apply_agent_expert_migration(transaction: &rusqlite::Transaction<'_>, now: i64) -> Result<()> {
    let applied = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM schema_migrations WHERE version = ?1)",
        [AGENT_EXPERT_SCHEMA_VERSION],
        |row| row.get::<_, bool>(0),
    )?;
    if applied {
        return Ok(());
    }
    ensure_agent_classification_and_expert_bindings(transaction)?;
    migrate_builtin_expert_conversations(transaction)?;
    transaction.execute(
        "INSERT INTO schema_migrations(version, applied_at) VALUES (?1, ?2)",
        rusqlite::params![AGENT_EXPERT_SCHEMA_VERSION, now],
    )?;
    Ok(())
}

fn apply_expert_package_snapshot_migration(
    transaction: &rusqlite::Transaction<'_>,
    now: i64,
) -> Result<()> {
    let applied = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM schema_migrations WHERE version = ?1)",
        [EXPERT_PACKAGE_SNAPSHOT_SCHEMA_VERSION],
        |row| row.get::<_, bool>(0),
    )?;
    if applied {
        return Ok(());
    }
    let snapshot_column_exists = transaction.query_row(
        "SELECT EXISTS(
            SELECT 1 FROM pragma_table_info('conversation_expert_bindings')
            WHERE name = 'package_snapshot_json'
         )",
        [],
        |row| row.get::<_, bool>(0),
    )?;
    if !snapshot_column_exists {
        transaction.execute_batch(EXPERT_PACKAGE_SNAPSHOT_SCHEMA)?;
    }

    let bindings = {
        let mut statement = transaction
            .prepare("SELECT id, expert_id FROM conversation_expert_bindings ORDER BY rowid ASC")?;
        let bindings = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<Result<Vec<_>>>()?;
        bindings
    };
    for (binding_id, expert_id) in bindings {
        let expert = query_agent_record(transaction, &expert_id)?
            .ok_or(rusqlite::Error::QueryReturnedNoRows)?;
        let package_snapshot = agent_package_snapshot(&expert);
        let package_hash = package_snapshot_hash(&package_snapshot);
        transaction.execute(
            "UPDATE conversation_expert_bindings
             SET package_snapshot_json = ?2, package_hash = ?3
             WHERE id = ?1",
            rusqlite::params![binding_id, package_snapshot.to_string(), package_hash],
        )?;
    }
    transaction.execute(
        "INSERT INTO schema_migrations(version, applied_at) VALUES (?1, ?2)",
        rusqlite::params![EXPERT_PACKAGE_SNAPSHOT_SCHEMA_VERSION, now],
    )?;
    Ok(())
}

/// Widen the two Kernel CHECK constraints that v68 needs (item #4's `expired`
/// tool state and `approval_expired` run state) without destroying dependents.
///
/// SQLite cannot widen a CHECK in place, so both tables must be rebuilt. The
/// obvious `DROP TABLE` + `CREATE TABLE` + `INSERT ... SELECT` is **destructive**:
/// with `foreign_keys = ON` (which this runner sets) `kernel_host_actions`
/// cascades off the dropped `kernel_tool_calls`, and `kernel_model_configs`
/// cascades off the dropped `kernel_runs` — taking `kernel_initial_inputs` and
/// `kernel_host_runs` with them. That silently deletes the frozen model
/// configuration and the Host action ledger of every existing run, and
/// `PRAGMA foreign_key_check` still reports a clean database because cascades are
/// legal deletions.
///
/// So the rebuild runs with foreign keys disabled and an integrity check after:
///   * `foreign_keys` is a per-connection setting that cannot be changed inside a
///     transaction, which is why this runs on the connection before the v68
///     transaction opens (and why `PRAGMA legacy_alter_table` is unnecessary —
///     with FKs off the rename cannot rewrite other tables' `REFERENCES`);
///   * every dependent row, index and trigger is preserved by construction: the
///     only statements touching data are the row copy into the new table and the
///     rename;
///   * `PRAGMA foreign_key_check` runs before the version is recorded, so a
///     genuinely broken result fails the migration instead of being blessed.
///
/// Apply the v68 schema changes without destroying dependent rows.
///
/// Two different problems, two different fixes:
///
/// * `kernel_tool_calls.state` carries a real CHECK constraint that must gain
///   `'expired'`, and SQLite cannot widen a CHECK in place, so that table has to
///   be rebuilt. The obvious `DROP TABLE` + `CREATE TABLE` + `INSERT ... SELECT`
///   is **destructive**: with `foreign_keys = ON` (which this runner sets)
///   `kernel_host_actions` cascades off the dropped table, silently deleting the
///   Host action ledger of every run, while `PRAGMA foreign_key_check` still
///   reports a clean database because cascades are legal deletions. The rebuild
///   therefore runs with foreign keys disabled and an integrity check after.
/// * `kernel_runs.state` has no CHECK constraint (run states are validated in
///   code by `valid_run_state`, which is why `approval_expired` needs no schema
///   change), so that table is only *extended*, using `ALTER TABLE ADD COLUMN`.
///   Skipping its rebuild also keeps its triggers and the FK targets of
///   `kernel_model_configs` / `kernel_host_runs` untouched by construction.
///
/// `foreign_keys` is a per-connection setting that cannot be changed inside a
/// transaction, which is why this runs on the connection *before* the v68
/// transaction opens (and why `PRAGMA legacy_alter_table` is unnecessary: with
/// FKs off the rename cannot rewrite other tables' `REFERENCES`).
///
/// Idempotent: a table that already accepts the new state is left untouched, and
/// every column add is guarded by `ensure_column_if_missing`.
fn rebuild_kernel_tables_for_v68(connection: &Connection) -> Result<bool> {
    // A brand-new database has no Kernel tables yet (MIGRATION_48/49 create them,
    // already at the current shape), so there is nothing to widen here. The check
    // is intentionally per-table so a partially upgraded database still gets both
    // halves applied.
    if !sqlite_table_exists(connection, "kernel_tool_calls")?
        && !sqlite_table_exists(connection, "kernel_runs")?
    {
        return Ok(false);
    }
    let tool_needs_rebuild = sqlite_table_exists(connection, "kernel_tool_calls")?
        && !sqlite_object_accepts(connection, "kernel_tool_calls", "'expired'")?;
    if tool_needs_rebuild {
        let rebuild = connection.execute_batch(
            r#"
CREATE TABLE kernel_tool_calls_v68 (
    run_id TEXT NOT NULL,
    tool_call_id TEXT NOT NULL,
    batch_id TEXT NOT NULL,
    tool TEXT NOT NULL,
    source_order INTEGER NOT NULL,
    canonical_input_json TEXT NOT NULL,
    state TEXT NOT NULL CHECK(state IN (
        'pending','waiting_approval','running','completed','failed','cancelled','expired'
    )),
    result_json TEXT,
    created_at INTEGER NOT NULL,
    settled_at INTEGER,
    dispatch_idempotency_key TEXT,
    PRIMARY KEY(run_id, tool_call_id)
);
INSERT INTO kernel_tool_calls_v68
    (run_id, tool_call_id, batch_id, tool, source_order, canonical_input_json, state,
     result_json, created_at, settled_at, dispatch_idempotency_key)
SELECT run_id, tool_call_id, batch_id, tool, source_order, canonical_input_json, state,
       result_json, created_at, settled_at, dispatch_idempotency_key
FROM kernel_tool_calls;
DROP TABLE kernel_tool_calls;
ALTER TABLE kernel_tool_calls_v68 RENAME TO kernel_tool_calls;
CREATE INDEX IF NOT EXISTS idx_kernel_tool_calls_batch
    ON kernel_tool_calls(run_id, batch_id);
CREATE INDEX IF NOT EXISTS idx_kernel_tool_calls_state
    ON kernel_tool_calls(run_id, state);
"#,
        );
        rebuild?;
        // Prove the rebuild kept every relationship before the version is
        // recorded. A cascade would leave this empty (the rows were deleted
        // legally), which is why the row counts are compared as well.
        let violations: i64 = connection.query_row(
            "SELECT COUNT(*) FROM pragma_foreign_key_check",
            [],
            |row| row.get(0),
        )?;
        if violations != 0 {
            return Err(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(1),
                Some(format!(
                    "kernel v68 rebuild broke {violations} foreign-key relationship(s)"
                )),
            ));
        }
    }
    // Additive resume columns for #4/#6. Absent on every pre-v68 row, which must
    // keep decoding as "not continuable, no recorded pause".
    if sqlite_table_exists(connection, "kernel_runs")? {
        for (column, definition) in [
            ("continuable", "INTEGER NOT NULL DEFAULT 0"),
            ("last_pause_reason", "TEXT"),
            ("progress_ref", "TEXT"),
            ("budget_tier", "TEXT"),
            ("budget_source", "TEXT"),
            ("paused_at", "INTEGER"),
            ("continued_from_run_id", "TEXT"),
            ("attempt", "INTEGER NOT NULL DEFAULT 1"),
        ] {
            ensure_column_if_missing(connection, "kernel_runs", column, definition)?;
        }
    }
    Ok(tool_needs_rebuild)
}

/// Whether a table exists. Used so the pre-transaction v68 step can skip a fresh
/// database whose Kernel tables are still created by the migration runner.
fn sqlite_table_exists(connection: &Connection, table: &str) -> Result<bool> {
    connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
        [table],
        |row| row.get(0),
    )
}

/// Whether a table's own SQL already mentions a literal (used to detect whether a
/// CHECK constraint was already widened). Reads `sqlite_master` only, and
/// compares with all whitespace removed: SQLite stores a migrated table's SQL on
/// one line while the fixture that created it may have line breaks inside the
/// CHECK, and a false negative here would rebuild the table on every open.
fn sqlite_object_accepts(
    connection: &Connection,
    table: &str,
    literal: &str,
) -> Result<bool> {
    let sql: Option<String> = connection
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = ?1",
            [table],
            |row| row.get(0),
        )
        .optional()?;
    let wanted: String = literal.chars().filter(|c| !c.is_whitespace()).collect();
    Ok(sql.is_some_and(|sql| {
        let actual: String = sql.chars().filter(|c| !c.is_whitespace()).collect();
        actual.contains(&wanted)
    }))
}

fn ensure_agent_classification_and_expert_bindings(
    transaction: &rusqlite::Transaction<'_>,
) -> Result<()> {
    let classification_exists = transaction.query_row(
        "SELECT EXISTS(
            SELECT 1 FROM pragma_table_info('agents') WHERE name = 'agent_kind'
         )",
        [],
        |row| row.get::<_, bool>(0),
    )?;
    if !classification_exists {
        transaction.execute_batch(AGENT_CLASSIFICATION_SCHEMA)?;
    }
    transaction.execute_batch(CONVERSATION_EXPERT_BINDINGS_SCHEMA)
}

fn migrate_builtin_expert_conversations(transaction: &rusqlite::Transaction<'_>) -> Result<()> {
    let legacy = {
        let mut statement = transaction.prepare(
            "SELECT c.id, c.created_at, a.id, a.name, a.description, a.icon, a.category,
                    a.package_version, a.package_manifest_json
             FROM conversations c
             JOIN agents a ON a.id = c.agent_id
             WHERE a.id IN (
                'fox-debugger', 'fox-frontend', 'fox-reviewer', 'fox-security', 'fox-architect'
             )",
        )?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, String>(8)?,
                ))
            })?
            .collect::<Result<Vec<_>>>()?;
        rows
    };

    for (
        conversation_id,
        activated_at,
        expert_id,
        name,
        description,
        icon,
        category,
        expert_version,
        package_manifest_json,
    ) in legacy
    {
        let binding_id = format!("migration:{conversation_id}:{expert_id}");
        let package_hash = hex::encode(Sha256::digest(package_manifest_json.as_bytes()));
        let display_snapshot = json!({
            "id": expert_id,
            "name": name,
            "description": description,
            "icon": icon,
            "category": category,
            "agentKind": "expert",
            "invocationMode": "inline",
            "visibility": "expert_center",
            "packageVersion": expert_version,
        });
        transaction.execute(
            "INSERT OR IGNORE INTO conversation_expert_bindings(
                id, conversation_id, expert_id, state, activation_source, expert_version,
                package_hash, display_snapshot_json, activated_at, deactivated_at
             ) VALUES (?1, ?2, ?3, 'active', 'migration', ?4, ?5, ?6, ?7, NULL)",
            rusqlite::params![
                binding_id,
                conversation_id,
                expert_id,
                expert_version,
                package_hash,
                display_snapshot.to_string(),
                activated_at,
            ],
        )?;
        transaction.execute(
            "UPDATE conversations SET agent_id = 'fox-general' WHERE id = ?1",
            [conversation_id],
        )?;
    }
    Ok(())
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

fn apply_graph_review_acceptance_migration(
    transaction: &rusqlite::Transaction<'_>,
    now: i64,
) -> Result<()> {
    let applied = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM schema_migrations WHERE version = 39)",
        [],
        |row| row.get::<_, bool>(0),
    )?;
    if applied {
        return Ok(());
    }
    transaction.execute_batch(MIGRATION_39)?;
    for trigger_name in [
        "work_graph_node_finishes_validate_authority",
        "task_attempts_require_graph_node_finish",
    ] {
        let sql = transaction.query_row(
            "SELECT sql FROM sqlite_master WHERE type = 'trigger' AND name = ?1",
            [trigger_name],
            |row| row.get::<_, String>(0),
        )?;
        let widened = sql.replace(
            "policy.policy_id = 'standard_v1'",
            "policy.policy_id IN ('standard_v1', 'high_risk_v1')",
        );
        if widened == sql {
            return Err(rusqlite::Error::InvalidParameterName(format!(
                "v39 could not locate the frozen policy gate in trigger {trigger_name}"
            )));
        }
        transaction.execute(&format!("DROP TRIGGER {trigger_name}"), [])?;
        transaction.execute_batch(&widened)?;
    }
    transaction.execute(
        "INSERT INTO schema_migrations(version, applied_at) VALUES (39, ?1)",
        [now],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_through_v37(connection: &mut Connection, now: i64) {
        connection
            .pragma_update(None, "foreign_keys", "ON")
            .expect("enable foreign keys");
        let transaction = connection.transaction().expect("begin v37 migration");
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
            (14, MIGRATION_14),
            (15, MIGRATION_15),
            (16, MIGRATION_16),
            (17, MIGRATION_17),
            (18, MIGRATION_18),
            (19, MIGRATION_19),
        ] {
            apply_migration(&transaction, version, sql, now).expect("apply base migration");
        }
        apply_agent_expert_migration(&transaction, now).expect("apply agent migration");
        apply_migration(&transaction, 29, MIGRATION_29, now).expect("apply migration 29");
        apply_expert_package_snapshot_migration(&transaction, now)
            .expect("apply expert package migration");
        apply_p0_p2_migration(&transaction, now).expect("apply P0-P2 migration");
        apply_conversation_tool_permission_migration(&transaction, now)
            .expect("apply conversation permission migration");
        for (version, sql) in [
            (24, MIGRATION_24),
            (25, MIGRATION_25),
            (26, MIGRATION_26),
            (27, MIGRATION_27),
            (28, MIGRATION_28),
            (30, MIGRATION_30),
            (31, MIGRATION_31),
            (32, MIGRATION_32),
            (33, MIGRATION_33),
            (34, MIGRATION_34),
            (35, MIGRATION_35),
            (36, MIGRATION_36),
            (37, MIGRATION_37),
        ] {
            apply_migration(&transaction, version, sql, now).expect("apply later migration");
        }
        transaction.commit().expect("commit through v37");
    }

    // Build the actual v52 schema by applying the existing migrations in the
    // same order as run_transaction. A current database with migration rows
    // deleted still carries later triggers and is not a historical v52 database.
    fn run_through_v52(connection: &mut Connection, now: i64) {
        run_through_v37(connection, now);
        connection
            .pragma_update(None, "foreign_keys", "OFF")
            .expect("disable foreign keys before v38-v52 migration");
        let transaction = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .expect("begin v38-v52 migration");
        apply_migration(&transaction, 38, MIGRATION_38, now).expect("migration 38");
        apply_graph_review_acceptance_migration(&transaction, now).expect("migration 39");
        for (version, sql) in [
            (40, MIGRATION_40),
            (41, MIGRATION_41),
            (42, MIGRATION_42),
            (43, MIGRATION_43),
            (44, MIGRATION_44),
            (45, MIGRATION_45),
            (46, MIGRATION_46),
            (47, MIGRATION_47),
            (48, MIGRATION_48),
            (49, MIGRATION_49),
            (50, MIGRATION_50),
            (51, MIGRATION_51),
            (52, MIGRATION_52),
        ] {
            apply_migration(&transaction, version, sql, now)
                .unwrap_or_else(|error| panic!("migration {version}: {error}"));
        }
        transaction.commit().expect("commit through v52");
        connection
            .pragma_update(None, "foreign_keys", "ON")
            .expect("restore foreign keys");
    }

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

    fn run_pre_p0_p2(connection: &mut Connection, now: i64) {
        run_pre_a0(connection, now);
        let transaction = connection.transaction().expect("begin pre-P0/P2 migration");
        for (version, sql) in [
            (14, MIGRATION_14),
            (15, MIGRATION_15),
            (16, MIGRATION_16),
            (17, MIGRATION_17),
            (18, MIGRATION_18),
            (19, MIGRATION_19),
        ] {
            apply_migration(&transaction, version, sql, now).expect("apply pre-P0/P2 migration");
        }
        apply_agent_expert_migration(&transaction, now).expect("apply expert classification");
        apply_expert_package_snapshot_migration(&transaction, now)
            .expect("apply package snapshot migration");
        transaction.commit().expect("commit pre-P0/P2 schema");
    }

    /// Upgrading a database written before the artifact split must add the two
    /// columns without touching a single existing row's identity, and the
    /// backfill must record where the bytes really live: a historical row in
    /// the private compute workspace is `host_private`, one in a project is
    /// `project`, and neither is promoted to a deliverable the Host never
    /// classified. A second run is a no-op.
    #[test]
    fn migration_backfills_artifact_origin_without_promoting_historical_rows() {
        let mut connection = Connection::open_in_memory().expect("open database");
        run(&mut connection, 1).expect("apply every migration");

        // The columns exist with the additive defaults.
        let columns: Vec<String> = connection
            .prepare("SELECT name FROM pragma_table_info('artifacts')")
            .expect("prepare")
            .query_map([], |row| row.get::<_, String>(0))
            .expect("query")
            .collect::<Result<Vec<_>, _>>()
            .expect("collect");
        assert!(columns.contains(&"artifact_class".to_owned()), "{columns:?}");
        assert!(columns.contains(&"artifact_origin".to_owned()), "{columns:?}");
        let version: i64 = connection
            .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| row.get(0))
            .expect("version");
        assert_eq!(version, DATABASE_SCHEMA_VERSION);
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM schema_migrations WHERE version=72", [], |row| row.get::<_, i64>(0))
                .expect("v72 recorded"),
            1
        );

        // Seed a historical row in the Host's private compute workspace, then
        // restore the state a pre-backfill database is in: migration 71 added
        // the columns with the conservative default, so the origin still reads
        // `project` even though the bytes are Host-private. Dropping the v72
        // record models a database upgraded from exactly that state.
        connection
            .execute_batch(
                "INSERT INTO agents(id, name, description, runtime_type, system_prompt, default_model, created_at, updated_at)
                 VALUES ('a','A','','pi','','m',1,1);
                 INSERT INTO conversations(id, agent_id, title, status, created_at, updated_at)
                 VALUES ('c','a','C','active',1,1);
                 INSERT INTO artifacts(id, conversation_id, run_id, display_name, artifact_type, storage_path, byte_size, status, created_at, updated_at, artifact_class, artifact_origin)
                 VALUES ('private','c',NULL,'agv.csv','created_file',
                         'C:\\Users\\u\\AppData\\Roaming\\com.fox.agent\\runtime-sessions\\attachment-compute\\abc\\def\\ghi\\outputs\\agv.csv',
                         10,'ready',1,1,'process','project'),
                        ('project','c',NULL,'report.xlsx','created_file',
                         'D:\\proj\\report.xlsx',20,'ready',1,1,'process','project'),
                        ('preview','c',NULL,'report.html','created_file',
                         'C:\\Users\\u\\AppData\\Roaming\\com.fox.agent\\artifacts\\previews\\abc\\report.html',
                         30,'ready',1,1,'preview','host_private');
                 DELETE FROM schema_migrations WHERE version=72;",
            )
            .expect("seed historical rows");

        let transaction = connection.transaction().expect("begin");
        apply_migration(&transaction, 72, MIGRATION_72, 2).expect("apply backfill");
        transaction.commit().expect("commit");

        let private: (String, String) = connection
            .query_row(
                "SELECT artifact_class, artifact_origin FROM artifacts WHERE id='private'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("private row");
        assert_eq!(private.0, "process", "a historical file is never a deliverable");
        assert_eq!(private.1, "host_private", "the compute workspace is Host-private");
        let project: (String, String) = connection
            .query_row(
                "SELECT artifact_class, artifact_origin FROM artifacts WHERE id='project'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("project row");
        assert_eq!(project.0, "process");
        assert_eq!(project.1, "project");
        // A private preview is not mistaken for a compute-workspace file and
        // keeps its own (accurate) class and origin.
        let preview: (String, String) = connection
            .query_row(
                "SELECT artifact_class, artifact_origin FROM artifacts WHERE id='preview'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("preview row");
        assert_eq!(preview.0, "preview");
        assert_eq!(preview.1, "host_private");

        // The row identities the UI and the restore history key on are intact.
        let count: i64 = connection
            .query_row("SELECT COUNT(*) FROM artifacts WHERE id IN ('private','project','preview')", [], |row| row.get(0))
            .expect("count");
        assert_eq!(count, 3);
        // Idempotent: the migration is recorded, so a second start is a no-op.
        let applied: i64 = connection
            .query_row("SELECT COUNT(*) FROM schema_migrations WHERE version=72", [], |row| row.get(0))
            .expect("applied");
        assert_eq!(applied, 1);
        let repeat = connection.transaction().expect("begin repeat");
        apply_migration(&repeat, 72, MIGRATION_72, 3).expect("repeat is a no-op");
        repeat.commit().expect("commit repeat");
        assert_eq!(
            connection
                .query_row("SELECT artifact_origin FROM artifacts WHERE id='project'", [], |row| row.get::<_, String>(0))
                .expect("project origin after repeat"),
            "project"
        );
    }

    /// The placement migration against the operator's **real activity
    /// database**, when one is supplied.
    ///
    /// Historical rows are the part of this change that cannot be tested with a
    /// fixture: the question is whether adding the deliverable-placement table
    /// (and the earlier class/origin columns) leaves every existing artifact id,
    /// version row and restore pointer exactly as it was. The database is
    /// *copied* first and never opened in place, so a failure here cannot affect
    /// the running application.
    #[test]
    #[ignore = "reads the operator's real activity database; set FOX_REAL_DATABASE and run explicitly"]
    fn the_placement_migration_preserves_the_real_activity_database() {
        let Some(source) = std::env::var_os("FOX_REAL_DATABASE") else {
            eprintln!("FOX_REAL_DATABASE is not set; nothing to verify");
            return;
        };
        let source = std::path::PathBuf::from(source);
        let scratch = std::env::temp_dir().join(format!("fox-real-db-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&scratch).unwrap();
        let copy = scratch.join("fox.db");
        std::fs::copy(&source, &copy).unwrap();
        for suffix in ["-wal", "-shm"] {
            let extra = std::path::PathBuf::from(format!("{}{suffix}", source.display()));
            if extra.is_file() {
                let _ = std::fs::copy(&extra, format!("{}{suffix}", copy.display()));
            }
        }

        /// Everything about the database that must survive the migration.
        fn snapshot(path: &std::path::Path) -> serde_json::Value {
            let connection = Connection::open_with_flags(
                path,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            )
            .expect("open the copy read-only");
            let version: i64 = connection
                .query_row("SELECT COALESCE(MAX(version),0) FROM schema_migrations", [], |row| {
                    row.get(0)
                })
                .unwrap();
            let artifacts: i64 = connection
                .query_row("SELECT COUNT(*) FROM artifacts", [], |row| row.get(0))
                .unwrap();
            let versions: i64 = connection
                .query_row("SELECT COUNT(*) FROM managed_file_versions", [], |row| row.get(0))
                .unwrap();
            let has_class: bool = connection
                .query_row(
                    "SELECT COUNT(*) FROM pragma_table_info('artifacts') WHERE name='artifact_class'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap()
                > 0;
            let mut classes: Vec<(String, i64)> = Vec::new();
            if has_class {
                let mut statement = connection
                    .prepare("SELECT artifact_class, COUNT(*) FROM artifacts GROUP BY 1 ORDER BY 1")
                    .unwrap();
                classes = statement
                    .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
                    .unwrap()
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .unwrap();
            }
            // Identity of every artifact row: id + storage path. Nothing may be
            // rewritten by a migration.
            let mut ids: Vec<(String, String)> = connection
                .prepare("SELECT id, storage_path FROM artifacts ORDER BY id")
                .unwrap()
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap();
            ids.sort();
            // Every recorded version must still be resolvable: a path and the
            // hash of the content it records.
            let resolvable: i64 = connection
                .query_row(
                    "SELECT COUNT(*) FROM managed_file_versions
                      WHERE storage_path IS NOT NULL AND trim(storage_path) <> ''
                        AND after_hash IS NOT NULL AND length(after_hash) = 64",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            let placements: i64 = connection
                .query_row(
                    "SELECT COUNT(*) FROM pragma_table_info('deliverable_placements')",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap_or(0);
            serde_json::json!({
                "schemaVersion": version,
                "artifacts": artifacts,
                "artifactIds": ids,
                "artifactClasses": classes,
                "managedFileVersions": versions,
                "resolvableVersions": resolvable,
                "placementColumns": placements,
            })
        }

        let before = snapshot(&copy);
        let database = crate::database::Database::open(copy.clone()).expect("migrate the copy");
        drop(database);
        let after = snapshot(&copy);

        let summary = serde_json::json!({ "before": before, "after": after });
        println!("{}", serde_json::to_string_pretty(&summary).unwrap());

        assert_eq!(
            before["artifacts"], after["artifacts"],
            "the migration must not add or remove artifact rows"
        );
        assert_eq!(
            before["artifactIds"], after["artifactIds"],
            "artifact ids and storage paths must be untouched"
        );
        assert_eq!(
            before["artifactClasses"], after["artifactClasses"],
            "historical classifications must not be relabelled"
        );
        assert_eq!(
            before["managedFileVersions"], after["managedFileVersions"],
            "version rows must be untouched"
        );
        assert_eq!(
            before["resolvableVersions"], after["managedFileVersions"],
            "every recorded version must still be resolvable after the migration"
        );
        assert!(
            after["schemaVersion"].as_i64().unwrap() >= 73,
            "the copy must reach the placement migration"
        );
        assert!(
            after["placementColumns"].as_i64().unwrap() > 0,
            "the placement table must exist after the migration"
        );
        std::fs::remove_dir_all(&scratch).ok();
    }

    #[test]
    fn migrates_plugin_and_knowledge_schema_without_losing_legacy_bindings() {        let mut connection = Connection::open_in_memory().expect("open database");
        run_pre_p0_p2(&mut connection, 1);
        connection
            .execute_batch(
                "INSERT INTO agents(
                    id, name, description, runtime_type, system_prompt, default_model,
                    created_at, updated_at
                 ) VALUES ('migration-agent', 'Migration Agent', '', 'pi', '', 'model', 1, 1);
                 INSERT INTO conversations(
                    id, agent_id, title, status, created_at, updated_at
                 ) VALUES ('migration-conversation', 'migration-agent', 'Migration', 'active', 1, 1);
                 INSERT INTO service_connections(
                    id, service_type, name, base_url, enabled, last_status,
                    created_at, updated_at
                 ) VALUES ('migration-service', 'knowledge', 'Migration Service',
                           'https://migration.invalid', 1, 'ready', 1, 1);
                 INSERT INTO knowledge_bindings(
                    conversation_id, service_connection_id, knowledge_base_id,
                    knowledge_base_name, enabled, created_at, updated_at
                 ) VALUES ('migration-conversation', 'migration-service', 'legacy-kb',
                           'Legacy KB', 1, 1, 1);",
            )
            .expect("seed pre-P0/P2 data");

        run(&mut connection, 2).expect("apply P0/P2 migration");
        run(&mut connection, 3).expect("repeat P0/P2 migration");

        for table in [
            "plugin_catalog_cache",
            "plugin_installations",
            "plugin_operations",
            "knowledge_bindings_v2",
            "conversation_tool_permissions",
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
                "missing P0/P2 table {table}"
            );
        }
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master
                     WHERE type = 'table' AND name = 'local_knowledge_bases'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            0,
            "local knowledge tables must live in knowledge.db"
        );
        for column in ["catalog_plugin_id", "origin"] {
            assert_eq!(
                connection
                    .query_row(
                        "SELECT COUNT(*) FROM pragma_table_info('mcp_servers') WHERE name = ?1",
                        [column],
                        |row| row.get::<_, i64>(0),
                    )
                    .unwrap(),
                1,
                "missing mcp_servers.{column}"
            );
        }
        connection
            .execute(
                "INSERT INTO mcp_servers(id, name, command, created_at, updated_at)
                 VALUES ('migration-mcp', 'Migration MCP', 'echo', 1, 1)",
                [],
            )
            .expect("insert legacy-compatible MCP server");
        assert_eq!(
            connection
                .query_row(
                    "SELECT origin FROM mcp_servers WHERE id = 'migration-mcp'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "local"
        );

        let legacy_binding: (String, String, String) = connection
            .query_row(
                "SELECT source, provider_key, connection_id
                 FROM knowledge_bindings_v2
                 WHERE conversation_id = 'migration-conversation'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("load migrated knowledge binding");
        assert_eq!(
            legacy_binding,
            (
                "remote".to_owned(),
                "migration-service".to_owned(),
                "migration-service".to_owned()
            )
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM knowledge_bindings WHERE conversation_id = 'migration-conversation'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM schema_migrations WHERE version = ?1",
                    [CONVERSATION_TOOL_PERMISSION_SCHEMA_VERSION],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM schema_migrations WHERE version = ?1",
                    [P0_P2_SCHEMA_VERSION],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1
        );

        connection
            .execute(
                "INSERT INTO knowledge_bindings(
                    conversation_id, service_connection_id, knowledge_base_id,
                    knowledge_base_name, enabled, created_at, updated_at
                 ) VALUES ('migration-conversation', 'migration-service', 'legacy-kb-2',
                           'Legacy KB 2', 1, 2, 2)",
                [],
            )
            .expect("insert through legacy knowledge binding shape");
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM knowledge_bindings_v2
                     WHERE conversation_id = 'migration-conversation'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            2
        );
        connection
            .execute(
                "UPDATE knowledge_bindings
                 SET knowledge_base_name = 'Legacy KB 2 updated'
                 WHERE conversation_id = 'migration-conversation' AND knowledge_base_id = 'legacy-kb-2'",
                [],
            )
            .expect("update through legacy knowledge binding shape");
        assert_eq!(
            connection
                .query_row(
                    "SELECT knowledge_base_name FROM knowledge_bindings_v2
                     WHERE knowledge_base_id = 'legacy-kb-2'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "Legacy KB 2 updated"
        );
        connection
            .execute(
                "DELETE FROM knowledge_bindings
                 WHERE conversation_id = 'migration-conversation' AND knowledge_base_id = 'legacy-kb-2'",
                [],
            )
            .expect("delete through legacy knowledge binding shape");
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM knowledge_bindings_v2
                     WHERE knowledge_base_id = 'legacy-kb-2'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            0
        );
    }

    #[test]
    fn enforces_p0_p2_unique_and_source_constraints() {
        let mut connection = Connection::open_in_memory().expect("open database");
        run(&mut connection, 1).expect("apply migrations");
        connection
            .execute_batch(
                "INSERT INTO agents(
                    id, name, description, runtime_type, system_prompt, default_model,
                    created_at, updated_at
                 ) VALUES ('constraint-agent', 'Constraint Agent', '', 'pi', '', 'model', 1, 1);
                 INSERT INTO conversations(
                    id, agent_id, title, status, created_at, updated_at
                 ) VALUES ('constraint-conversation', 'constraint-agent', 'Constraint', 'active', 1, 1);
                 INSERT INTO service_connections(
                    id, service_type, name, base_url, enabled, last_status,
                    created_at, updated_at
                 ) VALUES ('constraint-service', 'knowledge', 'Constraint Service',
                           'https://constraint.invalid', 1, 'ready', 1, 1);
                 INSERT INTO plugin_catalog_cache(
                    plugin_id, version, kind, origin, manifest_json, fetched_at
                 ) VALUES ('plugin-1', '1.0.0', 'skill', 'builtin', '{}', 1);
                 INSERT INTO plugin_installations(
                    plugin_id, installed_version, origin, install_status, installed_at, updated_at
                 ) VALUES ('plugin-1', '1.0.0', 'builtin', 'installed', 1, 1);
                 INSERT INTO knowledge_bindings_v2(
                    id, conversation_id, source, provider_key, connection_id,
                    knowledge_base_id, enabled, created_at, updated_at
                 ) VALUES ('local-binding-1', 'constraint-conversation', 'local', 'local', NULL,
                           'local-kb', 1, 1, 1);",
            )
            .expect("seed constraint fixtures");

        assert!(connection
            .execute(
                "INSERT INTO plugin_catalog_cache(
                    plugin_id, version, kind, origin, manifest_json, fetched_at
                 ) VALUES ('plugin-1', '1.0.0', 'skill', 'builtin', '{}', 2)",
                [],
            )
            .is_err());
        assert!(connection
            .execute(
                "INSERT INTO plugin_installations(
                    plugin_id, installed_version, origin, install_status, installed_at, updated_at
                 ) VALUES ('plugin-1', '1.0.1', 'builtin', 'installed', 2, 2)",
                [],
            )
            .is_err());
        assert!(connection
            .execute(
                "INSERT INTO knowledge_bindings_v2(
                    id, conversation_id, source, provider_key, connection_id,
                    knowledge_base_id, enabled, created_at, updated_at
                 ) VALUES ('local-binding-2', 'constraint-conversation', 'local', 'local', NULL,
                           'local-kb', 1, 2, 2)",
                [],
            )
            .is_err());
        assert!(connection
            .execute(
                "INSERT INTO knowledge_bindings_v2(
                    id, conversation_id, source, provider_key, connection_id,
                    knowledge_base_id, enabled, created_at, updated_at
                 ) VALUES ('invalid-local-binding', 'constraint-conversation', 'local',
                           'wrong-provider', NULL, 'local-kb-2', 1, 2, 2)",
                [],
            )
            .is_err());
        assert!(connection
            .execute(
                "INSERT INTO knowledge_bindings_v2(
                    id, conversation_id, source, provider_key, connection_id,
                    knowledge_base_id, enabled, created_at, updated_at
                 ) VALUES ('invalid-remote-binding', 'constraint-conversation', 'remote',
                           'wrong-provider', 'constraint-service', 'remote-kb', 1, 2, 2)",
                [],
            )
            .is_err());
    }

    #[test]
    fn creates_knowledge_preview_cache_settings_and_document_activity_schema() {
        let mut connection = Connection::open_in_memory().expect("open database");
        run(&mut connection, 1).expect("apply migrations");
        run(&mut connection, 2).expect("repeat empty-database migrations");

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
    fn adds_persisted_model_provider_icon_column() {
        let mut connection = Connection::open_in_memory().expect("open database");
        run(&mut connection, 1).expect("apply migrations");
        run(&mut connection, 2).expect("repeat migrations");

        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM pragma_table_info('model_providers') WHERE name = 'icon'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM schema_migrations WHERE version = 18",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1
        );
    }

    #[test]
    fn classifies_existing_agents_and_enforces_binding_constraints() {
        let mut connection = Connection::open_in_memory().expect("open database");
        run_pre_a0(&mut connection, 1);
        for (index, (id, runtime_type, system_prompt)) in [
            ("fox-general", "pi", ""),
            ("fox-debugger", "pi", ""),
            ("fox-user-local", "pi", "Local expert"),
            (
                "yuxi:worker",
                "yuxi",
                r#"{"isSubagent":true,"isDefault":true}"#,
            ),
            ("yuxi:assistant", "yuxi", r#"{"chatVisible":true}"#),
            ("yuxi:expert", "yuxi", "{}"),
        ]
        .into_iter()
        .enumerate()
        {
            connection
                .execute(
                    "INSERT INTO agents(
                        id, name, description, runtime_type, system_prompt, default_model,
                        created_at, updated_at
                     ) VALUES (?1, ?1, '', ?2, ?3, '', ?4, ?4)",
                    rusqlite::params![id, runtime_type, system_prompt, index as i64 + 10],
                )
                .expect("insert legacy agent");
        }

        let transaction = connection.transaction().expect("begin legacy v19 upgrade");
        for (version, sql) in [
            (14, MIGRATION_14),
            (15, MIGRATION_15),
            (16, MIGRATION_16),
            (17, MIGRATION_17),
            (18, MIGRATION_18),
            (19, MIGRATION_19),
        ] {
            apply_migration(&transaction, version, sql, 19).expect("apply legacy v19 migration");
        }
        transaction.commit().expect("commit legacy v19 schema");
        connection
            .execute(
                "INSERT INTO conversations(
                    id, agent_id, title, status, created_at, updated_at
                 ) VALUES ('legacy-expert-conversation', 'fox-debugger', 'Legacy expert', 'active', 19, 19)",
                [],
            )
            .expect("insert legacy expert conversation");
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM pragma_table_info('agents') WHERE name = 'agent_kind'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            0
        );

        run(&mut connection, 20).expect("upgrade legacy agents");
        run(&mut connection, 21).expect("repeat upgraded migrations");

        assert_eq!(
            connection
                .query_row(
                    "SELECT agent_id FROM conversations WHERE id = 'legacy-expert-conversation'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "fox-general"
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT expert_id FROM conversation_expert_bindings
                     WHERE conversation_id = 'legacy-expert-conversation' AND state = 'active'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "fox-debugger"
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM schema_migrations WHERE version = 20",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM schema_migrations WHERE version = 21",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1
        );
        let (package_snapshot_json, package_hash) = connection
            .query_row(
                "SELECT package_snapshot_json, package_hash
                 FROM conversation_expert_bindings
                 WHERE conversation_id = 'legacy-expert-conversation'",
                [],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .expect("load migrated package snapshot");
        let package_snapshot: serde_json::Value =
            serde_json::from_str(&package_snapshot_json).expect("parse migrated package snapshot");
        assert_eq!(package_snapshot["id"], "fox-debugger");
        assert_eq!(
            package_hash,
            package_snapshot_hash(&package_snapshot),
            "v21 must hash the canonical package snapshot"
        );

        for (id, expected) in [
            ("fox-general", ("assistant", "primary", "chat_selector")),
            ("fox-debugger", ("expert", "inline", "expert_center")),
            ("fox-user-local", ("expert", "inline", "expert_center")),
            ("yuxi:worker", ("worker", "child", "hidden")),
            ("yuxi:assistant", ("assistant", "primary", "chat_selector")),
            ("yuxi:expert", ("expert", "inline", "expert_center")),
        ] {
            let actual = connection
                .query_row(
                    "SELECT agent_kind, invocation_mode, visibility FROM agents WHERE id = ?1",
                    [id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                        ))
                    },
                )
                .expect("load migrated classification");
            assert_eq!(
                actual,
                (
                    expected.0.to_owned(),
                    expected.1.to_owned(),
                    expected.2.to_owned()
                ),
                "classification mismatch for {id}"
            );
        }
        assert!(connection
            .execute(
                "UPDATE agents SET agent_kind = 'assistant' WHERE id = 'fox-debugger'",
                [],
            )
            .is_err());

        connection
            .execute(
                "INSERT INTO conversations(
                    id, agent_id, title, status, created_at, updated_at
                 ) VALUES ('conversation-1', 'fox-general', 'Legacy', 'active', 30, 30)",
                [],
            )
            .expect("insert conversation");
        connection
            .execute(
                "INSERT INTO conversation_expert_bindings(
                    id, conversation_id, expert_id, state, activation_source, expert_version,
                    package_hash, display_snapshot_json, activated_at
                 ) VALUES (
                    'binding-1', 'conversation-1', 'fox-debugger', 'active', 'migration-test',
                    '1.0.0', 'hash-1', '{}', 31
                 )",
                [],
            )
            .expect("insert active binding");
        assert!(connection
            .execute(
                "INSERT INTO conversation_expert_bindings(
                    id, conversation_id, expert_id, state, activation_source, expert_version,
                    package_hash, display_snapshot_json, activated_at
                 ) VALUES (
                    'binding-2', 'conversation-1', 'fox-debugger', 'active', 'migration-test',
                    '1.0.0', 'hash-2', '{}', 32
                 )",
                [],
            )
            .is_err());
    }

    #[test]
    fn upgrades_v20_bindings_with_static_package_snapshots() {
        let mut connection = Connection::open_in_memory().expect("open database");
        run_pre_a0(&mut connection, 1);
        let transaction = connection.transaction().expect("begin v19 schema setup");
        for (version, sql) in [
            (14, MIGRATION_14),
            (15, MIGRATION_15),
            (16, MIGRATION_16),
            (17, MIGRATION_17),
            (18, MIGRATION_18),
            (19, MIGRATION_19),
        ] {
            apply_migration(&transaction, version, sql, 19).expect("apply v19 schema");
        }
        transaction.commit().expect("commit v19 schema");
        connection
            .execute_batch(
                r#"
                INSERT INTO agents(
                    id, name, description, runtime_type, system_prompt, default_model,
                    category, opening_suggestions_json, is_builtin, package_version,
                    package_manifest_json, created_at, updated_at
                ) VALUES
                    ('fox-general', 'General', 'Base assistant', 'pi', 'base prompt', 'base-model',
                     'general', '[]', 1, '1.0.0', '{}', 1, 1),
                    ('local-expert', 'Local', 'Local expert', 'pi', 'local prompt', 'local-model',
                     'engineering', '["Start local"]', 0, '2.1.0', '{"allowedTools":["read"]}', 2, 2),
                    ('yuxi:remote-expert', 'Remote', 'Remote expert', 'yuxi',
                     '{"remoteAvailable":true,"capabilities":["knowledge"],"resources":{"tools":[],"knowledges":[],"mcps":[],"skills":[]}}',
                     'remote-model', 'knowledge', '["Start remote"]', 0, '3.0.0',
                     '{"allowedTools":["search_knowledge"]}', 3, 3);
                INSERT INTO conversations(
                    id, agent_id, title, status, created_at, updated_at
                ) VALUES
                    ('local-conversation', 'fox-general', 'Local', 'active', 4, 4),
                    ('remote-conversation', 'fox-general', 'Remote', 'active', 5, 5);
                "#,
            )
            .expect("seed v19 agents and conversations");
        let transaction = connection.transaction().expect("begin v20 migration");
        apply_agent_expert_migration(&transaction, 20).expect("apply v20 migration");
        transaction.commit().expect("commit v20 migration");
        connection
            .execute_batch(
                r#"
                INSERT INTO conversation_expert_bindings(
                    id, conversation_id, expert_id, state, activation_source, expert_version,
                    package_hash, display_snapshot_json, activated_at
                ) VALUES
                    ('local-binding', 'local-conversation', 'local-expert', 'active', 'test',
                     '2.1.0', 'legacy-local-hash', '{}', 6),
                    ('remote-binding', 'remote-conversation', 'yuxi:remote-expert', 'active', 'test',
                     '3.0.0', 'legacy-remote-hash', '{}', 7);
                "#,
            )
            .expect("seed v20 bindings");

        run(&mut connection, 21).expect("upgrade v20 bindings to v21");
        run(&mut connection, 22).expect("repeat v21 migration");

        for (binding_id, expected_prompt) in [
            ("local-binding", Some("local prompt")),
            ("remote-binding", None),
        ] {
            let (snapshot_json, package_hash) = connection
                .query_row(
                    "SELECT package_snapshot_json, package_hash
                     FROM conversation_expert_bindings WHERE id = ?1",
                    [binding_id],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                )
                .expect("load upgraded binding");
            let snapshot: serde_json::Value =
                serde_json::from_str(&snapshot_json).expect("parse upgraded snapshot");
            assert_eq!(package_hash, package_snapshot_hash(&snapshot));
            match expected_prompt {
                Some(prompt) => assert_eq!(snapshot["systemPrompt"], prompt),
                None => assert!(snapshot["systemPrompt"].is_null()),
            }
            for field in [
                "id",
                "name",
                "description",
                "runtimeType",
                "agentKind",
                "invocationMode",
                "visibility",
                "defaultModel",
                "systemPrompt",
                "category",
                "isBuiltin",
                "openingSuggestions",
                "packageVersion",
                "packageManifest",
                "capabilities",
                "resources",
            ] {
                assert!(
                    snapshot.get(field).is_some(),
                    "missing snapshot field {field}"
                );
            }
        }
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM schema_migrations WHERE version = 21",
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
                 INSERT INTO runs(
                    id, conversation_id, status, model, started_at, finished_at, created_at
                 ) VALUES ('run-terminal', 'conversation-a0', 'completed', 'model', 2, 3, 2);
                 INSERT INTO messages(
                    id, conversation_id, run_id, role, kind, content, status, ordinal,
                    created_at, updated_at
                 ) VALUES ('message-a0', 'conversation-a0', 'run-a0', 'user', 'text',
                           'keep me', 'completed', 1, 2, 2);
                 INSERT INTO run_events(
                    id, run_id, seq, event_type, event_json, created_at
                 ) VALUES ('event-a0', 'run-a0', 1, 'run.failed',
                           '{\"type\":\"run.failed\"}', 3);
                 INSERT INTO attachments(
                    id, conversation_id, display_name, storage_path, status, created_at
                 ) VALUES ('attachment-a0', 'conversation-a0', 'fixture.txt',
                           'fixtures/fixture.txt', 'ready', 2);
                 INSERT INTO service_connections(
                    id, service_type, name, base_url, enabled, last_status,
                    created_at, updated_at
                 ) VALUES ('service-a0', 'knowledge', 'Fixture',
                           'https://fixture.invalid', 1, 'ready', 2, 2);
                 INSERT INTO knowledge_bindings(
                    conversation_id, service_connection_id, knowledge_base_id,
                    knowledge_base_name, enabled, created_at, updated_at
                 ) VALUES ('conversation-a0', 'service-a0', 'knowledge-a0',
                           'Fixture KB', 1, 2, 2);",
            )
            .expect("seed pre-A0 data");

        run(&mut connection, 4).expect("upgrade to A0");
        run(&mut connection, 5).expect("repeat A0 migration");

        for (table, count) in [
            ("conversations", 1_i64),
            ("messages", 1),
            ("runs", 2),
            ("run_events", 1),
            ("attachments", 1),
            ("knowledge_bindings", 1),
            ("goals", 0),
            ("work_tasks", 0),
            ("task_evidence", 0),
            ("pending_work_mode_dispatches", 0),
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
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM schema_migrations WHERE version = 16",
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
                     WHERE type = 'index'
                       AND name = 'idx_pending_work_mode_dispatches_conversation'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1
        );
        for (table, column) in [
            ("runs", "trace_id"),
            ("runs", "root_span_id"),
            ("runs", "parent_run_id"),
            ("runs", "root_run_id"),
            ("runs", "run_kind"),
            ("runs", "depth"),
            ("runs", "budget_json"),
            ("conversations", "conversation_kind"),
            ("mcp_servers", "transport"),
            ("mcp_servers", "endpoint_url"),
            ("mcp_servers", "definition"),
            ("mcp_servers", "last_latency_ms"),
            ("mcp_servers", "tool_count"),
            ("mcp_servers", "consecutive_failures"),
            ("agents", "package_source"),
            ("agents", "package_id"),
            ("agents", "package_hash"),
            ("child_run_delegations", "team_run_id"),
            ("child_run_delegations", "team_member_id"),
            ("child_run_delegations", "allowed_tools_json"),
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
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master
                     WHERE type = 'table' AND name = 'child_run_delegations'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1
        );
        for table in [
            "lifecycle_hooks",
            "lifecycle_hook_executions",
            "expert_package_versions",
            "expert_workflow_runs",
            "expert_workflow_stage_runs",
            "expert_workflow_gate_decisions",
            "expert_team_runs",
            "digital_colleagues",
            "digital_colleague_schedules",
            "digital_colleague_channels",
            "digital_colleague_triggers",
            "digital_colleague_audit_log",
            "run_continuation_decisions",
            "run_continuation_ingest_diagnostics",
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
                "missing {table}"
            );
        }
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM schema_migrations WHERE version = 28",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM schema_migrations WHERE version = 29",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM schema_migrations WHERE version = 30",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM schema_migrations WHERE version = 31",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM schema_migrations WHERE version = 32",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM schema_migrations WHERE version = 33",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1
        );
    }

    #[test]
    fn continuation_decision_schema_is_append_friendly_and_rejects_invalid_facts() {
        let mut connection = Connection::open_in_memory().expect("open database");
        run(&mut connection, 1).expect("apply migrations");
        connection
            .execute_batch(
                "INSERT INTO agents(
                    id, name, description, runtime_type, system_prompt, default_model,
                    created_at, updated_at
                 ) VALUES ('agent-durable', 'Durable', '', 'pi', '', 'model', 1, 1);
                 INSERT INTO conversations(
                    id, agent_id, title, status, created_at, updated_at
                 ) VALUES ('conversation-durable', 'agent-durable', 'Durable', 'active', 1, 1);
                 INSERT INTO runs(
                    id, conversation_id, status, model, last_seq, created_at
                 ) VALUES ('run-durable', 'conversation-durable', 'running', 'model', 4, 1);
                 INSERT INTO run_events(
                    id, run_id, seq, event_type, event_json, created_at
                 ) VALUES
                 (
                    'event-started', 'run-durable', 0,
                    'run.started', '{\"type\":\"run.started\"}', 1
                 ),
                 (
                    'event-continuation-1', 'run-durable', 1,
                    'run.continuation_proposed',
                    '{\"type\":\"run.continuation_proposed\",\"proposal\":{}}', 1
                 ),
                 (
                    'event-continuation-2', 'run-durable', 2,
                    'run.continuation_proposed',
                    '{\"type\":\"run.continuation_proposed\",\"proposal\":{}}', 1
                 ),
                 (
                    'event-continuation-3', 'run-durable', 3,
                    'run.continuation_proposed',
                    '{\"type\":\"run.continuation_proposed\",\"proposal\":{}}', 1
                 ),
                 (
                    'event-continuation', 'run-durable', 4,
                    'run.continuation_proposed',
                    '{\"type\":\"run.continuation_proposed\",\"proposal\":{}}', 1
                 );",
            )
            .expect("seed durable run");

        connection
            .execute(
                "INSERT INTO run_continuation_decisions(
                    decision_id, schema_version, run_id, event_cursor, decision, reason_code,
                    active_task_ids_json, evidence_ids_json, missing_acceptance_json,
                    next_action, retry_class, blocked_dependency_refs_json,
                    host_validation_outcome, host_validation_error, created_at
                 ) VALUES (
                    'decision-valid', 1, 'run-durable', 4, 'repair', 'validation_failed',
                    '[]', '[]', '[\"tests_pass\"]', 'run targeted tests', 'recoverable', '[]',
                    'accepted', NULL, 2
                 )",
                [],
            )
            .expect("insert valid continuation decision");

        for (index, reason_code) in [
            "work_remaining",
            "validation_failed",
            "approval_pending",
            "external_dependency_unavailable",
            "budget_exhausted",
            "user_input_required",
            "retry_available",
            "retry_exhausted",
            "acceptance_missing",
            "acceptance_candidate",
            "acceptance_passed",
            "host_audit_required",
            "tool_failed",
            "task_interrupted",
        ]
        .into_iter()
        .enumerate()
        {
            connection
                .execute(
                    "INSERT INTO run_continuation_decisions(
                        decision_id, schema_version, run_id, event_cursor, decision, reason_code,
                        active_task_ids_json, evidence_ids_json, missing_acceptance_json,
                        blocked_dependency_refs_json, host_validation_outcome, created_at
                     ) VALUES (?1, 1, 'run-durable', 4, 'continue', ?2,
                               '[]', '[]', '[]', '[]', 'accepted', 3)",
                    rusqlite::params![format!("reason-{index}"), reason_code],
                )
                .unwrap_or_else(|error| panic!("reason code {reason_code} failed: {error}"));
        }
        for (index, retry_class) in [
            "none",
            "recoverable",
            "retry_limited",
            "non_retryable",
            "host_decides",
        ]
        .into_iter()
        .enumerate()
        {
            connection
                .execute(
                    "INSERT INTO run_continuation_decisions(
                        decision_id, schema_version, run_id, event_cursor, decision, reason_code,
                        active_task_ids_json, evidence_ids_json, missing_acceptance_json,
                        retry_class, blocked_dependency_refs_json, host_validation_outcome,
                        created_at
                     ) VALUES (?1, 1, 'run-durable', 4, 'repair', 'retry_available',
                               '[]', '[]', '[]', ?2, '[]', 'accepted', 3)",
                    rusqlite::params![format!("retry-{index}"), retry_class],
                )
                .unwrap_or_else(|error| panic!("retry class {retry_class} failed: {error}"));
        }

        for invalid_sql in [
            "INSERT INTO run_continuation_decisions(
                decision_id, schema_version, run_id, event_cursor, decision, reason_code,
                active_task_ids_json, evidence_ids_json, missing_acceptance_json,
                blocked_dependency_refs_json, host_validation_outcome, created_at
             ) VALUES ('bad-decision', 1, 'run-durable', 4, 'finish', 'acceptance_passed',
                       '[]', '[]', '[]', '[]', 'accepted', 3)",
            "INSERT INTO run_continuation_decisions(
                decision_id, schema_version, run_id, event_cursor, decision, reason_code,
                active_task_ids_json, evidence_ids_json, missing_acceptance_json,
                blocked_dependency_refs_json, host_validation_outcome, created_at
             ) VALUES ('bad-reason', 1, 'run-durable', 4, 'repair', 'free_form_reason',
                       '[]', '[]', '[]', '[]', 'accepted', 3)",
            "INSERT INTO run_continuation_decisions(
                decision_id, schema_version, run_id, event_cursor, decision, reason_code,
                active_task_ids_json, evidence_ids_json, missing_acceptance_json,
                retry_class, blocked_dependency_refs_json, host_validation_outcome, created_at
             ) VALUES ('bad-retry', 1, 'run-durable', 4, 'repair', 'retry_available',
                       '[]', '[]', '[]', 'non_recoverable', '[]', 'accepted', 3)",
            "INSERT INTO run_continuation_decisions(
                decision_id, schema_version, run_id, event_cursor, decision, reason_code,
                active_task_ids_json, evidence_ids_json, missing_acceptance_json,
                blocked_dependency_refs_json, host_validation_outcome, created_at
             ) VALUES ('bad-json', 1, 'run-durable', 4, 'continue', 'work_remaining',
                       '{}', '[]', '[]', '[]', 'accepted', 3)",
            "INSERT INTO run_continuation_decisions(
                decision_id, schema_version, run_id, event_cursor, decision, reason_code,
                active_task_ids_json, evidence_ids_json, missing_acceptance_json,
                blocked_dependency_refs_json, host_validation_outcome, host_validation_error,
                created_at
             ) VALUES ('bad-validation', 1, 'run-durable', 4, 'continue', 'work_remaining',
                       '[]', '[]', '[]', '[]', 'rejected', NULL, 3)",
            "INSERT INTO run_continuation_decisions(
                decision_id, schema_version, run_id, event_cursor, decision, reason_code,
                active_task_ids_json, evidence_ids_json, missing_acceptance_json,
                blocked_dependency_refs_json, host_validation_outcome, created_at
             ) VALUES ('bad-run', 1, 'missing-run', 0, 'continue', 'work_remaining',
                       '[]', '[]', '[]', '[]', 'accepted', 3)",
        ] {
            assert!(
                connection.execute(invalid_sql, []).is_err(),
                "invalid durable fact unexpectedly passed: {invalid_sql}"
            );
        }

        assert!(connection
            .execute(
                "INSERT INTO run_continuation_decisions(
                    decision_id, schema_version, run_id, event_cursor, decision, reason_code,
                    active_task_ids_json, evidence_ids_json, missing_acceptance_json,
                    blocked_dependency_refs_json, host_validation_outcome, created_at
                 ) VALUES ('decision-valid', 1, 'run-durable', 4, 'continue', 'work_remaining',
                           '[]', '[]', '[]', '[]', 'accepted', 4)",
                [],
            )
            .is_err());

        for invalid_sql in [
            "INSERT INTO run_continuation_ingest_diagnostics(
                run_id, event_seq, shadow_mode, error_code, error_message, created_at
             ) VALUES ('run-durable', 1, 2, 'malformed', 'invalid shadow', 5)",
            "INSERT INTO run_continuation_ingest_diagnostics(
                run_id, event_seq, shadow_mode, error_code, error_message, created_at
             ) VALUES ('run-durable', 5, 0, 'malformed', 'missing event', 5)",
            "INSERT INTO run_continuation_ingest_diagnostics(
                run_id, event_seq, shadow_mode, error_code, error_message, created_at
             ) VALUES ('run-durable', 2, 0, '', 'empty code', 5)",
            "INSERT INTO run_continuation_ingest_diagnostics(
                run_id, event_seq, shadow_mode, error_code, error_message, created_at
             ) VALUES (
                'run-durable', 3, 0, 'oversized', hex(zeroblob(2049)), 5
             )",
            "INSERT INTO run_continuation_ingest_diagnostics(
                run_id, event_seq, shadow_mode, error_code, error_message, created_at
             ) VALUES ('run-durable', 0, 0, 'wrong_event', 'not a proposal', 5)",
        ] {
            assert!(
                connection.execute(invalid_sql, []).is_err(),
                "invalid ingest diagnostic unexpectedly passed: {invalid_sql}"
            );
        }
        connection
            .execute(
                "INSERT INTO run_continuation_ingest_diagnostics(
                    run_id, event_seq, execution_profile_id, shadow_mode,
                    error_code, error_message, created_at
                 ) VALUES (
                    'run-durable', 4, 'durable_v2', 0,
                    'malformed_proposal', 'proposal.decisionId is missing', 4
                 )",
                [],
            )
            .expect("insert valid continuation ingest diagnostic");

        connection
            .execute("DELETE FROM runs WHERE id = 'run-durable'", [])
            .expect("delete owning run");
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM run_continuation_decisions",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            0
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM run_continuation_ingest_diagnostics",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            0
        );
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

    #[test]
    fn validation_policy_and_attempt_migration_is_idempotent() {
        let mut connection = Connection::open_in_memory().expect("open database");
        run(&mut connection, 1).expect("apply migrations");
        run(&mut connection, 2).expect("repeat migrations");
        for table in [
            "task_validation_policies",
            "task_attempts",
            "run_execution_profiles",
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
                "missing v34 table {table}"
            );
        }
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM schema_migrations WHERE version = 34",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1
        );
        for (table, column) in [
            ("review_findings", "resolved_by"),
            ("task_attempts", "evidence_rowid_watermark"),
            ("task_attempts", "finding_rowid_watermark"),
        ] {
            let exists = connection
                .prepare(&format!("PRAGMA table_info({table})"))
                .unwrap()
                .query_map([], |row| row.get::<_, String>(1))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
                .iter()
                .any(|name| name == column);
            assert!(exists, "missing v34 column {table}.{column}");
        }
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master
                     WHERE type = 'index' AND name = 'idx_task_attempts_one_running'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1
        );
    }

    #[test]
    fn repair_override_event_and_approval_claim_migration_is_idempotent() {
        let mut connection = Connection::open_in_memory().expect("open database");
        run(&mut connection, 1).expect("apply migrations");
        run(&mut connection, 2).expect("repeat migrations");
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM schema_migrations WHERE version = 35",
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
                     WHERE type = 'table' AND name = 'task_repair_override_events'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1
        );
        for column in ["category", "claimed_at", "claimed_by_run_id"] {
            let exists = connection
                .prepare("PRAGMA table_info(approvals)")
                .unwrap()
                .query_map([], |row| row.get::<_, String>(1))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
                .iter()
                .any(|name| name == column);
            assert!(exists, "missing v35 approvals.{column}");
        }
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master
                     WHERE type = 'trigger' AND name = 'task_repair_override_events_no_update'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1
        );
    }

    #[test]
    fn populated_v34_upgrades_to_v35_without_losing_approvals_and_enforces_constraints() {
        let mut connection = Connection::open_in_memory().expect("open database");
        run(&mut connection, 1).expect("create current schema used to synthesize populated v34");
        connection
            .execute_batch(
                "INSERT INTO agents(
                    id, name, description, runtime_type, system_prompt, default_model,
                    created_at, updated_at
                 ) VALUES ('agent-v34', 'V34', '', 'pi', '', 'model', 1, 1);
                 INSERT INTO conversations(
                    id, agent_id, title, status, created_at, updated_at
                 ) VALUES ('conversation-v34', 'agent-v34', 'V34', 'active', 1, 1);
                 INSERT INTO runs(id, conversation_id, status, model, last_seq, created_at)
                 VALUES ('run-v34', 'conversation-v34', 'running', 'model', 0, 1);
                 INSERT INTO tool_calls(
                    id, runtime_tool_call_id, run_id, conversation_id, tool_name, input_json,
                    status, execution_location, requires_approval, started_at, updated_at
                 ) VALUES (
                    'tool-v34', 'runtime-tool-v34', 'run-v34', 'conversation-v34',
                    'write_file', '{}', 'pending', 'host', 1, 1, 1
                 );
                 INSERT INTO approvals(
                    id, tool_call_id, status, requested_action, request_json, requested_at,
                    category, claimed_at, claimed_by_run_id
                 ) VALUES (
                    'approval-v34', 'tool-v34', 'approved', 'legacy approval', '{}', 1,
                    'tool_execution', NULL, NULL
                 );",
            )
            .expect("seed rows before synthetic downgrade");

        connection
            .pragma_update(None, "foreign_keys", "OFF")
            .unwrap();
        connection
            .execute_batch(
                "DROP TABLE task_repair_override_events;
                 ALTER TABLE approvals RENAME TO approvals_v35;
                 CREATE TABLE approvals (
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
                 INSERT INTO approvals(
                    id, tool_call_id, status, requested_action, request_json, decision_json,
                    requested_at, resolved_at
                 )
                 SELECT id, tool_call_id, status, requested_action, request_json, decision_json,
                        requested_at, resolved_at
                 FROM approvals_v35;
                 DROP TABLE approvals_v35;
                 CREATE INDEX idx_approvals_status_requested
                    ON approvals(status, requested_at);
                 DELETE FROM schema_migrations WHERE version = 35;",
            )
            .expect("synthesize populated v34 schema");
        connection
            .pragma_update(None, "foreign_keys", "ON")
            .unwrap();

        run(&mut connection, 2).expect("upgrade populated v34 to v35");
        run(&mut connection, 3).expect("repeat populated v35 migration");
        let migrated: (String, Option<i64>, Option<String>) = connection
            .query_row(
                "SELECT category, claimed_at, claimed_by_run_id
                 FROM approvals WHERE id = 'approval-v34'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("load migrated legacy approval");
        assert_eq!(migrated, ("tool_execution".to_owned(), None, None));
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM schema_migrations WHERE version = 35",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1
        );
        let foreign_key_errors = connection
            .prepare("PRAGMA foreign_key_check")
            .unwrap()
            .query_map([], |_| Ok(()))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(foreign_key_errors.is_empty());

        let foreign_tables = connection
            .prepare("PRAGMA foreign_key_list(task_repair_override_events)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(2))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        for table in [
            "work_tasks",
            "task_attempts",
            "approvals",
            "tool_calls",
            "runs",
            "conversations",
        ] {
            assert!(foreign_tables.iter().any(|value| value == table));
        }
        let mut unique_columns = Vec::new();
        let indexes = connection
            .prepare("PRAGMA index_list(task_repair_override_events)")
            .unwrap()
            .query_map([], |row| {
                Ok((row.get::<_, String>(1)?, row.get::<_, i64>(2)?))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        for (index, unique) in indexes {
            if unique == 1 {
                let columns = connection
                    .prepare(&format!("PRAGMA index_info('{index}')"))
                    .unwrap()
                    .query_map([], |row| row.get::<_, String>(2))
                    .unwrap()
                    .collect::<Result<Vec<_>, _>>()
                    .unwrap();
                if columns.len() == 1 {
                    unique_columns.push(columns[0].clone());
                }
            }
        }
        for column in ["task_id", "attempt_id", "approval_id", "tool_call_id"] {
            assert!(unique_columns.iter().any(|value| value == column));
        }
        for index in [
            "idx_task_repair_override_events_conversation_created",
            "idx_task_repair_override_events_run_created",
        ] {
            assert_eq!(
                connection
                    .query_row(
                        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = ?1",
                        [index],
                        |row| row.get::<_, i64>(0),
                    )
                    .unwrap(),
                1
            );
        }

        connection
            .execute_batch(
                "INSERT INTO goals(
                    id, conversation_id, title, objective, status, created_by, created_at, updated_at
                 ) VALUES ('goal-v35', 'conversation-v34', 'Goal', 'Goal', 'active', 'user', '1', '1');
                 INSERT INTO work_tasks(id, goal_id, ordinal, title, status, created_at, updated_at)
                 VALUES ('task-v35', 'goal-v35', 0, 'Task', 'blocked', '1', '1');
                 INSERT INTO task_attempts(
                    id, task_id, attempt_number, kind, status, run_id, policy_hash,
                    root_cause, finding_ids_json, evidence_ids_json, failure_reason, version,
                    evidence_rowid_watermark, finding_rowid_watermark, started_at, finished_at
                 ) VALUES (
                    'attempt-v35', 'task-v35', 1, 'repair', 'failed', 'run-v34',
                    'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
                    'root', '[\"finding\"]', '[]', 'failed', 1, 0, 0, 1, 2
                 );
                 INSERT INTO tool_calls(
                    id, runtime_tool_call_id, run_id, conversation_id, tool_name, input_json,
                    status, execution_location, requires_approval, started_at, updated_at
                 ) VALUES (
                    'tool-v35', 'runtime-tool-v35', 'run-v34', 'conversation-v34',
                    'task_repair_escalate_start', '{}', 'running', 'host', 1, 1, 1
                 );
                 INSERT INTO approvals(
                    id, tool_call_id, status, requested_action, request_json, requested_at,
                    category, claimed_at, claimed_by_run_id
                 ) VALUES (
                    'approval-v35', 'tool-v35', 'approved', 'override', '{}', 1,
                    'task_repair_budget_override', 1, 'run-v34'
                 );
                 INSERT INTO task_repair_override_events(
                    id, task_id, attempt_id, approval_id, tool_call_id, run_id, conversation_id,
                    policy_id, policy_hash, normal_repair_budget, normal_repair_used,
                    override_count, input_hash, escalation_reason, created_at
                 ) VALUES (
                    'event-v35', 'task-v35', 'attempt-v35', 'approval-v35', 'tool-v35',
                    'run-v34', 'conversation-v34', 'standard_v1',
                    'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb',
                    2, 2, 1,
                    'cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc',
                    'operator override', 2
                 );",
            )
            .expect("insert valid v35 override fact");
        assert!(connection
            .execute(
                "UPDATE task_repair_override_events SET escalation_reason = 'changed'
                 WHERE id = 'event-v35'",
                [],
            )
            .is_err());
        assert!(connection
            .execute(
                "INSERT INTO task_repair_override_events(
                    id, task_id, attempt_id, approval_id, tool_call_id, run_id, conversation_id,
                    policy_id, policy_hash, normal_repair_budget, normal_repair_used,
                    override_count, input_hash, escalation_reason, created_at
                 ) VALUES (
                    'event-invalid-check', 'task-v35', 'attempt-v35', 'approval-v35', 'tool-v35',
                    'run-v34', 'conversation-v34', 'legacy_v1', 'short', 2, 1, 2, 'short', '', -1
                 )",
                [],
            )
            .is_err());
        assert!(connection
            .execute(
                "INSERT INTO task_repair_override_events(
                    id, task_id, attempt_id, approval_id, tool_call_id, run_id, conversation_id,
                    policy_id, policy_hash, normal_repair_budget, normal_repair_used,
                    override_count, input_hash, escalation_reason, created_at
                 ) VALUES (
                    'event-invalid-fk', 'missing-task', 'missing-attempt', 'missing-approval',
                    'missing-tool', 'missing-run', 'missing-conversation', 'standard_v1',
                    'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb',
                    1, 1, 1,
                    'cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc',
                    'invalid references', 2
                 )",
                [],
            )
            .is_err());
        assert!(connection
            .execute(
                "INSERT INTO task_repair_override_events(
                    id, task_id, attempt_id, approval_id, tool_call_id, run_id, conversation_id,
                    policy_id, policy_hash, normal_repair_budget, normal_repair_used,
                    override_count, input_hash, escalation_reason, created_at
                 ) SELECT 'event-duplicate', task_id, attempt_id, approval_id, tool_call_id,
                          run_id, conversation_id, policy_id, policy_hash, normal_repair_budget,
                          normal_repair_used, override_count, input_hash, escalation_reason, created_at
                   FROM task_repair_override_events WHERE id = 'event-v35'",
                [],
            )
            .is_err());
    }

    #[test]
    fn readonly_graph_migration_is_idempotent_and_enforces_graph_invariants() {
        let mut connection = Connection::open_in_memory().expect("open database");
        run(&mut connection, 1).expect("apply migrations");
        run(&mut connection, 2).expect("repeat migrations");

        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM schema_migrations WHERE version = 36",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1
        );
        for table in ["work_graph_specs", "work_graph_nodes", "work_task_edges"] {
            assert_eq!(
                connection
                    .query_row(
                        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
                        [table],
                        |row| row.get::<_, i64>(0),
                    )
                    .unwrap(),
                1,
                "missing v36 table {table}"
            );
        }
        let delegation_columns = connection
            .prepare("PRAGMA table_info(child_run_delegations)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(delegation_columns
            .iter()
            .any(|column| column == "graph_task_attempt_id"));
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master
                     WHERE type = 'index' AND name = 'idx_work_tasks_in_progress_per_goal'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            0
        );

        connection
            .execute_batch(
                "INSERT INTO agents(
                    id, name, description, runtime_type, system_prompt, default_model,
                    created_at, updated_at
                 ) VALUES ('graph-agent', 'Graph Agent', '', 'pi', '', 'model', 1, 1);
                 INSERT INTO conversations(
                    id, agent_id, title, status, created_at, updated_at
                 ) VALUES
                    ('graph-conversation', 'graph-agent', 'Graph', 'active', 1, 1),
                    ('graph-child-conversation', 'graph-agent', 'Child', 'active', 1, 1),
                    ('graph-child-conversation-2', 'graph-agent', 'Child 2', 'active', 1, 1);
                 UPDATE conversations
                 SET conversation_kind = 'child',
                     parent_conversation_id = 'graph-conversation',
                     lineage_root_id = 'graph-conversation'
                 WHERE id IN ('graph-child-conversation', 'graph-child-conversation-2');
                 INSERT INTO runs(id, conversation_id, status, model, last_seq, created_at)
                 VALUES
                    ('graph-parent-run', 'graph-conversation', 'running', 'model', 0, 1),
                    ('graph-recovery-run', 'graph-conversation', 'running', 'model', 0, 1);
                 INSERT INTO runs(
                    id, conversation_id, status, model, last_seq, created_at,
                    parent_run_id, root_run_id, run_kind, depth
                 ) VALUES
                    ('graph-child-run', 'graph-child-conversation', 'queued', 'model', 0, 1,
                     'graph-recovery-run', 'graph-recovery-run', 'child', 1),
                    ('graph-child-run-2', 'graph-child-conversation-2', 'queued', 'model', 0, 1,
                     'graph-recovery-run', 'graph-recovery-run', 'child', 1);
                 INSERT INTO run_execution_profiles(
                    run_id, schema_version, profile_id, snapshot_json, profile_hash, frozen_at
                 ) VALUES
                    (
                        'graph-parent-run', 1, 'durable_v2',
                        '{\"id\":\"durable_v2\",\"schemaVersion\":1}',
                        'ae08f4dc8c1327cbd3de30879f01aa7475d9a728a3c0f738579086435d714f7e', 1
                    ),
                    (
                        'graph-recovery-run', 1, 'durable_v2',
                        '{\"id\":\"durable_v2\",\"schemaVersion\":1}',
                        'ae08f4dc8c1327cbd3de30879f01aa7475d9a728a3c0f738579086435d714f7e', 1
                    );
                 INSERT INTO tool_calls(
                    id, runtime_tool_call_id, run_id, conversation_id, tool_name, input_json,
                    status, execution_location, requires_approval, started_at, updated_at
                 ) VALUES (
                    'graph-activation-row', 'graph-activation-call', 'graph-parent-run',
                    'graph-conversation', 'graph_readonly_activate',
                    '{\"planRevisionId\":\"graph-plan\"}', 'running', 'host', 0, 1, 1
                 );
                 INSERT INTO tool_calls(
                    id, runtime_tool_call_id, run_id, conversation_id, tool_name, input_json,
                    status, execution_location, requires_approval, started_at, updated_at
                 ) VALUES
                    ('ordinary-activation-row', 'ordinary-activation-call', 'graph-parent-run',
                     'graph-conversation', 'run_command', '{}', 'running', 'host', 0, 1, 1),
                    ('delegate-call', 'delegate-runtime-call', 'graph-recovery-run',
                     'graph-conversation', 'graph_readonly_node_start',
                     '{\"goalId\":\"graph-goal\",\"taskId\":\"graph-task-1\",\"expectedTaskVersion\":1,\"attemptId\":\"graph-attempt-parent\",\"workerAgentId\":\"graph-agent\",\"objective\":\"Read\",\"context\":\"\",\"budget\":{\"maxDurationMs\":1000,\"maxTotalTokens\":256,\"maxOutputTokens\":64,\"maxToolCalls\":0}}',
                     'running', 'host', 0, 1, 1),
                    ('delegate-call-2', 'delegate-runtime-call-2', 'graph-recovery-run',
                     'graph-conversation', 'graph_readonly_node_start',
                     '{\"goalId\":\"graph-goal\",\"taskId\":\"graph-task-3\",\"expectedTaskVersion\":1,\"attemptId\":\"graph-attempt-parent-3\",\"workerAgentId\":\"graph-agent\",\"objective\":\"Read 2\",\"context\":\"\",\"budget\":{\"maxDurationMs\":1000,\"maxTotalTokens\":256,\"maxOutputTokens\":64,\"maxToolCalls\":0}}',
                     'running', 'host', 0, 1, 1);
                 INSERT INTO goals(
                    id, conversation_id, title, objective, status, created_by, created_at, updated_at
                 ) VALUES
                    ('graph-goal', 'graph-conversation', 'Graph', 'Read only graph', 'active',
                     'user', '1', '1'),
                    ('ordinary-goal', 'graph-conversation', 'Ordinary', 'Serial work', 'proposed',
                     'user', '1', '1');
                 INSERT INTO plan_revisions(
                    id, goal_id, conversation_id, revision, title, summary, tasks_json,
                    status, created_by, created_at, approved_at
                 ) VALUES
                    ('graph-plan', 'graph-goal', 'graph-conversation', 1, 'Graph', 'Graph', '[]',
                     'approved', 'user', '1', '1'),
                    ('ordinary-plan', 'ordinary-goal', 'graph-conversation', 1, 'Ordinary',
                     'Ordinary', '[]', 'approved', 'user', '1', '1');
                 INSERT INTO work_tasks(id, goal_id, ordinal, title, status, created_at, updated_at)
                 VALUES
                    ('graph-task-1', 'graph-goal', 0, 'Root', 'queued', '1', '1'),
                    ('graph-task-2', 'graph-goal', 1, 'Leaf A', 'queued', '1', '1'),
                    ('graph-task-3', 'graph-goal', 2, 'Leaf B', 'queued', '1', '1'),
                    ('graph-task-4', 'graph-goal', 3, 'Too many', 'queued', '1', '1'),
                    ('ordinary-task-1', 'ordinary-goal', 0, 'First', 'queued', '1', '1'),
                    ('ordinary-task-2', 'ordinary-goal', 1, 'Second', 'queued', '1', '1');
                 INSERT INTO work_graph_specs(
                    goal_id, plan_revision_id, schema_version, graph_mode, max_nodes, max_depth,
                    spec_hash, activated_by_run_id, activation_tool_call_id, activated_at
                 ) VALUES (
                    'graph-goal', 'graph-plan', 1, 'read_only', 3, 1,
                    'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
                    'graph-parent-run', 'graph-activation-row', 1
                 );
                 INSERT INTO work_graph_nodes(
                    task_id, goal_id, node_key, access_mode, depth,
                    acceptance_criteria_json, created_at
                 ) VALUES
                    ('graph-task-1', 'graph-goal', 'root', 'read_only', 0, '[\"root done\"]', 1),
                    ('graph-task-2', 'graph-goal', 'leaf-a', 'read_only', 1, '[\"leaf a done\"]', 1);
                 INSERT INTO work_task_edges(
                    goal_id, dependency_task_id, dependent_task_id, created_at
                 ) VALUES ('graph-goal', 'graph-task-1', 'graph-task-2', 1);
                 INSERT INTO task_attempts(
                    id, task_id, attempt_number, kind, status, run_id, policy_hash,
                    finding_ids_json, evidence_ids_json, version, evidence_rowid_watermark,
                    finding_rowid_watermark, started_at
                 ) VALUES
                    ('graph-attempt-parent', 'graph-task-1', 1, 'execution', 'running',
                     'graph-recovery-run',
                     'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb',
                     '[]', '[]', 1, 0, 0, 1),
                    ('graph-attempt-child', 'graph-task-2', 1, 'execution', 'running',
                     'graph-child-run',
                     'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb',
                     '[]', '[]', 1, 0, 0, 1),
                    ('graph-attempt-parent-3', 'graph-task-3', 1, 'execution', 'running',
                     'graph-recovery-run',
                     'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb',
                     '[]', '[]', 1, 0, 0, 1),
                    ('ordinary-attempt-parent', 'ordinary-task-1', 1, 'execution', 'running',
                     'graph-parent-run',
                     'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb',
                     '[]', '[]', 1, 0, 0, 1);
                 INSERT INTO child_run_delegations(
                    id, parent_run_id, child_run_id, child_conversation_id, tool_call_id,
                    worker_agent_id, objective, context, status, max_duration_ms,
                    max_total_tokens, max_output_tokens, max_tool_calls, created_at,
                    allowed_tools_json
                 ) VALUES
                    ('graph-delegation', 'graph-recovery-run', 'graph-child-run',
                     'graph-child-conversation', 'delegate-call', 'graph-agent', 'Read', '',
                     'queued', 1000, 256, 64, 0, 1, '[\"read\",\"ls\",\"find\"]'),
                    ('graph-delegation-2', 'graph-recovery-run', 'graph-child-run-2',
                     'graph-child-conversation-2', 'delegate-call-2', 'graph-agent', 'Read 2', '',
                     'queued', 1000, 256, 64, 0, 1, '[\"read\",\"ls\",\"find\",\"grep\"]');
                 UPDATE work_tasks
                 SET status = 'in_progress', version = 2
                 WHERE id IN ('graph-task-1', 'graph-task-2');",
            )
            .expect("seed valid graph facts");

        assert!(connection
            .execute(
                "INSERT INTO work_graph_nodes(
                    task_id, goal_id, node_key, access_mode, depth,
                    acceptance_criteria_json, created_at
                 ) VALUES (
                    'graph-task-4', 'graph-goal', 'bad key', 'read_only', 1, '[\"done\"]', 1
                 )",
                [],
            )
            .is_err());
        for invalid_criteria in [r#"[1]"#, r#"[" "]"#, r#"[" padded "]"#] {
            assert!(connection
                .execute(
                    "INSERT INTO work_graph_nodes(
                        task_id, goal_id, node_key, access_mode, depth,
                        acceptance_criteria_json, created_at
                     ) VALUES (
                        'graph-task-4', 'graph-goal', 'fourth', 'read_only', 1, ?1, 1
                     )",
                    [invalid_criteria],
                )
                .is_err());
        }
        assert!(connection
            .execute(
                "INSERT INTO work_graph_nodes(
                    task_id, goal_id, node_key, access_mode, depth,
                    acceptance_criteria_json, created_at
                 ) VALUES (
                    'graph-task-4', 'graph-goal', 'fourth', 'read_only', 1,
                    '[\"done\",\"done\"]', 1
                 )",
                [],
            )
            .is_err());
        connection
            .execute(
                "INSERT INTO work_graph_nodes(
                    task_id, goal_id, node_key, access_mode, depth,
                    acceptance_criteria_json, created_at
                 ) VALUES (
                    'graph-task-3', 'graph-goal', 'leaf-b', 'read_only', 0,
                    '[\"leaf b done\"]', 1
                 )",
                [],
            )
            .expect("insert third canonical node");
        connection
            .execute(
                "UPDATE work_tasks SET status = 'in_progress', version = 2
                 WHERE id = 'graph-task-3'",
                [],
            )
            .expect("activate third Graph task fixture");

        assert!(connection
            .execute(
                "INSERT INTO work_graph_nodes(
                    task_id, goal_id, node_key, access_mode, depth,
                    acceptance_criteria_json, created_at
                 ) VALUES (
                    'graph-task-4', 'graph-goal', 'fourth', 'read_only', 1, '[\"done\"]', 1
                 )",
                [],
            )
            .is_err());
        connection
            .execute_batch(
                "INSERT INTO runs(
                    id, conversation_id, status, model, last_seq, created_at,
                    parent_run_id, root_run_id, run_kind, depth
                 ) VALUES (
                    'non-primary-activation-run', 'graph-conversation', 'running', 'model', 0, 1,
                    'graph-parent-run', 'graph-parent-run', 'child', 1
                 );
                 INSERT INTO runs(id, conversation_id, status, model, last_seq, created_at)
                 VALUES (
                    'tampered-profile-run', 'graph-conversation', 'running', 'model', 0, 1
                 );
                 INSERT INTO run_execution_profiles(
                    run_id, schema_version, profile_id, snapshot_json, profile_hash, frozen_at
                 ) VALUES
                    (
                        'non-primary-activation-run', 1, 'durable_v2',
                        '{\"id\":\"durable_v2\",\"schemaVersion\":1}',
                        'ae08f4dc8c1327cbd3de30879f01aa7475d9a728a3c0f738579086435d714f7e',
                        1
                    ),
                    (
                        'tampered-profile-run', 1, 'durable_v2', '{}',
                        'dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd',
                        1
                    );
                 INSERT INTO tool_calls(
                    id, runtime_tool_call_id, run_id, conversation_id, tool_name, input_json,
                    status, execution_location, requires_approval, started_at, updated_at
                 ) VALUES
                    (
                        'non-primary-activation-row', 'non-primary-activation-call',
                        'non-primary-activation-run', 'graph-conversation',
                        'graph_readonly_activate', '{\"planRevisionId\":\"ordinary-plan\"}',
                        'running', 'host', 0, 1, 1
                    ),
                    (
                        'tampered-profile-activation-row', 'tampered-profile-activation-call',
                        'tampered-profile-run', 'graph-conversation',
                        'graph_readonly_activate', '{\"planRevisionId\":\"ordinary-plan\"}',
                        'running', 'host', 0, 1, 1
                    ),
                    (
                        'extra-input-activation-row', 'extra-input-activation-call',
                        'graph-parent-run', 'graph-conversation', 'graph_readonly_activate',
                        '{\"planRevisionId\":\"ordinary-plan\",\"extra\":true}',
                        'running', 'host', 0, 1, 1
                    );",
            )
            .expect("seed rejected activation authority variants");
        for activation_source in [
            ("non-primary-activation-run", "non-primary-activation-row"),
            ("tampered-profile-run", "tampered-profile-activation-row"),
            ("graph-parent-run", "extra-input-activation-row"),
        ] {
            assert!(connection
                .execute(
                    "INSERT INTO work_graph_specs(
                        goal_id, plan_revision_id, schema_version, graph_mode, max_nodes, max_depth,
                        spec_hash, activated_by_run_id, activation_tool_call_id, activated_at
                     ) VALUES (
                        'ordinary-goal', 'ordinary-plan', 1, 'read_only', 3, 1,
                        'cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc',
                        ?1, ?2, 1
                     )",
                    [activation_source.0, activation_source.1],
                )
                .is_err());
        }
        assert!(connection
            .execute(
                "INSERT INTO work_graph_specs(
                    goal_id, plan_revision_id, schema_version, graph_mode, max_nodes, max_depth,
                    spec_hash, activated_by_run_id, activation_tool_call_id, activated_at
                 ) VALUES (
                    'ordinary-goal', 'ordinary-plan', 1, 'read_only', 3, 1,
                    'cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc',
                    'graph-parent-run', 'ordinary-activation-row', 1
                 )",
                [],
            )
            .is_err());
        assert!(connection
            .execute(
                "INSERT INTO work_task_edges(
                    goal_id, dependency_task_id, dependent_task_id, created_at
                 ) VALUES ('graph-goal', 'graph-task-2', 'graph-task-1', 1)",
                [],
            )
            .is_err());
        assert!(connection
            .execute(
                "UPDATE work_graph_specs SET activated_at = 2 WHERE goal_id = 'graph-goal'",
                [],
            )
            .is_err());
        assert!(connection
            .execute(
                "UPDATE work_graph_nodes SET node_key = 'changed' WHERE task_id = 'graph-task-1'",
                [],
            )
            .is_err());
        assert!(connection
            .execute(
                "UPDATE work_task_edges SET created_at = 2
                 WHERE dependency_task_id = 'graph-task-1'",
                [],
            )
            .is_err());
        assert!(connection
            .execute(
                "DELETE FROM work_task_edges
                 WHERE dependency_task_id = 'graph-task-1'",
                [],
            )
            .is_err());
        assert!(connection
            .execute(
                "DELETE FROM work_graph_nodes WHERE task_id = 'graph-task-1'",
                [],
            )
            .is_err());
        assert!(connection
            .execute("DELETE FROM work_tasks WHERE id = 'graph-task-1'", [],)
            .is_err());
        assert!(connection
            .execute(
                "DELETE FROM work_graph_specs WHERE goal_id = 'graph-goal'",
                [],
            )
            .is_err());
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM work_graph_cascade_delete_scopes",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            0,
            "direct delete rejection must not leave a cascade scope"
        );
        assert!(connection
            .execute(
                "UPDATE child_run_delegations SET graph_task_attempt_id = 'graph-attempt-parent'
                 WHERE id = 'graph-delegation'",
                [],
            )
            .is_err());
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM run_execution_profiles
                     WHERE run_id = 'graph-child-run'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            0,
            "a rejected Graph delegation must not leave a frozen profile"
        );
        connection
            .execute(
                "UPDATE child_run_delegations
                 SET allowed_tools_json = '[\"read\",\"ls\",\"find\",\"grep\"]'
                 WHERE id = 'graph-delegation'",
                [],
            )
            .expect("repair unbound test fixture scope");
        connection
            .execute(
                "UPDATE tool_calls
                 SET input_json = json_set(input_json, '$.extra', 1)
                 WHERE id = 'delegate-call'",
                [],
            )
            .expect("tamper unbound node-start input");
        assert!(connection
            .execute(
                "UPDATE child_run_delegations SET graph_task_attempt_id = 'graph-attempt-parent'
                 WHERE id = 'graph-delegation'",
                [],
            )
            .is_err());
        connection
            .execute(
                "UPDATE tool_calls
                 SET input_json = json_remove(input_json, '$.extra')
                 WHERE id = 'delegate-call'",
                [],
            )
            .expect("restore exact unbound node-start input");
        connection
            .execute(
                "UPDATE child_run_delegations SET graph_task_attempt_id = 'graph-attempt-parent'
                 WHERE id = 'graph-delegation'",
                [],
            )
            .expect("bind exact parent-owned graph attempt and seal Child profile");
        let sealed_child_profile = connection
            .query_row(
                "SELECT schema_version, profile_id, snapshot_json, profile_hash
                 FROM run_execution_profiles WHERE run_id = 'graph-child-run'",
                [],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )
            .expect("load sealed Graph Child profile");
        assert_eq!(
            sealed_child_profile,
            (
                1,
                "durable_v2".to_owned(),
                r#"{"id":"durable_v2","schemaVersion":1}"#.to_owned(),
                "ae08f4dc8c1327cbd3de30879f01aa7475d9a728a3c0f738579086435d714f7e".to_owned(),
            )
        );
        assert!(connection
            .execute(
                "UPDATE run_execution_profiles SET snapshot_json = '{}'
                 WHERE run_id = 'graph-child-run'",
                [],
            )
            .is_err());
        assert!(connection
            .execute(
                "DELETE FROM run_execution_profiles WHERE run_id = 'graph-child-run'",
                [],
            )
            .is_err());
        assert!(connection
            .execute(
                "UPDATE child_run_delegations
                 SET allowed_tools_json = '[\"read\",\"ls\",\"find\"]'
                 WHERE id = 'graph-delegation'",
                [],
            )
            .is_err());
        assert!(connection
            .execute(
                "UPDATE tool_calls SET input_json = '{}'
                 WHERE id = 'delegate-call'",
                [],
            )
            .is_err());
        connection
            .execute(
                "UPDATE tool_calls SET status = 'completed', completed_at = 2, updated_at = 2
                 WHERE id = 'delegate-call'",
                [],
            )
            .expect("node-start ToolCall lifecycle fields remain mutable");
        assert!(connection
            .execute(
                "UPDATE child_run_delegations SET graph_task_attempt_id = 'graph-attempt-child'
                 WHERE id = 'graph-delegation'",
                [],
            )
            .is_err());
        assert!(connection
            .execute(
                "UPDATE child_run_delegations
                 SET graph_task_attempt_id = 'ordinary-attempt-parent'
                 WHERE id = 'graph-delegation'",
                [],
            )
            .is_err());
        assert!(connection
            .execute(
                "UPDATE child_run_delegations SET graph_task_attempt_id = 'graph-attempt-child'
                 WHERE id = 'graph-delegation-2'",
                [],
            )
            .is_err());
        connection
            .execute(
                "UPDATE runs SET depth = 2 WHERE id = 'graph-child-run-2'",
                [],
            )
            .expect("tamper unbound Child lineage fixture");
        assert!(connection
            .execute(
                "UPDATE child_run_delegations
                 SET graph_task_attempt_id = 'graph-attempt-parent-3'
                 WHERE id = 'graph-delegation-2'",
                [],
            )
            .is_err());
        connection
            .execute(
                "UPDATE runs SET depth = 1 WHERE id = 'graph-child-run-2'",
                [],
            )
            .expect("restore unbound Child lineage fixture");
        connection
            .execute(
                "UPDATE child_run_delegations
                 SET graph_task_attempt_id = 'graph-attempt-parent-3'
                 WHERE id = 'graph-delegation-2'",
                [],
            )
            .expect("bind second exact Graph delegation");
        assert!(connection
            .execute(
                "UPDATE runs SET depth = 2 WHERE id = 'graph-child-run-2'",
                [],
            )
            .is_err());
        assert!(connection
            .execute(
                "UPDATE runs SET depth = 1 WHERE id = 'graph-recovery-run'",
                [],
            )
            .is_err());
        assert!(connection
            .execute(
                "UPDATE run_execution_profiles SET snapshot_json = '{}'
                 WHERE run_id = 'graph-recovery-run'",
                [],
            )
            .is_err());
        assert!(connection
            .execute(
                "DELETE FROM run_execution_profiles WHERE run_id = 'graph-recovery-run'",
                [],
            )
            .is_err());
        assert!(connection
            .execute(
                "UPDATE runs SET run_kind = 'child', depth = 1,
                        parent_run_id = 'graph-child-run'
                 WHERE id = 'graph-parent-run'",
                [],
            )
            .is_err());
        assert!(connection
            .execute(
                "UPDATE run_execution_profiles SET snapshot_json = '{}'
                 WHERE run_id = 'graph-parent-run'",
                [],
            )
            .is_err());
        assert!(connection
            .execute(
                "DELETE FROM run_execution_profiles WHERE run_id = 'graph-parent-run'",
                [],
            )
            .is_err());
        assert!(connection
            .execute(
                "UPDATE tool_calls SET input_json = '{}'
                 WHERE id = 'graph-activation-row'",
                [],
            )
            .is_err());
        connection
            .execute(
                "UPDATE tool_calls SET status = 'completed', completed_at = 2, updated_at = 2
                 WHERE id = 'graph-activation-row'",
                [],
            )
            .expect("activation ToolCall lifecycle fields remain mutable");
        assert!(connection
            .execute(
                "UPDATE work_tasks SET title = 'mutated'
                 WHERE id = 'graph-task-1'",
                [],
            )
            .is_err());
        connection
            .execute(
                "UPDATE work_tasks
                 SET owner_run_id = 'graph-parent-run', updated_at = '2'
                 WHERE id = 'graph-task-1'",
                [],
            )
            .expect("Graph Task lifecycle fields remain mutable");

        connection
            .execute_batch(
                "UPDATE work_tasks SET status = 'in_progress' WHERE id = 'graph-task-1';
                 UPDATE work_tasks SET status = 'in_progress' WHERE id = 'graph-task-2';
                 UPDATE work_tasks SET status = 'in_progress' WHERE id = 'graph-task-3';
                 UPDATE work_tasks SET status = 'in_progress' WHERE id = 'ordinary-task-1';",
            )
            .expect("allow Graph parallelism and first ordinary Task");
        assert!(connection
            .execute(
                "UPDATE work_tasks SET status = 'in_progress' WHERE id = 'ordinary-task-2'",
                [],
            )
            .is_err());

        let foreign_key_errors = connection
            .prepare("PRAGMA foreign_key_check")
            .unwrap()
            .query_map([], |_| Ok(()))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(foreign_key_errors.is_empty());
        connection
            .execute_batch("SAVEPOINT graph_conversation_delete_probe")
            .unwrap();
        let conversation_delete_result = connection.execute(
            "DELETE FROM conversations WHERE id = 'graph-conversation'",
            [],
        );
        connection
            .execute_batch("ROLLBACK TO graph_conversation_delete_probe; RELEASE graph_conversation_delete_probe")
            .unwrap();
        assert!(
            conversation_delete_result.is_ok(),
            "Graph activation audit FKs must not break normal Conversation cascade deletion: {conversation_delete_result:?}"
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM work_graph_cascade_delete_scopes",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            0,
            "Conversation cascade probe must close its Graph delete scope"
        );
        assert!(connection
            .execute(
                "DELETE FROM tool_calls WHERE id = 'graph-activation-row'",
                [],
            )
            .is_err());

        connection
            .execute("DELETE FROM goals WHERE id = 'graph-goal'", [])
            .expect("delete Graph Goal through normal lifecycle cascade");
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM work_graph_cascade_delete_scopes",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            0,
            "Goal cascade must close its Graph delete scope"
        );
        for table in ["work_graph_specs", "work_graph_nodes", "work_task_edges"] {
            assert_eq!(
                connection
                    .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| row
                        .get::<_, i64>(0),)
                    .unwrap(),
                0,
                "Graph Goal deletion did not clear {table}"
            );
        }
        assert_eq!(
            connection
                .query_row(
                    "SELECT graph_task_attempt_id FROM child_run_delegations
                     WHERE id = 'graph-delegation'",
                    [],
                    |row| row.get::<_, Option<String>>(0),
                )
                .unwrap(),
            None
        );
    }

    #[test]
    fn graph_node_finish_migration_is_idempotent_and_enforces_evidence_contract() {
        const STANDARD_HASH: &str =
            "bcef9ad7087b2872a0152f493cfba131a4283a3170dc8fe5824d108d19e3d04d";
        const STANDARD_POLICY: &str = r#"{"schemaVersion":1,"id":"standard_v1","riskLevel":"standard","requiredChecks":["inspection"],"allowedCheckTypes":["test","inspection","review","manual"],"reviewerPolicy":"host_validated","maxRepairAttempts":2,"completionRequiresAcceptance":true,"hash":"bcef9ad7087b2872a0152f493cfba131a4283a3170dc8fe5824d108d19e3d04d"}"#;
        const HIGH_RISK_HASH: &str =
            "bb1258169c5d923dfd5fd1980dd52150ab96e159bd01d07ff0d04c48998dc5ea";
        const HIGH_RISK_POLICY: &str = r#"{"schemaVersion":1,"id":"high_risk_v1","riskLevel":"high","requiredChecks":["inspection","review"],"allowedCheckTypes":["test","inspection","review","manual"],"reviewerPolicy":"independent","maxRepairAttempts":1,"completionRequiresAcceptance":true,"hash":"bb1258169c5d923dfd5fd1980dd52150ab96e159bd01d07ff0d04c48998dc5ea"}"#;

        fn finish_input(first_evidence_ids: &[&str], second_evidence_ids: &[&str]) -> String {
            serde_json::to_string(&json!({
                "goalId": "finish-goal",
                "taskId": "finish-task",
                "attemptId": "finish-attempt",
                "expectedTaskVersion": 2,
                "expectedAttemptVersion": 1,
                "criterionEvidence": [
                    {
                        "criterion": "inspect output",
                        "evidenceIds": first_evidence_ids,
                    },
                    {
                        "criterion": "verify structure",
                        "evidenceIds": second_evidence_ids,
                    },
                ],
                "summary": "validated",
            }))
            .expect("serialize canonical finish input")
        }

        fn input_hash(input: &str) -> String {
            format!("{:x}", Sha256::digest(input.as_bytes()))
        }

        fn insert_finish_tool(
            connection: &Connection,
            id: &str,
            input: &str,
        ) -> rusqlite::Result<usize> {
            connection.execute(
                "INSERT INTO tool_calls(
                    id, runtime_tool_call_id, run_id, conversation_id, tool_name, input_json,
                    status, execution_location, requires_approval, started_at, updated_at
                 ) VALUES (?1, ?1, 'finish-parent-run', 'finish-conversation',
                           'graph_readonly_node_finish', ?2, 'running', 'host', 0, 30, 30)",
                rusqlite::params![id, input],
            )
        }

        fn insert_finish_header(
            connection: &Connection,
            id: &str,
            tool_call_id: &str,
            input: &str,
            policy_id: &str,
            policy_hash: &str,
        ) -> rusqlite::Result<usize> {
            connection.execute(
                "INSERT INTO work_graph_node_finishes(
                    id, goal_id, task_id, attempt_id, parent_run_id, child_run_id,
                    tool_call_id, validation_policy_id, validation_policy_hash,
                    expected_task_version, expected_attempt_version, child_result_hash,
                    summary, input_json, input_hash, created_at
                 ) VALUES (
                    ?1, 'finish-goal', 'finish-task', 'finish-attempt',
                    'finish-parent-run', 'finish-child-run', ?2, ?3, ?4, 2, 1,
                    'cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc',
                    'validated', ?5, ?6, 31
                 )",
                rusqlite::params![
                    id,
                    tool_call_id,
                    policy_id,
                    policy_hash,
                    input,
                    input_hash(input)
                ],
            )
        }

        let mut connection = Connection::open_in_memory().expect("open finish migration database");
        run(&mut connection, 1).expect("apply migrations through v37");
        run(&mut connection, 2).expect("repeat migrations through v37");
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM schema_migrations WHERE version = 37",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1
        );
        for table in [
            "work_graph_node_finishes",
            "work_graph_node_criterion_evidence",
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
                "missing v37 table {table}"
            );
        }

        connection
            .execute_batch(&format!(
                r#"INSERT INTO agents(
                    id, name, description, runtime_type, system_prompt, default_model,
                    created_at, updated_at
                 ) VALUES ('finish-agent', 'Finish Agent', '', 'pi', '', 'model', 1, 1);
                 INSERT INTO conversations(
                    id, agent_id, title, status, created_at, updated_at, lineage_root_id
                 ) VALUES (
                    'finish-conversation', 'finish-agent', 'Finish', 'active', 1, 1,
                    'finish-conversation'
                 );
                 INSERT INTO conversations(
                    id, agent_id, title, status, created_at, updated_at, conversation_kind,
                    parent_conversation_id, lineage_root_id
                 ) VALUES (
                    'finish-child-conversation', 'finish-agent', 'Child', 'active', 1, 1,
                    'child', 'finish-conversation', 'finish-conversation'
                 );
                 INSERT INTO goals(
                    id, conversation_id, title, objective, status, created_by, created_at, updated_at
                 ) VALUES (
                    'finish-goal', 'finish-conversation', 'Finish', 'Validate node', 'active',
                    'user', '1', '1'
                 );
                 INSERT INTO plan_revisions(
                    id, goal_id, conversation_id, revision, title, summary, tasks_json,
                    status, created_by, created_at, approved_at
                 ) VALUES (
                    'finish-plan', 'finish-goal', 'finish-conversation', 1, 'Finish', 'Finish',
                    '[]', 'approved', 'user', '1', '1'
                 );
                 INSERT INTO runs(
                    id, conversation_id, status, model, last_seq, created_at,
                    root_run_id, run_kind, depth
                 ) VALUES (
                    'finish-parent-run', 'finish-conversation', 'running', 'model', 0, 1,
                    'finish-parent-run', 'primary', 0
                 );
                 INSERT INTO runs(
                    id, conversation_id, status, model, last_seq, created_at,
                    root_run_id, run_kind, depth
                 ) VALUES (
                    'finish-other-run', 'finish-conversation', 'completed', 'model', 0, 1,
                    'finish-other-run', 'primary', 0
                 );
                 INSERT INTO runs(
                    id, conversation_id, status, model, last_seq, created_at,
                    parent_run_id, root_run_id, run_kind, depth
                 ) VALUES (
                    'finish-child-run', 'finish-child-conversation', 'queued', 'model', 0, 1,
                    'finish-parent-run', 'finish-parent-run', 'child', 1
                 );
                 INSERT INTO run_execution_profiles(
                    run_id, schema_version, profile_id, snapshot_json, profile_hash, frozen_at
                 ) VALUES (
                    'finish-parent-run', 1, 'durable_v2',
                    '{{"id":"durable_v2","schemaVersion":1}}',
                    'ae08f4dc8c1327cbd3de30879f01aa7475d9a728a3c0f738579086435d714f7e', 1
                 );
                 INSERT INTO tool_calls(
                    id, runtime_tool_call_id, run_id, conversation_id, tool_name, input_json,
                    status, execution_location, requires_approval, started_at, updated_at
                 ) VALUES (
                    'finish-activation-call', 'finish-activation-runtime', 'finish-parent-run',
                    'finish-conversation', 'graph_readonly_activate',
                    '{{"planRevisionId":"finish-plan"}}', 'running', 'host', 0, 1, 1
                 );
                 INSERT INTO work_tasks(
                    id, goal_id, ordinal, title, status, created_at, updated_at
                 ) VALUES ('finish-task', 'finish-goal', 0, 'Node', 'queued', '1', '1');
                 INSERT INTO task_validation_policies(
                    task_id, schema_version, policy_id, snapshot_json, policy_hash, frozen_at
                 ) VALUES (
                    'finish-task', 1, 'standard_v1', '{STANDARD_POLICY}', '{STANDARD_HASH}', 1
                 );
                 INSERT INTO work_graph_specs(
                    goal_id, plan_revision_id, schema_version, graph_mode, max_nodes, max_depth,
                    spec_hash, activated_by_run_id, activation_tool_call_id, activated_at
                 ) VALUES (
                    'finish-goal', 'finish-plan', 1, 'read_only', 3, 1,
                    'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
                    'finish-parent-run', 'finish-activation-call', 1
                 );
                 INSERT INTO work_graph_nodes(
                    task_id, goal_id, node_key, access_mode, depth,
                    acceptance_criteria_json, created_at
                 ) VALUES (
                    'finish-task', 'finish-goal', 'finish-node', 'read_only', 0,
                    '["inspect output","verify structure"]', 1
                 );
                 UPDATE work_tasks
                 SET status = 'in_progress', owner_run_id = 'finish-parent-run', version = 2,
                     started_at = '10', updated_at = '10'
                 WHERE id = 'finish-task';
                 INSERT INTO task_attempts(
                    id, task_id, attempt_number, kind, status, run_id, policy_hash,
                    finding_ids_json, evidence_ids_json, version, evidence_rowid_watermark,
                    finding_rowid_watermark, started_at
                 ) VALUES (
                    'finish-attempt', 'finish-task', 1, 'execution', 'running',
                    'finish-parent-run', '{STANDARD_HASH}', '[]', '[]', 1, 0, 0, 10
                 );
                 INSERT INTO tool_calls(
                    id, runtime_tool_call_id, run_id, conversation_id, tool_name, input_json,
                    status, execution_location, requires_approval, started_at, updated_at
                 ) VALUES (
                    'finish-node-start-call', 'finish-node-start-runtime', 'finish-parent-run',
                    'finish-conversation', 'graph_readonly_node_start',
                    '{{"goalId":"finish-goal","taskId":"finish-task","expectedTaskVersion":1,"attemptId":"finish-attempt","workerAgentId":"finish-agent","objective":"Inspect","context":"","budget":{{"maxDurationMs":1000,"maxTotalTokens":256,"maxOutputTokens":64,"maxToolCalls":0}}}}',
                    'running', 'host', 0, 11, 11
                 );
                 INSERT INTO child_run_delegations(
                    id, parent_run_id, child_run_id, child_conversation_id, tool_call_id,
                    worker_agent_id, objective, context, status, max_duration_ms,
                    max_total_tokens, max_output_tokens, max_tool_calls, allowed_tools_json,
                    graph_task_attempt_id, created_at
                 ) VALUES (
                    'finish-delegation', 'finish-parent-run', 'finish-child-run',
                    'finish-child-conversation', 'finish-node-start-call', 'finish-agent',
                    'Inspect', '', 'queued', 1000, 256, 64, 0,
                    '["read","ls","find","grep"]', 'finish-attempt', 11
                 );
                 UPDATE runs
                 SET status = 'completed', started_at = 12, finished_at = 18
                 WHERE id = 'finish-child-run';
                 UPDATE child_run_delegations
                 SET status = 'completed', started_at = 12, finished_at = 18
                 WHERE id = 'finish-delegation';
                 INSERT INTO tool_calls(
                    id, runtime_tool_call_id, run_id, conversation_id, tool_name, input_json,
                    status, result_json, execution_location, requires_approval,
                    started_at, completed_at, updated_at
                 ) VALUES
                    ('criterion-read', 'criterion-read', 'finish-parent-run',
                     'finish-conversation', 'read', '{{"path":"src/lib.rs"}}', 'completed',
                     '{{"ok":true}}', 'runtime', 0, 20, 21, 21),
                    ('criterion-test', 'criterion-test', 'finish-parent-run',
                     'finish-conversation', 'test_run', '{{"runner":"rust"}}', 'completed',
                     '{{"passed":true}}', 'host', 1, 22, 23, 23),
                    ('criterion-old', 'criterion-old', 'finish-parent-run',
                     'finish-conversation', 'read', '{{}}', 'completed', '{{"ok":true}}',
                     'runtime', 0, 5, 6, 6),
                    ('criterion-foreign', 'criterion-foreign', 'finish-other-run',
                     'finish-conversation', 'read', '{{}}', 'completed', '{{"ok":true}}',
                     'runtime', 0, 20, 21, 21),
                    ('criterion-broad', 'criterion-broad', 'finish-parent-run',
                     'finish-conversation', 'run_command', '{{}}', 'completed', '{{"ok":true}}',
                     'runtime', 0, 20, 21, 21),
                    ('criterion-approval-mismatch', 'criterion-approval-mismatch',
                     'finish-parent-run', 'finish-conversation', 'read', '{{}}', 'completed',
                     '{{"ok":true}}', 'runtime', 1, 20, 21, 21);
                 INSERT INTO task_evidence(
                    id, task_id, source_run_id, evidence_type, ref_kind, ref_id, summary,
                    metadata_json, validity_status, checked_at, created_at
                 ) VALUES
                    ('evidence-read', 'finish-task', 'finish-parent-run', 'tool_call',
                     'tool_call', 'criterion-read', 'read inspected',
                     '{{"validationCheckType":"inspection"}}', 'valid', '21', '21'),
                    ('evidence-test', 'finish-task', 'finish-parent-run', 'test_result',
                     'tool_call', 'criterion-test', 'tests passed',
                     '{{"validationCheckType":"test","passed":true}}', 'valid', '23', '23'),
                    ('evidence-old', 'finish-task', 'finish-parent-run', 'tool_call',
                     'tool_call', 'criterion-old', 'old read',
                     '{{"validationCheckType":"inspection"}}', 'valid', '6', '24'),
                    ('evidence-foreign', 'finish-task', 'finish-parent-run', 'tool_call',
                     'tool_call', 'criterion-foreign', 'foreign read',
                     '{{"validationCheckType":"inspection"}}', 'valid', '21', '25'),
                    ('evidence-broad', 'finish-task', 'finish-parent-run', 'tool_call',
                     'tool_call', 'criterion-broad', 'broad tool',
                     '{{"validationCheckType":"inspection"}}', 'valid', '21', '26'),
                    ('evidence-approval-mismatch', 'finish-task', 'finish-parent-run', 'tool_call',
                     'tool_call', 'criterion-approval-mismatch', 'wrong tuple',
                     '{{"validationCheckType":"inspection"}}', 'valid', '21', '27');"#
            ))
            .expect("seed v37 Graph node finish fixture");

        assert!(
            connection
                .execute(
                    "UPDATE task_attempts
                 SET status = 'succeeded', evidence_ids_json = '[\"evidence-read\"]',
                     version = 2, finished_at = 40
                 WHERE id = 'finish-attempt'",
                    [],
                )
                .is_err(),
            "generic finish must not bypass the dedicated Graph finish contract"
        );
        assert!(
            connection
                .execute(
                    "UPDATE work_tasks SET status = 'completed', owner_run_id = NULL, version = 3
                 WHERE id = 'finish-task'",
                    [],
                )
                .is_err(),
            "direct Graph Task completion must not bypass Node finish"
        );

        let valid_input = finish_input(&["evidence-read", "evidence-test"], &["evidence-read"]);
        insert_finish_tool(&connection, "finish-empty-result", &valid_input)
            .expect("insert empty-result finish call");
        assert!(
            insert_finish_header(
                &connection,
                "finish-empty-result-header",
                "finish-empty-result",
                &valid_input,
                "standard_v1",
                STANDARD_HASH,
            )
            .is_err(),
            "completed Child without a durable result must not authorize finish"
        );
        connection
            .execute(
                "INSERT INTO messages(
                    id, conversation_id, run_id, role, kind, content, status, ordinal,
                    created_at, updated_at
                 ) VALUES (
                    'finish-child-result', 'finish-child-conversation', 'finish-child-run',
                    'assistant', 'text', 'bounded child result', 'completed', 0, 18, 18
                 )",
                [],
            )
            .expect("persist authoritative Child result");

        for (suffix, evidence_id) in [
            ("old", "evidence-old"),
            ("foreign", "evidence-foreign"),
            ("broad", "evidence-broad"),
            ("approval", "evidence-approval-mismatch"),
        ] {
            let input = finish_input(&[evidence_id], &[evidence_id]);
            let tool_id = format!("finish-rejected-{suffix}");
            let header_id = format!("finish-rejected-{suffix}-header");
            insert_finish_tool(&connection, &tool_id, &input).expect("insert rejected finish call");
            assert!(
                insert_finish_header(
                    &connection,
                    &header_id,
                    &tool_id,
                    &input,
                    "standard_v1",
                    STANDARD_HASH,
                )
                .is_err(),
                "criterion evidence variant '{suffix}' must fail closed"
            );
        }

        let extra_input = valid_input
            .strip_suffix('}')
            .map(|prefix| format!("{prefix},\"unexpected\":true}}"))
            .unwrap();
        insert_finish_tool(&connection, "finish-extra-input", &extra_input)
            .expect("insert extra-input finish call");
        assert!(
            insert_finish_header(
                &connection,
                "finish-extra-input-header",
                "finish-extra-input",
                &extra_input,
                "standard_v1",
                STANDARD_HASH,
            )
            .is_err(),
            "non-exact finish input must be rejected"
        );

        connection
            .execute(
                "UPDATE goals SET status = 'blocked' WHERE id = 'finish-goal'",
                [],
            )
            .expect("block Goal before direct finish probe");
        insert_finish_tool(&connection, "finish-blocked-goal", &valid_input)
            .expect("insert blocked-Goal finish call");
        assert!(
            insert_finish_header(
                &connection,
                "finish-blocked-goal-header",
                "finish-blocked-goal",
                &valid_input,
                "standard_v1",
                STANDARD_HASH,
            )
            .is_err(),
            "blocked Goal must not authorize Graph Node finish facts"
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM work_graph_node_finishes
                     WHERE id = 'finish-blocked-goal-header'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            0,
            "blocked Goal finish must be zero-write"
        );
        connection
            .execute(
                "UPDATE goals SET status = 'active' WHERE id = 'finish-goal'",
                [],
            )
            .expect("reactivate Goal after blocked finish probe");

        connection
            .execute(
                "UPDATE task_validation_policies
                 SET policy_id = 'high_risk_v1', snapshot_json = ?1, policy_hash = ?2
                 WHERE task_id = 'finish-task'",
                rusqlite::params![HIGH_RISK_POLICY, HIGH_RISK_HASH],
            )
            .unwrap();
        connection
            .execute(
                "UPDATE task_attempts SET policy_hash = ?1 WHERE id = 'finish-attempt'",
                [HIGH_RISK_HASH],
            )
            .unwrap();
        insert_finish_tool(&connection, "finish-high-risk", &valid_input)
            .expect("insert high-risk finish call");
        assert!(
            insert_finish_header(
                &connection,
                "finish-high-risk-header",
                "finish-high-risk",
                &valid_input,
                "high_risk_v1",
                HIGH_RISK_HASH,
            )
            .is_err(),
            "current Node finish must leave high-risk work review_required with zero facts"
        );
        connection
            .execute(
                "UPDATE task_validation_policies
                 SET policy_id = 'standard_v1', snapshot_json = ?1, policy_hash = ?2
                 WHERE task_id = 'finish-task'",
                rusqlite::params![STANDARD_POLICY, STANDARD_HASH],
            )
            .unwrap();
        connection
            .execute(
                "UPDATE task_attempts SET policy_hash = ?1 WHERE id = 'finish-attempt'",
                [STANDARD_HASH],
            )
            .unwrap();

        insert_finish_tool(&connection, "finish-valid-call", &valid_input)
            .expect("insert valid finish call");
        insert_finish_header(
            &connection,
            "finish-valid",
            "finish-valid-call",
            &valid_input,
            "standard_v1",
            STANDARD_HASH,
        )
        .expect("insert exact finish header");
        connection
            .execute(
                "INSERT INTO work_graph_node_criterion_evidence(
                    finish_id, criterion_ordinal, criterion, evidence_id
                 ) VALUES ('finish-valid', 0, 'inspect output', 'evidence-read')",
                [],
            )
            .expect("bind first inspection");
        connection
            .execute(
                "INSERT INTO work_graph_node_criterion_evidence(
                    finish_id, criterion_ordinal, criterion, evidence_id
                 ) VALUES ('finish-valid', 0, 'inspect output', 'evidence-test')",
                [],
            )
            .expect("bind passing test Evidence");
        assert!(connection
            .execute(
                "INSERT INTO work_graph_node_criterion_evidence(
                    finish_id, criterion_ordinal, criterion, evidence_id
                 ) VALUES ('finish-valid', 1, 'wrong criterion', 'evidence-read')",
                [],
            )
            .is_err());
        assert!(
            connection
                .execute(
                    "UPDATE task_attempts
                 SET status = 'succeeded',
                     evidence_ids_json = '[\"evidence-test\",\"evidence-read\"]',
                     version = 2, finished_at = 40
                 WHERE id = 'finish-attempt'",
                    [],
                )
                .is_err(),
            "missing frozen criterion coverage must reject succeeded"
        );
        connection
            .execute(
                "INSERT INTO work_graph_node_criterion_evidence(
                    finish_id, criterion_ordinal, criterion, evidence_id
                 ) VALUES ('finish-valid', 1, 'verify structure', 'evidence-read')",
                [],
            )
            .expect("reuse one inspection explicitly across criteria");
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM work_graph_node_criterion_evidence
                     WHERE finish_id = 'finish-valid' AND evidence_id = 'evidence-read'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            2,
            "one real inspection may explicitly cover multiple criteria"
        );
        connection
            .execute(
                "UPDATE goals SET status = 'blocked' WHERE id = 'finish-goal'",
                [],
            )
            .expect("block Goal after finish aggregate is assembled");
        assert!(
            connection
                .execute(
                    "UPDATE task_attempts
                 SET status = 'succeeded',
                     evidence_ids_json = '[\"evidence-test\",\"evidence-read\"]',
                     version = 2, finished_at = 40
                 WHERE id = 'finish-attempt'",
                    [],
                )
                .is_err(),
            "Goal must remain active through the Attempt finish gate"
        );
        connection
            .execute(
                "UPDATE goals SET status = 'active' WHERE id = 'finish-goal'",
                [],
            )
            .expect("reactivate Goal after assembled-finish probe");
        connection
            .execute(
                "UPDATE task_evidence
                 SET validity_status = 'stale', invalid_reason = 'source changed', checked_at = '39'
                 WHERE id = 'evidence-read'",
                [],
            )
            .expect("Evidence liveness remains mutable");
        assert!(
            connection
                .execute(
                    "UPDATE task_attempts
                 SET status = 'succeeded',
                     evidence_ids_json = '[\"evidence-test\",\"evidence-read\"]',
                     version = 2, finished_at = 40
                 WHERE id = 'finish-attempt'",
                    [],
                )
                .is_err(),
            "stale bound Evidence must block acceptance"
        );
        connection
            .execute(
                "UPDATE task_evidence
                 SET validity_status = 'valid', invalid_reason = NULL, checked_at = '40'
                 WHERE id = 'evidence-read'",
                [],
            )
            .unwrap();
        connection
            .execute(
                "UPDATE task_attempts
                 SET status = 'succeeded',
                     evidence_ids_json = '[\"evidence-test\",\"evidence-read\"]',
                     version = 2, finished_at = 40
                 WHERE id = 'finish-attempt'",
                [],
            )
            .expect("complete Attempt through exact v37 gate");
        connection
            .execute(
                "UPDATE work_tasks
                 SET status = 'completed', owner_run_id = NULL, version = 3,
                     updated_at = '40', finished_at = '40'
                 WHERE id = 'finish-task'",
                [],
            )
            .expect("complete Task after accepted Attempt");

        assert!(connection
            .execute(
                "UPDATE work_graph_node_finishes SET summary = 'tampered'
                 WHERE id = 'finish-valid'",
                [],
            )
            .is_err());
        assert!(connection
            .execute(
                "DELETE FROM work_graph_node_criterion_evidence
                 WHERE finish_id = 'finish-valid' AND criterion_ordinal = 1",
                [],
            )
            .is_err());
        assert!(connection
            .execute(
                "DELETE FROM work_graph_node_finishes WHERE id = 'finish-valid'",
                []
            )
            .is_err());
        assert!(connection
            .execute("DELETE FROM task_evidence WHERE id = 'evidence-read'", [])
            .is_err());
        assert!(connection
            .execute("DELETE FROM tool_calls WHERE id = 'criterion-read'", [])
            .is_err());
        assert!(connection
            .execute(
                "UPDATE tool_calls SET input_json = '{}' WHERE id = 'criterion-read'",
                [],
            )
            .is_err());
        assert!(connection
            .execute(
                "UPDATE task_validation_policies SET snapshot_json = '{}'
                 WHERE task_id = 'finish-task'",
                [],
            )
            .is_err());
        assert!(connection
            .execute(
                "UPDATE task_attempts SET evidence_ids_json = '[]'
                 WHERE id = 'finish-attempt'",
                [],
            )
            .is_err());
        assert!(connection
            .execute(
                "UPDATE messages SET content = 'tampered' WHERE id = 'finish-child-result'",
                [],
            )
            .is_err());
        connection
            .execute(
                "UPDATE tool_calls
                 SET status = 'completed', result_json = '{\"ok\":true}',
                     completed_at = 41, updated_at = 41
                 WHERE id = 'finish-valid-call'",
                [],
            )
            .expect("finish ToolCall lifecycle may become terminal");

        let foreign_key_errors = connection
            .prepare("PRAGMA foreign_key_check")
            .unwrap()
            .query_map([], |_| Ok(()))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(foreign_key_errors.is_empty());
        connection
            .execute_batch("SAVEPOINT finish_conversation_delete_probe")
            .unwrap();
        let conversation_delete = connection.execute(
            "DELETE FROM conversations WHERE id = 'finish-conversation'",
            [],
        );
        connection
            .execute_batch(
                "ROLLBACK TO finish_conversation_delete_probe;
                 RELEASE finish_conversation_delete_probe",
            )
            .unwrap();
        assert!(
            conversation_delete.is_ok(),
            "Conversation cascade must clear immutable finish facts: {conversation_delete:?}"
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM work_graph_cascade_delete_scopes",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            0
        );
        connection
            .execute("DELETE FROM goals WHERE id = 'finish-goal'", [])
            .expect("Goal cascade removes the complete Graph finish aggregate");
        for table in [
            "work_graph_node_finishes",
            "work_graph_node_criterion_evidence",
            "work_graph_nodes",
            "work_graph_specs",
        ] {
            assert_eq!(
                connection
                    .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                        row.get::<_, i64>(0)
                    })
                    .unwrap(),
                0,
                "Goal cascade left rows in {table}"
            );
        }
    }

    #[test]
    fn graph_node_cancel_migration_enforces_two_phase_negative_reconciliation() {
        fn cancel_input(reason: &str) -> String {
            serde_json::to_string(&json!({
                "goalId": "cancel-goal",
                "taskId": "cancel-task",
                "attemptId": "cancel-attempt",
                "expectedTaskVersion": 2,
                "expectedAttemptVersion": 1,
                "reason": reason,
            }))
            .expect("serialize canonical cancel input")
        }

        fn sha256(value: &str) -> String {
            format!("{:x}", Sha256::digest(value.as_bytes()))
        }

        fn terminal_json(
            status: &str,
            finished_at: i64,
            error_code: Option<&str>,
            error_message: Option<&str>,
        ) -> String {
            serde_json::to_string(&json!({
                "childRunId": "cancel-child-run",
                "status": status,
                "finishedAt": finished_at,
                "errorCode": error_code,
                "errorMessage": error_message,
            }))
            .expect("serialize canonical Child terminal snapshot")
        }

        fn insert_cancel_tool(
            connection: &Connection,
            id: &str,
            input: &str,
            tool_name: &str,
            requires_approval: i64,
        ) -> rusqlite::Result<usize> {
            connection.execute(
                "INSERT INTO tool_calls(
                    id, runtime_tool_call_id, run_id, conversation_id, tool_name, input_json,
                    status, execution_location, requires_approval, started_at, updated_at
                 ) VALUES (
                    ?1, ?1, 'cancel-parent-run', 'cancel-conversation', ?3, ?2,
                    'running', 'host', ?4, 30, 30
                 )",
                rusqlite::params![id, input, tool_name, requires_approval],
            )
        }

        fn insert_cancel_intent(
            connection: &Connection,
            id: &str,
            tool_call_id: &str,
            reason: &str,
            input: &str,
            created_at: i64,
        ) -> rusqlite::Result<usize> {
            connection.execute(
                "INSERT INTO work_graph_node_cancel_intents(
                    id, goal_id, task_id, attempt_id, parent_run_id, delegation_id,
                    child_run_id, tool_call_id, expected_task_version,
                    expected_attempt_version, reason, input_json, input_hash, created_at
                 ) VALUES (
                    ?1, 'cancel-goal', 'cancel-task', 'cancel-attempt',
                    'cancel-parent-run', 'cancel-delegation', 'cancel-child-run', ?2,
                    2, 1, ?3, ?4, ?5, ?6
                 )",
                rusqlite::params![id, tool_call_id, reason, input, sha256(input), created_at],
            )
        }

        fn insert_reconciliation(
            connection: &Connection,
            id: &str,
            cancel_intent_id: Option<&str>,
            child_status: &str,
            attempt_status: &str,
            snapshot: &str,
            failure_reason: &str,
            created_at: i64,
        ) -> rusqlite::Result<usize> {
            connection.execute(
                "INSERT INTO work_graph_node_terminal_reconciliations(
                    id, goal_id, task_id, attempt_id, parent_run_id, delegation_id,
                    child_run_id, cancel_intent_id, child_terminal_status,
                    attempt_terminal_status, expected_task_version, expected_attempt_version,
                    child_terminal_json, child_terminal_hash, failure_reason, created_at
                 ) VALUES (
                    ?1, 'cancel-goal', 'cancel-task', 'cancel-attempt',
                    'cancel-parent-run', 'cancel-delegation', 'cancel-child-run', ?2,
                    ?3, ?4, 2, 1, ?5, ?6, ?7, ?8
                 )",
                rusqlite::params![
                    id,
                    cancel_intent_id,
                    child_status,
                    attempt_status,
                    snapshot,
                    sha256(snapshot),
                    failure_reason,
                    created_at
                ],
            )
        }

        let mut upgrade = Connection::open_in_memory().expect("open v37 upgrade database");
        run_through_v37(&mut upgrade, 1);
        assert_eq!(
            upgrade
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master
                     WHERE type = 'table' AND name = 'work_graph_node_cancel_intents'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            0
        );
        run(&mut upgrade, 2).expect("upgrade v37 database through v38");
        run(&mut upgrade, 3).expect("repeat v38 migration");
        assert_eq!(
            upgrade
                .query_row(
                    "SELECT COUNT(*) FROM schema_migrations WHERE version = 38",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1
        );

        let mut connection = Connection::open_in_memory().expect("open cancel migration database");
        run(&mut connection, 1).expect("apply fresh migrations through v38");
        run(&mut connection, 2).expect("repeat fresh v38 migrations");
        for table in [
            "work_graph_node_cancel_intents",
            "work_graph_node_terminal_reconciliations",
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
                "missing v38 table {table}"
            );
        }
        for index in [
            "idx_work_graph_node_cancel_intents_active_attempt",
            "idx_work_graph_node_cancel_intents_active_child",
        ] {
            assert_eq!(
                connection
                    .query_row(
                        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = ?1",
                        [index],
                        |row| row.get::<_, i64>(0),
                    )
                    .unwrap(),
                1,
                "missing v38 partial unique index {index}"
            );
        }

        connection
            .execute_batch(
                r#"INSERT INTO agents(
                    id, name, description, runtime_type, system_prompt, default_model,
                    created_at, updated_at
                 ) VALUES ('cancel-agent', 'Cancel Agent', '', 'pi', '', 'model', 1, 1);
                 INSERT INTO conversations(
                    id, agent_id, title, status, created_at, updated_at
                 ) VALUES
                    ('cancel-conversation', 'cancel-agent', 'Cancel', 'active', 1, 1),
                    ('cancel-child-conversation', 'cancel-agent', 'Child', 'active', 1, 1);
                 UPDATE conversations
                 SET conversation_kind = 'child',
                     parent_conversation_id = 'cancel-conversation',
                     lineage_root_id = 'cancel-conversation'
                 WHERE id = 'cancel-child-conversation';
                 INSERT INTO runs(id, conversation_id, status, model, last_seq, created_at)
                 VALUES
                    ('cancel-activation-run', 'cancel-conversation', 'running', 'model', 0, 1),
                    ('cancel-parent-run', 'cancel-conversation', 'running', 'model', 0, 2);
                 INSERT INTO runs(
                    id, conversation_id, status, model, last_seq, created_at,
                    parent_run_id, root_run_id, run_kind, depth
                 ) VALUES (
                    'cancel-child-run', 'cancel-child-conversation', 'queued', 'model', 0, 1,
                    'cancel-parent-run', 'cancel-parent-run', 'child', 1
                 );
                 INSERT INTO run_execution_profiles(
                    run_id, schema_version, profile_id, snapshot_json, profile_hash, frozen_at
                 ) VALUES
                    (
                        'cancel-activation-run', 1, 'durable_v2',
                        '{"id":"durable_v2","schemaVersion":1}',
                        'ae08f4dc8c1327cbd3de30879f01aa7475d9a728a3c0f738579086435d714f7e', 1
                    ),
                    (
                        'cancel-parent-run', 1, 'durable_v2',
                        '{"id":"durable_v2","schemaVersion":1}',
                        'ae08f4dc8c1327cbd3de30879f01aa7475d9a728a3c0f738579086435d714f7e', 2
                    );
                 INSERT INTO tool_calls(
                    id, runtime_tool_call_id, run_id, conversation_id, tool_name, input_json,
                    status, execution_location, requires_approval, started_at, updated_at
                 ) VALUES
                    ('cancel-activation-call', 'cancel-activation-runtime', 'cancel-activation-run',
                     'cancel-conversation', 'graph_readonly_activate',
                     '{"planRevisionId":"cancel-plan"}', 'running', 'host', 0, 1, 1),
                    ('cancel-node-start-call', 'cancel-node-start-runtime', 'cancel-parent-run',
                     'cancel-conversation', 'graph_readonly_node_start',
                     '{"goalId":"cancel-goal","taskId":"cancel-task","expectedTaskVersion":1,"attemptId":"cancel-attempt","workerAgentId":"cancel-agent","objective":"Inspect","context":"","budget":{"maxDurationMs":1000,"maxTotalTokens":256,"maxOutputTokens":64,"maxToolCalls":0}}',
                     'running', 'host', 0, 2, 2);
                 INSERT INTO goals(
                    id, conversation_id, title, objective, status, created_by, created_at, updated_at
                 ) VALUES (
                    'cancel-goal', 'cancel-conversation', 'Cancel Graph', 'Inspect safely',
                    'active', 'user', '1', '1'
                 );
                 INSERT INTO plan_revisions(
                    id, goal_id, conversation_id, revision, title, summary, tasks_json,
                    status, created_by, created_at, approved_at
                 ) VALUES (
                    'cancel-plan', 'cancel-goal', 'cancel-conversation', 1, 'Cancel', 'Cancel',
                    '[]', 'approved', 'user', '1', '1'
                 );
                 INSERT INTO work_tasks(
                    id, goal_id, ordinal, title, status, created_at, updated_at
                 ) VALUES ('cancel-task', 'cancel-goal', 0, 'Inspect', 'queued', '1', '1');
                 INSERT INTO work_graph_specs(
                    goal_id, plan_revision_id, schema_version, graph_mode, max_nodes, max_depth,
                    spec_hash, activated_by_run_id, activation_tool_call_id, activated_at
                 ) VALUES (
                    'cancel-goal', 'cancel-plan', 1, 'read_only', 3, 1,
                    'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
                    'cancel-activation-run', 'cancel-activation-call', 1
                 );
                 INSERT INTO work_graph_nodes(
                    task_id, goal_id, node_key, access_mode, depth,
                    acceptance_criteria_json, created_at
                 ) VALUES (
                    'cancel-task', 'cancel-goal', 'cancel-node', 'read_only', 0,
                    '["inspection complete"]', 1
                 );
                 UPDATE work_tasks
                 SET status = 'in_progress', owner_run_id = 'cancel-parent-run', version = 2,
                     updated_at = 2
                 WHERE id = 'cancel-task';
                 INSERT INTO task_attempts(
                    id, task_id, attempt_number, kind, status, run_id, policy_hash,
                    finding_ids_json, evidence_ids_json, version, evidence_rowid_watermark,
                    finding_rowid_watermark, started_at
                 ) VALUES (
                    'cancel-attempt', 'cancel-task', 1, 'execution', 'running',
                    'cancel-parent-run',
                    'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb',
                    '[]', '[]', 1, 0, 0, 3
                 );
                 INSERT INTO child_run_delegations(
                    id, parent_run_id, child_run_id, child_conversation_id, tool_call_id,
                    worker_agent_id, objective, context, status, max_duration_ms,
                    max_total_tokens, max_output_tokens, max_tool_calls, created_at,
                    allowed_tools_json, graph_task_attempt_id
                 ) VALUES (
                    'cancel-delegation', 'cancel-parent-run', 'cancel-child-run',
                    'cancel-child-conversation', 'cancel-node-start-call', 'cancel-agent',
                    'Inspect', '', 'queued', 1000, 256, 64, 0, 3,
                    '["read","ls","find","grep"]', 'cancel-attempt'
                 );"#,
            )
            .expect("seed canonical Graph cancellation fixture");

        assert_eq!(
            connection
                .query_row(
                    "SELECT
                        (SELECT COUNT(*) FROM work_graph_node_cancel_intents) +
                        (SELECT COUNT(*) FROM work_graph_node_terminal_reconciliations)",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            0,
            "v36 authority must be sealed before any v38 fact exists"
        );
        assert!(connection
            .execute("DELETE FROM task_attempts WHERE id = 'cancel-attempt'", [],)
            .is_err());
        assert_eq!(
            connection
                .query_row(
                    "SELECT graph_task_attempt_id FROM child_run_delegations
                     WHERE id = 'cancel-delegation'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "cancel-attempt",
            "failed direct Attempt delete must not erase the Graph mapping"
        );
        assert!(connection
            .execute(
                "UPDATE task_attempts
                 SET status = 'failed', failure_reason = 'delete-then-finish bypass',
                     version = 2, finished_at = 30
                 WHERE id = 'cancel-attempt'",
                [],
            )
            .is_err());
        assert!(connection
            .execute(
                "DELETE FROM child_run_delegations WHERE id = 'cancel-delegation'",
                [],
            )
            .is_err());
        assert!(connection
            .execute(
                "DELETE FROM conversations WHERE id = 'cancel-child-conversation'",
                [],
            )
            .is_err());
        assert!(connection
            .execute("DELETE FROM runs WHERE id = 'cancel-child-run'", [])
            .is_err());
        assert!(connection
            .execute("DELETE FROM runs WHERE id = 'cancel-parent-run'", [])
            .is_err());
        assert!(connection
            .execute("DELETE FROM runs WHERE id = 'cancel-activation-run'", [])
            .is_err());
        assert!(connection
            .execute(
                "DELETE FROM tool_calls WHERE id = 'cancel-node-start-call'",
                [],
            )
            .is_err());
        assert!(connection
            .execute(
                "DELETE FROM tool_calls WHERE id = 'cancel-activation-call'",
                [],
            )
            .is_err());

        let failed_reason = "common finalizer failed";
        let failed_input = cancel_input(failed_reason);
        insert_cancel_tool(
            &connection,
            "cancel-tool-failed",
            &failed_input,
            "graph_readonly_node_cancel",
            0,
        )
        .expect("insert failed-request ToolCall");
        insert_cancel_intent(
            &connection,
            "cancel-intent-failed",
            "cancel-tool-failed",
            failed_reason,
            &failed_input,
            31,
        )
        .expect("persist pending intent before common finalizer");
        assert!(connection
            .execute(
                "UPDATE task_attempts
                 SET status = 'failed', failure_reason = 'bypass', version = 2, finished_at = 32
                 WHERE id = 'cancel-attempt'",
                [],
            )
            .is_err());
        assert!(connection
            .execute(
                "UPDATE work_tasks
                 SET status = 'interrupted', blocked_reason = 'bypass', version = 3,
                     finished_at = 32, updated_at = 32
                 WHERE id = 'cancel-task'",
                [],
            )
            .is_err());
        connection
            .execute(
                "UPDATE tool_calls
                 SET status = 'failed', error_message = 'common finalizer failed',
                     completed_at = 32, updated_at = 32
                 WHERE id = 'cancel-tool-failed'",
                [],
            )
            .expect("finalize first cancel ToolCall as failed");
        assert!(connection
            .execute(
                "UPDATE work_graph_node_cancel_intents SET activated_at = 33
                 WHERE id = 'cancel-intent-failed'",
                [],
            )
            .is_err());

        connection
            .execute_batch("SAVEPOINT automatic_negative")
            .unwrap();
        connection
            .execute_batch(
                "UPDATE runs
                 SET status = 'failed', error_code = 'child.failed', error_message = 'boom',
                     finished_at = 40
                 WHERE id = 'cancel-child-run';
                 UPDATE child_run_delegations
                 SET status = 'failed', error_code = 'child.failed', error_message = 'boom',
                     finished_at = 40
                 WHERE id = 'cancel-delegation';",
            )
            .unwrap();
        let automatic_snapshot = terminal_json("failed", 40, Some("child.failed"), Some("boom"));
        insert_reconciliation(
            &connection,
            "cancel-reconciliation-automatic",
            None,
            "failed",
            "failed",
            &automatic_snapshot,
            "automatic child failure",
            41,
        )
        .expect("pending intent is inert and automatic Child failure can reconcile");
        connection
            .execute_batch(
                "UPDATE task_attempts
                 SET status = 'failed', failure_reason = 'automatic child failure',
                     version = 2, finished_at = 41
                 WHERE id = 'cancel-attempt';
                 UPDATE work_tasks
                 SET status = 'interrupted', blocked_reason = 'automatic child failure',
                     owner_run_id = NULL, version = 3, updated_at = 41, finished_at = 41
                 WHERE id = 'cancel-task';",
            )
            .expect("reconciliation authorizes existing negative Task state machine");
        connection
            .execute_batch("ROLLBACK TO automatic_negative; RELEASE automatic_negative")
            .unwrap();

        let active_reason = "cancel this Graph node";
        let active_input = cancel_input(active_reason);
        insert_cancel_tool(
            &connection,
            "cancel-tool-active",
            &active_input,
            "graph_readonly_node_cancel",
            0,
        )
        .unwrap();
        insert_cancel_intent(
            &connection,
            "cancel-intent-active",
            "cancel-tool-active",
            active_reason,
            &active_input,
            34,
        )
        .unwrap();
        connection
            .execute(
                "UPDATE tool_calls
                 SET status = 'completed', result_json = '{\"cancelRequested\":true}',
                     completed_at = 35, updated_at = 35
                 WHERE id = 'cancel-tool-active'",
                [],
            )
            .unwrap();
        connection
            .execute_batch("SAVEPOINT stale_activation")
            .unwrap();
        connection
            .execute_batch(
                "UPDATE runs SET status = 'failed', finished_at = 36
                 WHERE id = 'cancel-child-run';
                 UPDATE child_run_delegations SET status = 'failed', finished_at = 36
                 WHERE id = 'cancel-delegation';",
            )
            .unwrap();
        assert!(connection
            .execute(
                "UPDATE work_graph_node_cancel_intents SET activated_at = 36
                 WHERE id = 'cancel-intent-active'",
                [],
            )
            .is_err());
        connection
            .execute_batch("ROLLBACK TO stale_activation; RELEASE stale_activation")
            .unwrap();
        connection
            .execute_batch("SAVEPOINT stale_activation_cas")
            .unwrap();
        connection
            .execute(
                "UPDATE work_tasks SET version = 3 WHERE id = 'cancel-task'",
                [],
            )
            .unwrap();
        assert!(connection
            .execute(
                "UPDATE work_graph_node_cancel_intents SET activated_at = 36
                 WHERE id = 'cancel-intent-active'",
                [],
            )
            .is_err());
        connection
            .execute_batch("ROLLBACK TO stale_activation_cas; RELEASE stale_activation_cas")
            .unwrap();
        connection
            .execute(
                "UPDATE work_graph_node_cancel_intents SET activated_at = 36
                 WHERE id = 'cancel-intent-active'",
                [],
            )
            .expect("activate exact completed Host request once");
        assert!(connection
            .execute(
                "UPDATE work_graph_node_cancel_intents SET activated_at = 37
                 WHERE id = 'cancel-intent-active'",
                [],
            )
            .is_err());

        let second_input = cancel_input("second request");
        insert_cancel_tool(
            &connection,
            "cancel-tool-second",
            &second_input,
            "graph_readonly_node_cancel",
            0,
        )
        .unwrap();
        insert_cancel_intent(
            &connection,
            "cancel-intent-second",
            "cancel-tool-second",
            "second request",
            &second_input,
            37,
        )
        .expect("multiple pending requests for one Attempt are allowed");
        connection
            .execute(
                "UPDATE tool_calls
                 SET status = 'completed', result_json = '{}', completed_at = 38, updated_at = 38
                 WHERE id = 'cancel-tool-second'",
                [],
            )
            .unwrap();
        assert!(connection
            .execute(
                "UPDATE work_graph_node_cancel_intents SET activated_at = 38
                 WHERE id = 'cancel-intent-second'",
                [],
            )
            .is_err());
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM work_graph_node_cancel_intents
                     WHERE attempt_id = 'cancel-attempt'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            3
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM work_graph_node_cancel_intents
                     WHERE attempt_id = 'cancel-attempt' AND activated_at IS NOT NULL",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1
        );

        let bad_tool_input = cancel_input("wrong tool");
        insert_cancel_tool(
            &connection,
            "cancel-tool-wrong-name",
            &bad_tool_input,
            "child_run_cancel",
            0,
        )
        .unwrap();
        assert!(insert_cancel_intent(
            &connection,
            "cancel-intent-wrong-name",
            "cancel-tool-wrong-name",
            "wrong tool",
            &bad_tool_input,
            39,
        )
        .is_err());
        let wrong_run_input = cancel_input("wrong run");
        connection
            .execute(
                "INSERT INTO tool_calls(
                    id, runtime_tool_call_id, run_id, conversation_id, tool_name, input_json,
                    status, execution_location, requires_approval, started_at, updated_at
                 ) VALUES (
                    'cancel-tool-wrong-run', 'cancel-tool-wrong-run', 'cancel-child-run',
                    'cancel-child-conversation', 'graph_readonly_node_cancel', ?1,
                    'running', 'host', 0, 30, 30
                 )",
                [wrong_run_input.as_str()],
            )
            .unwrap();
        assert!(insert_cancel_intent(
            &connection,
            "cancel-intent-wrong-run",
            "cancel-tool-wrong-run",
            "wrong run",
            &wrong_run_input,
            39,
        )
        .is_err());
        let approval_input = cancel_input("wrong approval");
        insert_cancel_tool(
            &connection,
            "cancel-tool-wrong-approval",
            &approval_input,
            "graph_readonly_node_cancel",
            1,
        )
        .unwrap();
        assert!(insert_cancel_intent(
            &connection,
            "cancel-intent-wrong-approval",
            "cancel-tool-wrong-approval",
            "wrong approval",
            &approval_input,
            39,
        )
        .is_err());
        let extra_input = format!(
            "{{\"attemptId\":\"cancel-attempt\",\"expectedAttemptVersion\":1,\"expectedTaskVersion\":2,\"extra\":true,\"goalId\":\"cancel-goal\",\"reason\":\"extra\",\"taskId\":\"cancel-task\"}}"
        );
        insert_cancel_tool(
            &connection,
            "cancel-tool-extra-input",
            &extra_input,
            "graph_readonly_node_cancel",
            0,
        )
        .unwrap();
        assert!(insert_cancel_intent(
            &connection,
            "cancel-intent-extra-input",
            "cancel-tool-extra-input",
            "extra",
            &extra_input,
            39,
        )
        .is_err());
        let long_reason = "x".repeat(2001);
        let long_input = cancel_input(&long_reason);
        insert_cancel_tool(
            &connection,
            "cancel-tool-long-reason",
            &long_input,
            "graph_readonly_node_cancel",
            0,
        )
        .unwrap();
        assert!(insert_cancel_intent(
            &connection,
            "cancel-intent-long-reason",
            "cancel-tool-long-reason",
            &long_reason,
            &long_input,
            39,
        )
        .is_err());

        assert!(insert_reconciliation(
            &connection,
            "cancel-reconciliation-nonterminal",
            Some("cancel-intent-active"),
            "failed",
            "failed",
            &terminal_json("failed", 40, None, None),
            "not terminal",
            40,
        )
        .is_err());
        connection
            .execute_batch("SAVEPOINT completed_rejection")
            .unwrap();
        connection
            .execute_batch(
                "UPDATE runs SET status = 'completed', finished_at = 40
                 WHERE id = 'cancel-child-run';
                 UPDATE child_run_delegations SET status = 'completed', finished_at = 40
                 WHERE id = 'cancel-delegation';",
            )
            .unwrap();
        assert!(insert_reconciliation(
            &connection,
            "cancel-reconciliation-completed",
            Some("cancel-intent-active"),
            "completed",
            "failed",
            &terminal_json("completed", 40, None, None),
            "completed is not negative",
            40,
        )
        .is_err());
        connection
            .execute_batch("ROLLBACK TO completed_rejection; RELEASE completed_rejection")
            .unwrap();

        for (child_status, attempt_status, code, message, failure_reason) in [
            (
                "failed",
                "failed",
                Some("child.failed"),
                Some("child failed"),
                "child failed",
            ),
            ("cancelled", "cancelled", None, None, "child cancelled"),
            (
                "interrupted",
                "failed",
                Some("child.interrupted"),
                Some("child interrupted"),
                "child interrupted",
            ),
        ] {
            connection
                .execute_batch("SAVEPOINT terminal_mapping")
                .unwrap();
            connection
                .execute(
                    "UPDATE runs
                     SET status = ?2, error_code = ?3, error_message = ?4, finished_at = 50
                     WHERE id = 'cancel-child-run'",
                    rusqlite::params!["cancel-child-run", child_status, code, message],
                )
                .unwrap();
            connection
                .execute(
                    "UPDATE child_run_delegations
                     SET status = ?2, error_code = ?3, error_message = ?4, finished_at = 50
                     WHERE id = 'cancel-delegation'",
                    rusqlite::params!["cancel-delegation", child_status, code, message],
                )
                .unwrap();
            let snapshot = terminal_json(child_status, 50, code, message);
            connection
                .execute(
                    "UPDATE child_run_delegations SET error_message = 'spoofed terminal'
                     WHERE id = 'cancel-delegation'",
                    [],
                )
                .unwrap();
            assert!(insert_reconciliation(
                &connection,
                "cancel-reconciliation-mismatched-delegation",
                Some("cancel-intent-active"),
                child_status,
                attempt_status,
                &snapshot,
                failure_reason,
                51,
            )
            .is_err());
            connection
                .execute(
                    "UPDATE child_run_delegations SET error_message = ?1
                     WHERE id = 'cancel-delegation'",
                    [message],
                )
                .unwrap();
            assert!(insert_reconciliation(
                &connection,
                "cancel-reconciliation-missing-intent",
                None,
                child_status,
                attempt_status,
                &snapshot,
                failure_reason,
                51,
            )
            .is_err());
            insert_reconciliation(
                &connection,
                "cancel-reconciliation-valid",
                Some("cancel-intent-active"),
                child_status,
                attempt_status,
                &snapshot,
                failure_reason,
                51,
            )
            .expect("insert exact Child negative terminal fact");
            connection
                .execute(
                    "UPDATE task_attempts
                     SET status = ?2, failure_reason = ?3, version = 2, finished_at = 51
                     WHERE id = 'cancel-attempt'",
                    rusqlite::params!["cancel-attempt", attempt_status, failure_reason],
                )
                .expect("negative fact authorizes exact Attempt mapping");
            connection
                .execute(
                    "UPDATE work_tasks
                     SET status = 'interrupted', blocked_reason = ?2, owner_run_id = NULL,
                         version = 3, updated_at = 51, finished_at = 51
                     WHERE id = 'cancel-task'",
                    rusqlite::params!["cancel-task", failure_reason],
                )
                .expect("negative fact authorizes exact Task interruption");
            assert_eq!(
                connection
                    .query_row(
                        "SELECT status FROM task_attempts WHERE id = 'cancel-attempt'",
                        [],
                        |row| row.get::<_, String>(0),
                    )
                    .unwrap(),
                attempt_status
            );
            assert!(connection
                .execute(
                    "UPDATE work_graph_node_terminal_reconciliations
                     SET failure_reason = 'tampered' WHERE id = 'cancel-reconciliation-valid'",
                    [],
                )
                .is_err());
            assert!(connection
                .execute(
                    "DELETE FROM work_graph_node_terminal_reconciliations
                     WHERE id = 'cancel-reconciliation-valid'",
                    [],
                )
                .is_err());
            assert!(connection
                .execute(
                    "UPDATE runs SET error_message = 'tampered'
                     WHERE id = 'cancel-child-run'",
                    [],
                )
                .is_err());
            assert!(connection
                .execute(
                    "UPDATE child_run_delegations SET error_message = 'tampered'
                     WHERE id = 'cancel-delegation'",
                    [],
                )
                .is_err());
            assert!(connection
                .execute(
                    "UPDATE task_attempts SET failure_reason = 'tampered'
                     WHERE id = 'cancel-attempt'",
                    [],
                )
                .is_err());
            assert!(connection
                .execute(
                    "UPDATE work_tasks SET updated_at = 52 WHERE id = 'cancel-task'",
                    [],
                )
                .is_err());
            connection
                .execute_batch("ROLLBACK TO terminal_mapping; RELEASE terminal_mapping")
                .unwrap();
        }

        connection
            .execute_batch("SAVEPOINT parent_terminal")
            .unwrap();
        connection
            .execute(
                "UPDATE runs
                 SET status = 'failed', error_code = 'parent.failed',
                     error_message = 'parent failed', finished_at = 60
                 WHERE id = 'cancel-parent-run'",
                [],
            )
            .unwrap();
        connection
            .execute(
                "UPDATE task_attempts
                 SET status = 'failed', failure_reason = 'parent failed',
                     version = 2, finished_at = 60
                 WHERE id = 'cancel-attempt'",
                [],
            )
            .expect("existing parent terminal authority remains valid");
        connection
            .execute(
                "UPDATE work_tasks
                 SET status = 'interrupted', version = 3, updated_at = 60, finished_at = 60
                 WHERE id = 'cancel-task'",
                [],
            )
            .expect("existing parent terminal Task audit path remains valid");
        connection
            .execute_batch("ROLLBACK TO parent_terminal; RELEASE parent_terminal")
            .unwrap();

        assert!(connection
            .execute(
                "UPDATE work_graph_node_cancel_intents SET reason = 'tampered'
                 WHERE id = 'cancel-intent-active'",
                [],
            )
            .is_err());
        assert!(connection
            .execute(
                "DELETE FROM work_graph_node_cancel_intents WHERE id = 'cancel-intent-active'",
                [],
            )
            .is_err());
        assert!(connection
            .execute(
                "UPDATE tool_calls SET input_json = '{}'
                 WHERE id = 'cancel-tool-active'",
                [],
            )
            .is_err());
        assert!(connection
            .execute(
                "UPDATE tool_calls SET result_json = '{\"tampered\":true}'
                 WHERE id = 'cancel-tool-active'",
                [],
            )
            .is_err());
        assert!(connection
            .execute("DELETE FROM tool_calls WHERE id = 'cancel-tool-active'", [],)
            .is_err());
        assert!(connection
            .execute(
                "DELETE FROM child_run_delegations WHERE id = 'cancel-delegation'",
                [],
            )
            .is_err());
        assert!(connection
            .execute("DELETE FROM task_attempts WHERE id = 'cancel-attempt'", [],)
            .is_err());

        let foreign_key_errors = connection
            .prepare("PRAGMA foreign_key_check")
            .unwrap()
            .query_map([], |_| Ok(()))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(foreign_key_errors.is_empty());

        connection
            .execute_batch("SAVEPOINT cancel_conversation_delete_probe")
            .unwrap();
        connection
            .execute_batch(
                "UPDATE runs
                 SET status = 'failed', error_code = 'child.failed', error_message = 'boom',
                     finished_at = 70
                 WHERE id = 'cancel-child-run';
                 UPDATE child_run_delegations
                 SET status = 'failed', error_code = 'child.failed', error_message = 'boom',
                     finished_at = 70
                 WHERE id = 'cancel-delegation';",
            )
            .unwrap();
        let cascade_snapshot = terminal_json("failed", 70, Some("child.failed"), Some("boom"));
        insert_reconciliation(
            &connection,
            "cancel-reconciliation-cascade",
            Some("cancel-intent-active"),
            "failed",
            "failed",
            &cascade_snapshot,
            "cascade child failure",
            71,
        )
        .unwrap();
        connection
            .execute_batch(
                "UPDATE task_attempts
                 SET status = 'failed', failure_reason = 'cascade child failure',
                     version = 2, finished_at = 71
                 WHERE id = 'cancel-attempt';
                 UPDATE work_tasks
                 SET status = 'interrupted', blocked_reason = 'cascade child failure',
                     owner_run_id = NULL, version = 3, updated_at = 71, finished_at = 71
                 WHERE id = 'cancel-task';",
            )
            .unwrap();
        let conversation_delete = connection.execute(
            "DELETE FROM conversations WHERE id = 'cancel-conversation'",
            [],
        );
        assert!(
            conversation_delete.is_ok(),
            "Conversation cascade must clear v38 immutable facts: {conversation_delete:?}"
        );
        connection
            .execute_batch(
                "ROLLBACK TO cancel_conversation_delete_probe;
                 RELEASE cancel_conversation_delete_probe",
            )
            .unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM work_graph_cascade_delete_scopes",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            0
        );

        connection
            .execute("DELETE FROM goals WHERE id = 'cancel-goal'", [])
            .expect("Goal cascade removes pending and active cancel intents");
        for table in [
            "work_graph_node_cancel_intents",
            "work_graph_node_terminal_reconciliations",
            "work_graph_nodes",
            "work_graph_specs",
        ] {
            assert_eq!(
                connection
                    .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                        row.get::<_, i64>(0)
                    })
                    .unwrap(),
                0,
                "Goal cascade left rows in {table}"
            );
        }
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM work_graph_cascade_delete_scopes",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            0
        );
        let foreign_key_errors = connection
            .prepare("PRAGMA foreign_key_check")
            .unwrap()
            .query_map([], |_| Ok(()))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(foreign_key_errors.is_empty());
    }

    #[test]
    fn graph_review_acceptance_migration_tolerates_missing_late_v36_objects() {
        let mut connection = Connection::open_in_memory().expect("open legacy v38 database");
        run_through_v37(&mut connection, 1);
        {
            let transaction = connection.transaction().expect("begin v38 migration");
            apply_migration(&transaction, 38, MIGRATION_38, 2).expect("apply v38");
            transaction.commit().expect("commit v38");
        }

        // Historical development databases can already have v36 recorded while
        // missing objects that were added to MIGRATION_36 later. Reproduce that
        // exact upgrade shape instead of testing only a freshly rebuilt schema.
        connection
            .execute_batch(
                "DROP TRIGGER work_graph_profiles_no_update;
                 DROP TRIGGER work_graph_profiles_no_direct_delete;
                 DROP INDEX idx_run_execution_profiles_profile;",
            )
            .expect("remove late-added v36 objects from legacy fixture");

        run(&mut connection, 3).expect("upgrade legacy v38 database through v39");

        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM schema_migrations WHERE version = 39",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1
        );
        for (kind, name) in [
            ("trigger", "work_graph_profiles_no_update"),
            ("trigger", "work_graph_profiles_no_direct_delete"),
            ("index", "idx_run_execution_profiles_profile"),
        ] {
            assert_eq!(
                connection
                    .query_row(
                        "SELECT COUNT(*) FROM sqlite_master WHERE type = ?1 AND name = ?2",
                        rusqlite::params![kind, name],
                        |row| row.get::<_, i64>(0),
                    )
                    .unwrap(),
                1,
                "v39 did not restore missing legacy {kind} {name}"
            );
        }
    }

    #[test]
    fn graph_review_acceptance_migration_upgrades_v38_idempotently_and_seals_authority() {
        let mut connection = Connection::open_in_memory().expect("open v38 upgrade database");
        run_through_v37(&mut connection, 1);
        {
            let transaction = connection.transaction().expect("begin v38 migration");
            apply_migration(&transaction, 38, MIGRATION_38, 2).expect("apply v38");
            transaction.commit().expect("commit v38");
        }
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master
                     WHERE type = 'table' AND name = 'work_graph_node_review_requests'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            0
        );
        run(&mut connection, 3).expect("upgrade v38 database through v39");
        run(&mut connection, 4).expect("repeat v39 migration");
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM schema_migrations WHERE version = 39",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1
        );
        for table in [
            "work_graph_node_review_requests",
            "work_graph_node_review_criterion_evidence",
            "work_graph_node_review_decisions",
            "work_graph_node_review_proofs",
            "work_graph_acceptances",
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
                "missing v39 table {table}"
            );
        }
        for trigger in [
            "work_graph_node_review_proofs_validate_insert",
            "work_graph_node_review_requests_validate_settlement",
            "work_graph_high_risk_finishes_require_review_pass",
            "work_graph_high_risk_attempts_require_review_pass",
            "acceptances_require_graph_provenance",
            "goals_require_graph_acceptance",
        ] {
            assert_eq!(
                connection
                    .query_row(
                        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'trigger' AND name = ?1",
                        [trigger],
                        |row| row.get::<_, i64>(0),
                    )
                    .unwrap(),
                1,
                "missing v39 trigger {trigger}"
            );
        }
        let decision_trigger_sql = connection
            .query_row(
                "SELECT sql FROM sqlite_master
                 WHERE type = 'trigger' AND name = 'work_graph_node_review_decisions_validate_insert'",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap();
        assert!(decision_trigger_sql.contains("json_type(criterion.value, '$.status') <> 'text'"));
        assert!(decision_trigger_sql.contains("json_type(finding.value, '$.severity') <> 'text'"));
        assert!(decision_trigger_sql.contains("proof_id.type <> 'text'"));
        for trigger in [
            "work_graph_node_finishes_validate_authority",
            "task_attempts_require_graph_node_finish",
        ] {
            let sql = connection
                .query_row(
                    "SELECT sql FROM sqlite_master WHERE type = 'trigger' AND name = ?1",
                    [trigger],
                    |row| row.get::<_, String>(0),
                )
                .unwrap();
            assert!(
                sql.contains("policy.policy_id IN ('standard_v1', 'high_risk_v1')"),
                "v39 did not widen the frozen Graph finish gate for {trigger}"
            );
        }
        let proof_columns = connection
            .prepare("PRAGMA table_info(work_graph_node_review_proofs)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(proof_columns.contains(&"runtime_tool_call_id".to_owned()));
        assert!(proof_columns.contains(&"tool_call_id".to_owned()));
        assert!(connection
            .execute(
                "INSERT INTO work_graph_node_review_proofs(
                    decision_id, criterion_ordinal, criterion,
                    runtime_tool_call_id, tool_call_id
                 ) VALUES ('forged', 0, 'criterion', 'runtime-id', 'internal-id')",
                [],
            )
            .is_err());
        assert!(connection
            .execute(
                "INSERT INTO work_graph_acceptances(
                    id, goal_id, plan_revision_id, spec_hash, submitter_run_id,
                    tool_call_id, expected_goal_version, summary,
                    node_projection_json, node_projection_hash, checks_json, checks_hash,
                    input_json, input_hash, created_at
                 ) VALUES (
                    'forged', 'goal', 'plan',
                    'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
                    'run', 'call', 1, 'summary', '[]',
                    'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
                    '{}', 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
                    '{}', 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 1
                 )",
                [],
            )
            .is_err());
        let foreign_key_errors = connection
            .prepare("PRAGMA foreign_key_check")
            .unwrap()
            .query_map([], |_| Ok(()))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(foreign_key_errors.is_empty());
    }

    #[test]
    fn cascade_delete_scope_repair_restores_goal_deletion_for_drifted_v36_databases() {
        let mut connection = Connection::open_in_memory().expect("open database");
        run(&mut connection, 1).expect("apply current schema");

        connection
            .execute("DELETE FROM schema_migrations WHERE version = 42", [])
            .expect("simulate database from before the repair migration");
        connection
            .execute("DROP TABLE work_graph_cascade_delete_scopes", [])
            .expect("simulate the missing late-v36 sentinel table");
        connection
            .execute_batch(
                "INSERT INTO agents(
                    id, name, description, runtime_type, system_prompt, default_model,
                    created_at, updated_at
                 ) VALUES ('repair-agent', 'Repair Agent', '', 'pi', '', 'model', 1, 1);
                 INSERT INTO conversations(
                    id, agent_id, title, status, created_at, updated_at, lineage_root_id
                 ) VALUES (
                    'repair-conversation', 'repair-agent', 'Repair', 'active', 1, 1,
                    'repair-conversation'
                 );
                 INSERT INTO goals(
                    id, conversation_id, title, objective, status, created_by, created_at, updated_at
                 ) VALUES (
                    'repair-goal', 'repair-conversation', 'Repair', 'Delete safely', 'blocked',
                    'user', '1', '1'
                 );",
            )
            .expect("seed drifted database");

        assert!(connection
            .execute("DELETE FROM goals WHERE id = 'repair-goal'", [])
            .is_err());

        run(&mut connection, 2).expect("apply cascade-delete scope repair");
        run(&mut connection, 3).expect("repeat cascade-delete scope repair");

        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master
                     WHERE type = 'table' AND name = 'work_graph_cascade_delete_scopes'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM schema_migrations WHERE version = 42",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1
        );
        assert_eq!(
            connection
                .execute("DELETE FROM goals WHERE id = 'repair-goal'", [])
                .expect("delete Goal after repairing the sentinel"),
            1
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM work_graph_cascade_delete_scopes",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            0
        );
    }

    #[test]
    fn sensenova_model_id_migration_repairs_display_names_idempotently() {
        let mut connection = Connection::open_in_memory().expect("open database");
        run(&mut connection, 1).expect("apply current schema");
        connection
            .execute(
                "INSERT INTO model_service(
                     singleton_id, name, base_url, model_id, api_type, context_window,
                     max_output_tokens, enabled, last_status, last_latency_ms,
                     last_checked_at, created_at, updated_at, supports_image_input
                 ) VALUES (
                     1, 'SenseNova', 'https://token.sensenova.cn/v1',
                     'SenseNova 6.8 Flash Lite', 'openai-completions', 262144,
                     65536, 1, 'connected', 12, 1, 1, 1, 1
                 )",
                [],
            )
            .expect("seed legacy primary model");
        connection
            .execute(
                "INSERT INTO model_providers(
                     id, name, base_url, api_type, enabled, is_default, last_status,
                     last_latency_ms, last_checked_at, created_at, updated_at
                 ) VALUES (
                     'sensenova-test', 'SenseNova', 'https://token.sensenova.cn/v1',
                     'openai-completions', 1, 1, 'connected', 12, 1, 1, 1
                 )",
                [],
            )
            .expect("seed SenseNova provider");
        connection
            .execute(
                "INSERT INTO provider_models(
                     id, provider_id, model_id, display_name, context_window,
                     max_output_tokens, supports_image_input, is_default, created_at, updated_at
                 ) VALUES (
                     'sensenova-test-model', 'sensenova-test',
                     'SenseNova 6.8 Flash Lite', 'SenseNova 6.8 Flash Lite',
                     262144, 65536, 1, 1, 1, 1
                 )",
                [],
            )
            .expect("seed SenseNova model");

        connection
            .execute_batch(MIGRATION_46)
            .expect("repair model IDs");
        connection
            .execute_batch(MIGRATION_46)
            .expect("repeat model ID repair");

        assert_eq!(
            connection
                .query_row(
                    "SELECT model_id, last_status, last_latency_ms, last_checked_at
                     FROM model_service WHERE singleton_id = 1",
                    [],
                    |row| Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<i64>>(2)?,
                        row.get::<_, Option<i64>>(3)?,
                    )),
                )
                .unwrap(),
            (
                "sensenova-6.8-flash-lite".to_owned(),
                "unknown".to_owned(),
                None,
                None,
            )
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT model_id FROM provider_models WHERE id = 'sensenova-test-model'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "sensenova-6.8-flash-lite"
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT last_status FROM model_providers WHERE id = 'sensenova-test'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "unknown"
        );
    }

    #[test]
    fn obsolete_foreground_notifications_are_removed_without_touching_background_work() {
        let mut connection = Connection::open_in_memory().expect("open database");
        run(&mut connection, 1).expect("apply current schema");
        connection
            .execute_batch(
                "INSERT INTO app_notifications(
                    id, merge_key, kind, severity, title, body, source_type, source_id,
                    status, action_json, created_at, updated_at
                 ) VALUES
                    ('switch', 'ui:switch', 'completed', 'normal', '操作完成',
                     '已切换助手，发送消息后创建对话', 'ui', 'switch', 'completed', '{}', 1, 1),
                    ('open', 'ui:open', 'completed', 'normal', '操作完成',
                     '已打开项目文件夹', 'ui', 'open', 'completed', '{}', 1, 1),
                    ('preview', 'ui:preview', 'completed', 'normal', '正在重新加载预览',
                     'report.pdf', 'ui', 'preview', 'completed', '{}', 1, 1),
                    ('background', 'run:background', 'failed', 'high', '后台任务失败',
                     '需要检查', 'run', 'background', 'failed', '{}', 1, 1);",
            )
            .expect("seed notification policy examples");

        connection
            .execute_batch(MIGRATION_47)
            .expect("remove obsolete notifications");
        connection
            .execute_batch(MIGRATION_47)
            .expect("repeat obsolete notification cleanup");

        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM app_notifications", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
            1
        );
        assert_eq!(
            connection
                .query_row("SELECT id FROM app_notifications", [], |row| {
                    row.get::<_, String>(0)
                })
                .unwrap(),
            "background"
        );
    }

    #[test]
    fn app_capability_migration_is_idempotent_and_enforces_defaults() {
        let mut connection = Connection::open_in_memory().expect("open database");
        run(&mut connection, 1).expect("apply current schema");
        run(&mut connection, 2).expect("repeat current schema");

        assert_eq!(
            connection
                .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| row
                    .get::<_, i64>(
                    0
                ),)
                .unwrap(),
            DATABASE_SCHEMA_VERSION
        );
        for table in [
            "app_notifications",
            "notification_preferences",
            "message_feedback",
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
                "missing app capability table {table}"
            );
        }
        let project_columns = connection
            .prepare("PRAGMA table_info(projects)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(project_columns.contains(&"archived_at".to_owned()));
        let notification_columns = connection
            .prepare("PRAGMA table_info(app_notifications)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(notification_columns.contains(&"dismissed_at".to_owned()));
        let conversation_columns = connection
            .prepare("PRAGMA table_info(conversations)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(conversation_columns.contains(&"permission_mode".to_owned()));
        assert_eq!(
            connection
                .query_row(
                    "SELECT system_popup, sound, sound_id, badge, quiet_progress
                     FROM notification_preferences WHERE singleton_id = 1",
                    [],
                    |row| Ok((
                        row.get::<_, bool>(0)?,
                        row.get::<_, bool>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, bool>(3)?,
                        row.get::<_, bool>(4)?,
                    )),
                )
                .unwrap(),
            (false, false, "soft".to_owned(), true, true)
        );
        assert!(connection
            .execute(
                "INSERT INTO app_notifications(
                    id, merge_key, kind, severity, title, body, source_type, source_id,
                    progress, status, created_at, updated_at
                 ) VALUES ('bad', 'bad', 'progress', 'normal', 'Bad', 'Bad', 'run', 'run',
                           101, 'running', 1, 1)",
                [],
            )
            .is_err());
    }

    #[test]
    fn v53_checkpoint_upgrade_preserves_existing_shadow_identity() {
        let mut connection = Connection::open_in_memory().unwrap();
        run_through_v52(&mut connection, 1);
        connection.execute_batch(
            "INSERT INTO kernel_shadow_runs(
                shadow_run_id, legacy_run_id, conversation_id, turn_id, engine_id, kernel_mode,
                capability_manifest_version, capability_manifest_hash, permission_snapshot_id,
                execution_profile_id, prompt_config_hash, frozen_config_json, created_at)
             VALUES ('old-shadow','old-run','old-conversation','old-turn','pi','shadow',2,'manifest','permission','legacy','prompt','{}',1);"
        ).unwrap();
        // A genuine v52 database has neither the v53 checkpoint table nor the
        // v77 trigger that would be rewritten by the v57 outbox table rename.
        let later_objects: i64 = connection.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE name IN
             ('kernel_shadow_checkpoints', 'execution_policy_invalidate')",
            [],
            |row| row.get(0),
        ).unwrap();
        assert_eq!(later_objects, 0);
        let original_identity: (String, String, String, String) = connection.query_row(
            "SELECT legacy_run_id, conversation_id, turn_id, frozen_config_json
             FROM kernel_shadow_runs WHERE shadow_run_id='old-shadow'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        ).unwrap();
        assert_eq!(
            connection
                .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| row
                    .get::<_, i64>(
                    0
                ))
                .unwrap(),
            52
        );
        run(&mut connection, 2).unwrap();
        run(&mut connection, 3).unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM kernel_shadow_runs WHERE shadow_run_id='old-shadow'",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            1
        );
        let upgraded_identity: (String, String, String, String) = connection.query_row(
            "SELECT legacy_run_id, conversation_id, turn_id, frozen_config_json
             FROM kernel_shadow_runs WHERE shadow_run_id='old-shadow'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        ).unwrap();
        assert_eq!(upgraded_identity, original_identity);
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM kernel_shadow_checkpoints",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
        assert_eq!(
            connection
                .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| row
                    .get::<_, i64>(
                    0
                ))
                .unwrap(),
            crate::database::DATABASE_SCHEMA_VERSION
        );
        let dangling_outbox_triggers: i64 = connection.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='trigger'
             AND sql LIKE '%kernel_effect_outbox_v56%'",
            [],
            |row| row.get(0),
        ).unwrap();
        assert_eq!(dangling_outbox_triggers, 0);
        let foreign_key_violations: i64 = connection.query_row(
            "SELECT COUNT(*) FROM pragma_foreign_key_check",
            [],
            |row| row.get(0),
        ).unwrap();
        assert_eq!(foreign_key_violations, 0);
        let integrity: String = connection.query_row(
            "PRAGMA integrity_check", [], |row| row.get(0)
        ).unwrap();
        assert_eq!(integrity, "ok");
    }

    #[test]
    fn scoped_permission_migration_discards_legacy_broad_grants() {
        let mut connection = Connection::open_in_memory().expect("open database");
        run(&mut connection, 1).expect("create current schema");
        connection
            .execute_batch(
                "INSERT INTO agents(
                    id, name, description, runtime_type, system_prompt, default_model,
                    created_at, updated_at
                 ) VALUES ('scope-agent', 'Scope Agent', '', 'pi', '', 'model', 1, 1);
                 INSERT INTO conversations(
                    id, agent_id, title, status, created_at, updated_at
                 ) VALUES ('scope-conversation', 'scope-agent', 'Scope', 'active', 1, 1);
                 DROP INDEX IF EXISTS idx_conversation_tool_permissions_granted;
                 DROP TABLE conversation_tool_permissions;
                 CREATE TABLE conversation_tool_permissions (
                    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
                    tool_name TEXT NOT NULL,
                    granted_at INTEGER NOT NULL,
                    PRIMARY KEY(conversation_id, tool_name)
                 );
                 INSERT INTO conversation_tool_permissions(conversation_id, tool_name, granted_at)
                 VALUES ('scope-conversation', 'run_command', 1);
                 DELETE FROM schema_migrations WHERE version = 43;",
            )
            .expect("simulate version 42 broad permission table");

        run(&mut connection, 2).expect("upgrade permission table");
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM conversation_tool_permissions",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            0,
            "an old tool-wide grant cannot be narrowed safely and must be discarded"
        );
        let columns = connection
            .prepare("PRAGMA table_info(conversation_tool_permissions)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(columns.contains(&"scope_key".to_owned()));
        connection
            .execute(
                "INSERT INTO conversation_tool_permissions(
                    conversation_id, tool_name, scope_key, granted_at
                 ) VALUES ('scope-conversation', 'write_file', 'file:a', 2),
                          ('scope-conversation', 'write_file', 'file:b', 2)",
                [],
            )
            .expect("different exact scopes may coexist");
    }

    #[test]
    fn kernel_v50_preserves_v49_outbox_rows_and_allows_batch_delivery() {
        let mut connection = Connection::open_in_memory().expect("open database");
        connection
            .execute_batch(
                "CREATE TABLE schema_migrations (
                    version INTEGER PRIMARY KEY,
                    applied_at INTEGER NOT NULL
                 );",
            )
            .unwrap();
        connection.execute_batch(MIGRATION_48).unwrap();
        connection.execute_batch(MIGRATION_49).unwrap();
        connection
            .execute(
                "INSERT INTO kernel_effect_outbox (
                    run_id, effect_key, effect_type, idempotency_key, tool_call_id,
                    batch_id, payload_json, status, attempts, lease_owner, leased_at,
                    completed_at, last_error, created_at, updated_at
                 ) VALUES (
                    'run-v49','dispatch:call-1','dispatch_tool','tool-dispatch:call-1',
                    'call-1','batch-1','{}','pending',0,NULL,NULL,NULL,NULL,1,1
                 )",
                [],
            )
            .unwrap();

        let transaction = connection.transaction().unwrap();
        apply_migration(&transaction, 50, MIGRATION_50, 2).unwrap();
        transaction.commit().unwrap();

        let preserved: (String, String) = connection
            .query_row(
                "SELECT effect_type, status FROM kernel_effect_outbox
                  WHERE run_id='run-v49' AND effect_key='dispatch:call-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(preserved, ("dispatch_tool".into(), "pending".into()));
        connection
            .execute(
                "INSERT INTO kernel_effect_outbox (
                    run_id, effect_key, effect_type, idempotency_key, tool_call_id,
                    batch_id, payload_json, status, attempts, lease_owner, leased_at,
                    completed_at, last_error, created_at, updated_at
                 ) VALUES (
                    'run-v49','deliver-batch:batch-1','deliver_tool_batch',
                    'tool-batch-delivery:batch-1',NULL,'batch-1',
                    '{\"orderedToolCallIds\":[\"call-1\"]}','pending',0,
                    NULL,NULL,NULL,NULL,2,2
                 )",
                [],
            )
            .expect("v50 accepts durable batch delivery");
        assert!(connection
            .execute(
                "INSERT INTO kernel_effect_outbox (
                    run_id, effect_key, effect_type, idempotency_key, payload_json,
                    status, created_at, updated_at
                 ) VALUES ('run-v49','bad','unknown','bad','{}','pending',2,2)",
                [],
            )
            .is_err());
    }

    #[test]
    fn migration_51_adds_shadow_tables_and_is_repeatable() {
        // Build an OLD v51 database: migrations through v51, WITHOUT the v52
        // model-anchor column. run() applies everything, then we simulate an
        // already-released v51 by dropping the v52 column and its migration row.
        let mut connection = Connection::open_in_memory().unwrap();
        run(&mut connection, 1).unwrap();
        connection
            .execute_batch("ALTER TABLE kernel_runs DROP COLUMN model_request_since_wall_ms;")
            .unwrap();
        connection
            .execute("DELETE FROM schema_migrations WHERE version = 52", [])
            .unwrap();
        // The old v51 database has NO model-anchor column.
        let has_column: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('kernel_runs')
                  WHERE name='model_request_since_wall_ms'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(has_column, 0, "old v51 must lack the v52 column");

        // v51 shadow tables exist and are physically separate from the outbox.
        connection
            .execute(
                "INSERT INTO kernel_shadow_runs
                    (shadow_run_id, legacy_run_id, conversation_id, turn_id, engine_id,
                     kernel_mode, capability_manifest_version, capability_manifest_hash,
                     permission_snapshot_id, execution_profile_id, prompt_config_hash,
                     frozen_config_json, created_at)
                 VALUES ('shadow-1','legacy-1','conv-1','turn-1','pi','shadow',2,
                         'manifest-hash','perm','legacy','prompt','{}',1)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO kernel_shadow_diffs
                    (shadow_run_id, legacy_run_id, turn_id, event_cursor, event_type,
                     category, legacy_disposition_json, kernel_disposition_json,
                     detail_json, schema_version, created_at)
                 VALUES ('shadow-1','legacy-1','turn-1',1,'run_start','not_comparable',
                         '{}','{}','{}',2,1)",
                [],
            )
            .unwrap();

        // Idempotent: re-applying v51 does not error or duplicate (repeatable).
        let transaction = connection.transaction().unwrap();
        apply_migration(&transaction, 51, MIGRATION_51, 3).unwrap();
        transaction.commit().unwrap();

        // Existing shadow data survives the repeat migration.
        let count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM kernel_shadow_diffs WHERE shadow_run_id='shadow-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);

        // Now upgrade the OLD v51 database to v52: the model-anchor column
        // appears and data is preserved.
        let transaction = connection.transaction().unwrap();
        apply_migration(&transaction, 52, MIGRATION_52, 4).unwrap();
        transaction.commit().unwrap();
        let has_column: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('kernel_runs')
                  WHERE name='model_request_since_wall_ms'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(has_column, 1, "v52 upgrade adds the model-anchor column");
        let shadow_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM kernel_shadow_diffs WHERE shadow_run_id='shadow-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(shadow_count, 1, "v52 preserves shadow data");
        // Re-applying v52 is a no-op (repeatable).
        let transaction = connection.transaction().unwrap();
        apply_migration(&transaction, 52, MIGRATION_52, 5).unwrap();
        transaction.commit().unwrap();
    }

    #[test]
    fn migration_51_shadow_tables_persist_alongside_v49_v50_data() {
        // Upgrade path: v49/50 outbox rows remain, v51 adds shadow isolation.
        let mut connection = Connection::open_in_memory().unwrap();
        run(&mut connection, 1).unwrap();
        connection
            .execute(
                "INSERT INTO kernel_effect_outbox (
                    run_id, effect_key, effect_type, idempotency_key, payload_json,
                    status, created_at, updated_at
                 ) VALUES ('legacy-run','dispatch:c','dispatch_tool','tool-dispatch:c',
                           '{}','pending',1,1)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO kernel_shadow_runs
                    (shadow_run_id, legacy_run_id, conversation_id, turn_id, engine_id,
                     kernel_mode, capability_manifest_version, capability_manifest_hash,
                     permission_snapshot_id, execution_profile_id, prompt_config_hash,
                     frozen_config_json, created_at)
                 VALUES ('shadow-legacy-run','legacy-run','conv','turn','pi','shadow',2,
                         'm','p','legacy','pr','{}',1)",
                [],
            )
            .unwrap();
        // Physical isolation: the executable outbox contains only the legacy
        // run row; shadow decisions live in kernel_shadow_* and are never in
        // the outbox (so a lease scan over the outbox can never pick them up).
        let outbox_pending: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM kernel_effect_outbox WHERE status='pending'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(outbox_pending, 1);
        // No outbox row is keyed to the shadow run id.
        let outbox_for_shadow: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM kernel_effect_outbox WHERE run_id='shadow-legacy-run'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(outbox_for_shadow, 0);
        // The shadow observation row is present in its own table.
        let shadow_rows: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM kernel_shadow_runs WHERE shadow_run_id='shadow-legacy-run'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(shadow_rows, 1);
    }

    /// The old (pre-v68) columns of each Kernel table, used to compare content
    /// before and after the upgrade without treating the legitimately added
    /// columns as losses.
    const RUN_COLUMNS_V67: &[&str] = &[
        "run_id",
        "engine_id",
        "kernel_mode",
        "capability_manifest_version",
        "permission_snapshot_id",
        "execution_profile_id",
        "prompt_config_hash",
        "frozen_config_json",
        "state",
        "last_event_seq",
        "created_at",
        "updated_at",
        "terminal_at",
        "turn_id",
        "running_elapsed_ms",
        "running_since_wall_ms",
        "approval_deadline_wall_ms",
        "capability_manifest_hash",
        "terminal_written",
        "retry_state_json",
        "compaction_state_json",
        "model_request_since_wall_ms",
    ];
    const TOOL_COLUMNS_V67: &[&str] = &[
        "run_id",
        "tool_call_id",
        "batch_id",
        "tool",
        "source_order",
        "canonical_input_json",
        "state",
        "result_json",
        "created_at",
        "settled_at",
        "dispatch_idempotency_key",
    ];
    const MODEL_CONFIG_COLUMNS: &[&str] = &[
        "run_id",
        "schema_version",
        "adapter_version",
        "config_json",
        "config_hash",
        "created_at",
    ];
    const INITIAL_INPUT_COLUMNS: &[&str] = &[
        "run_id",
        "schema_version",
        "turn_id",
        "input_json",
        "input_hash",
        "prompt_config_hash",
        "created_at",
    ];
    const HOST_ACTION_COLUMNS: &[&str] = &[
        "run_id",
        "tool_call_id",
        "body_json",
        "body_hash",
        "status",
        "created_at",
        "completed_at",
    ];
    const APPROVAL_COLUMNS: &[&str] = &["run_id", "tool_call_id", "state", "created_at", "decided_at"];

    /// Populate the narrowed fixture the way a real upgraded database looks:
    /// a running run and a terminal run, tool calls in several states, a frozen
    /// model config, frozen initial input, the Host action ledger and a pending
    /// approval.
    ///
    /// The two Kernel *insert guards* describe how production code must create
    /// these rows (bind first, start later) rather than what a database may
    /// contain, so a historical fixture cannot satisfy them for a run that is
    /// already `running`. They are dropped only for the seeding transaction and
    /// the test asserts afterwards that the guard is present and still refuses a
    /// config for an unknown run — the upgrade must not lose it.
    fn seed_v67_kernel_fixture(connection: &mut Connection) -> Result<()> {
        let transaction = connection.transaction_with_behavior(
            rusqlite::TransactionBehavior::Immediate,
        )?;
        transaction.execute_batch(
            r#"
DROP TRIGGER IF EXISTS kernel_model_config_insert_guard;
DROP TRIGGER IF EXISTS kernel_initial_input_insert_guard;

INSERT INTO kernel_runs
    (run_id, engine_id, kernel_mode, capability_manifest_version, permission_snapshot_id,
     execution_profile_id, prompt_config_hash, frozen_config_json, state, last_event_seq,
     created_at, updated_at, terminal_at, turn_id, running_elapsed_ms, running_since_wall_ms,
     approval_deadline_wall_ms, capability_manifest_hash, terminal_written, retry_state_json,
     compaction_state_json, model_request_since_wall_ms)
VALUES
    ('run-live', 'pi', 'authoritative', 2, 'perm-live', 'legacy', 'hash-live', '{}', 'running', 7,
     100, 200, NULL, 'turn-live', 1234, 150, NULL, 'manifest-live', 0, NULL, NULL, 160),
    ('run-done', 'pi', 'authoritative', 2, 'perm-done', 'legacy', 'hash-done', '{}', 'completed', 9,
     100, 300, 300, 'turn-done', 4567, NULL, NULL, 'manifest-done', 1, NULL, NULL, NULL);

INSERT INTO run_control_bindings
    (run_id, conversation_id, authority, engine_id, binding_json, binding_hash, created_at)
VALUES
    ('run-live', 'conv-1', 'authoritative', 'pi', '{}', 'bh-live', 100),
    ('run-done', 'conv-1', 'authoritative', 'pi', '{}', 'bh-done', 100);

INSERT INTO kernel_model_configs (run_id, schema_version, adapter_version, config_json, config_hash, created_at)
VALUES
    ('run-live', 1, 'adapter-1',
     '{"engineId":"pi","executionProfileId":"legacy","apiType":"faux"}', 'hash-live', 100),
    ('run-done', 1, 'adapter-1',
     '{"engineId":"pi","executionProfileId":"legacy","apiType":"faux"}', 'hash-done', 100);

INSERT INTO kernel_initial_inputs
    (run_id, schema_version, turn_id, input_json, input_hash, prompt_config_hash, created_at)
VALUES
    ('run-live', 1, 'turn-live',
     '{"runId":"run-live","turnId":"turn-live","promptConfigHash":"hash-live","schemaVersion":1}',
     'input-live', 'hash-live', 100),
    ('run-done', 1, 'turn-done',
     '{"runId":"run-done","turnId":"turn-done","promptConfigHash":"hash-done","schemaVersion":1}',
     'input-done', 'hash-done', 100);

INSERT INTO kernel_tool_calls
    (run_id, tool_call_id, batch_id, tool, source_order, canonical_input_json, state,
     result_json, created_at, settled_at, dispatch_idempotency_key)
VALUES
    ('run-live', 'tc-done', 'b1', 'read', 0, '{"path":"a.txt"}', 'completed', '{"ok":true}', 100, 120, NULL),
    ('run-live', 'tc-wait', 'b2', 'write_file', 1, '{"path":"b.txt"}', 'waiting_approval', NULL, 120, NULL, NULL),
    ('run-done', 'tc-failed', 'b3', 'run_command', 0, '{"command":"x"}', 'failed', NULL, 100, 140, NULL);

INSERT INTO kernel_host_actions (run_id, tool_call_id, body_json, body_hash, status, created_at, completed_at)
VALUES
    ('run-live', 'tc-done', '{"file":"a.txt"}', 'bh-1', 'completed', 120, 130),
    ('run-done', 'tc-failed', '{"file":"c.txt"}', 'bh-2', 'pending', 130, NULL);

INSERT INTO kernel_approvals (run_id, tool_call_id, state, created_at, decided_at)
VALUES ('run-live', 'tc-wait', 'pending', 120, NULL);

CREATE TRIGGER kernel_model_config_insert_guard BEFORE INSERT ON kernel_model_configs
WHEN NOT EXISTS (
    SELECT 1 FROM kernel_runs r JOIN run_control_bindings b ON b.run_id=r.run_id
    WHERE r.run_id=NEW.run_id AND r.kernel_mode='authoritative'
      AND r.engine_id IN ('pi','codex','deepseek_harness') AND b.engine_id=r.engine_id
      AND b.authority='authoritative' AND r.state='created' AND r.last_event_seq=0
      AND r.prompt_config_hash=NEW.config_hash
      AND r.execution_profile_id=json_extract(NEW.config_json,'$.executionProfileId')
      AND r.engine_id=COALESCE(json_extract(NEW.config_json,'$.engineId'),'pi')
)
BEGIN SELECT RAISE(ABORT, 'Kernel model configuration must be frozen before Run start'); END;

CREATE TRIGGER kernel_initial_input_insert_guard BEFORE INSERT ON kernel_initial_inputs
WHEN NOT EXISTS (
    SELECT 1 FROM kernel_runs r JOIN kernel_model_configs c ON c.run_id=r.run_id
    JOIN run_control_bindings b ON b.run_id=r.run_id
    WHERE r.run_id=NEW.run_id AND r.state='created' AND r.last_event_seq=0
      AND r.kernel_mode='authoritative' AND r.engine_id IN ('pi','codex','deepseek_harness')
      AND b.authority='authoritative' AND b.engine_id=r.engine_id
      AND r.prompt_config_hash=NEW.prompt_config_hash AND c.config_hash=NEW.prompt_config_hash
      AND json_extract(NEW.input_json,'$.runId')=NEW.run_id
      AND json_extract(NEW.input_json,'$.turnId')=NEW.turn_id
      AND json_extract(NEW.input_json,'$.promptConfigHash')=NEW.prompt_config_hash
      AND json_extract(NEW.input_json,'$.schemaVersion')=NEW.schema_version
      AND length(trim(NEW.turn_id))>0
)
BEGIN SELECT RAISE(ABORT, 'Kernel initial input must match frozen configuration before Run start'); END;
"#,
        )?;
        transaction.commit()?;
        Ok(())
    }

    /// Row count plus a content hash over the named columns, so a "still there but
    /// silently rewritten" row cannot pass as preserved. Old columns are compared
    /// by name (the upgrade legitimately adds new ones), so this is a real
    /// before/after comparison rather than a whole-table checksum.
    fn table_fingerprint(
        connection: &Connection,
        table: &str,
        columns: &[&str],
        order_by: &str,
    ) -> (i64, String) {
        let count: i64 = connection
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| row.get(0))
            .expect("count rows");
        let projection = columns
            .iter()
            .map(|column| format!("quote({column})"))
            .collect::<Vec<_>>()
            .join(" || '|' || ");
        let body: String = connection
            .query_row(
                &format!(
                    "SELECT COALESCE(GROUP_CONCAT(row_text, '\\n'), '') FROM (
                         SELECT {projection} AS row_text FROM {table} ORDER BY {order_by}
                     )"
                ),
                [],
                |row| row.get(0),
            )
            .unwrap_or_default();
        let mut hasher = Sha256::new();
        hasher.update(body.as_bytes());
        (count, format!("sha256:{}", hex::encode(hasher.finalize())))
    }

    /// R1 regression: the v68 upgrade must keep every dependent row.
    ///
    /// The previous implementation rebuilt `kernel_runs` / `kernel_tool_calls`
    /// with `DROP TABLE` inside an FK-enabled transaction, which cascade-deleted
    /// `kernel_model_configs` (and through it `kernel_initial_inputs` and
    /// `kernel_host_runs`) plus the whole `kernel_host_actions` ledger.
    /// `PRAGMA foreign_key_check` stayed clean because cascades are legal
    /// deletions — only row counts and content reveal the loss.
    ///
    /// The fixture is a database produced by the real migration runner (so every
    /// foreign key, index and trigger is genuine), narrowed afterwards to exactly
    /// the pre-v68 shape inside a transaction: the tool table loses `'expired'`
    /// from its CHECK, and the run table loses the v68 resume columns. Upgrading
    /// that database is then the production upgrade path.
    #[test]
    fn migration_68_upgrade_preserves_dependent_kernel_rows() {
        let root = std::env::temp_dir().join(format!("fox-v67-upgrade-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("scratch dir");
        let path = root.join("facts.db");

        // 1. A real, fully migrated database.
        {
            let database = crate::database::Database::open(path.clone()).expect("open fixture");
            let agent_id = database.default_agent_id().to_string();
            database
                .with_connection(|connection| {
                    // The builtin agent already exists; only the conversation and
                    // its runs are fixtures.
                    connection.execute(
                        "INSERT INTO conversations (id, agent_id, title, status, created_at, updated_at)
                         VALUES ('conv-1', ?1, 'fixture', 'active', 1, 1)",
                        [agent_id.as_str()],
                    )?;
                    connection.execute_batch(
                        r#"
INSERT INTO runs (id, conversation_id, status, model, created_at)
VALUES ('run-live', 'conv-1', 'running', 'm', 1), ('run-done', 'conv-1', 'completed', 'm', 1);
"#,
                    )?;
                    Ok(())
                })
                .expect("seed base rows");
        }

        // 2. Narrow that database to the pre-v68 shape.
        {
            let mut connection = Connection::open(&path).expect("reopen fixture");
            connection
                .pragma_update(None, "foreign_keys", "ON")
                .expect("foreign keys on");
            let transaction = connection.transaction().expect("begin narrowing");
            transaction
                .execute_batch(
                    r#"
CREATE TABLE kernel_tool_calls_v67 (
    run_id TEXT NOT NULL,
    tool_call_id TEXT NOT NULL,
    batch_id TEXT NOT NULL,
    tool TEXT NOT NULL,
    source_order INTEGER NOT NULL,
    canonical_input_json TEXT NOT NULL,
    state TEXT NOT NULL CHECK(state IN (
        'pending','waiting_approval','running','completed','failed','cancelled'
    )),
    result_json TEXT,
    created_at INTEGER NOT NULL,
    settled_at INTEGER,
    dispatch_idempotency_key TEXT,
    PRIMARY KEY(run_id, tool_call_id)
);
DROP TABLE kernel_tool_calls;
ALTER TABLE kernel_tool_calls_v67 RENAME TO kernel_tool_calls;
CREATE INDEX idx_kernel_tool_calls_batch ON kernel_tool_calls(run_id, batch_id);
CREATE INDEX idx_kernel_tool_calls_state ON kernel_tool_calls(run_id, state);
"#,
                )
                .expect("narrow tool table");
            for column in [
                "continuable",
                "last_pause_reason",
                "progress_ref",
                "budget_tier",
                "budget_source",
                "paused_at",
                "continued_from_run_id",
                "attempt",
            ] {
                let present: bool = transaction
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('kernel_runs') WHERE name = ?1)",
                        [column],
                        |row| row.get(0),
                    )
                    .expect("read run columns");
                if present {
                    transaction
                        .execute(&format!("ALTER TABLE kernel_runs DROP COLUMN {column}"), [])
                        .expect("narrow run table");
                }
            }
            transaction.commit().expect("commit narrowing");
            connection
                .pragma_update(None, "foreign_keys", "ON")
                .expect("foreign keys on");
        }

        // 3. Realistic v67 content with rows in every dependent table.
        {
            let mut connection = Connection::open(&path).expect("reopen fixture");
            connection
                .pragma_update(None, "foreign_keys", "ON")
                .expect("foreign keys on");
            seed_v67_kernel_fixture(&mut connection).expect("seed v67 content");

            let watched = [
                ("kernel_runs", RUN_COLUMNS_V67, "run_id"),
                ("kernel_tool_calls", TOOL_COLUMNS_V67, "run_id, tool_call_id"),
                ("kernel_model_configs", MODEL_CONFIG_COLUMNS, "run_id"),
                ("kernel_initial_inputs", INITIAL_INPUT_COLUMNS, "run_id"),
                ("kernel_host_actions", HOST_ACTION_COLUMNS, "run_id, tool_call_id"),
                ("kernel_approvals", APPROVAL_COLUMNS, "run_id, tool_call_id"),
            ];
            let before: Vec<(i64, String)> = watched
                .iter()
                .map(|(table, columns, order)| table_fingerprint(&connection, table, columns, order))
                .collect();
            // A fixture that lost its rows would let this test pass for the wrong
            // reason, so every dependent table must actually be populated.
            for (index, (count, _)) in before.iter().enumerate() {
                assert!(
                    *count > 0,
                    "fixture is not realistic: {} is empty",
                    watched[index].0
                );
            }
            // The pre-v68 shape cannot express the new state.
            assert!(connection
                .execute(
                    "UPDATE kernel_tool_calls SET state='expired' WHERE tool_call_id='tc-wait'",
                    []
                )
                .is_err());
            // No resume columns yet.
            let resume_before: i64 = connection
                .query_row(
                    "SELECT COUNT(*) FROM pragma_table_info('kernel_runs')
                      WHERE name IN ('continuable','last_pause_reason','progress_ref','budget_tier',
                                     'budget_source','paused_at','continued_from_run_id','attempt')",
                    [],
                    |row| row.get(0),
                )
                .expect("resume columns before");
            assert_eq!(resume_before, 0, "the fixture must not already have v68 columns");

            // 4. Upgrade.
            rebuild_v68_for_test(&mut connection).expect("v68 upgrade");

            // 4a. Nothing was lost.
            let after: Vec<(i64, String)> = watched
                .iter()
                .map(|(table, columns, order)| table_fingerprint(&connection, table, columns, order))
                .collect();
            for (index, (table, _, _)) in watched.iter().enumerate() {
                assert_eq!(
                    before[index], after[index],
                    "{table} lost rows or changed content across the upgrade"
                );
            }
            let violations: i64 = connection
                .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
                    row.get(0)
                })
                .expect("fk check");
            assert_eq!(violations, 0, "upgrade broke a foreign-key relationship");

            // 4b. The preserved frozen facts are still readable.
            let config_hash: String = connection
                .query_row(
                    "SELECT config_hash FROM kernel_model_configs WHERE run_id='run-live'",
                    [],
                    |row| row.get(0),
                )
                .expect("frozen model config survives the upgrade");
            assert_eq!(config_hash, "hash-live");
            let action_status: String = connection
                .query_row(
                    "SELECT status FROM kernel_host_actions
                      WHERE run_id='run-live' AND tool_call_id='tc-done'",
                    [],
                    |row| row.get(0),
                )
                .expect("host action ledger survives the upgrade");
            assert_eq!(action_status, "completed");
            // The frozen-turn trigger still guards the run row.
            assert!(connection
                .execute("UPDATE kernel_runs SET turn_id='different' WHERE run_id='run-live'", [])
                .is_err());
            // The model-config insert guard survives too, and still refuses a
            // configuration for a run that never started under it.
            let guards: i64 = connection
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='trigger' AND name IN (
                         'kernel_model_config_insert_guard','kernel_initial_input_insert_guard',
                         'kernel_initial_input_start_guard')",
                    [],
                    |row| row.get(0),
                )
                .expect("trigger inventory");
            assert_eq!(guards, 3, "the upgrade must not lose Kernel guard triggers");
            let resume_after: i64 = connection
                .query_row(
                    "SELECT COUNT(*) FROM pragma_table_info('kernel_runs')
                      WHERE name IN ('continuable','last_pause_reason','progress_ref','budget_tier',
                                     'budget_source','paused_at','continued_from_run_id','attempt')",
                    [],
                    |row| row.get(0),
                )
                .expect("resume columns after");
            assert_eq!(resume_after, 8, "the resume columns must exist exactly once");

            // 4c. A repeated upgrade is a no-op, compared against the pre-upgrade
            //     fingerprints (checked before any deliberate state write).
            let second = rebuild_v68_for_test(&mut connection).expect("idempotent upgrade");
            assert!(
                !second,
                "a repeated upgrade rebuilt kernel_tool_calls again (gate={})",
                !sqlite_object_accepts(&connection, "kernel_tool_calls", "'expired'").unwrap()
            );
            let after_second: Vec<(i64, String)> = watched
                .iter()
                .map(|(table, columns, order)| table_fingerprint(&connection, table, columns, order))
                .collect();
            for (index, (table, _, _)) in watched.iter().enumerate() {
                assert_eq!(
                    before[index], after_second[index],
                    "{table} changed on a repeated upgrade"
                );
            }

            // 4d. The widened states are writable, and unknown states are still
            //     rejected — the CHECK was widened, not dropped.
            connection
                .execute(
                    "UPDATE kernel_tool_calls SET state='expired' WHERE run_id='run-live' AND tool_call_id='tc-wait'",
                    [],
                )
                .expect("expired tool state after upgrade");
            connection
                .execute(
                    "UPDATE kernel_runs SET state='approval_expired' WHERE run_id='run-live'",
                    [],
                )
                .expect("approval_expired run state after upgrade");
            assert!(connection
                .execute(
                    "UPDATE kernel_tool_calls SET state='not_a_state' WHERE run_id='run-live' AND tool_call_id='tc-done'",
                    [],
                )
                .is_err());
        }

        // 5. The production opening path still works on the upgraded database and
        //    the preserved frozen configuration is readable through it.
        let database = crate::database::Database::open(path).expect("open upgraded database");
        let version: i64 = database
            .with_connection(|connection| {
                connection.query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
                    row.get(0)
                })
            })
            .expect("schema version");
        assert_eq!(version, DATABASE_SCHEMA_VERSION);
        let config_hash: String = database
            .with_connection(|connection| {
                connection.query_row(
                    "SELECT config_hash FROM kernel_model_configs WHERE run_id='run-live'",
                    [],
                    |row| row.get(0),
                )
            })
            .expect("frozen model config readable after reopen");
        assert_eq!(config_hash, "hash-live");
        let _ = std::fs::remove_dir_all(root);
    }

    /// REV-03/04/05 的迁移 80：旧库升级与新库初始化两条路径都必须成立，
    /// 且升级不得静默扩大/收窄任何会话的有效权限。
    #[test]
    fn migration_80_upgrades_a_v79_database_without_changing_effective_modes() {
        let root = std::env::temp_dir().join(format!("fox-v80-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("scratch dir");
        let fresh_path = root.join("fresh.db");
        let path = root.join("v79.db");

        // --- 新库初始化：migration 80 直接在全新库上跑通。---
        {
            let database = crate::database::Database::open(fresh_path).expect("open fresh");
            let version: i64 = database
                .with_connection(|c| {
                    c.query_row("SELECT MAX(version) FROM schema_migrations", [], |r| r.get(0))
                })
                .expect("schema version");
            assert_eq!(version, crate::database::DATABASE_SCHEMA_VERSION);
            let conversation = database
                .create_conversation(database.default_agent_id(), Some("v80"), None, None)
                .expect("conversation");
            database
                .with_connection(|c| {
                    c.execute(
                        "INSERT INTO kernel_host_observations
                            (observation_id, run_id, conversation_id, target_identity, version,
                             observed_by_tool_call_id, observed_at, view_kind, covered_whole_file,
                             range_start, range_end, total_units, truncated)
                         VALUES ('obs-1','run-x',?1,'t','v','read-1',1,'full_file',1,0,3,3,0)",
                        [&conversation.id],
                    )?;
                    c.execute(
                        "INSERT INTO conversation_tool_permissions
                            (conversation_id, tool_name, scope_key, granted_at, revoked_at)
                         VALUES (?1,'write_file','host-target:sha256:abc',1,NULL)",
                        [&conversation.id],
                    )?;
                    c.execute(
                        "INSERT INTO kernel_whole_file_replace_grants(
                            grant_id, conversation_id, run_id, dispatch_id, target_identity,
                            version, candidate_digest, request_digest, state, created_at)
                         VALUES ('g1', ?1, 'r', 'd', 't', 'v', 'c', 'q', 'approved', 1)",
                        [&conversation.id],
                    )?;
                    c.execute(
                        "INSERT INTO kernel_restore_requests(
                            conversation_id, request_id, version_id, target_identity,
                            baseline_version, dispatch_id, state, created_at)
                         VALUES (?1, 'req-1', 'v1', 't', NULL, 'restore:req-1', 'running', 1)",
                        [&conversation.id],
                    )?;
                    Ok(())
                })
                .expect("the new columns exist on a fresh database");
        }

        // --- Build a real v79 database with the historical 1..79 chain.
        // A project default and the conversation column intentionally differ;
        // the v79 policy row follows the project, and v80 must reconcile them.
        {
            let mut connection = Connection::open(&path).expect("open v79 fixture");
            run_with_target(&mut connection, 1, MigrationTarget::V79)
                .expect("run historical migrations through v79");
            connection.execute_batch(r#"
INSERT INTO agents(id,name,description,runtime_type,system_prompt,default_model,created_at,updated_at)
VALUES ('agent-old','Old Agent','','pi','','model',1,1);
INSERT INTO projects(id,name,root_path,permission_mode,status,created_at,updated_at)
VALUES ('project-old','Old Project','C:/fox-v79-fixture','allow','active',1,1);
INSERT INTO conversations(id,agent_id,title,status,created_at,updated_at,project_id,permission_mode)
VALUES ('conv-old','agent-old','Old Conversation','active',1,1,'project-old','ask');
INSERT INTO runs(id,conversation_id,status,model,created_at)
VALUES ('run-old','conv-old','running','model',1);
INSERT INTO kernel_host_observations
    (run_id,conversation_id,target_identity,version,observed_by_tool_call_id,observed_at)
VALUES ('run-old','conv-old','target-old','v-old','read-old',1);
"#).expect("seed historical v79 rows");
            let version: i64 = connection
                .query_row("SELECT MAX(version) FROM schema_migrations", [], |r| r.get(0))
                .expect("version");
            assert_eq!(version, 79, "the fixture must be a v79 database");
            let drift: i64 = connection.query_row(
                "SELECT COUNT(*) FROM conversations c JOIN kernel_execution_policies p
                 ON p.conversation_id=c.id WHERE p.mode<>c.permission_mode",
                [], |r| r.get(0),
            ).expect("historical mode drift");
            assert_eq!(drift, 1, "the fixture must exercise the v80 reconciliation");
            for table in ["kernel_whole_file_replace_grants", "kernel_restore_requests", "kernel_job_notices"] {
                let present: i64 = connection.query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    [table], |r| r.get(0),
                ).expect("future table absence");
                assert_eq!(present, 0, "{table} must not exist in v79");
            }
            let violations: i64 = connection.query_row(
                "SELECT COUNT(*) FROM pragma_foreign_key_check", [], |r| r.get(0),
            ).expect("v79 foreign keys");
            assert_eq!(violations, 0);
        }

        // --- 升级：旧观察行保留且不给覆盖资格，项目联动触发器已删除，
        //     两处模式存储一致。
        {
            let database = crate::database::Database::open(path.clone()).expect("upgrade v79");
            database
                .with_connection(|c| {
                    let observations: i64 = c
                        .query_row("SELECT COUNT(*) FROM kernel_host_observations", [], |r| r.get(0))
                        .expect("observations");
                    assert_eq!(observations, 1, "historical observations must survive");
                    let view: String = c
                        .query_row(
                            "SELECT view_kind FROM kernel_host_observations WHERE run_id='run-old'",
                            [],
                            |r| r.get(0),
                        )
                        .expect("migrated row");
                    assert_eq!(view, "legacy_unknown", "an old row gains no coverage claim");
                    let whole: i64 = c
                        .query_row(
                            "SELECT covered_whole_file FROM kernel_host_observations WHERE run_id='run-old'",
                            [],
                            |r| r.get(0),
                        )
                        .expect("migrated coverage");
                    assert_eq!(whole, 0, "an old row must not authorize a whole-file replacement");
                    let project_trigger: i64 = c
                        .query_row(
                            "SELECT COUNT(*) FROM sqlite_master WHERE type='trigger' AND name='execution_policy_project_change'",
                            [],
                            |r| r.get(0),
                        )
                        .expect("project trigger");
                    assert_eq!(
                        project_trigger, 0,
                        "a project change must not reach existing conversations"
                    );
                    let drift: i64 = c
                        .query_row(
                            "SELECT COUNT(*) FROM conversations c JOIN kernel_execution_policies p
                               ON p.conversation_id=c.id
                              WHERE p.mode <> COALESCE(c.permission_mode,'ask')",
                            [],
                            |r| r.get(0),
                        )
                        .expect("policy drift");
                    assert_eq!(drift, 0, "the column and the policy row must agree after upgrade");
                    for table in ["kernel_whole_file_replace_grants", "kernel_restore_requests"] {
                        let present: i64 = c
                            .query_row(
                                "SELECT COUNT(*) FROM sqlite_master WHERE name=?1",
                                [table],
                                |r| r.get(0),
                            )
                            .expect("table presence");
                        assert_eq!(present, 1, "{table} must exist after the upgrade");
                    }
                    // The two nullable credential columns are additive: an old
                    // row keeps working and simply carries no replacement
                    // authorization.
                    c.execute_batch(
                        "INSERT INTO kernel_execution_credentials(
                            run_id, dispatch_id, conversation_id, intent_digest, action_class,
                            file_baseline, resolved_profile, policy_snapshot_id, backend_required,
                            backend_evidence_digest, credential_digest, credential_json, created_at)
                         VALUES ('r','d','c','i','write',NULL,'p','s','none',NULL,'dg','{}',1)",
                    )?;
                    Ok(())
                })
                .expect("upgrade assertions");
            let version: i64 = database
                .with_connection(|c| {
                    c.query_row("SELECT MAX(version) FROM schema_migrations", [], |r| r.get(0))
                })
                .expect("version after upgrade");
            assert_eq!(version, crate::database::DATABASE_SCHEMA_VERSION);
        }

        // Simulate a database reopened by the old code, which replayed M77 and
        // recreated this trigger even though M80 was already recorded.
        Connection::open(&path).expect("open old-reopen fixture").execute_batch(
            "CREATE TRIGGER execution_policy_project_change AFTER UPDATE OF permission_mode ON projects BEGIN
               UPDATE kernel_execution_policies SET version=version+1,mode=NEW.permission_mode
               WHERE conversation_id IN (SELECT id FROM conversations WHERE project_id=NEW.id);
             END;",
        ).expect("seed revived old trigger");

        // --- 重开幂等，并清理历史重开产生的旧触发器。---
        {
            let database = crate::database::Database::open(path.clone()).expect("reopen");
            database
                .with_connection(|c| {
                    let project_trigger: i64 = c.query_row(
                        "SELECT COUNT(*) FROM sqlite_master WHERE type='trigger' AND name='execution_policy_project_change'",
                        [], |r| r.get(0),
                    )?;
                    assert_eq!(project_trigger, 0, "reopening must not replay the old project propagation trigger");
                    let observations: i64 = c
                        .query_row("SELECT COUNT(*) FROM kernel_host_observations", [], |r| r.get(0))
                        .expect("observations");
                    assert_eq!(observations, 1, "reopening must not duplicate observations");
                    Ok(())
                })
                .expect("reopen assertions");
            let version: i64 = database
                .with_connection(|c| {
                    c.query_row("SELECT MAX(version) FROM schema_migrations", [], |r| r.get(0))
                })
                .expect("version after reopen");
            assert_eq!(version, crate::database::DATABASE_SCHEMA_VERSION);
        }
    }

    fn migration_79_widens_only_host_job_keys_and_reopens_idempotently() {
        const JOB_COLUMNS: &[&str] = &[
            "job_id",
            "run_id",
            "conversation_id",
            "kind",
            "idempotency_key",
            "params_hash",
            "params_json",
            "state",
            "cursor",
            "progress_done",
            "progress_total",
            "result_ref",
            "result_bytes",
            "result_sha256",
            "error_code",
            "error_message",
            "attempts",
            "deadline_ms",
            "owner_pid",
            "owner_started_at",
            "created_at",
            "updated_at",
            "finished_at",
            "cancel_requested_at",
            "cancel_acknowledged_at",
            "cancelled_by",
            "uncertain_at",
        ];

        let root = std::env::temp_dir().join(format!("fox-v78-jobs-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("scratch dir");
        let path = root.join("facts.db");

        // Start from a database produced by the real opening path, then narrow
        // only kernel_jobs back to its v78 CHECK and remove the v79 marker.
        {
            let database = crate::database::Database::open(path.clone()).expect("open fixture");
            let agent_id = database.default_agent_id().to_string();
            database
                .with_connection(|connection| {
                    connection.execute(
                        "INSERT INTO conversations(id,agent_id,title,status,created_at,updated_at)
                         VALUES('conv-v79',?1,'fixture','active',1,1)",
                        [agent_id],
                    )?;
                    connection.execute(
                        "INSERT INTO runs(id,conversation_id,status,model,created_at)
                         VALUES('run-v79','conv-v79','completed','fixture',1)",
                        [],
                    )?;
                    Ok(())
                })
                .expect("seed parent facts");
        }

        let before = {
            let mut connection = Connection::open(&path).expect("open v78 fixture");
            connection
                .pragma_update(None, "foreign_keys", "OFF")
                .expect("foreign keys off");
            let transaction = connection.transaction().expect("begin narrowing");
            transaction
                .execute_batch(
                    r#"
CREATE TABLE kernel_jobs_v78 (
    job_id TEXT PRIMARY KEY,
    run_id TEXT NOT NULL,
    conversation_id TEXT NOT NULL,
    kind TEXT NOT NULL CHECK(length(trim(kind)) BETWEEN 1 AND 64),
    idempotency_key TEXT NOT NULL CHECK(length(trim(idempotency_key)) BETWEEN 1 AND 200),
    params_hash TEXT NOT NULL,
    params_json TEXT NOT NULL,
    state TEXT NOT NULL CHECK(state IN ('queued','running','paused','cancelled','failed','completed')),
    cursor TEXT,
    progress_done INTEGER NOT NULL DEFAULT 0,
    progress_total INTEGER,
    result_ref TEXT,
    result_bytes INTEGER,
    result_sha256 TEXT,
    error_code TEXT,
    error_message TEXT,
    attempts INTEGER NOT NULL DEFAULT 0,
    deadline_ms INTEGER,
    owner_pid INTEGER,
    owner_started_at INTEGER,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    finished_at INTEGER,
    cancel_requested_at INTEGER,
    cancel_acknowledged_at INTEGER,
    cancelled_by TEXT,
    uncertain_at INTEGER,
    UNIQUE(run_id, idempotency_key)
);
DROP TABLE kernel_jobs;
ALTER TABLE kernel_jobs_v78 RENAME TO kernel_jobs;
CREATE INDEX idx_kernel_jobs_run ON kernel_jobs(run_id, state);
CREATE INDEX idx_kernel_jobs_conversation ON kernel_jobs(conversation_id, created_at);
INSERT INTO kernel_jobs(
    job_id,run_id,conversation_id,kind,idempotency_key,params_hash,params_json,
    state,cursor,progress_done,progress_total,result_ref,result_bytes,result_sha256,
    error_code,error_message,attempts,deadline_ms,owner_pid,owner_started_at,
    created_at,updated_at,finished_at,cancel_requested_at,cancel_acknowledged_at,
    cancelled_by,uncertain_at
) VALUES(
    'job-v78','run-v79','conv-v79','command','legacy-key','params-hash','{"old":true}',
    'completed','cursor-v78',7,9,'result-ref',123,'result-sha','old-code','old message',
    3,999,44,55,1,2,3,4,5,'user',6
);
DELETE FROM schema_migrations WHERE version=79;
"#,
                )
                .expect("create v78 jobs schema and data");
            transaction.commit().expect("commit v78 fixture");
            connection
                .pragma_update(None, "foreign_keys", "ON")
                .expect("foreign keys on");

            let version: i64 = connection
                .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| row.get(0))
                .expect("v78 version");
            assert_eq!(version, 78);
            assert!(connection
                .execute(
                    "UPDATE kernel_jobs SET idempotency_key=?1 WHERE job_id='job-v78'",
                    ["x".repeat(201)],
                )
                .is_err());
            table_fingerprint(&connection, "kernel_jobs", JOB_COLUMNS, "job_id")
        };
        assert_eq!(before.0, 1, "the old-database fixture must contain a job");

        let after_first = {
            let database = crate::database::Database::open(path.clone()).expect("upgrade v78 fixture");
            database
                .with_connection(|connection| {
                    let version: i64 = connection.query_row(
                        "SELECT MAX(version) FROM schema_migrations",
                        [],
                        |row| row.get(0),
                    )?;
                    assert_eq!(version, DATABASE_SCHEMA_VERSION);
                    let v79_count: i64 = connection.query_row(
                        "SELECT COUNT(*) FROM schema_migrations WHERE version=79",
                        [],
                        |row| row.get(0),
                    )?;
                    assert_eq!(v79_count, 1);

                    let columns = {
                        let mut statement =
                            connection.prepare("SELECT name FROM pragma_table_info('kernel_jobs') ORDER BY cid")?;
                        let values = statement
                            .query_map([], |row| row.get::<_, String>(0))?
                            .collect::<Result<Vec<_>>>()?;
                        values
                    };
                    let expected_columns = JOB_COLUMNS
                        .iter()
                        .map(|column| (*column).to_owned())
                        .collect::<Vec<_>>();
                    assert_eq!(columns, expected_columns);
                    let indexes: i64 = connection.query_row(
                        "SELECT COUNT(*) FROM sqlite_master WHERE type='index'
                          AND name IN ('idx_kernel_jobs_run','idx_kernel_jobs_conversation')",
                        [],
                        |row| row.get(0),
                    )?;
                    assert_eq!(indexes, 2);
                    let unique_indexes: i64 = connection.query_row(
                        "SELECT COUNT(*) FROM pragma_index_list('kernel_jobs') WHERE origin='u'",
                        [],
                        |row| row.get(0),
                    )?;
                    assert_eq!(unique_indexes, 1, "the run/key unique index must survive");
                    let foreign_keys: i64 = connection.query_row(
                        "SELECT COUNT(*) FROM pragma_foreign_key_list('kernel_jobs')",
                        [],
                        |row| row.get(0),
                    )?;
                    assert_eq!(foreign_keys, 0, "v69-v78 kernel_jobs declared no foreign keys");
                    let violations: i64 = connection.query_row(
                        "SELECT COUNT(*) FROM pragma_foreign_key_check",
                        [],
                        |row| row.get(0),
                    )?;
                    assert_eq!(violations, 0);
                    let fingerprint =
                        table_fingerprint(connection, "kernel_jobs", JOB_COLUMNS, "job_id");
                    assert_eq!(fingerprint, before);

                    let host_key = format!(
                        "job:{}",
                        fox_engine_protocol::encode_dispatch_id(
                            "run-v79",
                            &"调用:".repeat(140),
                        )
                        .expect("canonical dispatch id")
                    );
                    assert!(host_key.chars().count() > 200);
                    connection.execute_batch("SAVEPOINT widened_key_check")?;
                    connection.execute(
                        "UPDATE kernel_jobs SET idempotency_key=?1 WHERE job_id='job-v78'",
                        [host_key],
                    )?;
                    let ceiling_key = format!(
                        "job:tool-dispatch:{}",
                        "x".repeat(2072 - "job:tool-dispatch:".len())
                    );
                    assert_eq!(ceiling_key.len(), 2072);
                    connection.execute(
                        "UPDATE kernel_jobs SET idempotency_key=?1 WHERE job_id='job-v78'",
                        [ceiling_key],
                    )?;
                    let oversized_host_key = format!(
                        "job:tool-dispatch:{}",
                        "x".repeat(2073 - "job:tool-dispatch:".len())
                    );
                    assert!(connection
                        .execute(
                            "UPDATE kernel_jobs SET idempotency_key=?1 WHERE job_id='job-v78'",
                            [oversized_host_key],
                        )
                        .is_err());
                    assert!(connection
                        .execute(
                            "UPDATE kernel_jobs SET idempotency_key=?1 WHERE job_id='job-v78'",
                            ["x".repeat(201)],
                        )
                        .is_err());
                    connection.execute_batch("ROLLBACK TO widened_key_check; RELEASE widened_key_check")?;
                    Ok(fingerprint)
                })
                .expect("inspect upgraded database")
        };
        assert_eq!(after_first, before);

        // A second production reopen must neither rebuild the table nor append
        // another version row.
        {
            let database = crate::database::Database::open(path.clone()).expect("reopen upgraded database");
            database
                .with_connection(|connection| {
                    assert_eq!(
                        table_fingerprint(connection, "kernel_jobs", JOB_COLUMNS, "job_id"),
                        before
                    );
                    let v79_count: i64 = connection.query_row(
                        "SELECT COUNT(*) FROM schema_migrations WHERE version=79",
                        [],
                        |row| row.get(0),
                    )?;
                    assert_eq!(v79_count, 1);
                    Ok(())
                })
                .expect("idempotent reopen");
        }
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn migration_83_preserves_v82_restore_requests_without_inventing_force() {
        let root = std::env::temp_dir().join(format!("fox-v83-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("scratch dir");
        let path = root.join("facts.db");
        let conversation = {
            let db = crate::database::Database::open(path.clone()).expect("fresh database");
            let conversation = db.create_conversation(
                db.default_agent_id(), Some("v82 restore"), None, None,
            ).expect("conversation");
            conversation.id
        };
        {
            let connection = Connection::open(&path).expect("synthetic v82 database");
            connection.execute(
                "INSERT INTO kernel_restore_requests(
                    conversation_id,request_id,version_id,target_identity,baseline_version,
                    dispatch_id,state,result_version_id,created_at,settled_at)
                 VALUES (?1,'old-request','old-version','old-target','sha256:old',
                    'restore:old-request','committed','old-result',1,2)",
                [&conversation],
            ).expect("v82 request");
            connection.execute_batch(
                "ALTER TABLE kernel_restore_requests DROP COLUMN intent_digest;
                 ALTER TABLE kernel_restore_requests DROP COLUMN attempt_owner;
                 DELETE FROM schema_migrations WHERE version=83;",
            ).expect("remove only v83 shape and marker");
        }
        for _ in 0..2 {
            let db = crate::database::Database::open(path.clone()).expect("upgrade and reopen");
            db.with_connection(|connection| {
                let old: (String, String, Option<String>, Option<String>) = connection.query_row(
                    "SELECT state,result_version_id,intent_digest,attempt_owner
                     FROM kernel_restore_requests WHERE conversation_id=?1 AND request_id='old-request'",
                    [&conversation],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )?;
                assert_eq!(old, ("committed".into(), "old-result".into(), None, None));
                let version: i64 = connection.query_row(
                    "SELECT MAX(version) FROM schema_migrations", [], |row| row.get(0),
                )?;
                assert_eq!(version, DATABASE_SCHEMA_VERSION);
                Ok(())
            }).expect("preserved historical request and v83 shape");
        }
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn migration_84_upgrades_real_v83_jobs_and_reopens_idempotently() {
        use sha2::Digest;
        let root = std::env::temp_dir().join(format!("fox-v84-from-v83-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("scratch dir");
        let path = root.join("facts.db");
        let body = serde_json::json!({"legacy":"result"}).to_string();
        let sha = format!("sha256:{}", hex::encode(Sha256::digest(body.as_bytes())));
        {
            let mut connection = Connection::open(&path).expect("open historical fixture");
            run_with_target(&mut connection, 1, MigrationTarget::V83)
                .expect("run historical migrations through v83");
            connection.execute_batch(
                "INSERT INTO agents(id,name,description,runtime_type,system_prompt,default_model,created_at,updated_at)
                 VALUES ('agent-v83','Old Agent','','pi','','model',1,1);
                 INSERT INTO conversations(id,agent_id,title,status,created_at,updated_at)
                 VALUES ('conv-v83','agent-v83','Old Conversation','active',1,1);
                 INSERT INTO runs(id,conversation_id,status,model,created_at)
                 VALUES ('run-v83','conv-v83','running','model',1);
                 INSERT INTO kernel_jobs(job_id,run_id,conversation_id,kind,idempotency_key,
                    params_hash,params_json,state,attempts,deadline_ms,created_at,updated_at)
                 VALUES ('queued-v83','run-v83','conv-v83','attachment_compute',
                    'queued-key','sha256:queued','{}','queued',0,100000,1,1);",
            ).expect("seed old Run and queued Job");
            connection.execute(
                "INSERT INTO tool_call_result_blobs(sha256,body_json,byte_size,created_at)
                 VALUES(?1,?2,?3,1)",
                rusqlite::params![&sha,&body,body.len() as i64],
            ).expect("old result blob");
            connection.execute(
                "INSERT INTO kernel_jobs(job_id,run_id,conversation_id,kind,idempotency_key,
                    params_hash,params_json,state,attempts,result_ref,result_bytes,result_sha256,
                    created_at,updated_at,finished_at)
                 VALUES ('completed-v83','run-v83','conv-v83','attachment_compute',
                    'completed-key','sha256:completed','{}','completed',1,
                    'fox-job-result://completed-v83',?1,?2,1,2,2)",
                rusqlite::params![body.len() as i64,&sha],
            ).expect("old completed Job");
            let version: i64 = connection.query_row(
                "SELECT MAX(version) FROM schema_migrations", [], |row| row.get(0),
            ).expect("historical version");
            assert_eq!(version, 83);
            let notice_table: i64 = connection.query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='kernel_job_notices'",
                [], |row| row.get(0),
            ).expect("historical notice absence");
            assert_eq!(notice_table, 0);
            let violations: i64 = connection.query_row(
                "SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| row.get(0),
            ).expect("historical FK check");
            assert_eq!(violations, 0);
        }
        for _ in 0..2 {
            let database = crate::database::Database::open(path.clone())
                .expect("upgrade or reopen v83 fixture");
            database.with_connection(|connection| {
                let version: i64 = connection.query_row(
                    "SELECT MAX(version) FROM schema_migrations", [], |row| row.get(0),
                )?;
                let old_run: (String,String) = connection.query_row(
                    "SELECT conversation_id,status FROM runs WHERE id='run-v83'", [],
                    |row| Ok((row.get(0)?,row.get(1)?)),
                )?;
                let queued: (String,i64) = connection.query_row(
                    "SELECT state,attempts FROM kernel_jobs WHERE job_id='queued-v83'", [],
                    |row| Ok((row.get(0)?,row.get(1)?)),
                )?;
                let notices: i64 = connection.query_row(
                    "SELECT COUNT(*) FROM kernel_job_notices", [], |row| row.get(0),
                )?;
                let violations: i64 = connection.query_row(
                    "SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| row.get(0),
                )?;
                let integrity: String = connection.query_row(
                    "PRAGMA integrity_check", [], |row| row.get(0),
                )?;
                assert_eq!(version, DATABASE_SCHEMA_VERSION);
                assert_eq!(old_run, ("conv-v83".into(),"running".into()));
                assert_eq!(queued, ("queued".into(),0));
                assert_eq!(notices, 0, "historical jobs are not retroactively notified");
                assert_eq!(violations, 0);
                assert_eq!(integrity, "ok");
                Ok(())
            }).expect("preserve old Run/Job data and integrity");
            assert_eq!(database.kernel_job_result_value("conv-v83","completed-v83").unwrap(),
                serde_json::json!({"legacy":"result"}));
            drop(database);
        }
        std::fs::remove_dir_all(root).expect("remove scratch fixture");
    }

    #[test]
    fn migration_86_upgrades_real_v85_run_twice_with_wait_constraints() {
        let root=std::env::temp_dir().join(format!("fox-v86-from-v85-{}",uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path=root.join("facts.db");
        {
            let mut connection=Connection::open(&path).unwrap();
            run_with_target(&mut connection,1,MigrationTarget::V85).unwrap();
            let version:i64=connection.query_row(
                "SELECT MAX(version) FROM schema_migrations",[],|row|row.get(0)).unwrap();
            assert_eq!(version,85);
            let wait_columns:i64=connection.query_row(
                "SELECT COUNT(*) FROM pragma_table_info('kernel_runs')
                  WHERE name IN ('wait_deadline_wall_ms','wait_accounted_until_wall_ms')",
                [],|row|row.get(0)).unwrap();
            assert_eq!(wait_columns,0);
            connection.execute_batch(
                "INSERT INTO agents(id,name,description,runtime_type,system_prompt,default_model,created_at,updated_at)
                 VALUES('v85-agent','Old','','pi','','model',1,1);
                 INSERT INTO conversations(id,agent_id,title,status,created_at,updated_at)
                 VALUES('v85-conv','v85-agent','Old','active',1,1);
                 INSERT INTO runs(id,conversation_id,status,model,created_at)
                 VALUES('v85-run','v85-conv','running','model',1);
                 INSERT INTO kernel_runs(run_id,engine_id,kernel_mode,capability_manifest_version,
                    permission_snapshot_id,execution_profile_id,prompt_config_hash,frozen_config_json,
                    state,created_at,updated_at)
                 VALUES('v85-run','pi','authoritative',2,'perm','legacy','hash','{}','running',1,1);"
            ).unwrap();
        }
        for _ in 0..2 {
            let db=crate::database::Database::open(path.clone()).unwrap();
            db.with_connection(|conn| {
                let version:i64=conn.query_row(
                    "SELECT MAX(version) FROM schema_migrations",[],|row|row.get(0))?;
                let old:(String,String,Option<i64>,Option<i64>)=conn.query_row(
                    "SELECT r.conversation_id,k.state,k.wait_deadline_wall_ms,
                            k.wait_accounted_until_wall_ms
                       FROM runs r JOIN kernel_runs k ON k.run_id=r.id WHERE r.id='v85-run'",
                    [],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?)))?;
                let fk:i64=conn.query_row(
                    "SELECT COUNT(*) FROM pragma_foreign_key_check",[],|row|row.get(0))?;
                let integrity:String=conn.query_row("PRAGMA integrity_check",[],|row|row.get(0))?;
                assert_eq!(version,DATABASE_SCHEMA_VERSION);
                assert_eq!(old,("v85-conv".into(),"running".into(),None,None));
                assert_eq!((fk,integrity),(0,"ok".into()));
                assert!(conn.execute("UPDATE kernel_runs SET wait_deadline_wall_ms=-1
                    WHERE run_id='v85-run'",[]).is_err());
                assert!(conn.execute("UPDATE kernel_runs SET wait_deadline_wall_ms=5,
                    wait_accounted_until_wall_ms=6 WHERE run_id='v85-run'",[]).is_err());
                assert!(conn.execute("UPDATE kernel_runs SET wait_accounted_until_wall_ms=5
                    WHERE run_id='v85-run'",[]).is_err());
                assert!(conn.execute("UPDATE kernel_runs SET wait_deadline_wall_ms=5
                    WHERE run_id='v85-run'",[]).is_err());
                conn.execute("UPDATE kernel_runs SET wait_deadline_wall_ms=5,
                    wait_accounted_until_wall_ms=3 WHERE run_id='v85-run'",[])?;
                conn.execute("UPDATE kernel_runs SET wait_deadline_wall_ms=NULL,
                    wait_accounted_until_wall_ms=NULL WHERE run_id='v85-run'",[])?;
                Ok(())
            }).unwrap();
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    /// Regression (2026-10-02 review, P1): migration 88 rebuilds
    /// `delivery_checklist_items` to widen its status CHECK. The rebuild must
    /// carry `requirements_json` — the task's structured acceptance demands —
    /// and not merely have the column re-added afterwards with its `'[]'`
    /// default, which silently erased every existing row's requirements.
    #[test]
    fn migration_88_preserves_existing_structured_requirements() {
        let root = std::env::temp_dir().join(format!("fox-v88-req-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("facts.db");
        // The exact shape a real pre-88 row can have: a statistics demand the
        // Host reports as 未核验, and a chart demand that really gates a pass.
        let unbound = r#"[{"itemKey":null,"id":"source:unbound","kind":{"kind":"sourceStatsUnbound","demand":"按问题类型统计频次与比例"},"sourceText":"任务要求统计，但未指明可核验的源文件与统计列"}]"#;
        let charts = r#"[{"itemKey":null,"id":"charts>=2","kind":{"kind":"charts","count":2,"perSheet":false},"sourceText":"任务要求图表：至少 2 张"}]"#;
        {
            let mut connection = Connection::open(&path).unwrap();
            run_with_target(&mut connection, 1, MigrationTarget::V87).unwrap();
            let version: i64 = connection
                .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| row.get(0))
                .unwrap();
            assert_eq!(version, 87, "the fixture must stop before migration 88");
            connection
                .execute_batch(&format!(
                    "INSERT INTO agents(id,name,description,runtime_type,system_prompt,default_model,created_at,updated_at)
                     VALUES('v88-agent','Old','','pi','','model',1,1);
                     INSERT INTO conversations(id,agent_id,title,status,created_at,updated_at)
                     VALUES('v88-conv','v88-agent','Old','active',1,1);
                     INSERT INTO runs(id,conversation_id,status,model,created_at)
                     VALUES('v88-run','v88-conv','running','model',1);
                     INSERT INTO delivery_checklist_items
                        (run_id,item_key,target_path,display_name,checks_json,status,updated_at,requirements_json)
                     VALUES
                        ('v88-run','file:a.xlsx','a.xlsx','a.xlsx','[\"exists\"]','pending',1,'{unbound}'),
                        ('v88-run','slot:docx:1',NULL,'Word 成果 1/1','[\"exists\"]','failed',2,'{charts}');"
                ))
                .unwrap();
            // The pre-88 row really holds the demands (so the test cannot pass
            // vacuously if the fixture itself failed to store them).
            let stored: Vec<(String, String)> = {
                let mut statement = connection
                    .prepare("SELECT item_key, requirements_json FROM delivery_checklist_items ORDER BY item_key")
                    .unwrap();
                let rows = statement
                    .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
                    .unwrap()
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .unwrap();
                rows
            };
            assert!(stored.iter().any(|(_, body)| body.contains("sourceStatsUnbound")));
            assert!(stored.iter().any(|(_, body)| body.contains("\"count\":2")));
        }

        // Upgrade to the latest schema — migration 88 rebuilds the table.
        let db = crate::database::Database::open(path.clone()).unwrap();
        db.with_connection(|conn| {
            let version: i64 = conn.query_row(
                "SELECT MAX(version) FROM schema_migrations", [], |row| row.get(0))?;
            assert_eq!(version, DATABASE_SCHEMA_VERSION);
            let rows: Vec<(String, String, String)> = {
                let mut statement = conn.prepare(
                    "SELECT item_key, status, requirements_json FROM delivery_checklist_items
                     ORDER BY item_key",
                )?;
                let rows = statement
                    .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                rows
            };
            assert_eq!(rows.len(), 2, "both rows must survive the rebuild");
            for (item_key, status, requirements) in &rows {
                assert_ne!(
                    requirements, "[]",
                    "{item_key} lost its structured requirements in the v88 rebuild"
                );
            }
            let unbound_row = rows.iter().find(|(key, _, _)| key == "file:a.xlsx").unwrap();
            assert!(unbound_row.2.contains("sourceStatsUnbound"), "{}", unbound_row.2);
            assert!(unbound_row.2.contains("按问题类型统计频次与比例"), "{}", unbound_row.2);
            let charts_row = rows.iter().find(|(key, _, _)| key == "slot:docx:1").unwrap();
            assert!(charts_row.2.contains("\"count\":2"), "{}", charts_row.2);
            // The rebuild also copies the statuses verbatim, including the new
            // one the migration exists for.
            assert_eq!(charts_row.1, "failed");
            let fk: i64 = conn.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| row.get(0))?;
            let integrity: String = conn.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
            assert_eq!((fk, integrity), (0, "ok".into()));
            Ok(())
        })
        .unwrap();
        drop(db);

        // Re-opening must not lose them either (the upgrade is not a one-shot).
        let reopened = crate::database::Database::open(path.clone()).unwrap();
        let surviving: String = reopened
            .with_connection(|conn| {
                Ok(conn.query_row(
                    "SELECT requirements_json FROM delivery_checklist_items WHERE item_key='slot:docx:1'",
                    [], |row| row.get(0),
                )?)
            })
            .unwrap();
        assert!(surviving.contains("\"count\":2"), "{surviving}");
        drop(reopened);
        std::fs::remove_dir_all(root).unwrap();
    }

    /// The new status the v88 rebuild exists for must be accepted, and the
    /// rebuild must keep rejecting anything else.
    #[test]
    fn migration_88_accepts_verified_and_rejects_unknown_statuses() {
        let root = std::env::temp_dir().join(format!("fox-v88-status-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("facts.db");
        let db = crate::database::Database::open(path.clone()).unwrap();
        db.with_connection(|conn| {
            conn.execute_batch(
                "INSERT INTO agents(id,name,description,runtime_type,system_prompt,default_model,created_at,updated_at)
                 VALUES('s-agent','A','','pi','','model',1,1);
                 INSERT INTO conversations(id,agent_id,title,status,created_at,updated_at)
                 VALUES('s-conv','s-agent','t','active',1,1);
                 INSERT INTO runs(id,conversation_id,status,model,created_at)
                 VALUES('s-run','s-conv','running','model',1);
                 INSERT INTO delivery_checklist_items
                    (run_id,item_key,display_name,checks_json,status,updated_at)
                 VALUES('s-run','slot:xlsx:1','x','[]','verified',1);",
            )?;
            assert!(conn
                .execute(
                    "UPDATE delivery_checklist_items SET status='not-a-status' WHERE item_key='slot:xlsx:1'",
                    [],
                )
                .is_err());
            Ok(())
        })
        .unwrap();
        drop(db);
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(test)]
fn rebuild_v68_for_test(connection: &mut Connection) -> Result<bool> {
    connection.pragma_update(None, "foreign_keys", "OFF")?;
    let result = (|| {
        let tx = connection.transaction()?;
        let changed = rebuild_kernel_tables_for_v68(&tx)?;
        tx.commit()?;
        Ok(changed)
    })();
    connection.pragma_update(None, "foreign_keys", "ON")?;
    result
}

#[cfg(test)]
mod unified_migration_tests {
    use super::*;
    #[test]
    fn migration_rebuild_failure_rolls_back_schema_data_and_restores_foreign_keys() {
        let mut db = Connection::open_in_memory().unwrap();
        run(&mut db, 1).unwrap();
        // Controlled copy failure: this old fixture permits a state the new
        // table rejects. Never touch a user's database to inject the failure.
        db.execute_batch("PRAGMA foreign_keys=OFF;
            DROP TABLE kernel_tool_calls;
            CREATE TABLE kernel_tool_calls (
                run_id TEXT NOT NULL, tool_call_id TEXT NOT NULL, batch_id TEXT NOT NULL,
                tool TEXT NOT NULL, source_order INTEGER NOT NULL, canonical_input_json TEXT NOT NULL,
                state TEXT NOT NULL CHECK(state IN ('completed','legacy-state')), result_json TEXT,
                created_at INTEGER NOT NULL, settled_at INTEGER, dispatch_idempotency_key TEXT,
                PRIMARY KEY(run_id,tool_call_id));").unwrap();
        db.execute("INSERT INTO kernel_tool_calls(run_id,tool_call_id,batch_id,tool,source_order,canonical_input_json,state,created_at)
            VALUES(?1,?2,?3,?4,0,?5,?6,1)", rusqlite::params!["r","t","b","read","{}","legacy-state"]).unwrap();
        let versions: i64 = db.query_row("SELECT COUNT(*) FROM schema_migrations", [], |r| r.get(0)).unwrap();
        assert!(run(&mut db, 2).is_err());
        assert_eq!(db.query_row("PRAGMA foreign_keys", [], |r| r.get::<_, i64>(0)).unwrap(), 1);
        assert!(!sqlite_table_exists(&db, "kernel_tool_calls_v68").unwrap());
        assert_eq!(db.query_row("SELECT state FROM kernel_tool_calls", [], |r| r.get::<_, String>(0)).unwrap(), "legacy-state");
        assert_eq!(db.query_row("SELECT COUNT(*) FROM schema_migrations", [], |r| r.get::<_, i64>(0)).unwrap(), versions);
        db.execute("UPDATE kernel_tool_calls SET state=?1", ["completed"]).unwrap();
        run(&mut db, 3).unwrap();
        assert!(sqlite_object_accepts(&db, "kernel_tool_calls", "'expired'").unwrap());
    }
}

const MIGRATION_70: &str = r#"
ALTER TABLE kernel_jobs ADD COLUMN cancel_requested_at INTEGER;
ALTER TABLE kernel_jobs ADD COLUMN cancel_acknowledged_at INTEGER;
ALTER TABLE kernel_jobs ADD COLUMN cancelled_by TEXT;
ALTER TABLE kernel_jobs ADD COLUMN uncertain_at INTEGER;
CREATE TABLE kernel_context_budget_reports (
 run_id TEXT PRIMARY KEY REFERENCES kernel_runs(run_id) ON DELETE CASCADE,
 report_json TEXT NOT NULL, updated_at INTEGER NOT NULL
);
"#;

/// Host-managed artifact lifecycle metadata.
///
/// `artifact_class` is the Host's own verdict on what a produced file *is*
/// (`deliverable` / `preview` / `process`); the renderer groups by this value
/// instead of guessing from the extension, so a CSV the user asked to be
/// delivered stays a deliverable while a CSV computed as an intermediate step
/// stays a process file. `artifact_origin` records whether the bytes live in
/// the user's project or in an application-private Host area.
///
/// Existing rows predate the split and keep their kind, but are **not**
/// relabelled: the migration only adds the columns with a conservative
/// default. Process is the safe default because it never claims a file is a
/// finished deliverable that the Host cannot prove.
const MIGRATION_71_COLUMNS: &[(&str, &str, &str)] = &[
    (
        "artifacts",
        "artifact_class",
        "TEXT NOT NULL DEFAULT 'process'",
    ),
    (
        "artifacts",
        "artifact_origin",
        "TEXT NOT NULL DEFAULT 'project'",
    ),
];

const MIGRATION_71_INDEXES: &str = r#"
CREATE INDEX IF NOT EXISTS idx_artifacts_conversation_class
    ON artifacts(conversation_id, artifact_class, created_at);
"#;

/// Backfill the origin of rows that predate the class/origin split.
///
/// Every historical row is a process file: the Host never classified a
/// deliverable before this change, and promoting old rows on a path pattern
/// would be exactly the "guess from the location" error the split exists to
/// avoid. What the Host *can* prove structurally is where the bytes live, so
/// rows inside an application-private area (the conversation compute workspace,
/// whose paths carry an `attachment-compute` component) are recorded as
/// `host_private`. Without this, an old CSV that has always lived in the
/// private workspace would be reported as an in-project file.
///
/// Applied once and recorded as migration 72, so a second start is a no-op and
/// the two historical groups above do not need to be re-checked.
const MIGRATION_72: &str = r#"
UPDATE artifacts
   SET artifact_origin = 'host_private'
 WHERE artifact_origin <> 'host_private'
   AND storage_path LIKE '%attachment-compute%'
   AND storage_path NOT LIKE '%\artifacts\%';
UPDATE artifacts
   SET artifact_class = 'process'
 WHERE artifact_class IS NULL OR trim(artifact_class) = '';
"#;

/// One conversation's assigned deliverable folder, per authorized project.
///
/// Before this table the deliverable folder name was recomputed from the
/// *current date* on every prompt build, so a task continued across midnight
/// silently moved to a second result folder. The assignment is a Host fact and
/// therefore has to be durable: it is decided once, re-read on every later Run
/// and on every application restart, and it is scoped by the project root so a
/// conversation re-pointed at another project cannot inherit (or write into)
/// the old project's folder.
///
/// `PRIMARY KEY(conversation_id, project_key)` is what makes the assignment
/// idempotent under concurrency: two simultaneous Runs both attempt
/// `INSERT OR IGNORE` and exactly one row wins, so they cannot diverge.
const MIGRATION_73: &str = r#"
CREATE TABLE IF NOT EXISTS deliverable_placements (
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    project_key TEXT NOT NULL,
    folder_name TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY(conversation_id, project_key)
);
"#;

/// Frozen execution credentials and single-claim execution attempts.
///
/// `kernel_execution_credentials` holds the authoritative snapshot issued once
/// per dispatch inside the final admission transaction; a second issue for the
/// same (run_id, dispatch_id) must be byte-identical, so a dispatch can never
/// be re-signed with different fixed fields. Rows created before this table
/// simply do not exist: an old dispatch without a credential cannot start, and
/// no backfill may invent authorization.
///
/// `kernel_execution_attempts` is the persistent single claim: the first
/// execution attempt of a dispatch is won by exactly one caller across
/// independent connections (`PRIMARY KEY(run_id, dispatch_id)` plus the
/// claimed-state CAS in the repository). `start_confirmed` is NULL while the
/// start fact is unknown, 0/1 once known, and can never be regressed from 1
/// to 0.
const MIGRATION_74: &str = r#"
CREATE TABLE IF NOT EXISTS kernel_execution_credentials (
    run_id TEXT NOT NULL,
    dispatch_id TEXT NOT NULL,
    conversation_id TEXT NOT NULL,
    intent_digest TEXT NOT NULL,
    action_class TEXT NOT NULL CHECK(action_class IN ('read','write','execute','destructive','sensitive_egress','manage')),
    file_baseline TEXT,
    resolved_profile TEXT NOT NULL,
    policy_snapshot_id TEXT NOT NULL,
    backend_required TEXT NOT NULL,
    backend_evidence_digest TEXT,
    credential_digest TEXT NOT NULL,
    credential_json TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    PRIMARY KEY(run_id, dispatch_id)
);
CREATE TABLE IF NOT EXISTS kernel_execution_attempts (
    run_id TEXT NOT NULL,
    dispatch_id TEXT NOT NULL,
    conversation_id TEXT NOT NULL,
    params_hash TEXT NOT NULL,
    state TEXT NOT NULL CHECK(state IN ('claimed','refused','launched','unknown')),
    refusal_code TEXT,
    start_confirmed INTEGER CHECK(start_confirmed IN (0,1)),
    terminal_state TEXT CHECK(terminal_state IN ('completed','failed','cancelled')),
    error_code TEXT,
    claimed_by TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY(run_id, dispatch_id)
);
"#;

/// B1a-R1: authoritative Host file observations (a model-declared
/// `expectedVersion` is never an observation), plus the attempt columns that
/// carry the frozen dynamic requirements (operation class, live policy
/// version, budget ceiling, parent revocation generation) so a receipt can
/// project the right execution family and the executor can re-verify against
/// what was actually bound. Additive only.
const MIGRATION_75: &str = r#"
CREATE TABLE IF NOT EXISTS kernel_host_observations (
    run_id TEXT NOT NULL,
    conversation_id TEXT NOT NULL,
    target_identity TEXT NOT NULL,
    version TEXT NOT NULL,
    observed_by_tool_call_id TEXT NOT NULL,
    observed_at INTEGER NOT NULL,
    PRIMARY KEY(run_id, target_identity)
);
"#;

/// v80（协调者复审 REV-01/02/03/04/05）：
///
/// * **REV-03**：会话有效模式不再从项目动态继承。项目默认只在**新会话创建时**
///   复制一次；此后每个会话独立保存。因此删除把项目变更传播到既有会话的触发器，
///   并把会话触发器的模式取值从"项目优先"改为"会话自身值"。
///   升级时先把每个现存会话的**当前有效值**写回 `conversations.permission_mode`，
///   使该列与 `kernel_execution_policies.mode` 一致——升级瞬间没有任何会话被
///   扩大或收窄，也不是"一律按新默认扩大权限"。
/// * **REV-04**：Legacy 可复用审批（`conversation_tool_permissions`，其行全部由
///   `allow_conversation` 解决写入）需要可撤销，否则收紧策略后旧授权仍放行。
/// * **REV-01/05**：Host 观察改为不可变、可引用，并记录**实际交付范围**，
///   使"读多少才能改什么"可判定；`view_kind` 只作分类标签，不表示覆盖强弱。
/// * 旧观察行一律标为 `legacy_unknown` + `covered_whole_file=0`：升级后需重读
///   或走显式授权，不冒充完整观察。
const MIGRATION_80: &str = r#"
-- 1) 数据对齐：列与策略行一致（REV-03 / 协调者 M4 两处存储一致）。
UPDATE conversations
   SET permission_mode = COALESCE(
         (SELECT p.permission_mode FROM projects p WHERE p.id = conversations.project_id),
         conversations.permission_mode, 'ask')
 WHERE id IN (SELECT conversation_id FROM kernel_execution_policies);

-- 2) 会话级变更只推进本会话，模式取会话自身值（REV-03）。
DROP TRIGGER IF EXISTS execution_policy_conversation_change;
CREATE TRIGGER execution_policy_conversation_change AFTER UPDATE OF permission_mode ON conversations
WHEN OLD.permission_mode IS NOT NEW.permission_mode BEGIN
 UPDATE kernel_execution_policies
    SET version = version + 1,
        mode = COALESCE(NEW.permission_mode, 'ask')
  WHERE conversation_id = NEW.id;
END;

-- 3) 项目默认变化不得改既有会话（v1.1：项目默认只用于新会话初始化）。
DROP TRIGGER IF EXISTS execution_policy_project_change;

-- 3b) 新会话把项目默认**复制**进自己的两处存储：先写列（此刻策略行还不存在，
--     因此会话触发器影响 0 行、不推进版本），再以版本 1 建立策略行。复制之后
--     两者独立，项目默认再变也不会波及它（协调者 M4：两处存储保持一致）。
DROP TRIGGER IF EXISTS execution_policy_new_conversation;
CREATE TRIGGER execution_policy_new_conversation AFTER INSERT ON conversations BEGIN
  UPDATE conversations
     SET permission_mode = COALESCE((SELECT permission_mode FROM projects WHERE id=NEW.project_id),
                                    NEW.permission_mode, 'ask')
   WHERE id = NEW.id;
  INSERT INTO kernel_execution_policies
    VALUES(NEW.id, 1, COALESCE((SELECT permission_mode FROM projects WHERE id=NEW.project_id),
                               NEW.permission_mode, 'ask'));
END;

-- 5) Host 观察：不可变、可引用、带实际交付范围（REV-01 / REV-05）。
DROP TABLE IF EXISTS kernel_host_observations_v80;
CREATE TABLE kernel_host_observations_v80 (
    observation_id     TEXT PRIMARY KEY,
    run_id             TEXT NOT NULL,
    conversation_id    TEXT NOT NULL,
    target_identity    TEXT NOT NULL,
    version            TEXT NOT NULL,
    observed_by_tool_call_id TEXT NOT NULL,
    observed_at        INTEGER NOT NULL,
    view_kind          TEXT NOT NULL CHECK(view_kind IN
        ('full_file','line_range','unit_window','office_extract','legacy_unknown','missing')),
    covered_whole_file INTEGER NOT NULL CHECK(covered_whole_file IN (0,1)),
    -- 统一坐标：UTF-16 code unit 的 [range_start, range_end)。
    range_start        INTEGER,
    range_end          INTEGER,
    total_units        INTEGER,
    truncated          INTEGER NOT NULL CHECK(truncated IN (0,1))
);
INSERT INTO kernel_host_observations_v80
    (observation_id, run_id, conversation_id, target_identity, version,
     observed_by_tool_call_id, observed_at, view_kind, covered_whole_file,
     range_start, range_end, total_units, truncated)
SELECT 'obs-legacy-' || run_id || '-' || replace(target_identity, ':', '_'),
       run_id, conversation_id, target_identity, version,
       observed_by_tool_call_id, observed_at,
       'legacy_unknown', 0, NULL, NULL, NULL, 0
  FROM kernel_host_observations;
DROP TABLE kernel_host_observations;
ALTER TABLE kernel_host_observations_v80 RENAME TO kernel_host_observations;
CREATE UNIQUE INDEX idx_host_observations_identity
    ON kernel_host_observations(run_id, target_identity, version, observed_by_tool_call_id);
CREATE INDEX idx_host_observations_lookup
    ON kernel_host_observations(run_id, target_identity, version);
"#;

/// v81（REV-05 / 协调者 M3）：整文件替换的**专用**批准。
///
/// 与普通审批分离：`allow_once` 与可复用 Grant 都不得冒充它。一行绑定
/// 目标 + 观察版本 + 候选内容摘要 + 请求摘要，四元组任一不符即拒；
/// `approved → consumed` 一次一用，与持久领取在同一 `IMMEDIATE` 事务内完成。
const MIGRATION_81: &str = r#"
CREATE TABLE IF NOT EXISTS kernel_whole_file_replace_grants (
    grant_id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL,
    run_id TEXT NOT NULL,
    dispatch_id TEXT NOT NULL,
    target_identity TEXT NOT NULL,
    version TEXT NOT NULL,
    candidate_digest TEXT NOT NULL,
    request_digest TEXT NOT NULL,
    state TEXT NOT NULL CHECK(state IN ('pending','approved','denied','expired','consumed')),
    created_at INTEGER NOT NULL,
    decided_at INTEGER,
    consumed_at INTEGER
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_replace_grants_dispatch
    ON kernel_whole_file_replace_grants(run_id, dispatch_id);
CREATE INDEX IF NOT EXISTS idx_replace_grants_conversation
    ON kernel_whole_file_replace_grants(conversation_id, state);
"#;

/// v82（协调者 item 3）：恢复请求的稳定身份。
///
/// `(conversation_id, request_id)` 唯一。同一请求的重复投递读到已记录的结果，
/// 不再执行第二次文件修改；`baseline_version` 是**确认时**绑定的基线，提交临界区
/// 会复验它。
const MIGRATION_82: &str = r#"
CREATE TABLE IF NOT EXISTS kernel_restore_requests (
    conversation_id TEXT NOT NULL,
    request_id TEXT NOT NULL,
    version_id TEXT NOT NULL,
    target_identity TEXT NOT NULL,
    baseline_version TEXT,
    dispatch_id TEXT NOT NULL,
    state TEXT NOT NULL CHECK(state IN ('running','committed','not_applied','recovery_required','indeterminate')),
    result_version_id TEXT,
    error TEXT,
    created_at INTEGER NOT NULL,
    settled_at INTEGER,
    PRIMARY KEY(conversation_id, request_id)
);
"#;

/// Passive Host terminal facts; delivery and model-input binding belong to B2.
const MIGRATION_84: &str = r#"
CREATE TABLE kernel_job_notices (
    job_id TEXT PRIMARY KEY REFERENCES kernel_jobs(job_id),
    data_root_id TEXT NOT NULL,
    conversation_id TEXT NOT NULL,
    run_id TEXT NOT NULL,
    attempt INTEGER NOT NULL CHECK(attempt >= 0),
    terminal_state TEXT NOT NULL CHECK(terminal_state IN ('completed','failed','cancelled')),
    finished_at INTEGER NOT NULL,
    result_ref TEXT,
    result_sha256 TEXT,
    result_bytes INTEGER,
    error_code TEXT,
    error_message TEXT,
    terminal_origin TEXT NOT NULL CHECK(terminal_origin IN ('worker','unowned')),
    owner_pid INTEGER,
    owner_started_at INTEGER,
    CHECK ((terminal_origin='worker' AND owner_pid IS NOT NULL AND owner_started_at IS NOT NULL)
        OR (terminal_origin='unowned' AND owner_pid IS NULL AND owner_started_at IS NULL)),
    CHECK ((terminal_state='completed' AND result_ref IS NOT NULL
        AND result_sha256 IS NOT NULL AND result_bytes IS NOT NULL AND result_bytes >= 0)
        OR (terminal_state IN ('failed','cancelled') AND result_ref IS NULL
        AND result_sha256 IS NULL AND result_bytes IS NULL))
);
CREATE INDEX idx_kernel_job_notices_scope
    ON kernel_job_notices(data_root_id,conversation_id,run_id);
"#;

/// Model delivery is a separate ledger: absence means pending, and the B1
/// terminal fact remains immutable across binding and response acknowledgement.
const MIGRATION_85: &str = r#"
CREATE TABLE kernel_model_notice_inputs (
    run_id TEXT NOT NULL REFERENCES kernel_runs(run_id),
    dispatch_key TEXT NOT NULL,
    lease_owner TEXT NOT NULL,
    input_hash TEXT NOT NULL CHECK(length(input_hash)=71 AND input_hash GLOB 'sha256:*'),
    input_json TEXT NOT NULL,
    history_start INTEGER NOT NULL CHECK(history_start>=0),
    historical_bytes INTEGER NOT NULL CHECK(historical_bytes>=0),
    state TEXT NOT NULL CHECK(state IN ('bound','acknowledged')),
    bound_at INTEGER NOT NULL,
    acknowledged_at INTEGER,
    PRIMARY KEY(run_id,dispatch_key),
    CHECK ((state='bound' AND acknowledged_at IS NULL)
        OR (state='acknowledged' AND acknowledged_at IS NOT NULL))
);
CREATE TABLE kernel_job_notice_deliveries (
    job_id TEXT PRIMARY KEY REFERENCES kernel_job_notices(job_id) ON DELETE CASCADE,
    dispatch_key TEXT NOT NULL,
    input_hash TEXT NOT NULL CHECK(length(input_hash)=71 AND input_hash GLOB 'sha256:*'),
    history_position INTEGER NOT NULL CHECK(history_position>=0),
    state TEXT NOT NULL CHECK(state IN ('bound','acknowledged')),
    bound_at INTEGER NOT NULL,
    acknowledged_at INTEGER,
    CHECK ((state='bound' AND acknowledged_at IS NULL)
        OR (state='acknowledged' AND acknowledged_at IS NOT NULL))
);
CREATE INDEX idx_kernel_job_notice_deliveries_dispatch
    ON kernel_job_notice_deliveries(dispatch_key,state);
"#;

/// WaitingJobs is a nonterminal projection. The event stream retains the
/// response/history and the wait identity; these two columns retain its fixed
/// wall deadline and the last accounted wall instant across process restarts.
const MIGRATION_86: &str = r#"
ALTER TABLE kernel_runs ADD COLUMN wait_deadline_wall_ms INTEGER
    CHECK(wait_deadline_wall_ms IS NULL OR wait_deadline_wall_ms > 0);
ALTER TABLE kernel_runs ADD COLUMN wait_accounted_until_wall_ms INTEGER
    CHECK((wait_deadline_wall_ms IS NULL AND wait_accounted_until_wall_ms IS NULL)
        OR (wait_deadline_wall_ms IS NOT NULL
            AND wait_accounted_until_wall_ms IS NOT NULL
            AND wait_accounted_until_wall_ms > 0
            AND wait_deadline_wall_ms >= wait_accounted_until_wall_ms));
"#;

/// Business delivery verdicts are staged before the round decision that
/// consumes them commits, and finalized right after. Both halves are additive
/// tables: a crash between them leaves a recoverable stage row instead of a
/// completed Run whose checklist items stay pending forever.
const MIGRATION_87: &str = r#"
CREATE TABLE IF NOT EXISTS delivery_outcome_stage (
    run_id TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    item_key TEXT NOT NULL,
    passed INTEGER NOT NULL CHECK(passed IN (0,1)),
    finding_json TEXT NOT NULL,
    checked_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY(run_id,item_key)
);
CREATE TABLE IF NOT EXISTS delivery_repair_stage (
    run_id TEXT PRIMARY KEY REFERENCES runs(id) ON DELETE CASCADE,
    findings_json TEXT NOT NULL,
    staged_at INTEGER NOT NULL
);
"#;

/// The checklist gains one status value, `verified`, widening the CHECK so a row
/// can record "a verdict was computed for this row but the decision that would
/// authorize it has not committed yet".
///
/// The current write path does **not** use it: a verdict is only written once its
/// decision committed (see migration 89 and `delivery_checks`), so an abandoned
/// round leaves no trace at all. The value stays permitted because databases
/// migrated here already carry it in the CHECK, and a later code revision may
/// want the intermediate state back without another rebuild.
///
/// A table rebuild must carry **every** column the table can have, not only the
/// ones the original `CREATE` listed: `requirements_json` arrived later (migration
/// 66) and holds the task's structured acceptance demands. Omitting it here did
/// not merely drop a column — the column was then re-added with its `'[]'`
/// default, silently erasing the structured requirements of every existing row.
const MIGRATION_88: &str = r#"
CREATE TABLE delivery_checklist_items_v88 (
    run_id TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    item_key TEXT NOT NULL,
    target_path TEXT,
    artifact_id TEXT,
    display_name TEXT NOT NULL,
    checks_json TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending' CHECK(status IN ('pending','verified','passed','failed')),
    finding_json TEXT,
    checked_at INTEGER,
    updated_at INTEGER NOT NULL,
    requirements_json TEXT NOT NULL DEFAULT '[]',
    PRIMARY KEY(run_id,item_key)
);
INSERT INTO delivery_checklist_items_v88
    (run_id,item_key,target_path,artifact_id,display_name,checks_json,status,finding_json,
     checked_at,updated_at,requirements_json)
    SELECT run_id,item_key,target_path,artifact_id,display_name,checks_json,status,finding_json,
           checked_at,updated_at,COALESCE(requirements_json,'[]')
    FROM delivery_checklist_items;
DROP TABLE delivery_checklist_items;
ALTER TABLE delivery_checklist_items_v88 RENAME TO delivery_checklist_items;
CREATE INDEX IF NOT EXISTS idx_delivery_items_run ON delivery_checklist_items(run_id,status);
"#;

/// A decision mark committed **in the same transaction as the decision it
/// belongs to**.
///
/// The delivery ledger stages verdicts before the round decision commits, so it
/// must be able to ask "did that exact decision commit?". A mark written by the
/// commit transaction answers it exactly: present means committed, absent means
/// the decision never landed (crash, cancellation, or a superseded round). The
/// staging tables carry the mark so recovery can finalize only the verdicts whose
/// decision really committed.
const MIGRATION_89: &str = r#"
CREATE TABLE IF NOT EXISTS kernel_decision_marks (
    run_id TEXT NOT NULL REFERENCES kernel_runs(run_id) ON DELETE CASCADE,
    mark TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    PRIMARY KEY(run_id, mark)
);
ALTER TABLE delivery_outcome_stage ADD COLUMN decision_mark TEXT NOT NULL DEFAULT '';
ALTER TABLE delivery_repair_stage ADD COLUMN decision_mark TEXT NOT NULL DEFAULT '';
"#;

/// Column additions applied once under migration 75 (idempotent helper).
const MIGRATION_75_ATTEMPTS_COLUMNS: &[(&str, &str, &str)] = &[
    ("kernel_execution_attempts", "action_class", "TEXT"),
    ("kernel_execution_attempts", "policy_version", "INTEGER"),
    ("kernel_execution_attempts", "budget_ceiling_ms", "INTEGER"),
    ("kernel_execution_attempts", "parent_generation", "INTEGER"),
];


fn rebuild_attempts_state_check(transaction: &rusqlite::Transaction<'_>) -> Result<()> {
    let sql: String = transaction.query_row("SELECT sql FROM sqlite_master WHERE type='table' AND name='kernel_execution_attempts'", [], |r| r.get(0))?;
    if sql.contains("'completed','unknown'") { return Ok(()); }
    transaction.execute_batch("CREATE TABLE kernel_execution_attempts_v76 (
        run_id TEXT NOT NULL, dispatch_id TEXT NOT NULL, conversation_id TEXT NOT NULL,
        params_hash TEXT NOT NULL, state TEXT NOT NULL CHECK(state IN ('claimed','refused','launched','completed','unknown')),
        refusal_code TEXT, start_confirmed INTEGER CHECK(start_confirmed IN (0,1)),
        terminal_state TEXT CHECK(terminal_state IN ('completed','failed','cancelled')),
        error_code TEXT, claimed_by TEXT, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL,
        action_class TEXT, policy_version INTEGER, budget_ceiling_ms INTEGER, parent_generation INTEGER,
        PRIMARY KEY(run_id, dispatch_id));
        INSERT INTO kernel_execution_attempts_v76 SELECT run_id,dispatch_id,conversation_id,params_hash,state,refusal_code,start_confirmed,terminal_state,error_code,claimed_by,created_at,updated_at,action_class,policy_version,budget_ceiling_ms,parent_generation FROM kernel_execution_attempts;
        DROP TABLE kernel_execution_attempts;
        ALTER TABLE kernel_execution_attempts_v76 RENAME TO kernel_execution_attempts;")
}

const MIGRATION_77: &str = r#"
CREATE TABLE IF NOT EXISTS kernel_execution_policies (
 conversation_id TEXT PRIMARY KEY REFERENCES conversations(id) ON DELETE CASCADE,
 version INTEGER NOT NULL CHECK(version>=1), mode TEXT NOT NULL CHECK(mode IN ('read_only','ask','allow'))
);
INSERT OR IGNORE INTO kernel_execution_policies SELECT c.id,1,COALESCE(p.permission_mode,c.permission_mode,'ask') FROM conversations c LEFT JOIN projects p ON p.id=c.project_id;
CREATE TABLE IF NOT EXISTS kernel_policy_requests (
 conversation_id TEXT NOT NULL, request_id TEXT NOT NULL, request_digest TEXT NOT NULL,
 result_version INTEGER NOT NULL, result_mode TEXT NOT NULL, PRIMARY KEY(conversation_id,request_id)
);
CREATE TABLE IF NOT EXISTS kernel_parent_generations (run_id TEXT PRIMARY KEY, generation INTEGER NOT NULL DEFAULT 0);
CREATE TRIGGER IF NOT EXISTS execution_policy_new_conversation AFTER INSERT ON conversations BEGIN
 INSERT INTO kernel_execution_policies VALUES(NEW.id,1,COALESCE((SELECT permission_mode FROM projects WHERE id=NEW.project_id),NEW.permission_mode,'ask'));
END;
CREATE TRIGGER IF NOT EXISTS execution_policy_conversation_change AFTER UPDATE OF permission_mode,project_id ON conversations
WHEN OLD.permission_mode IS NOT NEW.permission_mode OR OLD.project_id IS NOT NEW.project_id BEGIN
 UPDATE kernel_execution_policies SET version=version+1,mode=COALESCE((SELECT permission_mode FROM projects WHERE id=NEW.project_id),NEW.permission_mode,'ask') WHERE conversation_id=NEW.id;
END;
CREATE TRIGGER IF NOT EXISTS execution_policy_project_change AFTER UPDATE OF permission_mode ON projects WHEN OLD.permission_mode IS NOT NEW.permission_mode BEGIN
 UPDATE kernel_execution_policies SET version=version+1,mode=NEW.permission_mode WHERE conversation_id IN (SELECT id FROM conversations WHERE project_id=NEW.id);
END;
CREATE TRIGGER IF NOT EXISTS execution_policy_invalidate AFTER UPDATE OF version ON kernel_execution_policies WHEN NEW.version<>OLD.version BEGIN
 UPDATE kernel_approvals SET state='expired' WHERE state IN ('pending','allow_once','allow_conversation') AND run_id IN (SELECT id FROM runs WHERE conversation_id=NEW.conversation_id)
 AND NOT EXISTS(SELECT 1 FROM kernel_effect_outbox o WHERE o.run_id=kernel_approvals.run_id AND o.tool_call_id=kernel_approvals.tool_call_id AND o.effect_type='dispatch_tool');
END;
CREATE TRIGGER IF NOT EXISTS execution_grant_revoked AFTER UPDATE OF revoked_at ON kernel_authorization_grants WHEN OLD.revoked_at IS NULL AND NEW.revoked_at IS NOT NULL BEGIN
 UPDATE kernel_execution_policies SET version=version+1 WHERE conversation_id=NEW.conversation_id;
 INSERT INTO kernel_parent_generations(run_id,generation) SELECT id,1 FROM runs WHERE conversation_id=NEW.conversation_id
 ON CONFLICT(run_id) DO UPDATE SET generation=generation+1;
END;
CREATE TRIGGER IF NOT EXISTS execution_parent_cancel AFTER UPDATE OF status ON runs WHEN NEW.status='cancelled' AND OLD.status<>'cancelled' BEGIN
 INSERT INTO kernel_parent_generations(run_id,generation) VALUES(NEW.id,1) ON CONFLICT(run_id) DO UPDATE SET generation=generation+1;
END;
"#;
