//! Read side of the Kernel context-budget report. The Kernel compaction writer
//! was removed with the Kernel loop (B1); historical reports stay readable.
use super::Database;
use rusqlite::{params, OptionalExtension};
use serde_json::Value;

fn invalid(message: impl Into<String>) -> rusqlite::Error {
    rusqlite::Error::ToSqlConversionFailure(Box::new(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        message.into(),
    )))
}

impl Database {
    pub(crate) fn kernel_context_budget(&self, conversation: &str, run_id: &str) -> Result<Option<Value>,String> {
        self.with_connection(|c| {
            let body:Option<String>=c.query_row("SELECT b.report_json FROM kernel_context_budget_reports b JOIN runs r ON r.id=b.run_id WHERE b.run_id=?1 AND r.conversation_id=?2",params![run_id,conversation],|r|r.get(0)).optional()?;
            body.map(|v|serde_json::from_str(&v).map_err(|_|invalid("invalid budget report"))).transpose()
        })
    }
}
