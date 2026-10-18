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

/// The durable delivery verdict bound to this artifact's current version, if any.
///
/// Returns `(status, same_version, reason, withdraw_pass)`. `withdraw_pass` is the
/// shared display guard's answer for this row, computed from the **raw** durable
/// `finding_json` — never from a projected/truncated form, and never from this
/// view's own notion of what "passed" means. Carrying it out of this function is
/// what lets the caller answer the pass question the same way the checklist
/// projection does.
fn bound_check(
    connection: &Connection,
    run: &str,
    artifact: &ArtifactRecord,
    version_id: &str,
    root: &Path,
) -> rusqlite::Result<Option<(String, bool, Option<String>, bool)>> {
    let mut query = connection.prepare(
        "SELECT target_path,artifact_id,status,finding_json,requirements_json,item_key
         FROM delivery_checklist_items WHERE run_id=?1 ORDER BY item_key",
    )?;
    let rows = query.query_map([run], |row| {
        Ok((
            row.get::<_, Option<String>>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, Option<String>>(3)?,
            row.get::<_, Option<String>>(4)?,
            row.get::<_, String>(5)?,
        ))
    })?;
    for row in rows {
        let (target, artifact_id, status, finding, requirements, item_key) = row?;
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
        // The same identity-aware guard the checklist projection consults. Judged
        // on the raw columns so a truncated projection cannot make a contradicted
        // pass look legitimate, and so a legitimate pass is never withheld on
        // presentation grounds. The row's own contract and its ledger key are read
        // in the same statement as its finding, so neither the identity nor the
        // row binding can describe another row.
        let withdraw_pass = crate::runtime_host::delivery::passed_qualification_of_row(
            &status,
            finding.as_deref(),
            requirements.as_deref(),
            &item_key,
        )
        .withdraws_pass();
        return Ok(Some((status, same_version, reason, withdraw_pass)));
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
    // A file the model handed over through `present_files` is a delivery, not a
    // claim awaiting a verdict. Its detail line describes the file and the
    // hand-over; it must not borrow the verification vocabulary of Runs whose
    // contracts were frozen under the previous design. That machinery never ran
    // for these rows — there is no checklist to be pending against — so saying
    // "not yet verified" would invent an outstanding obligation.
    //
    // Rows written before the declaration columns existed have `declared_at`
    // NULL and keep the verification projection below unchanged.
    if artifact.declared_at.is_some() {
        let mut view = ArtifactDeliveryView {
            version_id: None,
            version_no: None,
            source_tool_call_id: artifact.turn_call_id.clone(),
            purpose_source: purpose_source.into(),
            verification_status: "declared".into(),
            summary: "已作为交付文件登记；文件当前可访问。".into(),
        };
        match std::fs::metadata(path) {
            Ok(value) if value.is_file() => {
                let note = artifact
                    .declared_description
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty());
                view.summary = match note {
                    Some(note) => format!("已交付：{note}"),
                    None => "已作为交付文件登记；文件当前可访问。".into(),
                };
            }
            _ => {
                view.verification_status = "unavailable".into();
                view.summary = "文件不存在或当前无法访问。".into();
            }
        }
        artifact.delivery = Some(view);
        return Ok(());
    }
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
        // A `passed` row whose own durable facts do not support a pass is not shown
        // as passed — the same rule the checklist projection applies, so the two
        // views cannot report opposite conclusions for one row. It falls back to
        // the existing "not yet verified" state rather than `failed`: the record is
        // unconfirmable, and this view must not invent the verdict a checker
        // withheld. The identity/version/hash checks above keep their priority, so
        // a real staleness is still reported as `stale` and a legitimate `failed`
        // below is still reported as `failed`.
        //
        // `withdraw_pass` is only ever true for a row written `passed`, so this arm
        // cannot capture a legitimate `failed`.
        Some((_, true, _, true)) => {
            view.summary = "交付验收记录与自身事实矛盾，未确认通过。".into();
        }
        Some((written, true, _, _)) if written == "passed" => {
            view.verification_status = "passed".into();
            view.summary = "加载时文件与已通过验收的版本一致。".into();
        }
        Some((written, true, reason, _)) if written == "verified" => {
            view.verification_status = "limited".into();
            view.summary = reason.unwrap_or_else(|| "文件检查完成，业务验收范围有限。".into());
        }
        Some((written, true, reason, _)) if written == "failed" => {
            view.verification_status = "failed".into();
            view.summary = reason.unwrap_or_else(|| "交付验收失败。".into());
        }
        Some((written, false, _, _)) if written != "pending" => {
            view.verification_status = "stale".into();
            view.summary = "验收记录对应另一文件版本。".into();
        }
        Some((_, _, _, _)) => {
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
    fn a_declared_file_reads_as_a_delivery_not_a_pending_check() {
        let root = std::env::temp_dir().join(format!("fox-declared-view-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("final.txt");
        std::fs::write(&path, b"line one\nline two\n").unwrap();
        let (_hash, size) = crate::runtime_host::managed_files::hash_file(&path).unwrap();
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
                 artifact_id TEXT,status TEXT,finding_json TEXT,
                 requirements_json TEXT NOT NULL DEFAULT '[]');",
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
                "INSERT INTO messages VALUES ('run','user',1,'创建 final.txt 并交付')",
                [],
            )
            .unwrap();
        let mut artifact = ArtifactRecord {
            id: "artifact".into(),
            conversation_id: "conv".into(),
            run_id: Some("run".into()),
            display_name: "final.txt".into(),
            artifact_type: "created_file".into(),
            artifact_class: "deliverable".into(),
            artifact_origin: "project".into(),
            storage_path: path.to_string_lossy().into_owned(),
            media_type: None,
            byte_size: size,
            sha256: None,
            status: "ready".into(),
            created_at: 1,
            updated_at: 1,
            turn_call_id: Some("call_real".into()),
            declared_at: Some(2),
            declared_description: Some("两行文本".into()),
            delivery: None,
        };
        // No managed version and no checklist row: exactly the state a
        // present_files declaration leaves behind. The old projection called this
        // "尚未关联可核对的文件版本与交付验收", which reads as an outstanding check.
        enrich(&connection, &mut artifact).unwrap();
        let view = artifact.delivery.as_ref().unwrap();
        assert_eq!(view.verification_status, "declared");
        assert_eq!(view.summary, "已交付：两行文本");
        assert_eq!(view.source_tool_call_id.as_deref(), Some("call_real"));

        // The declaration keeps describing the file even with no note.
        artifact.declared_description = None;
        enrich(&connection, &mut artifact).unwrap();
        assert_eq!(
            artifact.delivery.as_ref().unwrap().summary,
            "已作为交付文件登记；文件当前可访问。"
        );

        // A declared file that later disappears still reports that plainly.
        std::fs::remove_file(&path).unwrap();
        enrich(&connection, &mut artifact).unwrap();
        let view = artifact.delivery.as_ref().unwrap();
        assert_eq!(view.verification_status, "unavailable");
        assert_eq!(view.summary, "文件不存在或当前无法访问。");

        std::fs::remove_dir(&root).unwrap();
    }

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
                 artifact_id TEXT,status TEXT,finding_json TEXT,
                 requirements_json TEXT NOT NULL DEFAULT '[]');",
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
        connection.execute("INSERT INTO delivery_checklist_items
            (run_id,item_key,target_path,artifact_id,status,finding_json)
            VALUES ('run','file:summary.json','summary.json',NULL,'passed',?1)",
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
            turn_call_id: None,
            declared_at: None,
            declared_description: None,
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
