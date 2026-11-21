//! Read side of the Kernel context-budget report. The Kernel compaction writer
//! was removed with the Kernel loop (B1); historical reports stay readable.
use super::Database;
use rusqlite::{params, OptionalExtension};
use serde_json::{json, Value};

fn invalid(message: impl Into<String>) -> rusqlite::Error {
    rusqlite::Error::ToSqlConversionFailure(Box::new(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        message.into(),
    )))
}

impl Database {
    pub(crate) fn kernel_context_budget(
        &self,
        conversation: &str,
        run_id: &str,
    ) -> Result<Option<Value>, String> {
        self.with_connection(|c| {
            let body:Option<String>=c.query_row("SELECT b.report_json FROM kernel_context_budget_reports b JOIN runs r ON r.id=b.run_id WHERE b.run_id=?1 AND r.conversation_id=?2",params![run_id,conversation],|r|r.get(0)).optional()?;
            body.map(|v|serde_json::from_str(&v).map_err(|_|invalid("invalid budget report"))).transpose()
        })
    }

    pub(crate) fn runtime_context_window(&self, run_id: &str) -> Result<Option<Value>, String> {
        self.with_connection(|connection| {
            // After a successful compaction, usage from the old context is stale.
            let usage: Option<(String, String)> = connection.query_row(
                "SELECT event_type, event_json FROM run_events
                 WHERE run_id = ?1 AND seq > COALESCE((SELECT MAX(seq) FROM run_events
                     WHERE run_id = ?1 AND event_type = 'context.compaction.completed'
                       AND COALESCE(json_extract(event_json, '$.aborted'), 0) = 0), 0)
                   AND (event_type = 'context.usage' OR (event_type = 'usage.request'
                     AND json_extract(event_json, '$.record.stage') = 'agent'
                     AND json_extract(event_json, '$.record.outcome') = 'success'
                     AND json_type(event_json, '$.record.usage.input') = 'integer'))
                 ORDER BY seq DESC, rowid DESC LIMIT 1",
                [run_id], |row| Ok((row.get(0)?, row.get(1)?)),
            ).optional()?;
            let Some((kind, body)) = usage else { return Ok(None) };
            let event: Value = serde_json::from_str(&body).map_err(|_| invalid("invalid context usage event"))?;
            if kind == "context.usage" {
                return Ok(event["usedTokens"].as_u64().zip(event["limitTokens"].as_u64().filter(|limit| *limit > 0))
                    .map(|(used, limit)| json!({"usedTokens": used, "limitTokens": limit, "source": "session"})));
            }
            let limit: Option<u64> = connection.query_row(
                "SELECT json_extract(event_json, '$.contextWindow') FROM run_events
                 WHERE run_id = ?1 AND event_type = 'run.request_snapshot'
                   AND json_extract(event_json, '$.contextWindow') > 0
                 ORDER BY seq DESC LIMIT 1",
                [run_id], |row| row.get(0),
            ).optional()?;
            let counters = &event["record"]["usage"];
            let used = ["input", "output", "cacheRead", "cacheWrite"].iter()
                .filter_map(|key| counters[*key].as_u64()).sum::<u64>();
            Ok(limit.map(|limit| json!({"usedTokens": used, "limitTokens": limit, "source": "last_request"})))
        })
    }
}
