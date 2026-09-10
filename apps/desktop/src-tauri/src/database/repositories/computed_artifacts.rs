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
