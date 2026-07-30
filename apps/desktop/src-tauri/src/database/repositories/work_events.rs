use super::{now_ms, Database};
use crate::database::WorkEventRecord;
use rusqlite::{params, OptionalExtension};
use serde_json::Value;
use uuid::Uuid;

pub const WORK_EVENT_SCHEMA_VERSION: u32 = 1;
pub const WORK_EVENT_TYPES: [&str; 12] = [
    "goal.proposed",
    "goal.activated",
    "goal.blocked",
    "goal.completed",
    "goal.cancelled",
    "task.created",
    "task.started",
    "task.completed",
    "task.blocked",
    "task.interrupted",
    "evidence.added",
    "evidence.validated",
];

impl Database {
    pub fn append_work_event(
        &self,
        event_type: &str,
        conversation_id: &str,
        goal_id: Option<&str>,
        task_id: Option<&str>,
        run_id: Option<&str>,
        data: Value,
    ) -> Result<WorkEventRecord, String> {
        if !known_event_type(event_type) {
            return Err(format!("unsupported work event type: {event_type}"));
        }
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            ensure_references_belong_to_conversation(
                &transaction,
                conversation_id,
                goal_id,
                task_id,
                run_id,
            )?;
            let sequence: i64 = transaction.query_row(
                "SELECT COALESCE(MAX(sequence), 0) + 1 FROM work_events WHERE conversation_id = ?1",
                [conversation_id],
                |row| row.get(0),
            )?;
            let (trace_id, span_id): (Option<String>, Option<String>) = match run_id {
                Some(run_id) => transaction.query_row(
                    "SELECT trace_id, root_span_id FROM runs WHERE id = ?1",
                    [run_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )?,
                None => (None, None),
            };
            let event = WorkEventRecord {
                event_type: event_type.to_owned(),
                schema_version: WORK_EVENT_SCHEMA_VERSION,
                conversation_id: conversation_id.to_owned(),
                goal_id: goal_id.map(str::to_owned),
                task_id: task_id.map(str::to_owned),
                run_id: run_id.map(str::to_owned),
                trace_id,
                span_id,
                sequence,
                timestamp: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                data,
            };
            insert_event(&transaction, &event)?;
            transaction.commit()?;
            Ok(event)
        })
    }

    pub fn apply_work_event(&self, event: &WorkEventRecord) -> Result<bool, String> {
        if !known_event_type(&event.event_type) {
            return Ok(false);
        }
        validate_event(event)?;
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            ensure_references_belong_to_conversation(
                &transaction,
                &event.conversation_id,
                event.goal_id.as_deref(),
                event.task_id.as_deref(),
                event.run_id.as_deref(),
            )?;
            let inserted = insert_event(&transaction, event)?;
            if inserted {
                transaction.commit()?;
            } else {
                transaction.rollback()?;
            }
            Ok(inserted)
        })
    }

    #[allow(dead_code)]
    pub fn list_work_events(&self, conversation_id: &str) -> Result<Vec<WorkEventRecord>, String> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT event_json FROM work_events
                 WHERE conversation_id = ?1 ORDER BY sequence",
            )?;
            let events = statement
                .query_map([conversation_id], |row| {
                    let json: String = row.get(0)?;
                    serde_json::from_str(&json).map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            0,
                            rusqlite::types::Type::Text,
                            Box::new(error),
                        )
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(events)
        })
    }
}

fn insert_event(
    connection: &rusqlite::Connection,
    event: &WorkEventRecord,
) -> rusqlite::Result<bool> {
    let event_json = serde_json::to_string(event)
        .map_err(|_| rusqlite::Error::InvalidParameterName("event_json".to_owned()))?;
    Ok(connection.execute(
        "INSERT OR IGNORE INTO work_events(
            id, conversation_id, sequence, event_type, schema_version, event_json, created_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            Uuid::new_v4().to_string(),
            event.conversation_id,
            event.sequence,
            event.event_type,
            event.schema_version,
            event_json,
            now_ms(),
        ],
    )? > 0)
}

fn validate_event(event: &WorkEventRecord) -> Result<(), String> {
    if event.schema_version != WORK_EVENT_SCHEMA_VERSION {
        return Err(format!(
            "unsupported work event schema version: {}",
            event.schema_version
        ));
    }
    if event.conversation_id.trim().is_empty() {
        return Err("work event conversationId is required".to_owned());
    }
    if event.sequence < 0 {
        return Err("work event sequence cannot be negative".to_owned());
    }
    chrono::DateTime::parse_from_rfc3339(&event.timestamp)
        .map_err(|_| "work event timestamp must be ISO-8601".to_owned())?;
    Ok(())
}

fn known_event_type(event_type: &str) -> bool {
    WORK_EVENT_TYPES.contains(&event_type)
}

fn ensure_references_belong_to_conversation(
    connection: &rusqlite::Connection,
    conversation_id: &str,
    goal_id: Option<&str>,
    task_id: Option<&str>,
    run_id: Option<&str>,
) -> rusqlite::Result<()> {
    let conversation_exists: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM conversations WHERE id = ?1)",
        [conversation_id],
        |row| row.get(0),
    )?;
    if !conversation_exists {
        return Err(rusqlite::Error::QueryReturnedNoRows);
    }
    if let Some(goal_id) = goal_id {
        let owner: Option<String> = connection
            .query_row(
                "SELECT conversation_id FROM goals WHERE id = ?1",
                [goal_id],
                |row| row.get(0),
            )
            .optional()?;
        if owner.as_deref() != Some(conversation_id) {
            return Err(rusqlite::Error::InvalidQuery);
        }
    }
    if let Some(task_id) = task_id {
        let owner: Option<String> = connection
            .query_row(
                "SELECT goals.conversation_id FROM work_tasks
                 JOIN goals ON goals.id = work_tasks.goal_id WHERE work_tasks.id = ?1",
                [task_id],
                |row| row.get(0),
            )
            .optional()?;
        if owner.as_deref() != Some(conversation_id) {
            return Err(rusqlite::Error::InvalidQuery);
        }
    }
    if let Some(run_id) = run_id {
        let owner: Option<String> = connection
            .query_row(
                "SELECT conversation_id FROM runs WHERE id = ?1",
                [run_id],
                |row| row.get(0),
            )
            .optional()?;
        if owner.as_deref() != Some(conversation_id) {
            return Err(rusqlite::Error::InvalidQuery);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_conversation_sequence_is_idempotent_and_unknown_types_are_ignored() {
        let path = std::env::temp_dir().join(format!("fox-work-events-{}.db", Uuid::new_v4()));
        let database = Database::open(path.clone()).expect("open database");
        let conversation = database
            .create_conversation(database.default_agent_id(), Some("events"), None, None)
            .expect("create conversation");
        let event = WorkEventRecord {
            event_type: "goal.proposed".to_owned(),
            schema_version: WORK_EVENT_SCHEMA_VERSION,
            conversation_id: conversation.id.clone(),
            goal_id: None,
            task_id: None,
            run_id: None,
            trace_id: None,
            span_id: None,
            sequence: 7,
            timestamp: "2026-07-30T00:00:00.000Z".to_owned(),
            data: serde_json::json!({}),
        };
        assert!(database.apply_work_event(&event).expect("first event"));
        assert!(!database.apply_work_event(&event).expect("duplicate event"));
        let mut unknown = event.clone();
        unknown.event_type = "future.work.event".to_owned();
        unknown.sequence = 8;
        assert!(!database.apply_work_event(&unknown).expect("unknown event"));
        assert_eq!(
            database
                .list_work_events(&conversation.id)
                .expect("list events")
                .len(),
            1
        );
        drop(database);
        let _ = std::fs::remove_file(path);
    }
}
