use super::*;
use rusqlite::OptionalExtension;

impl Database {
    pub(crate) fn computed_artifact_id(path: &str) -> String {
        format!(
            "compute-artifact:{}",
            hex::encode(Sha256::digest(path.as_bytes()))
        )
    }
}

/// The conversation's authorized project root, read inside the caller's own
/// transaction.
///
/// Artifact classification needs the root to tell "the model deliberately put
/// this result in the conversation deliverable folder" from "this is an
/// intermediate file", and the projection runs inside a transaction, so it
/// cannot open the pooled read path.
pub(super) fn conversation_project_root(
    tx: &rusqlite::Transaction<'_>,
    conversation_id: &str,
) -> rusqlite::Result<Option<String>> {
    tx.query_row(
        "SELECT COALESCE(p.root_path, c.project_root)
         FROM conversations c LEFT JOIN projects p ON p.id = c.project_id
         WHERE c.id = ?1",
        rusqlite::params![conversation_id],
        |row| row.get::<_, Option<String>>(0),
    )
    .optional()
    .map(Option::flatten)
}

/// Project the Host's verified generated files for either execution engine.
///
/// Every generated file here is an **intermediate computation artifact**: it
/// lives in the conversation's private compute workspace and is consumed by
/// reference (`artifactId`) rather than being a user-facing deliverable. That
/// is a Host fact about where the bytes are, not a property of the extension,
/// so a CSV saved with `saveFile` is a process file here even though a CSV the
/// model deliberately writes into the conversation deliverable folder is a
/// deliverable.
pub(super) fn persist(
    tx: &rusqlite::Transaction<'_>,
    run: &str,
    _call: &str,
    details: &Value,
    now: i64,
) -> rusqlite::Result<()> {
    let Some(files) = details["files"].as_array() else {
        return Ok(());
    };
    let conversation: Option<String> = tx
        .query_row(
            "SELECT conversation_id FROM run_control_bindings WHERE run_id=?1",
            rusqlite::params![run],
            |row| row.get(0),
        )
        .optional()?;
    let project_root = match conversation.as_deref() {
        Some(conversation_id) => conversation_project_root(tx, conversation_id)?,
        None => None,
    };
    let project_root = project_root.map(std::path::PathBuf::from);
    for file in files {
        let Some(path) = file["path"].as_str() else {
            continue;
        };
        let Some(bytes) = file["bytes"].as_i64() else {
            continue;
        };
        let Some(sha) = file["sha256"].as_str() else {
            continue;
        };
        if bytes < 0 || sha.len() != 64 || !sha.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(rusqlite::Error::InvalidQuery);
        }
        let name = std::path::Path::new(path)
            .file_name()
            .and_then(|p| p.to_str())
            .unwrap_or("result");
        let (class, origin) = crate::runtime_host::artifact_store::classify(
            project_root.as_deref(),
            std::path::Path::new(path),
            "attachment_compute",
            true,
        );
        tx.execute("INSERT OR IGNORE INTO artifacts(id,conversation_id,run_id,display_name,artifact_type,artifact_class,artifact_origin,storage_path,byte_size,sha256,media_type,status,created_at,updated_at)
            SELECT ?1,conversation_id,?2,?3,'created_file',?9,?10,?4,?5,?6,?7,'ready',?8,?8 FROM run_control_bindings WHERE run_id=?2",
            rusqlite::params![Database::computed_artifact_id(path),run,name,path,bytes,sha,file["mediaType"].as_str(),now,class.as_str(),origin.as_str()])?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;
    use std::fs;

    /// The real consumer seam: a completed `attachment_compute` result's
    /// `details.files` is registered under exactly the stable
    /// `compute-artifact:` id the model received as `files[].id` (and later
    /// passes to `office_import_data`). A file far larger than the 64 KiB
    /// inline-result cap is registered with its FULL byte size/hash — the
    /// cap applies to the result summary, not to outer `files` artifacts.
    #[test]
    fn persist_maps_details_files_to_full_size_compute_artifacts() {
        let db_path = std::env::temp_dir().join(format!("fox-computed-artifacts-{}.db", Uuid::new_v4()));
        let database = Database::open(db_path).unwrap();
        let conversation = database
            .create_conversation(database.default_agent_id(), Some("computed files"), None, None)
            .unwrap();
        let run = database.create_run(&conversation.id, "computed files", None).unwrap().run.id;
        database
            .with_connection(|c| {
                c.execute(
                    "INSERT INTO run_control_bindings(run_id,conversation_id,authority,engine_id,binding_json,binding_hash,created_at)
                     VALUES (?1,?2,'legacy','pi','{}','h',?3)",
                    params![&run, &conversation.id, crate::database::now_ms()],
                )
            })
            .unwrap();

        // Simulate one >64 KiB CSV the executor really wrote to the workspace.
        let output = std::env::temp_dir().join(format!("fox-computed-out-{}", Uuid::new_v4()));
        fs::create_dir_all(&output).unwrap();
        let csv_path = output.join("agv.csv");
        let mut csv = String::from("id,tag,v\n");
        for index in 0..6000u32 {
            csv.push_str(&format!("{index},agv-{index},{}\n", 10_000 + index));
        }
        fs::write(&csv_path, csv.as_bytes()).unwrap();
        let bytes = csv.len() as i64;
        let sha = hex::encode(Sha256::digest(csv.as_bytes()));
        let path_string = csv_path.to_string_lossy().into_owned();
        let details = serde_json::json!({ "files": [{
            "path": path_string,
            "displayName": "agv.csv",
            "bytes": bytes,
            "sha256": sha,
            "mediaType": "text/csv",
        }]});

        database
            .with_connection(|c| {
                let tx = c.transaction()?;
                persist(&tx, &run, "call-files", &details, crate::database::now_ms())?;
                tx.commit()?;
                Ok::<_, rusqlite::Error>(())
            })
            .unwrap();

        // The id the model uses is the same id the consumer registered.
        let id = Database::computed_artifact_id(&path_string);
        let (registered_bytes, registered_sha, status, name): (i64, String, String, String) =
            database
                .with_connection(|c| {
                    c.query_row(
                        "SELECT byte_size, sha256, status, display_name FROM artifacts WHERE id=?1 AND conversation_id=?2",
                        params![&id, &conversation.id],
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                    )
                })
                .unwrap();
        assert_eq!(registered_bytes, bytes, "the full (>64 KiB) file size is registered, not a truncated summary");
        assert!(registered_bytes > 65_536);
        assert_eq!(registered_sha, sha);
        assert_eq!(status, "ready");
        assert_eq!(name, "agv.csv");
        // Resolvable through the same ownership-scoped lookup the Office path uses.
        assert!(database.artifact_for_conversation(&conversation.id, &id).unwrap().is_some());
        // A foreign conversation cannot resolve it.
        let other = database
            .create_conversation(database.default_agent_id(), Some("other"), None, None)
            .unwrap();
        assert!(database.artifact_for_conversation(&other.id, &id).unwrap().is_none());
    }
}
