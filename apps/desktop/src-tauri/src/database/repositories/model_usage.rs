//! Request accounting is telemetry, not a second controller or budget authority.
use super::{now_ms, Database};
use rusqlite::{params, OptionalExtension, Transaction};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

pub(super) fn store(tx: &Transaction<'_>, run: &str, record: &Value) -> rusqlite::Result<()> {
    let bad = || rusqlite::Error::InvalidParameterName("invalid usage-v1 request record".into());
    if record["schemaVersion"] != "usage-v1"
        || record["runId"] != run
        || record.to_string().len() > 32768
    {
        return Err(bad());
    }
    for key in ["eventId", "requestId", "attemptId", "provider", "modelId"] {
        if record[key]
            .as_str()
            .is_none_or(|s| s.is_empty() || s.len() > 512)
        {
            return Err(bad());
        }
    }
    if ![
        "agent",
        "planner",
        "reviewer",
        "compaction",
        "subtask",
        "other",
    ]
    .contains(&record["stage"].as_str().unwrap_or(""))
        || !["success", "failure", "cancelled", "timeout", "unknown"]
            .contains(&record["outcome"].as_str().unwrap_or(""))
    {
        return Err(bad());
    }
    for key in ["input", "output", "cacheRead", "cacheWrite"] {
        let v = &record["usage"][key];
        if !v.is_null() && v.as_u64().is_none_or(|n| n > 9_007_199_254_740_991) {
            return Err(bad());
        }
    }
    let identity = json!([run, record["requestId"], record["attemptId"]]).to_string();
    let id = format!(
        "usage-request:{}",
        hex::encode(Sha256::digest(identity.as_bytes()))
    );
    let payload = json!({"type":"usage.request","record":record}).to_string();
    let old: Option<String> = tx
        .query_row(
            "SELECT event_json FROM run_events WHERE id=?1",
            [&id],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(old) = old {
        return if old == payload { Ok(()) } else { Err(bad()) };
    }
    let exists: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM runs WHERE id=?1)",
        [run],
        |r| r.get(0),
    )?;
    if !exists {
        return Err(bad());
    }
    let seq: i64 = tx.query_row(
        "SELECT COALESCE(MAX(seq),0)+1 FROM run_events WHERE run_id=?1",
        [run],
        |r| r.get(0),
    )?;
    tx.execute("INSERT INTO run_events(id,run_id,seq,event_type,event_json,created_at) VALUES(?1,?2,?3,'usage.request',?4,?5)",params![id,run,seq,payload,now_ms()])?;
    Ok(())
}
impl Database {
    pub(crate) fn record_model_usage(&self, run: &str, record: &Value) -> Result<(), String> {
        self.with_connection(|conn| {
            let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            store(&tx, run, record)?;
            tx.commit()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn usage_records_are_durable_deduplicated_and_run_bound() {
        let path =
            std::env::temp_dir().join(format!("fox-request-usage-{}.db", uuid::Uuid::new_v4()));
        let db = Database::open(path.clone()).unwrap();
        let conversation = db
            .create_conversation(db.default_agent_id(), None, None, None)
            .unwrap();
        let run = db
            .create_run(&conversation.id, "usage", None)
            .unwrap()
            .run
            .id;
        let value = json!({"schemaVersion":"usage-v1","eventId":"event-1","requestId":"request-1","attemptId":"attempt-0",
            "runId":run,"provider":"fixture","modelId":"fixture","stage":"compaction","outcome":"failure",
            "usage":{"input":null,"output":null,"cacheRead":null,"cacheWrite":null,"completeness":"unavailable"},
            "cost":{"knownCost":null,"costComplete":false},"config":{"sent":null}});
        db.record_model_usage(&run, &value).unwrap();
        db.record_model_usage(&run, &value).unwrap();
        assert!(db.record_model_usage("different-run", &value).is_err());
        let mut conflict = value.clone();
        conflict["usage"]["input"] = json!(1);
        assert!(db.record_model_usage(&run, &conflict).is_err());
        drop(db);
        let db = Database::open(path.clone()).unwrap();
        let rows:Vec<String>=db.with_connection(|c|{let mut q=c.prepare("SELECT event_json FROM run_events WHERE run_id=?1 AND event_type='usage.request'")?;
            let rows=q.query_map([&run],|r|r.get(0))?;rows.collect()}).unwrap();
        assert_eq!(rows.len(), 1);
        let actual: Value = serde_json::from_str(&rows[0]).unwrap();
        assert_eq!(actual["record"], value);
        drop(db);
        let _ = std::fs::remove_file(path);
    }
}
