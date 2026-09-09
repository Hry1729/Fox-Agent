//! Durable display content, separate from Kernel execution facts and replay inputs.
use super::{now_ms, Database};
use rusqlite::{params, OptionalExtension, Transaction};
use serde_json::{json, Value};

pub(super) fn event(
    tx: &Transaction<'_>,
    run: &str,
    key: &str,
    payload: &Value,
    now: i64,
) -> rusqlite::Result<()> {
    tx.execute(
        "INSERT INTO run_events(id,run_id,seq,event_type,event_json,created_at)
        VALUES(?1,?2,(SELECT COALESCE(MAX(seq),0)+1 FROM run_events WHERE run_id=?2),?3,?4,?5)
        ON CONFLICT(id) DO UPDATE SET event_json=excluded.event_json",
        params![
            format!("kernel-display:{run}:{key}"),
            run,
            payload["type"].as_str().unwrap_or("kernel.display"),
            payload.to_string(),
            now
        ],
    )?;
    Ok(())
}

pub(super) fn reasoning(
    tx: &Transaction<'_>,
    run: &str,
    checkpoint: u64,
    text: &str,
    now: i64,
) -> rusqlite::Result<()> {
    if !text.is_empty() {
        event(
            tx,
            run,
            &format!("reasoning:{checkpoint}"),
            &json!({"type":"reasoning.delta","delta":text,"source":format!("kernel-model:{checkpoint}")}),
            now,
        )?;
    }
    Ok(())
}

pub(super) fn activity(tx: &Transaction<'_>, run: &str, now: i64) -> rusqlite::Result<()> {
    let state: String = tx.query_row(
        "SELECT state FROM kernel_runs WHERE run_id=?1",
        [run],
        |r| r.get(0),
    )?;
    if state != "created" {
        event(tx, run, "started", &json!({"type":"run.started"}), now)?;
    }
    let mut stmt=tx.prepare("SELECT tool_call_id,tool,state,canonical_input_json,result_json FROM kernel_tool_calls WHERE run_id=?1 ORDER BY created_at,source_order")?;
    let rows = stmt
        .query_map([run], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, Option<String>>(4)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    for (id, name, status, input, result) in rows {
        let input: Value =
            serde_json::from_str(&input).map_err(|_| rusqlite::Error::InvalidQuery)?;
        event(
            tx,
            run,
            &format!("tool:{id}:started"),
            &json!({"type":"tool.started","toolCallId":id,"tool":name,"input":input,"status":status}),
            now,
        )?;
        if matches!(status.as_str(), "completed" | "failed" | "cancelled") {
            let result = result
                .and_then(|s| serde_json::from_str::<Value>(&s).ok())
                .unwrap_or(Value::Null);
            event(
                tx,
                run,
                &format!("tool:{id}:completed"),
                &json!({"type":"tool.completed","toolCallId":id,"tool":name,"result":result,"isError":status!="completed","status":status}),
                now,
            )?;
        }
    }
    Ok(())
}

impl Database {
    /// Save received text before publishing it. It cannot settle a Run, approve
    /// a tool or become a replay checkpoint. Stale/foreign frames are ignored.
    pub(crate) fn save_kernel_model_display(
        &self,
        notice: &fox_engine_protocol::KernelModelPreview,
    ) -> Result<bool, String> {
        notice.validate()?;
        let saved = self.with_connection(|connection| {
            let tx=connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let owns:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM kernel_runs r JOIN run_control_bindings b ON b.run_id=r.run_id
                WHERE r.run_id=?1 AND b.conversation_id=?2 AND r.turn_id=?3 AND r.state='running'
                AND r.kernel_mode='authoritative' AND b.authority='authoritative' AND r.last_event_seq=?4)",
                params![notice.run_id,notice.conversation_id,notice.turn_id,notice.checkpoint_seq+1],|r|r.get(0))?;
            if !owns { return Ok(false); }
            let id=format!("kernel-message:{}:{}",notice.run_id,notice.checkpoint_seq);
            let previous:Option<(String,i64)>=tx.query_row("SELECT status,COALESCE(json_extract(metadata_json,'$.displayRevision'),0) FROM messages WHERE id=?1",[&id],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
            if previous.is_some_and(|(status,revision)|status!="streaming" || revision>=notice.revision as i64) { return Ok(false); }
            let now=now_ms();
            let metadata=json!({"authority":"kernel","checkpointSeq":notice.checkpoint_seq,"displayRevision":notice.revision,"partial":true});
            tx.execute("INSERT INTO messages(id,conversation_id,run_id,role,kind,content,status,ordinal,runtime_message_id,metadata_json,created_at,updated_at)
                VALUES(?1,?2,?3,'assistant','text',?4,'streaming',(SELECT COALESCE(MAX(ordinal),0)+1 FROM messages WHERE conversation_id=?2),?1,?5,?6,?6)
                ON CONFLICT(id) DO UPDATE SET content=excluded.content,metadata_json=excluded.metadata_json,updated_at=excluded.updated_at",
                params![id,notice.conversation_id,notice.run_id,notice.text,metadata.to_string(),now])?;
            reasoning(&tx,&notice.run_id,notice.checkpoint_seq,&notice.reasoning,now)?;
            tx.execute("UPDATE conversations SET updated_at=?2,last_message_at=?2 WHERE id=?1",params![notice.conversation_id,now])?;
            tx.commit()?;
            Ok(true)
        })?;
        if saved {
            self.kernel_changes.committed();
        }
        Ok(saved)
    }
}

pub(super) fn artifacts(
    tx: &Transaction<'_>,
    run: &str,
    previous_seq: i64,
    now: i64,
) -> rusqlite::Result<()> {
    let mut query=tx.prepare("SELECT t.tool_call_id,t.tool,t.result_json FROM kernel_events e JOIN kernel_tool_calls t
        ON t.run_id=e.run_id AND t.tool_call_id=json_extract(e.payload_json,'$.toolCallId')
        WHERE e.run_id=?1 AND e.seq>?2 AND e.event_type='tool.completed' AND t.state='completed' AND t.tool IN ('write_file','edit_file') ORDER BY e.seq")?;
    let results = query
        .query_map(params![run, previous_seq], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    for (id, tool, result) in results {
        let result: Value =
            serde_json::from_str(&result).map_err(|_| rusqlite::Error::InvalidQuery)?;
        let Some(path) = result["details"]["path"].as_str() else {
            continue;
        };
        let name = std::path::Path::new(path)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(path);
        let bytes = result["details"]["bytes"].as_i64().unwrap_or(0);
        let kind = if tool == "edit_file" || result["details"]["operation"] == "modified" {
            "modified_file"
        } else {
            "created_file"
        };
        let media =
            if path.to_lowercase().ends_with(".html") || path.to_lowercase().ends_with(".htm") {
                Some("text/html")
            } else {
                None
            };
        let changed=tx.execute("UPDATE artifacts SET display_name=?2,byte_size=?3,status='ready',updated_at=?4,
            artifact_type=CASE WHEN artifact_type='created_file' THEN artifact_type ELSE ?5 END,media_type=COALESCE(?6,media_type)
            WHERE run_id=?1 AND storage_path=?7",params![run,name,bytes,now,kind,media,path])?;
        if changed == 0 {
            tx.execute("INSERT INTO artifacts(id,conversation_id,run_id,display_name,artifact_type,storage_path,byte_size,media_type,status,created_at,updated_at)
                SELECT ?1,conversation_id,?2,?3,?4,?5,?6,?7,'ready',?8,?8 FROM run_control_bindings WHERE run_id=?2",
                params![format!("kernel-artifact:{run}:{id}"),run,name,kind,path,bytes,media,now])?;
        }
    }
    Ok(())
}
