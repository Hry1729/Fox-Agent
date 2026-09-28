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
    } else {
        // The final/preview reasoning is a replacement, including an empty
        // replacement. Do not leave a superseded streaming draft on reload.
        tx.execute("UPDATE run_events SET event_json=?2 WHERE id=?1", params![
            format!("kernel-display:{run}:reasoning:{checkpoint}"),
            json!({"type":"reasoning.delta","delta":"","source":format!("kernel-model:{checkpoint}")}).to_string(),
        ])?;
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
        WHERE e.run_id=?1 AND e.seq>?2 AND e.event_type='tool.completed' AND t.state='completed' AND t.tool IN ('write_file','edit_file','attachment_compute','call_mcp_tool') ORDER BY e.seq")?;
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
        if tool == "attachment_compute" {
            super::computed_artifacts::persist(tx,run,&id,&result["details"],now)?;
            continue;
        }
        // A built-in Office mutation (call_mcp_tool → fox-office) is a
        // user-facing result; project the Host-verified managed write into
        // the artifact list so the xlsx/docx appears alongside compute files.
        // The lifecycle class is the Host's own verdict over the verified
        // operation and resolved path, never a file-extension guess and never
        // the connector's own claim, so a CSV delivered on purpose stays a
        // deliverable while an intermediate file stays a process file.
        if tool == "call_mcp_tool" {
            if let Some(meta) = result["details"].get("foxPreview") {
                // A rendered preview: a Host-private view of a result, reached
                // through that result's preview entry. It carries no `kind`
                // (nothing was created or modified in the user's project) and
                // never enters the version registry.
                if let Some(path) = meta["storagePath"].as_str() {
                    project_office_preview(tx, run, &id, meta, path, now)?;
                    continue;
                }
            }
            let Some(meta) = result["details"].get("foxManagedFile") else { continue };
            let Some(path) = meta["storagePath"].as_str() else { continue };
            let name = meta["displayName"]
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| {
                    std::path::Path::new(path)
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or(path)
                        .to_owned()
                });
            let kind = if meta["changeKind"].as_str() == Some("created") {
                "created_file"
            } else {
                "modified_file"
            };
            let bytes = meta["afterSize"].as_i64().unwrap_or(0);
            let sha = meta["afterHash"].as_str();
            let (class, origin) = office_artifact_lifecycle(tx, meta, path, run)?;
            let changed = tx.execute(
                "UPDATE artifacts SET display_name=?2,byte_size=?3,sha256=COALESCE(?4,sha256),status='ready',updated_at=?5,
                    artifact_type=CASE WHEN artifact_type='created_file' THEN artifact_type ELSE ?6 END,
                    artifact_class=?8,artifact_origin=?9
                    WHERE run_id=?1 AND storage_path=?7",
                params![run,name,bytes,sha,now,kind,path,class,origin],
            )?;
            if changed == 0 {
                tx.execute("INSERT INTO artifacts(id,conversation_id,run_id,display_name,artifact_type,artifact_class,artifact_origin,storage_path,byte_size,sha256,status,created_at,updated_at)
                    SELECT ?1,conversation_id,?2,?3,?4,?9,?10,?5,?6,?7,'ready',?8,?8 FROM run_control_bindings WHERE run_id=?2",
                    params![format!("kernel-artifact:{run}:{id}"),run,name,kind,path,bytes,sha,now,class,origin])?;
            }
            continue;
        }
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
        // `write_file`/`edit_file` are the general project file tools. A file
        // the Host resolves inside the conversation deliverable folder is a
        // deliverable; anything else is a process file. Still a Host fact
        // (the resolved path), not an extension guess.
        let conversation: Option<String> = tx
            .query_row(
                "SELECT conversation_id FROM run_control_bindings WHERE run_id=?1",
                params![run],
                |row| row.get(0),
            )
            .optional()?;
        let project_root = match conversation.as_deref() {
            Some(conversation_id) => {
                super::computed_artifacts::conversation_project_root(tx, conversation_id)?
            }
            None => None,
        };
        let (class, origin) = crate::runtime_host::artifact_store::classify(
            project_root.as_deref().map(std::path::Path::new),
            std::path::Path::new(path),
            tool.as_str(),
            false,
        );
        let changed=tx.execute("UPDATE artifacts SET display_name=?2,byte_size=?3,status='ready',updated_at=?4,
            artifact_type=CASE WHEN artifact_type='created_file' THEN artifact_type ELSE ?5 END,media_type=COALESCE(?6,media_type),
            artifact_class=?8,artifact_origin=?9
            WHERE run_id=?1 AND storage_path=?7",params![run,name,bytes,now,kind,media,path,class.as_str(),origin.as_str()])?;
        if changed == 0 {
            tx.execute("INSERT INTO artifacts(id,conversation_id,run_id,display_name,artifact_type,artifact_class,artifact_origin,storage_path,byte_size,media_type,status,created_at,updated_at)
                SELECT ?1,conversation_id,?2,?3,?4,?9,?10,?5,?6,?7,'ready',?8,?8 FROM run_control_bindings WHERE run_id=?2",
                params![format!("kernel-artifact:{run}:{id}"),run,name,kind,path,bytes,media,now,class.as_str(),origin.as_str()])?;
        }
    }
    Ok(())
}

/// Project a rendered `office_render` preview into the artifact list.
///
/// The preview is a Host-private view: `artifact_class='preview'` and
/// `artifact_origin='host_private'` for the default Host-cached render, or
/// `project` when the caller deliberately rendered into the conversation
/// deliverable folder. The class is recomputed from the Host's own roots, so
/// the declaration cannot promote a cache file into a deliverable.
fn project_office_preview(
    tx: &Transaction<'_>,
    run: &str,
    call: &str,
    meta: &Value,
    path: &str,
    now: i64,
) -> rusqlite::Result<()> {
    let conversation: Option<String> = tx
        .query_row(
            "SELECT conversation_id FROM run_control_bindings WHERE run_id=?1",
            params![run],
            |row| row.get(0),
        )
        .optional()?;
    let project_root = match conversation.as_deref() {
        Some(conversation_id) => {
            super::computed_artifacts::conversation_project_root(tx, conversation_id)?
        }
        None => None,
    };
    let host_private = match project_root.as_deref().map(std::path::Path::new) {
        // Recompute the placement structurally instead of trusting the declared
        // `artifactOrigin`: a preview the Host cached is by definition outside
        // the project, and a preview that stayed inside the project is an
        // explicit export. A forged origin field therefore changes nothing.
        Some(root) => {
            !crate::runtime_host::artifact_store::is_inside(root, std::path::Path::new(path))
        }
        // Without an authorized root the Host cannot place the bytes at all, so
        // the conservative answer is "private", never "deliverable".
        None => true,
    };
    let (class, origin) = crate::runtime_host::artifact_store::classify(
        project_root.as_deref().map(std::path::Path::new),
        std::path::Path::new(path),
        meta["tool"].as_str().unwrap_or("office_render"),
        host_private,
    );
    // The card must name the *result* the preview belongs to, not the cache
    // file: a preview is a view of something the user recognises. The renderer
    // already labels it "预览", so showing the source document here is what
    // makes the association visible without a new column. The preview's own path
    // is still what the open action uses.
    let name = meta["sourceFile"]
        .as_str()
        .and_then(|source| {
            std::path::Path::new(source)
                .file_name()
                .and_then(|value| value.to_str())
        })
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .or_else(|| meta["displayName"].as_str().map(str::to_owned))
        .unwrap_or_else(|| {
            std::path::Path::new(path)
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or(path)
                .to_owned()
        });
    let bytes = meta["afterSize"].as_i64().unwrap_or(0);
    let sha = meta["afterHash"].as_str();
    let media = meta["mediaType"].as_str();
    let changed = tx.execute(
        "UPDATE artifacts SET display_name=?2,byte_size=?3,sha256=COALESCE(?4,sha256),
             media_type=COALESCE(?5,media_type),status='ready',updated_at=?6,
             artifact_class=?7,artifact_origin=?8
         WHERE run_id=?1 AND storage_path=?9",
        params![run, name, bytes, sha, media, now, class.as_str(), origin.as_str(), path],
    )?;
    if changed == 0 {
        tx.execute(
            "INSERT INTO artifacts(id,conversation_id,run_id,display_name,artifact_type,artifact_class,artifact_origin,storage_path,byte_size,sha256,media_type,status,created_at,updated_at)
             SELECT ?1,conversation_id,?2,?3,'created_file',?9,?10,?4,?5,?6,?7,'ready',?8,?8 FROM run_control_bindings WHERE run_id=?2",
            params![
                format!("kernel-artifact:{run}:{call}"),
                run,
                name,
                path,
                bytes,
                sha,
                media,
                now,
                class.as_str(),
                origin.as_str()
            ],
        )?;
    }
    Ok(())
}

/// Lifecycle class/origin for a verified Office declaration, derived from the
/// Host's own project root so a connector cannot relabel an intermediate file
/// as a deliverable.
///
/// `meta["artifactClass"]` is the connector's declaration. It is not trusted:
/// the class comes from the Host's own `classify` verdict over the verified
/// tool and resolved path, so a result claiming "deliverable" for a file in the
/// project root still lands as a process file.
fn office_artifact_lifecycle(
    tx: &Transaction<'_>,
    meta: &Value,
    path: &str,
    run: &str,
) -> rusqlite::Result<(&'static str, &'static str)> {
    let conversation: Option<String> = tx
        .query_row(
            "SELECT conversation_id FROM run_control_bindings WHERE run_id=?1",
            params![run],
            |row| row.get(0),
        )
        .optional()?;
    let project_root = match conversation.as_deref() {
        Some(conversation_id) => {
            super::computed_artifacts::conversation_project_root(tx, conversation_id)?
        }
        None => None,
    };
    Ok(office_artifact_lifecycle_for(
        project_root.as_deref().map(std::path::Path::new),
        meta,
        path,
        false,
    ))
}

/// The pure part of the projection: no database, no declarations.
///
/// `host_private` is passed in already recomputed from the Host's own roots by
/// the caller, so every input here is a Host fact. The declared
/// `artifactClass` / `artifactOrigin` fields in `meta` are deliberately never
/// read.
fn office_artifact_lifecycle_for(
    project_root: Option<&std::path::Path>,
    meta: &Value,
    path: &str,
    host_private: bool,
) -> (&'static str, &'static str) {
    let tool = meta["tool"].as_str().unwrap_or_default();
    let (class, origin) = crate::runtime_host::artifact_store::classify(
        project_root,
        std::path::Path::new(path),
        tool,
        host_private,
    );
    (class.as_str(), origin.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A forged `artifactClass`/`artifactOrigin` in a tool result can never move
    /// a file between buckets: the projection recomputes the verdict from the
    /// Host's own tool name, path and private-root decision.
    #[test]
    fn a_declared_artifact_class_cannot_relabel_a_projected_file() {
        let root = std::path::Path::new(r"C:\proj");
        let host_private_path = r"C:\Users\x\AppData\Roaming\com.fox.agent\artifacts\previews\a\b.html";
        let in_project = r"C:\proj\fox\case\report.xlsx";

        // A cached render that *claims* to be a project deliverable.
        let forged = json!({
            "tool": "office_render",
            "artifactClass": "deliverable",
            "artifactOrigin": "project",
        });
        assert_eq!(
            office_artifact_lifecycle_for(Some(root), &forged, host_private_path, true),
            ("preview", "host_private")
        );

        // A genuine deliverable that *claims* to be a process file.
        let forged = json!({
            "tool": "office_create",
            "artifactClass": "process",
            "artifactOrigin": "host_private",
        });
        assert_eq!(
            office_artifact_lifecycle_for(Some(root), &forged, in_project, false),
            ("deliverable", "project")
        );

        // An explicitly exported render inside the project is the user's result,
        // and a general file tool writing into the conversation result folder is
        // a result too.
        let render = json!({
            "tool": "office_render",
            "artifactClass": "preview",
            "artifactOrigin": "host_private",
        });
        assert_eq!(
            office_artifact_lifecycle_for(Some(root), &render, r"C:\proj\fox\case\p.html", false),
            ("deliverable", "project")
        );
        let writer = json!({ "tool": "write_file" });
        assert_eq!(
            office_artifact_lifecycle_for(Some(root), &writer, r"C:\proj\fox\case\a.csv", false),
            ("deliverable", "project")
        );
        assert_eq!(
            office_artifact_lifecycle_for(Some(root), &writer, r"C:\proj\notes.txt", false),
            ("process", "project")
        );

        // An intermediate compute file stays a Host-private process file, and a
        // missing project root never yields a deliverable claim.
        assert_eq!(
            office_artifact_lifecycle_for(Some(root), &forged, r"C:\proj\data\rows.csv", true),
            ("process", "host_private")
        );
        assert_eq!(
            office_artifact_lifecycle_for(None, &forged, in_project, false),
            ("process", "host_private")
        );
    }
}
