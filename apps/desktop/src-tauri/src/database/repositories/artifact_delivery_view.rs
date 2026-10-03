use super::{ArtifactDeliveryView, ArtifactRecord};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;
use std::path::{Path, PathBuf};

const MAX_DISPLAY_HASH_BYTES: u64 = 64 * 1024 * 1024;

fn path_key(value: &str) -> String {
    value
        .strip_prefix(r"\\?\")
        .unwrap_or(value)
        .replace('\\', "/")
        .to_lowercase()
}

fn user_named_this_output(
    connection: &Connection,
    run: &str,
    root: &Path,
    path: &Path,
) -> rusqlite::Result<bool> {
    let mut current = run.to_owned();
    for _ in 0..32 {
        let text: Option<String> = connection.query_row(
            "SELECT content FROM messages WHERE run_id=?1 AND role='user' ORDER BY ordinal LIMIT 1",
            [&current], |row| row.get(0),
        ).optional()?;
        if text
            .as_deref()
            .and_then(|text| crate::runtime_host::delivery::explicit_file_purpose(text, root, path))
            == Some(crate::runtime_host::artifact_store::ArtifactClass::Deliverable)
        {
            return Ok(true);
        }
        let parent: Option<String> = connection
            .query_row(
                "SELECT continued_from_run_id FROM kernel_runs WHERE run_id=?1",
                [&current],
                |row| row.get(0),
            )
            .optional()?
            .flatten();
        let Some(parent) = parent else { break };
        current = parent;
    }
    Ok(false)
}

fn bound_check(
    connection: &Connection,
    run: &str,
    artifact: &ArtifactRecord,
    version_id: &str,
    root: &Path,
) -> rusqlite::Result<Option<(String, bool, Option<String>)>> {
    let mut query = connection.prepare(
        "SELECT target_path,artifact_id,status,finding_json
         FROM delivery_checklist_items WHERE run_id=?1 ORDER BY item_key",
    )?;
    let rows = query.query_map([run], |row| {
        Ok((
            row.get::<_, Option<String>>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, Option<String>>(3)?,
        ))
    })?;
    for row in rows {
        let (target, artifact_id, status, finding) = row?;
        let parsed = finding
            .as_deref()
            .and_then(|value| serde_json::from_str::<Value>(value).ok());
        let bound_path = parsed
            .as_ref()
            .and_then(|value| value.get("boundPath"))
            .and_then(Value::as_str);
        let target_path = target.as_deref().map(|value| {
            let candidate = PathBuf::from(value);
            if candidate.is_absolute() {
                candidate
            } else {
                root.join(candidate)
            }
        });
        let matches = artifact_id.as_deref() == Some(artifact.id.as_str())
            || bound_path.is_some_and(|value| path_key(value) == path_key(&artifact.storage_path))
            || target_path.as_deref().is_some_and(|value| {
                path_key(&value.to_string_lossy()) == path_key(&artifact.storage_path)
            });
        if !matches {
            continue;
        }
        let same_version = parsed
            .as_ref()
            .and_then(|value| value.get("managedVersionId"))
            .and_then(Value::as_str)
            == Some(version_id);
        let reason = parsed
            .as_ref()
            .and_then(|value| value.get("reason"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        return Ok(Some((status, same_version, reason)));
    }
    Ok(None)
}

pub(super) fn enrich(
    connection: &Connection,
    artifact: &mut ArtifactRecord,
) -> rusqlite::Result<()> {
    if artifact.artifact_class != "deliverable" {
        return Ok(());
    }
    let Some(run) = artifact.run_id.as_deref() else {
        return Ok(());
    };
    if artifact.artifact_origin != "project" {
        artifact.delivery = Some(ArtifactDeliveryView {
            version_id: None,
            version_no: None,
            source_tool_call_id: None,
            purpose_source: "unknown".into(),
            verification_status: "unavailable".into(),
            summary: "Host 私有文件尚未通过授权导出。".into(),
        });
        return Ok(());
    }
    let root: Option<String> = connection
        .query_row(
            "SELECT COALESCE(p.root_path,c.project_root) FROM conversations c
         LEFT JOIN projects p ON p.id=c.project_id WHERE c.id=?1",
            [&artifact.conversation_id],
            |row| row.get(0),
        )
        .optional()?
        .flatten();
    let Some(root) = root else { return Ok(()) };
    let root = Path::new(&root);
    let path = Path::new(&artifact.storage_path);
    if !crate::runtime_host::artifact_store::is_inside(root, path) {
        artifact.delivery = Some(ArtifactDeliveryView {
            version_id: None,
            version_no: None,
            source_tool_call_id: None,
            purpose_source: "unknown".into(),
            verification_status: "unavailable".into(),
            summary: "文件已不在当前授权的项目目录内。".into(),
        });
        return Ok(());
    }
    let purpose_source = if user_named_this_output(connection, run, root, path)? {
        "user_request"
    } else {
        "host_rule"
    };
    let version: Option<(String, i64, Option<String>, Option<String>)> = connection
        .query_row(
            "SELECT id,version_no,tool_call_id,after_hash FROM managed_file_versions
         WHERE conversation_id=?1 AND run_id=?2 AND storage_path=?3
         ORDER BY version_no DESC LIMIT 1",
            params![artifact.conversation_id, run, artifact.storage_path],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()?;
    let mut view = ArtifactDeliveryView {
        version_id: version.as_ref().map(|value| value.0.clone()),
        version_no: version.as_ref().map(|value| value.1),
        source_tool_call_id: version.as_ref().and_then(|value| value.2.clone()),
        purpose_source: purpose_source.into(),
        verification_status: "unverified".into(),
        summary: "尚未关联可核对的文件版本与交付验收。".into(),
    };
    let metadata = match std::fs::metadata(path) {
        Ok(value) if value.is_file() => value,
        _ => {
            view.verification_status = "unavailable".into();
            view.summary = "文件不存在或当前无法访问。".into();
            artifact.delivery = Some(view);
            return Ok(());
        }
    };
    let Some((version_id, _, _, Some(expected_hash))) = version else {
        artifact.delivery = Some(view);
        return Ok(());
    };
    if metadata.len() > MAX_DISPLAY_HASH_BYTES {
        view.summary = "文件超过当前展示核对上限，验收状态未确认。".into();
        artifact.delivery = Some(view);
        return Ok(());
    }
    let Some((current_hash, _)) = crate::runtime_host::managed_files::hash_file(path) else {
        view.verification_status = "unavailable".into();
        view.summary = "无法读取当前文件。".into();
        artifact.delivery = Some(view);
        return Ok(());
    };
    if current_hash
        != expected_hash
            .strip_prefix("sha256:")
            .unwrap_or(&expected_hash)
    {
        view.verification_status = "stale".into();
        view.summary = "当前文件与该版本记录不同，旧验收不能证明当前内容。".into();
        artifact.delivery = Some(view);
        return Ok(());
    }
    match bound_check(connection, run, artifact, &version_id, root)? {
        Some((status, true, reason)) if status == "passed" => {
            view.verification_status = "passed".into();
            view.summary = "加载时文件与已通过验收的版本一致。".into();
        }
        Some((status, true, reason)) if status == "verified" => {
            view.verification_status = "limited".into();
            view.summary = reason.unwrap_or_else(|| "文件检查完成，业务验收范围有限。".into());
        }
        Some((status, true, reason)) if status == "failed" => {
            view.verification_status = "failed".into();
            view.summary = reason.unwrap_or_else(|| "交付验收失败。".into());
        }
        Some((status, false, _)) if status != "pending" => {
            view.verification_status = "stale".into();
            view.summary = "验收记录对应另一文件版本。".into();
        }
        Some((_, _, _)) => {
            view.summary = "交付验收尚未完成。".into();
        }
        None => {
            view.summary = "文件版本已登记，未找到对应的交付验收。".into();
        }
    }
    artifact.delivery = Some(view);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;

    #[test]
    fn verdict_applies_only_to_the_recorded_current_file_version() {
        let root = std::env::temp_dir().join(format!("fox-delivery-view-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("summary.json");
        std::fs::write(&path, br#"{"ok":true}"#).unwrap();
        let (hash, size) = crate::runtime_host::managed_files::hash_file(&path).unwrap();
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE conversations(id TEXT,project_id TEXT,project_root TEXT);
             CREATE TABLE projects(id TEXT,root_path TEXT);
             CREATE TABLE messages(run_id TEXT,role TEXT,ordinal INTEGER,content TEXT);
             CREATE TABLE kernel_runs(run_id TEXT,continued_from_run_id TEXT);
             CREATE TABLE managed_file_versions(id TEXT,version_no INTEGER,tool_call_id TEXT,
                 after_hash TEXT,conversation_id TEXT,run_id TEXT,storage_path TEXT);
             CREATE TABLE delivery_checklist_items(run_id TEXT,item_key TEXT,target_path TEXT,
                 artifact_id TEXT,status TEXT,finding_json TEXT);",
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO conversations VALUES ('conv',NULL,?1)",
                [root.to_string_lossy().as_ref()],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO messages VALUES ('run','user',1,'请生成 summary.json')",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO managed_file_versions VALUES ('v1',1,'tool',?1,'conv','run',?2)",
                params![hash, path.to_string_lossy().as_ref()],
            )
            .unwrap();
        connection.execute("INSERT INTO delivery_checklist_items VALUES ('run','file:summary.json','summary.json',NULL,'passed',?1)",
            [serde_json::json!({"boundPath":path,"managedVersionId":"v1","reason":null}).to_string()]).unwrap();
        let mut artifact = ArtifactRecord {
            id: "artifact".into(),
            conversation_id: "conv".into(),
            run_id: Some("run".into()),
            display_name: "summary.json".into(),
            artifact_type: "created_file".into(),
            artifact_class: "deliverable".into(),
            artifact_origin: "project".into(),
            storage_path: path.to_string_lossy().into_owned(),
            media_type: None,
            byte_size: size,
            sha256: Some(hash),
            status: "ready".into(),
            created_at: 1,
            updated_at: 1,
            delivery: None,
        };
        enrich(&connection, &mut artifact).unwrap();
        let view = artifact.delivery.as_ref().unwrap();
        assert_eq!(view.verification_status, "passed");
        assert_eq!(view.version_id.as_deref(), Some("v1"));
        assert_eq!(view.purpose_source, "user_request");

        connection.execute("UPDATE delivery_checklist_items SET status='failed',finding_json=?1", [
            serde_json::json!({"boundPath":path,"managedVersionId":"v1","reason":"内容检查失败"}).to_string(),
        ]).unwrap();
        enrich(&connection, &mut artifact).unwrap();
        assert_eq!(
            artifact.delivery.as_ref().unwrap().verification_status,
            "failed"
        );
        assert_eq!(artifact.delivery.as_ref().unwrap().summary, "内容检查失败");

        std::fs::write(&path, br#"{"ok":false}"#).unwrap();
        enrich(&connection, &mut artifact).unwrap();
        assert_eq!(
            artifact.delivery.as_ref().unwrap().verification_status,
            "stale"
        );
        std::fs::remove_file(&path).unwrap();
        enrich(&connection, &mut artifact).unwrap();
        assert_eq!(
            artifact.delivery.as_ref().unwrap().verification_status,
            "unavailable"
        );
        std::fs::write(&path, br#"{"ok":true}"#).unwrap();
        connection
            .execute("DELETE FROM managed_file_versions", [])
            .unwrap();
        enrich(&connection, &mut artifact).unwrap();
        assert_eq!(
            artifact.delivery.as_ref().unwrap().verification_status,
            "unverified"
        );
        std::fs::remove_file(&path).unwrap();
        std::fs::remove_dir(&root).unwrap();
    }
}
