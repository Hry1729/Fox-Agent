use super::*;

impl Database {
    pub(crate) fn computed_artifact_id(path: &str) -> String {
        format!(
            "compute-artifact:{}",
            hex::encode(Sha256::digest(path.as_bytes()))
        )
    }
}

/// Project the Host's verified generated files for either execution engine.
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
        tx.execute("INSERT OR IGNORE INTO artifacts(id,conversation_id,run_id,display_name,artifact_type,storage_path,byte_size,sha256,media_type,status,created_at,updated_at)
            SELECT ?1,conversation_id,?2,?3,'created_file',?4,?5,?6,?7,'ready',?8,?8 FROM run_control_bindings WHERE run_id=?2",
            params![Database::computed_artifact_id(path),run,name,path,bytes,sha,file["mediaType"].as_str(),now])?;
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
