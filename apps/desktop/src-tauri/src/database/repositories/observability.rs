use super::{now_ms, Database};
use crate::database::{
    EvaluationRunSummary, EvaluationSuiteSummary, LatencyMetric, ObservabilityStatistics,
    TraceRunSummary,
};
use rusqlite::{params, OptionalExtension, Transaction};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use uuid::Uuid;

pub const TRACE_SCHEMA_VERSION: i64 = 1;
const MAX_EVALUATION_REPORT_BYTES: usize = 1024 * 1024;

fn new_trace_id() -> String {
    Uuid::new_v4().simple().to_string()
}

fn new_span_id() -> String {
    Uuid::new_v4().simple().to_string()[..16].to_owned()
}

fn valid_trace_id(value: &str) -> bool {
    value.len() == 32 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn valid_span_id(value: &str) -> bool {
    value.len() == 16 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

pub(super) fn initialize_run_trace(
    transaction: &Transaction<'_>,
    run_id: &str,
    fallback_started_at: i64,
) -> rusqlite::Result<(String, String)> {
    let (conversation_id, model, existing_trace_id, existing_root_span_id, created_at): (
        String,
        String,
        Option<String>,
        Option<String>,
        i64,
    ) = transaction.query_row(
        "SELECT conversation_id, model, trace_id, root_span_id, created_at
         FROM runs WHERE id = ?1",
        [run_id],
        |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        },
    )?;
    let trace_id = existing_trace_id
        .filter(|value| valid_trace_id(value))
        .unwrap_or_else(new_trace_id);
    let root_span_id = existing_root_span_id
        .filter(|value| valid_span_id(value))
        .unwrap_or_else(new_span_id);
    transaction.execute(
        "UPDATE runs SET trace_id = ?2, root_span_id = ?3 WHERE id = ?1",
        params![run_id, trace_id, root_span_id],
    )?;
    let attributes = serde_json::to_string(&json!({
        "gen_ai.operation.name": "invoke_agent",
        "gen_ai.agent.name": "Fox",
        "gen_ai.request.model": model,
        "fox.trace.schema_version": TRACE_SCHEMA_VERSION,
    }))
    .unwrap_or_else(|_| "{}".to_owned());
    transaction.execute(
        "INSERT OR IGNORE INTO trace_spans(
            id, trace_id, parent_span_id, run_id, conversation_id, name, operation,
            category, status, started_at, attributes_json, schema_version
         ) VALUES (?1, ?2, NULL, ?3, ?4, 'Fox invoke_agent', 'invoke_agent',
                   'run', 'unset', ?5, ?6, ?7)",
        params![
            root_span_id,
            trace_id,
            run_id,
            conversation_id,
            created_at.max(fallback_started_at.min(created_at)),
            attributes,
            TRACE_SCHEMA_VERSION,
        ],
    )?;
    Ok((trace_id, root_span_id))
}

pub(super) fn initialize_child_run_trace(
    transaction: &Transaction<'_>,
    child_run_id: &str,
    parent_run_id: &str,
    delegation_tool_call_id: &str,
    worker_agent_id: &str,
    worker_agent_name: &str,
    started_at: i64,
) -> rusqlite::Result<(String, String)> {
    let (trace_id, parent_root_span_id): (String, String) = transaction.query_row(
        "SELECT trace_id, root_span_id FROM runs WHERE id = ?1",
        [parent_run_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let delegation_span_id = transaction
        .query_row(
            "SELECT span_id FROM tool_calls
             WHERE run_id = ?1 AND runtime_tool_call_id = ?2",
            params![parent_run_id, delegation_tool_call_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten()
        .filter(|value| valid_span_id(value))
        .unwrap_or(parent_root_span_id);
    let root_span_id = new_span_id();
    let (conversation_id, model): (String, String) = transaction.query_row(
        "SELECT conversation_id, model FROM runs WHERE id = ?1",
        [child_run_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    transaction.execute(
        "UPDATE runs SET trace_id = ?2, root_span_id = ?3 WHERE id = ?1",
        params![child_run_id, trace_id, root_span_id],
    )?;
    let attributes = serde_json::to_string(&json!({
        "gen_ai.operation.name": "invoke_agent",
        "gen_ai.agent.id": worker_agent_id,
        "gen_ai.agent.name": worker_agent_name,
        "gen_ai.request.model": model,
        "fox.run.kind": "child",
        "fox.parent_run_id": parent_run_id,
        "fox.trace.schema_version": TRACE_SCHEMA_VERSION,
    }))
    .unwrap_or_else(|_| "{}".to_owned());
    transaction.execute(
        "INSERT INTO trace_spans(
            id, trace_id, parent_span_id, run_id, conversation_id, name, operation,
            category, status, started_at, attributes_json, schema_version
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'invoke_agent',
                   'run', 'unset', ?7, ?8, ?9)",
        params![
            root_span_id,
            trace_id,
            delegation_span_id,
            child_run_id,
            conversation_id,
            format!("{worker_agent_name} invoke_agent"),
            started_at,
            attributes,
            TRACE_SCHEMA_VERSION,
        ],
    )?;
    Ok((trace_id, root_span_id))
}

fn current_phase_span(
    transaction: &Transaction<'_>,
    run_id: &str,
) -> rusqlite::Result<Option<String>> {
    transaction
        .query_row(
            "SELECT id FROM trace_spans
             WHERE run_id = ?1 AND category = 'phase' AND ended_at IS NULL
             ORDER BY started_at DESC, rowid DESC LIMIT 1",
            [run_id],
            |row| row.get(0),
        )
        .optional()
}

fn close_open_spans(
    transaction: &Transaction<'_>,
    run_id: &str,
    category: &str,
    now: i64,
    status: &str,
    error_type: Option<&str>,
    error_message: Option<&str>,
) -> rusqlite::Result<()> {
    transaction.execute(
        "UPDATE trace_spans
         SET ended_at = ?3,
             duration_ms = MAX(0, ?3 - started_at),
             status = ?4,
             error_type = COALESCE(?5, error_type),
             error_message = COALESCE(?6, error_message)
         WHERE run_id = ?1 AND category = ?2 AND ended_at IS NULL",
        params![run_id, category, now, status, error_type, error_message],
    )?;
    Ok(())
}

fn phase_operation(phase: &str) -> &'static str {
    match phase {
        "planning" => "plan",
        "model_streaming" | "recovering" | "compacting" => "inference",
        "preparing" => "host_prepare",
        "finalizing" => "finalize",
        _ => "phase",
    }
}

fn insert_phase_span(
    transaction: &Transaction<'_>,
    run_id: &str,
    trace_id: &str,
    root_span_id: &str,
    phase: &str,
    now: i64,
) -> rusqlite::Result<String> {
    let conversation_id: String = transaction.query_row(
        "SELECT conversation_id FROM runs WHERE id = ?1",
        [run_id],
        |row| row.get(0),
    )?;
    let span_id = new_span_id();
    let operation = phase_operation(phase);
    let attributes = serde_json::to_string(&json!({
        "fox.phase": phase,
        "gen_ai.operation.name": operation,
    }))
    .unwrap_or_else(|_| "{}".to_owned());
    transaction.execute(
        "INSERT INTO trace_spans(
            id, trace_id, parent_span_id, run_id, conversation_id, name, operation,
            category, status, started_at, attributes_json, schema_version
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'phase', 'unset', ?8, ?9, ?10)",
        params![
            span_id,
            trace_id,
            root_span_id,
            run_id,
            conversation_id,
            format!("agent.{phase}"),
            operation,
            now,
            attributes,
            TRACE_SCHEMA_VERSION,
        ],
    )?;
    Ok(span_id)
}

fn tool_span(
    transaction: &Transaction<'_>,
    run_id: &str,
    trace_id: &str,
    root_span_id: &str,
    payload: &Value,
    now: i64,
) -> rusqlite::Result<String> {
    let tool_call_id = payload
        .get("toolCallId")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    if let Some(existing) = transaction
        .query_row(
            "SELECT id FROM trace_spans
             WHERE run_id = ?1 AND category = 'tool' AND entity_id = ?2
             ORDER BY started_at DESC LIMIT 1",
            params![run_id, tool_call_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?
    {
        return Ok(existing);
    }
    let conversation_id: String = transaction.query_row(
        "SELECT conversation_id FROM runs WHERE id = ?1",
        [run_id],
        |row| row.get(0),
    )?;
    let tool_name = payload
        .get("tool")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let parent =
        current_phase_span(transaction, run_id)?.unwrap_or_else(|| root_span_id.to_owned());
    let span_id = new_span_id();
    let attributes = serde_json::to_string(&json!({
        "gen_ai.operation.name": "execute_tool",
        "gen_ai.tool.name": tool_name,
        "fox.tool.execution_location": "runtime",
    }))
    .unwrap_or_else(|_| "{}".to_owned());
    transaction.execute(
        "INSERT INTO trace_spans(
            id, trace_id, parent_span_id, run_id, conversation_id, name, operation,
            category, status, started_at, attributes_json, entity_id, schema_version
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'execute_tool', 'tool', 'unset',
                   ?7, ?8, ?9, ?10)",
        params![
            span_id,
            trace_id,
            parent,
            run_id,
            conversation_id,
            format!("execute_tool {tool_name}"),
            now,
            attributes,
            tool_call_id,
            TRACE_SCHEMA_VERSION,
        ],
    )?;
    Ok(span_id)
}

pub(super) fn project_runtime_span(
    transaction: &Transaction<'_>,
    run_id: &str,
    event_type: &str,
    payload: &Value,
    now: i64,
) -> rusqlite::Result<(String, String)> {
    let (trace_id, root_span_id) = initialize_run_trace(transaction, run_id, now)?;
    let span_id = match event_type {
        "run.phase" => {
            close_open_spans(transaction, run_id, "phase", now, "ok", None, None)?;
            let phase = payload
                .get("phase")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            insert_phase_span(transaction, run_id, &trace_id, &root_span_id, phase, now)?
        }
        "tool.started" | "tool.updated" | "tool.completed" => {
            let span_id = tool_span(transaction, run_id, &trace_id, &root_span_id, payload, now)?;
            if event_type == "tool.completed" {
                let is_error = payload
                    .get("isError")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                transaction.execute(
                    "UPDATE trace_spans
                     SET ended_at = ?2, duration_ms = MAX(0, ?2 - started_at),
                         status = ?3, error_type = CASE WHEN ?4 THEN 'tool_error' ELSE NULL END
                     WHERE id = ?1 AND ended_at IS NULL",
                    params![
                        span_id,
                        now,
                        if is_error { "error" } else { "ok" },
                        is_error
                    ],
                )?;
            }
            span_id
        }
        "run.completed" | "run.cancelled" | "run.interrupted" | "run.failed" => {
            let failed = matches!(event_type, "run.interrupted" | "run.failed");
            let error_type = payload.get("code").and_then(Value::as_str);
            let error_message = payload
                .get("message")
                .and_then(Value::as_str)
                .map(|value| value.chars().take(1000).collect::<String>());
            close_run_trace_in_transaction(
                transaction,
                run_id,
                now,
                if failed { "error" } else { "ok" },
                error_type,
                error_message.as_deref(),
            )?;
            root_span_id.clone()
        }
        "run.request_snapshot" => {
            let attributes = json!({
                "gen_ai.operation.name": "invoke_agent",
                "gen_ai.request.model": payload.get("model").and_then(Value::as_str),
                "gen_ai.provider.name": payload.get("provider").and_then(Value::as_str),
                "fox.model.api_type": payload.get("apiType").and_then(Value::as_str),
                "fox.prompt.stable_hash": payload.get("stablePromptHash").and_then(Value::as_str),
                "fox.prompt.context_hash": payload.get("contextHash").and_then(Value::as_str),
            });
            transaction.execute(
                "UPDATE trace_spans SET attributes_json = ?2 WHERE id = ?1",
                params![
                    root_span_id,
                    serde_json::to_string(&attributes).unwrap_or_else(|_| "{}".to_owned())
                ],
            )?;
            current_phase_span(transaction, run_id)?.unwrap_or_else(|| root_span_id.clone())
        }
        _ => current_phase_span(transaction, run_id)?.unwrap_or_else(|| root_span_id.clone()),
    };
    Ok((trace_id, span_id))
}

pub(super) fn close_run_trace_in_transaction(
    transaction: &Transaction<'_>,
    run_id: &str,
    now: i64,
    status: &str,
    error_type: Option<&str>,
    error_message: Option<&str>,
) -> rusqlite::Result<()> {
    let (_, root_span_id) = initialize_run_trace(transaction, run_id, now)?;
    for category in ["phase", "tool"] {
        close_open_spans(
            transaction,
            run_id,
            category,
            now,
            status,
            error_type,
            error_message,
        )?;
    }
    transaction.execute(
        "UPDATE trace_spans
         SET ended_at = ?2, duration_ms = MAX(0, ?2 - started_at), status = ?3,
             error_type = ?4, error_message = ?5
         WHERE id = ?1",
        params![root_span_id, now, status, error_type, error_message],
    )?;
    Ok(())
}

impl Database {
    /// A zero-length trace point observed by this process. `attempt_id` is a
    /// fresh identifier for one dispatch invocation, not a durable effect key:
    /// retries of the same effect must remain separate observations.
    pub fn record_host_stage_point(
        &self,
        run_id: &str,
        attempt_id: &str,
        stage: &str,
        observed_at: i64,
    ) -> Result<(), String> {
        if attempt_id.is_empty() || attempt_id.len() > 128
            || !attempt_id.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        {
            return Err("invalid host stage attempt".to_owned());
        }
        if !matches!(stage,
            "queued" | "dispatch_granted" | "context_prepare_started"
            | "context_prepare_finished" | "worker_handoff"
            | "first_valid_worker_event")
        {
            return Err("unsupported host stage".to_owned());
        }
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            let (trace_id, root_span_id) = initialize_run_trace(&transaction, run_id, observed_at)?;
            let conversation_id: String = transaction.query_row(
                "SELECT conversation_id FROM runs WHERE id = ?1", [run_id], |row| row.get(0),
            )?;
            let entity_id = format!("{attempt_id}:{stage}");
            let exists: bool = transaction.query_row(
                "SELECT EXISTS(SELECT 1 FROM trace_spans WHERE run_id = ?1 AND category = 'phase' AND entity_id = ?2)",
                params![run_id, entity_id], |row| row.get(0),
            )?;
            if !exists {
                let attributes = serde_json::to_string(&json!({
                    "fox.host.stage": stage,
                    "fox.host.attempt_id": attempt_id,
                    "fox.host.clock": "host_unix_ms",
                    "fox.host.boundary": "point"
                })).unwrap_or_else(|_| "{}".to_owned());
                transaction.execute(
                    "INSERT INTO trace_spans(id, trace_id, parent_span_id, run_id, conversation_id,
                     name, operation, category, status, started_at, ended_at, duration_ms,
                     attributes_json, entity_id, schema_version)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'host_stage_point', 'phase', 'ok',
                             ?7, ?7, 0, ?8, ?9, ?10)",
                    params![new_span_id(), trace_id, root_span_id, run_id, conversation_id,
                        format!("host.{stage}"), observed_at, attributes, entity_id,
                        TRACE_SCHEMA_VERSION],
                )?;
            }
            transaction.commit()?;
            Ok(())
        })
    }

    pub fn record_ui_metric(
        &self,
        run_id: &str,
        metric: &str,
        duration_ms: i64,
    ) -> Result<(), String> {
        if !matches!(metric, "ui.first_event" | "ui.terminal_render") {
            return Err("unsupported UI metric".to_owned());
        }
        if !(0..=600_000).contains(&duration_ms) {
            return Err("UI metric duration is outside the accepted range".to_owned());
        }
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            let now = now_ms();
            let (trace_id, root_span_id) = initialize_run_trace(&transaction, run_id, now)?;
            let conversation_id: String = transaction.query_row(
                "SELECT conversation_id FROM runs WHERE id = ?1",
                [run_id],
                |row| row.get(0),
            )?;
            let exists = transaction.query_row(
                "SELECT EXISTS(SELECT 1 FROM trace_spans
                 WHERE run_id = ?1 AND category = 'ui' AND entity_id = ?2)",
                params![run_id, metric],
                |row| row.get::<_, bool>(0),
            )?;
            if !exists {
                let attributes = serde_json::to_string(&json!({ "fox.ui.metric": metric }))
                    .unwrap_or_else(|_| "{}".to_owned());
                transaction.execute(
                    "INSERT INTO trace_spans(
                        id, trace_id, parent_span_id, run_id, conversation_id, name, operation,
                        category, status, started_at, ended_at, duration_ms, attributes_json,
                        entity_id, schema_version
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'ui_render', 'ui', 'ok',
                               ?7, ?8, ?9, ?10, ?6, ?11)",
                    params![
                        new_span_id(),
                        trace_id,
                        root_span_id,
                        run_id,
                        conversation_id,
                        metric,
                        now - duration_ms,
                        now,
                        duration_ms,
                        attributes,
                        TRACE_SCHEMA_VERSION,
                    ],
                )?;
            }
            transaction.commit()?;
            Ok(())
        })
    }

    pub fn record_evaluation_report(
        &self,
        report: &Value,
        duration_ms: i64,
    ) -> Result<EvaluationRunSummary, String> {
        let serialized = serde_json::to_string(report).map_err(|error| error.to_string())?;
        if serialized.len() > MAX_EVALUATION_REPORT_BYTES {
            return Err("evaluation report is too large".to_owned());
        }
        if report.get("kind").and_then(Value::as_str) != Some("fox-offline-agent-eval") {
            return Err("evaluation report kind is unsupported".to_owned());
        }
        let schema_version = report
            .get("schemaVersion")
            .and_then(Value::as_i64)
            .ok_or_else(|| "evaluation report schemaVersion is missing".to_owned())?;
        let generated_at = report
            .get("generatedAt")
            .and_then(Value::as_str)
            .ok_or_else(|| "evaluation report generatedAt is missing".to_owned())?;
        let summary = report
            .get("summary")
            .and_then(Value::as_object)
            .ok_or_else(|| "evaluation report summary is missing".to_owned())?;
        let suite_count = summary.get("suites").and_then(Value::as_i64).unwrap_or(0);
        let total_count = summary.get("total").and_then(Value::as_i64).unwrap_or(0);
        let passed_count = summary.get("passed").and_then(Value::as_i64).unwrap_or(0);
        let failed_count = summary.get("failed").and_then(Value::as_i64).unwrap_or(0);
        if suite_count < 0
            || total_count < 0
            || passed_count < 0
            || failed_count < 0
            || passed_count + failed_count != total_count
        {
            return Err("evaluation report summary is inconsistent".to_owned());
        }
        let suites = report
            .get("suites")
            .and_then(Value::as_array)
            .ok_or_else(|| "evaluation report suites are missing".to_owned())?;
        if suites.len() as i64 != suite_count || suites.len() > 64 {
            return Err("evaluation report suite count is inconsistent".to_owned());
        }
        let id = Uuid::new_v4().to_string();
        let report_hash = hex::encode(Sha256::digest(serialized.as_bytes()));
        let recorded_at = now_ms();
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            transaction.execute(
                "INSERT INTO evaluation_runs(
                    id, kind, schema_version, generated_at, recorded_at, duration_ms,
                    report_hash, suite_count, total_count, passed_count, failed_count, report_json
                 ) VALUES (?1, 'fox-offline-agent-eval', ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    id,
                    schema_version,
                    generated_at,
                    recorded_at,
                    duration_ms.max(0),
                    report_hash,
                    suite_count,
                    total_count,
                    passed_count,
                    failed_count,
                    serialized,
                ],
            )?;
            for suite in suites {
                let name = suite
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("unnamed")
                    .chars()
                    .take(200)
                    .collect::<String>();
                let cases = suite
                    .get("cases")
                    .and_then(Value::as_array)
                    .map(Vec::as_slice)
                    .unwrap_or(&[]);
                let passed = cases
                    .iter()
                    .filter(|case| case.get("passed").and_then(Value::as_bool) == Some(true))
                    .count() as i64;
                let total = cases.len() as i64;
                transaction.execute(
                    "INSERT INTO evaluation_suite_results(
                        evaluation_run_id, suite_name, source, source_url,
                        total_count, passed_count, failed_count
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![
                        id,
                        name,
                        suite.get("source").and_then(Value::as_str),
                        suite.get("sourceUrl").and_then(Value::as_str),
                        total,
                        passed,
                        total - passed,
                    ],
                )?;
            }
            transaction.commit()?;
            Ok(())
        })?;
        self.evaluation_run_summary(&id)?
            .ok_or_else(|| "saved evaluation report could not be loaded".to_owned())
    }

    fn evaluation_run_summary(&self, id: &str) -> Result<Option<EvaluationRunSummary>, String> {
        self.with_connection(|connection| {
            let record = connection
                .query_row(
                    "SELECT id, generated_at, recorded_at, duration_ms, report_hash,
                            suite_count, total_count, passed_count, failed_count
                     FROM evaluation_runs WHERE id = ?1",
                    [id],
                    |row| {
                        Ok(EvaluationRunSummary {
                            id: row.get(0)?,
                            generated_at: row.get(1)?,
                            recorded_at: row.get(2)?,
                            duration_ms: row.get(3)?,
                            report_hash: row.get(4)?,
                            suites: row.get(5)?,
                            total: row.get(6)?,
                            passed: row.get(7)?,
                            failed: row.get(8)?,
                            suite_results: Vec::new(),
                        })
                    },
                )
                .optional()?;
            let Some(mut record) = record else {
                return Ok(None);
            };
            record.suite_results = query_suite_results(connection, id)?;
            Ok(Some(record))
        })
    }

    pub fn observability_statistics(&self) -> Result<ObservabilityStatistics, String> {
        self.with_connection(|connection| {
            let total_run_count = connection.query_row("SELECT COUNT(*) FROM runs", [], |row| row.get(0))?;
            let traced_run_count = connection.query_row(
                "SELECT COUNT(*) FROM runs
                 WHERE trace_id IS NOT NULL AND root_span_id IS NOT NULL
                   AND length(trace_id) = 32 AND length(root_span_id) = 16",
                [],
                |row| row.get(0),
            )?;
            let mut recent_statement = connection.prepare(
                "SELECT r.id, r.conversation_id, r.trace_id, r.root_span_id, r.status, r.model,
                        root.started_at, root.ended_at, root.duration_ms,
                        COALESCE(SUM(CASE WHEN child.operation = 'plan' THEN child.duration_ms ELSE 0 END), 0),
                        COALESCE(SUM(CASE WHEN child.operation = 'inference' THEN child.duration_ms ELSE 0 END), 0),
                        COALESCE(SUM(CASE WHEN child.operation = 'execute_tool' THEN child.duration_ms ELSE 0 END), 0),
                        COALESCE(SUM(CASE WHEN child.operation = 'ui_render' THEN child.duration_ms ELSE 0 END), 0),
                        COUNT(child.id) + 1
                 FROM runs r
                 JOIN trace_spans root ON root.run_id = r.id AND root.category = 'run'
                 LEFT JOIN trace_spans child ON child.run_id = r.id AND child.category != 'run'
                 GROUP BY r.id, root.id
                 ORDER BY root.started_at DESC LIMIT 20",
            )?;
            let recent_runs = recent_statement
                .query_map([], |row| {
                    Ok(TraceRunSummary {
                        run_id: row.get(0)?,
                        conversation_id: row.get(1)?,
                        trace_id: row.get(2)?,
                        root_span_id: row.get(3)?,
                        status: row.get(4)?,
                        model: row.get(5)?,
                        started_at: row.get(6)?,
                        finished_at: row.get(7)?,
                        total_duration_ms: row.get(8)?,
                        planning_duration_ms: row.get(9)?,
                        model_duration_ms: row.get(10)?,
                        tool_duration_ms: row.get(11)?,
                        ui_duration_ms: row.get(12)?,
                        span_count: row.get(13)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;

            let mut durations: BTreeMap<String, Vec<i64>> = BTreeMap::new();
            let mut duration_statement = connection.prepare(
                "SELECT operation, duration_ms FROM trace_spans
                 WHERE category != 'run' AND operation != 'host_stage_point'
                   AND duration_ms IS NOT NULL
                 ORDER BY started_at DESC LIMIT 2000",
            )?;
            for row in duration_statement.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })? {
                let (operation, duration) = row?;
                durations.entry(operation).or_default().push(duration);
            }
            // Derive only intervals whose two real boundaries were observed on
            // this Host clock and in the same dispatch attempt. Missing points
            // stay unknown; no neighboring span is used as a substitute.
            let mut stage_statement = connection.prepare(
                "SELECT run_id, entity_id, started_at FROM trace_spans
                 WHERE operation = 'host_stage_point'
                 ORDER BY started_at DESC LIMIT 4000",
            )?;
            let mut attempts: BTreeMap<(String, String), BTreeMap<String, i64>> = BTreeMap::new();
            for row in stage_statement.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, i64>(2)?))
            })? {
                let (run_id, entity_id, at) = row?;
                if let Some((attempt, stage)) = entity_id.rsplit_once(':') {
                    attempts.entry((run_id, attempt.to_owned())).or_default()
                        .insert(stage.to_owned(), at);
                }
            }
            for stages in attempts.values() {
                for (from, to, operation) in [
                    ("queued", "dispatch_granted", "host.queue_wait"),
                    ("context_prepare_started", "context_prepare_finished", "host.context_prepare"),
                    ("context_prepare_finished", "worker_handoff", "host.prepare_to_handoff"),
                    ("worker_handoff", "first_valid_worker_event", "host.worker_to_first_event"),
                ] {
                    if let (Some(start), Some(end)) = (stages.get(from), stages.get(to)) {
                        if end >= start {
                            durations.entry(operation.to_owned()).or_default().push(end - start);
                        }
                    }
                }
            }
            let latency_metrics = durations
                .into_iter()
                .map(|(operation, mut values)| {
                    values.sort_unstable();
                    let sample_count = values.len() as i64;
                    let average_ms = values.iter().sum::<i64>() / sample_count.max(1);
                    LatencyMetric {
                        operation,
                        sample_count,
                        average_ms,
                        p50_ms: percentile(&values, 50),
                        p95_ms: percentile(&values, 95),
                        max_ms: values.last().copied().unwrap_or(0),
                    }
                })
                .collect();

            let mut evaluation_statement = connection.prepare(
                "SELECT id, generated_at, recorded_at, duration_ms, report_hash,
                        suite_count, total_count, passed_count, failed_count
                 FROM evaluation_runs ORDER BY recorded_at DESC, id DESC LIMIT 12",
            )?;
            let mut evaluation_history = evaluation_statement
                .query_map([], |row| {
                    Ok(EvaluationRunSummary {
                        id: row.get(0)?,
                        generated_at: row.get(1)?,
                        recorded_at: row.get(2)?,
                        duration_ms: row.get(3)?,
                        report_hash: row.get(4)?,
                        suites: row.get(5)?,
                        total: row.get(6)?,
                        passed: row.get(7)?,
                        failed: row.get(8)?,
                        suite_results: Vec::new(),
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            for evaluation in &mut evaluation_history {
                evaluation.suite_results = query_suite_results(connection, &evaluation.id)?;
            }
            Ok(ObservabilityStatistics {
                trace_schema_version: TRACE_SCHEMA_VERSION,
                traced_run_count,
                total_run_count,
                trace_coverage_percent: if total_run_count == 0 {
                    100
                } else {
                    (traced_run_count * 100 / total_run_count).clamp(0, 100)
                },
                recent_runs,
                latency_metrics,
                evaluation_history,
            })
        })
    }
}

fn query_suite_results(
    connection: &rusqlite::Connection,
    evaluation_run_id: &str,
) -> rusqlite::Result<Vec<EvaluationSuiteSummary>> {
    connection
        .prepare(
            "SELECT suite_name, source, source_url, total_count, passed_count, failed_count
             FROM evaluation_suite_results WHERE evaluation_run_id = ?1
             ORDER BY suite_name COLLATE NOCASE",
        )?
        .query_map([evaluation_run_id], |row| {
            Ok(EvaluationSuiteSummary {
                name: row.get(0)?,
                source: row.get(1)?,
                source_url: row.get(2)?,
                total: row.get(3)?,
                passed: row.get(4)?,
                failed: row.get(5)?,
            })
        })?
        .collect()
}

fn percentile(values: &[i64], percentile: usize) -> i64 {
    if values.is_empty() {
        return 0;
    }
    let index = ((values.len() - 1) * percentile + 99) / 100;
    values[index.min(values.len() - 1)]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::StartRunResult;

    fn setup() -> (Database, StartRunResult) {
        let path = std::env::temp_dir().join(format!("fox-observability-{}.db", Uuid::new_v4()));
        let database = Database::open(path).expect("database");
        let conversation = database
            .create_conversation(database.default_agent_id(), None, None, None)
            .expect("conversation");
        let run = database
            .create_run(&conversation.id, "trace this", None)
            .expect("run");
        (database, run)
    }

    #[test]
    fn host_stage_points_keep_actual_boundaries_and_distinct_attempts() {
        let (database, run) = setup();
        let run_id = &run.run.id;
        database.record_host_stage_point(run_id, "attempt-one", "worker_handoff", 1_000).unwrap();
        database.record_host_stage_point(run_id, "attempt-one", "first_valid_worker_event", 1_075).unwrap();
        database.record_host_stage_point(run_id, "attempt-one", "worker_handoff", 9_999).unwrap();
        database.record_host_stage_point(run_id, "attempt-two", "worker_handoff", 2_000).unwrap();
        let points = database.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT entity_id, started_at, ended_at, duration_ms FROM trace_spans
                 WHERE run_id = ?1 AND operation = 'host_stage_point' ORDER BY started_at",
            )?;
            let rows = statement.query_map([run_id], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?, row.get::<_, i64>(3)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        }).unwrap();
        assert_eq!(points, vec![
            ("attempt-one:worker_handoff".into(), 1_000, 1_000, 0),
            ("attempt-one:first_valid_worker_event".into(), 1_075, 1_075, 0),
            ("attempt-two:worker_handoff".into(), 2_000, 2_000, 0),
        ]);
        assert_eq!(points[1].1 - points[0].1, 75);
        let metrics = database.observability_statistics().unwrap().latency_metrics;
        let worker = metrics.iter().find(|metric| metric.operation == "host.worker_to_first_event").unwrap();
        assert_eq!((worker.sample_count, worker.average_ms), (1, 75));
        assert!(!metrics.iter().any(|metric| metric.operation == "host.context_prepare"));
        assert!(database.record_host_stage_point(run_id, "attempt-one", "http_sent", 3_000).is_err());
    }

    #[test]
    fn delayed_host_boundary_is_assigned_only_to_its_observed_interval() {
        let (database, run) = setup();
        let run_id = &run.run.id;
        let start = crate::database::now_ms();
        database.record_host_stage_point(run_id, "delayed-attempt", "context_prepare_started", start).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(45));
        let end = crate::database::now_ms();
        database.record_host_stage_point(run_id, "delayed-attempt", "context_prepare_finished", end).unwrap();
        let metrics = database.observability_statistics().unwrap().latency_metrics;
        let preparation = metrics.iter().find(|metric| metric.operation == "host.context_prepare").unwrap();
        assert!(preparation.average_ms >= 40);
        assert!(!metrics.iter().any(|metric| metric.operation == "host.worker_to_first_event"));
    }

    #[test]
    fn projects_trace_tree_metrics_and_evaluation_history() {
        let (database, run) = setup();
        assert_eq!(run.run.trace_id.as_deref().map(str::len), Some(32));
        assert_eq!(run.run.root_span_id.as_deref().map(str::len), Some(16));
        for (seq, event) in [
            (1, json!({ "type": "run.started" })),
            (2, json!({ "type": "run.phase", "phase": "planning" })),
            (
                3,
                json!({ "type": "run.phase", "phase": "model_streaming" }),
            ),
            (
                4,
                json!({ "type": "tool.started", "toolCallId": "tool-1", "tool": "read_file" }),
            ),
            (
                5,
                json!({ "type": "tool.completed", "toolCallId": "tool-1", "tool": "read_file", "result": {} }),
            ),
            (6, json!({ "type": "run.completed" })),
        ] {
            database
                .apply_runtime_event(&run.run.id, seq, &event)
                .expect("event");
        }
        database
            .record_ui_metric(&run.run.id, "ui.first_event", 12)
            .expect("UI metric");
        let report = json!({
            "kind": "fox-offline-agent-eval",
            "schemaVersion": 2,
            "generatedAt": "2026-08-23T00:00:00.000Z",
            "summary": { "suites": 1, "total": 1, "passed": 1, "failed": 0 },
            "suites": [{
                "name": "Fox contract",
                "source": "Fox",
                "sourceUrl": "https://github.com/example/fox",
                "cases": [{ "id": "one", "passed": true }]
            }]
        });
        database
            .record_evaluation_report(&report, 25)
            .expect("eval");
        let statistics = database.observability_statistics().expect("statistics");
        assert_eq!(statistics.trace_coverage_percent, 100);
        assert_eq!(statistics.recent_runs[0].span_count, 5);
        assert_eq!(statistics.evaluation_history[0].passed, 1);
        assert!(statistics
            .latency_metrics
            .iter()
            .any(|metric| metric.operation == "execute_tool"));
    }
}
