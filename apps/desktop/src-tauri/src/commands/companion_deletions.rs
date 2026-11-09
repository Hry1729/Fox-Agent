use crate::{
    app_state::AppState,
    data_paths::{companion_notes_dir, companion_workspace},
    database::{now_ms, ApiResponse, Database},
    runtime_host::attachment_compute::conversation_compute_root,
};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fs, io,
    path::{Path, PathBuf},
};
use tauri::State;

type DeletionResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FileEntry {
    path: PathBuf,
    kind: String,
    disposition: String,
    bytes: u64,
}

/// K2: one entry per contact background task. Deleting the contact removes the
/// task's execution conversation, so the manifest has to list what goes with it
/// instead of only counting the private chat.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TaskEntry {
    colleague_id: String,
    name: String,
    conversation_id: String,
    runs: i64,
    messages: i64,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Plan {
    companion_id: String,
    template_agent_id: String,
    display_name: String,
    conversation_id: String,
    /// Stored plans from before contact tasks existed deserialize with no tasks.
    #[serde(default)]
    tasks: Vec<TaskEntry>,
    workspace_path: PathBuf,
    workspace_files: u64,
    workspace_bytes: u64,
    messages: i64,
    notes: i64,
    topics: i64,
    shared_memories: i64,
    generations: i64,
    granted_projects: Vec<String>,
    files: Vec<FileEntry>,
}

fn on_db<T>(
    db: &Database,
    action: impl FnOnce(&mut Connection) -> DeletionResult<T>,
) -> Result<T, String> {
    db.with_connection(|conn| Ok(action(conn).map_err(|e| e.to_string())))?
}

fn hash(plan: &Plan) -> DeletionResult<String> {
    Ok(hex::encode(Sha256::digest(crate::canonical_json::to_vec(plan)?)))
}

fn normalized(path: &Path) -> io::Result<PathBuf> {
    match fs::symlink_metadata(path) {
        Ok(_) => path.canonicalize(),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            let parent = path
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .ok_or(e)?;
            let name = path
                .file_name()
                .ok_or_else(|| io::Error::other("路径无法解析"))?;
            Ok(normalized(parent)?.join(name))
        }
        Err(e) => Err(e),
    }
}

fn linked(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        return metadata.file_attributes() & 0x400 != 0;
    }
    #[cfg(not(windows))]
    metadata.is_symlink()
}

fn size(path: &Path) -> io::Result<(u64, u64)> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok((0, 0)),
        Err(e) => return Err(e),
    };
    if linked(&metadata) {
        return Ok((0, 0));
    }
    if metadata.is_file() {
        return Ok((1, metadata.len()));
    }
    let mut result = (0, 0);
    for child in fs::read_dir(path)? {
        let (files, bytes) = size(&child?.path())?;
        result.0 += files;
        result.1 += bytes;
    }
    Ok(result)
}

const SOURCES: &str = "SELECT conversation_id,storage_path AS path,'attachment' AS kind FROM attachments
    UNION ALL SELECT conversation_id,storage_path,'artifact' FROM artifacts
    UNION ALL SELECT conversation_id,backup_path,'managed_backup' FROM managed_file_versions WHERE backup_path IS NOT NULL
    UNION ALL SELECT conversation_id,after_backup_path,'managed_backup' FROM managed_file_versions WHERE after_backup_path IS NOT NULL";

fn managed_path(path: &Path, original_data: &Path, data: &Path) -> io::Result<Option<PathBuf>> {
    let Ok(relative) = path
        .strip_prefix(data)
        .or_else(|_| path.strip_prefix(original_data))
    else {
        return Ok(None);
    };
    let resolved = normalized(&data.join(relative))?;
    Ok(resolved.starts_with(data).then_some(resolved))
}

fn references(
    conn: &Connection,
    conversation: &str,
    original_data: &Path,
    data: &Path,
) -> DeletionResult<BTreeSet<PathBuf>> {
    let mut statement = conn.prepare(&format!(
        "SELECT DISTINCT path FROM ({SOURCES}) WHERE conversation_id<>?1"
    ))?;
    let paths = statement.query_map([conversation], |row| row.get::<_, String>(0))?;
    let mut result = BTreeSet::new();
    for path in paths {
        let path = PathBuf::from(path?);
        if let Some(path) = managed_path(&path, original_data, data)? {
            result.insert(path);
        }
    }
    Ok(result)
}

fn in_managed_area(data: &Path, compute: &Path, path: &Path, kind: &str) -> bool {
    let under = |directory: &str| {
        let root = data.join(directory);
        path != root && path.starts_with(root)
    };
    match kind {
        "attachment" => under("attachments"),
        "artifact" => under("artifacts") || (path != compute && path.starts_with(compute)),
        "managed_backup" => under("managed-file-backups"),
        "compute_dir" => path == compute,
        _ => false,
    }
}

fn file_entries(
    conn: &Connection,
    original_data: &Path,
    data: &Path,
    conversation: &str,
    workspace: &Path,
) -> DeletionResult<Vec<FileEntry>> {
    let compute = conversation_compute_root(&data.join("runtime-sessions"), conversation);
    let shared = references(conn, conversation, original_data, data)?;
    let mut statement = conn.prepare(&format!(
        "SELECT path,kind FROM ({SOURCES}) WHERE conversation_id=?1 ORDER BY kind,path"
    ))?;
    let rows = statement.query_map([conversation], |r| {
        Ok((
            PathBuf::from(r.get::<_, String>(0)?),
            r.get::<_, String>(1)?,
        ))
    })?;
    let mut files = rows.collect::<rusqlite::Result<Vec<_>>>()?;
    if compute.try_exists()? {
        files.push((compute.clone(), "compute_dir".into()));
    }
    let mut entries = BTreeMap::new();
    for (path, kind) in files {
        let Some(path) = managed_path(&path, original_data, data)? else {
            continue;
        };
        let shared_path = shared.contains(&path)
            || (kind == "compute_dir" && shared.iter().any(|p| p.starts_with(&path)));
        let remove = !path.starts_with(workspace)
            && !shared_path
            && in_managed_area(data, &compute, &path, &kind);
        let bytes = if remove && kind == "compute_dir" {
            size(&path)?.1
        } else {
            fs::symlink_metadata(&path)
                .ok()
                .filter(|m| m.is_file())
                .map_or(0, |m| m.len())
        };
        let disposition = if remove { "remove" } else { "retain" };
        let entry = FileEntry {
            path: path.clone(),
            kind,
            bytes,
            disposition: disposition.into(),
        };
        entries.insert(path, entry);
    }
    Ok(entries.into_values().collect())
}

fn plan(conn: &Connection, data: &Path, id: &str) -> DeletionResult<Plan> {
    let (template, name, conversation, default, stored): (String, String, String, bool, String) =
        conn.query_row(
            "SELECT p.template_agent_id,p.display_name,c.id,p.is_default,p.workspace_dir
         FROM companion_profiles p JOIN conversations c ON c.companion_id=p.id WHERE p.id=?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )?;
    if default {
        return Err("companion.default_not_deletable".into());
    }
    let count = |sql: &str, key: &str| conn.query_row(sql, [key], |r| r.get::<_, i64>(0));
    // K2: a contact task executes in its own conversation, so the stop check and
    // the purge have to cover the whole serial range, not just the private chat.
    let mut task_query = conn.prepare(
        "SELECT dc.id, dc.name, dc.conversation_id,
                (SELECT count(*) FROM runs r WHERE r.conversation_id=dc.conversation_id),
                (SELECT count(*) FROM messages m WHERE m.conversation_id=dc.conversation_id)
         FROM digital_colleagues dc
         WHERE dc.companion_id=?1 AND dc.companion_bound=1 ORDER BY dc.conversation_id",
    )?;
    let tasks = task_query
        .query_map([id], |r| {
            Ok(TaskEntry {
                colleague_id: r.get(0)?,
                name: r.get(1)?,
                conversation_id: r.get(2)?,
                runs: r.get(3)?,
                messages: r.get(4)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(task_query);
    let active = |conversation: &str| {
        count(
            "SELECT count(*) FROM runs WHERE conversation_id=?1 AND status IN ('queued','running','cancelling','awaiting_confirmation')",
            conversation,
        )
    };
    if active(&conversation)? > 0 {
        return Err("companion.run_active".into());
    }
    for task in &tasks {
        if active(&task.conversation_id)? > 0 {
            return Err("companion.run_active".into());
        }
    }
    if count("SELECT EXISTS(SELECT 1 FROM digital_colleagues WHERE conversation_id=?1
        UNION ALL SELECT 1 FROM work_graph_node_review_decisions WHERE reviewer_conversation_id=?1
        UNION ALL SELECT 1 FROM work_graph_specs s JOIN goals g ON g.id=s.goal_id WHERE g.conversation_id=?1)", &conversation)? != 0 {
        return Err("companion.in_use".into());
    }
    if !id.starts_with("cmp_") || Path::new(id).file_name().and_then(|s| s.to_str()) != Some(id) {
        return Err("无效的联系人目录".into());
    }
    let workspace = companion_workspace(data, id);
    if Path::new(&stored) != workspace {
        return Err("工作目录与联系人身份不符".into());
    }
    let canonical_data = data.canonicalize()?;
    let expected_root = companion_workspace(&canonical_data, id);
    if normalized(&workspace)? != expected_root {
        return Err("联系人的工作目录已指向别处".into());
    }
    let (workspace_files, workspace_bytes) = size(&expected_root)?;
    let files = file_entries(conn, data, &canonical_data, &conversation, &expected_root)?;
    let mut grants = conn.prepare(
        "SELECT p.name FROM companion_project_grants g JOIN projects p ON p.id=g.project_id
        WHERE companion_id=?1 AND revoked_at IS NULL ORDER BY p.name,p.id",
    )?;
    let names = grants
        .query_map([id], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(Plan {
        companion_id: id.into(),
        template_agent_id: template,
        display_name: name,
        messages: count(
            "SELECT count(*) FROM messages WHERE conversation_id=?1",
            &conversation,
        )?,
        notes: count(
            "SELECT count(*) FROM memory_entities WHERE scope='agent' AND scope_key=?1",
            id,
        )?,
        topics: count(
            "SELECT count(*) FROM companion_note_topics WHERE companion_id=?1",
            id,
        )?,
        shared_memories: count(
            "SELECT count(*) FROM memory_entities WHERE scope='global' AND source_companion_id=?1 AND state='confirmed' AND deleted_at IS NULL",
            id,
        )?,
        generations: count(
            "SELECT (SELECT count(*) FROM runtime_sessions WHERE conversation_id=?1)
            +(SELECT count(*) FROM runtime_session_generations WHERE conversation_id=?1)",
            &conversation,
        )?,
        conversation_id: conversation,
        tasks,
        workspace_path: workspace,
        workspace_files,
        workspace_bytes,
        files,
        granted_projects: names,
    })
}

fn collect(db: &Database, data: &Path, id: &str, expected: &str) -> Result<(), String> {
    on_db(db, |conn| {
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let plan = plan(&tx, data, id)?;
        let fingerprint = hash(&plan)?;
        if fingerprint != expected {
            return Err("companion.plan_changed".into());
        }
        let now = now_ms();
        tx.execute("INSERT INTO companion_deletions(companion_id,template_agent_id,display_name,plan_json,plan_hash,state,started_at)
            VALUES(?1,?2,?3,?4,?5,'deleting',?6) ON CONFLICT(companion_id) DO NOTHING",
            params![id,plan.template_agent_id,plan.display_name,crate::canonical_json::to_string(&plan)?,fingerprint,now])?;
        tx.execute(
            "UPDATE companion_profiles SET deleting_at=?2 WHERE id=?1",
            params![id, now],
        )?;
        tx.execute("UPDATE memory_entities SET source_label=?2 WHERE source_companion_id=?1 AND scope='global'", params![id,plan.display_name])?;
        for entry in plan.files {
            tx.execute("INSERT OR IGNORE INTO companion_deletion_files(companion_id,path,kind,disposition,state)
                VALUES(?1,?2,?3,?4,CASE ?4 WHEN 'retain' THEN 'kept' ELSE 'pending' END)",
                params![id,entry.path.to_string_lossy(),entry.kind,entry.disposition])?;
        }
        tx.execute(
            "UPDATE companion_deletions SET files_collected_at=?2 WHERE companion_id=?1",
            params![id, now],
        )?;
        tx.commit()?;
        Ok(())
    })
}

fn purge_rows(db: &Database, plan: &Plan) -> Result<(), String> {
    on_db(db, |conn| {
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "DELETE FROM memory_entities WHERE id IN (
                 SELECT candidate_memory_id FROM companion_shared_requests
                 WHERE companion_id=?1 AND status IN ('open','stale','rejected'))",
            [&plan.companion_id],
        )?;
        tx.execute(
            "DELETE FROM memory_entities WHERE scope='agent' AND scope_key=?1",
            [&plan.companion_id],
        )?;
        tx.execute(
            "DELETE FROM conversations WHERE id=?1 AND companion_id=?2",
            params![plan.conversation_id, plan.companion_id],
        )?;
        // K2: a contact task owns its execution conversation and references the
        // profile with ON DELETE RESTRICT. Both go before the profile; runs,
        // messages, triggers and report ledger rows cascade from them.
        for task in &plan.tasks {
            tx.execute("DELETE FROM conversations WHERE id=?1", [&task.conversation_id])?;
        }
        tx.execute(
            "DELETE FROM digital_colleagues WHERE companion_id=?1 AND companion_bound=1",
            [&plan.companion_id],
        )?;
        tx.execute(
            "DELETE FROM companion_profiles WHERE id=?1",
            [&plan.companion_id],
        )?;
        tx.execute("UPDATE companion_deletions SET db_purged_at=?2,retained_workspace_path=?3 WHERE companion_id=?1",
            params![plan.companion_id,now_ms(),plan.workspace_path.to_string_lossy()])?;
        tx.commit()?;
        Ok(())
    })
}

fn remove_file_entry(
    db: &Database,
    data: &Path,
    plan: &Plan,
    path: &Path,
    kind: &str,
) -> DeletionResult<()> {
    let data = data.canonicalize()?;
    let compute = conversation_compute_root(&data.join("runtime-sessions"), &plan.conversation_id);
    let resolved = normalized(path)?;
    let retained = resolved != path || !in_managed_area(&data, &compute, &resolved, kind);
    if !retained {
        let removal = if kind == "compute_dir" {
            fs::remove_dir_all(path)
        } else {
            fs::remove_file(path)
        };
        match removal {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    let state = if retained { "kept" } else { "removed" };
    db.with_connection(|c| {
        c.execute("UPDATE companion_deletion_files SET state=?3,error=?4 WHERE companion_id=?1 AND path=?2",
            params![plan.companion_id,path.to_string_lossy(),state,retained.then_some("路径归属已改变，保留文件")])?;
        Ok(())
    })?;
    Ok(())
}

fn finish(
    db: &Database,
    data: &Path,
    id: &str,
    mut purge_sessions: impl FnMut(&str) -> Result<(), String>,
) -> Result<Value, String> {
    on_db(db, |c| {
        let (text, signature, collected, purged, state): (String,String,bool,bool,String) = c.query_row(
            "SELECT plan_json,plan_hash,files_collected_at IS NOT NULL,db_purged_at IS NOT NULL,state FROM companion_deletions WHERE companion_id=?1",
            [id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?)))?;
        Ok((serde_json::from_str::<Plan>(&text)?,signature,collected,purged,state))
    }).and_then(|(plan, signature, collected, purged, state)| {
        if state == "done" { return result(db, &plan); }
        if !collected { collect(db,data,id,&signature)?; }
        if !purged {
            purge_sessions(&plan.conversation_id)?;
            for task in &plan.tasks {
                purge_sessions(&task.conversation_id)?;
            }
            let notes = companion_notes_dir(&data.canonicalize().map_err(|e| e.to_string())?, id);
            if normalized(&notes).map_err(|e|e.to_string())? != notes { return Err("笔记目录已指向其他位置".into()); }
            match fs::remove_dir_all(&notes) {
                Ok(()) => {}
                Err(e) if e.kind()==io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.to_string()),
            }
            purge_rows(db,&plan)?;
        }
        let pending = db.with_connection(|c| {
            let mut q = c.prepare("SELECT path,kind FROM companion_deletion_files WHERE companion_id=?1 AND disposition='remove' AND state='pending' ORDER BY path")?;
            let rows = q.query_map([id], |r| Ok((PathBuf::from(r.get::<_,String>(0)?),r.get::<_,String>(1)?)))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
        })?;
        for (path,kind) in pending { remove_file_entry(db,data,&plan,&path,&kind).map_err(|e|e.to_string())?; }
        db.with_connection(|c| {
            c.execute("UPDATE companion_deletions SET state='done',error=NULL,finished_at=?2 WHERE companion_id=?1",params![id,now_ms()])?;
            Ok(())
        })?;
        result(db,&plan)
    })
}

fn result(db: &Database, plan: &Plan) -> Result<Value, String> {
    db.with_connection(|c| {
        let (removed, kept): (i64, i64) = c.query_row(
            "SELECT count(*) FILTER(WHERE state='removed'),count(*) FILTER(WHERE state='kept')
            FROM companion_deletion_files WHERE companion_id=?1",
            [&plan.companion_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        Ok(json!({"plan":plan,"removedFiles":removed,"keptFiles":kept}))
    })
}

fn resume(state: &AppState, id: &str) -> Result<Value, String> {
    let result = finish(&state.database, &state.data_dir, id, |conversation| {
        state
            .runtime_host
            .purge_conversation_session_files(conversation)
            .map(|_| ())
            .map_err(|e| e.message)
    });
    if let Err(error) = &result {
        state.database.with_connection(|c| {
            c.execute("UPDATE companion_deletions SET error=?2 WHERE companion_id=?1 AND state='deleting'", params![id,error])?;
            Ok(())
        })?;
    }
    result
}

pub(crate) fn resume_pending(state: &AppState) {
    let pending = state.database.with_connection(|c| {
        let mut q = c.prepare("SELECT companion_id FROM companion_deletions WHERE state='deleting' ORDER BY started_at")?;
        let ids = q.query_map([], |r| r.get::<_,String>(0))?;
        ids.collect::<rusqlite::Result<Vec<_>>>()
    });
    match pending {
        Ok(ids) => {
            for id in ids {
                // K2: the restart sweep leaves a parked work-mode confirmation
                // non-terminal on purpose, so recovery has to stop the contact's
                // runs the same way an explicit delete does before purging.
                let host = &state.runtime_host;
                let outcome = host
                    .begin_companion_stop(&id)
                    .and_then(|()| host.drain_companion_runs(&id))
                    .and_then(|()| {
                        host.companion_transition()
                            .and_then(|_guard| resume(state, &id))
                    });
                host.end_companion_stop(&id);
                if let Err(error) = outcome {
                    eprintln!("联系人删除续做失败 {id}：{error}");
                }
            }
        }
        Err(error) => eprintln!("无法读取待完成的联系人删除记录：{error}"),
    }
}

fn response<T: Serialize>(value: Result<T, String>) -> ApiResponse<T> {
    match value {
        Ok(data) => ApiResponse::success(data),
        Err(e) => {
            let (code, message) = match e.as_str() {
                "companion.default_not_deletable" => (e.as_str(), "小狐不能删除"),
                "companion.run_active" => (e.as_str(), "请先停止当前生成"),
                "companion.in_use" => (e.as_str(), "数字同事或工作图仍在使用这个私聊"),
                "companion.plan_changed" => (e.as_str(), "删除清单已变化，请重新查看"),
                _ => ("companion.delete_failed", e.as_str()),
            };
            ApiResponse::failure(code, message, true)
        }
    }
}

#[tauri::command]
pub(crate) fn companion_delete_preview(
    state: State<'_, AppState>,
    companion_id: String,
) -> ApiResponse<Value> {
    response(on_db(&state.database, |c| {
        let plan = plan(c, &state.data_dir, &companion_id)?;
        Ok(json!({"planHash":hash(&plan)?,"plan":plan}))
    }))
}

#[tauri::command]
pub(crate) fn companion_delete(
    state: State<'_, AppState>,
    companion_id: String,
    plan_hash: String,
) -> ApiResponse<Value> {
    // K2: stop the contact's whole serial range before the deletion transition.
    // Cancelling a Run takes the transition lock, so the sweep runs outside it
    // and the dispatch gate stays closed until the profile is marked deleting.
    let host = &state.runtime_host;
    let result = host
        .begin_companion_stop(&companion_id)
        .and_then(|()| host.drain_companion_runs(&companion_id))
        .and_then(|()| {
            host.companion_transition().and_then(|_guard| {
                let existing: Option<String> = state.database.with_connection(|c| {
                    c.query_row(
                        "SELECT plan_hash FROM companion_deletions WHERE companion_id=?1",
                        [&companion_id],
                        |r| r.get(0),
                    )
                    .optional()
                })?;
                match existing {
                    Some(signature) if signature != plan_hash => {
                        return Err("companion.plan_changed".into())
                    }
                    Some(_) => {}
                    None => collect(&state.database, &state.data_dir, &companion_id, &plan_hash)?,
                }
                resume(&state, &companion_id)
            })
        });
    host.end_companion_stop(&companion_id);
    response(result)
}

#[tauri::command]
pub(crate) fn companion_delete_resume(
    state: State<'_, AppState>,
    companion_id: String,
) -> ApiResponse<Value> {
    let host = &state.runtime_host;
    let result = host
        .begin_companion_stop(&companion_id)
        .and_then(|()| host.drain_companion_runs(&companion_id))
        .and_then(|()| {
            host.companion_transition()
                .and_then(|_guard| resume(&state, &companion_id))
        });
    host.end_companion_stop(&companion_id);
    response(result)
}

#[tauri::command]
pub(crate) fn companion_retained_list(state: State<'_, AppState>) -> ApiResponse<Vec<Value>> {
    response(state.database.with_connection(|c| {
        let mut q = c.prepare("SELECT companion_id,display_name,retained_workspace_path FROM companion_deletions
            WHERE state='done' AND retained_workspace_path IS NOT NULL AND retained_dismissed_at IS NULL ORDER BY finished_at DESC")?;
        let rows = q.query_map([], |r| Ok(json!({"companionId":r.get::<_,String>(0)?,"displayName":r.get::<_,String>(1)?,"workspacePath":r.get::<_,String>(2)?})))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
    }))
}

#[tauri::command]
pub(crate) fn companion_retained_open(
    state: State<'_, AppState>,
    companion_id: String,
) -> ApiResponse<bool> {
    response(on_db(&state.database, |c| {
        let recorded: String = c.query_row(
            "SELECT retained_workspace_path FROM companion_deletions
            WHERE companion_id=?1 AND state='done' AND retained_dismissed_at IS NULL",
            [&companion_id],
            |r| r.get(0),
        )?;
        let data = state.data_dir.canonicalize()?;
        let path = Path::new(&recorded).canonicalize()?;
        if path != companion_workspace(&data, &companion_id) {
            return Err("保留目录已指向其他位置".into());
        }
        crate::app_capabilities::open_folder(&path)?;
        Ok(true)
    }))
}

#[tauri::command]
pub(crate) fn companion_retained_dismiss(
    state: State<'_, AppState>,
    companion_id: String,
) -> ApiResponse<bool> {
    response(state.database.with_connection(|c| {
        Ok(c.execute(
            "UPDATE companion_deletions SET retained_dismissed_at=?2
        WHERE companion_id=?1 AND state='done' AND retained_dismissed_at IS NULL",
            params![companion_id, now_ms()],
        )? != 0)
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn companion_deletion_resumes_after_collection_and_after_first_removed_file() {
        for pause_after_file in [false, true] {
            let data =
                std::env::temp_dir().join(format!("fox-contact-deletion-{}", uuid::Uuid::new_v4()));
            fs::create_dir(&data).unwrap();
            let db = Database::open(data.join("fox.db")).unwrap();
            let a = db.add_companion(&data, "fox-debugger").unwrap();
            let other = db.add_companion(&data, "fox-reviewer").unwrap();
            let id = &a.profile.id;
            let workspace = crate::companion_workspace::repair(&db, &data, id).unwrap();
            let retained = workspace.join("报价.txt");
            fs::write(&retained, b"final quote").unwrap();
            let imported = workspace.join("收到的文件").join("原始报价.txt");
            fs::write(&imported, b"original quote").unwrap();
            let notes = companion_notes_dir(&data, id);
            fs::write(notes.join("topics").join("quote.md"), b"private notes").unwrap();
            let attachment = data.join("attachments").join("managed-copy.txt");
            let preview = data.join("artifacts").join("preview.txt");
            let backup = data
                .join("managed-file-backups")
                .join("managed-before.foxbak");
            let shared = data
                .join("managed-file-backups")
                .join("managed-after.foxbak");
            for p in [&attachment, &preview, &backup, &shared] {
                fs::create_dir_all(p.parent().unwrap()).unwrap();
                fs::write(p, b"private bytes").unwrap();
            }
            let compute =
                conversation_compute_root(&data.join("runtime-sessions"), &a.conversation_id);
            fs::create_dir_all(&compute).unwrap();
            fs::write(compute.join("output.csv"), b"a,b").unwrap();
            db.with_connection(|c| {
                c.execute("INSERT INTO attachments(id,conversation_id,display_name,storage_path,created_at) VALUES('a',?1,'copy',?2,0)",
                    params![a.conversation_id,attachment.to_string_lossy()])?;
                for (name, path, origin) in [("quote",&retained,"project"),("preview",&preview,"host_private")] {
                    c.execute("INSERT INTO artifacts(id,conversation_id,display_name,artifact_type,storage_path,artifact_origin,created_at,updated_at)
                        VALUES(?1,?2,?1,'file',?3,?4,1,1)",params![name,a.conversation_id,path.to_string_lossy(),origin])?;
                }
                c.execute("INSERT INTO managed_file_versions(id,conversation_id,tool,storage_path,display_name,version_no,change_kind,backup_path,after_backup_path,created_at)
                    VALUES('v1',?1,'edit_file',?2,'quote',1,'modified',?3,?4,1)",
                    params![a.conversation_id,retained.to_string_lossy(),backup.to_string_lossy(),shared.to_string_lossy()])?;
                c.execute("INSERT INTO managed_file_versions(id,conversation_id,tool,storage_path,display_name,version_no,change_kind,backup_path,created_at)
                    VALUES('v2',?1,'edit_file','other-path','quote',1,'modified',?2,1)",params![other.conversation_id,shared.to_string_lossy()])?;
                Ok(())
            }).unwrap();
            db.with_connection(|c| {
                for (memory, status) in [("saved","confirmed"),("unresolved","candidate")] {
                    c.execute("INSERT INTO memory_entities(id,scope,kind,canonical_key,content,state,created_by,source_companion_id,created_at,updated_at)
                        VALUES(?1,'global','fact',?1,'记忆',?2,'agent',?3,1,1)",params![memory,status,id])?;
                }
                c.execute("INSERT INTO companion_shared_requests(id,companion_id,candidate_memory_id,candidate_version,created_at)
                    VALUES('req',?1,'unresolved',1,1)",[id])?;
                Ok(())
            }).unwrap();
            // K2: a bound task executes in its own conversation, so the manifest
            // lists the task together with the runs and messages removed with it.
            let colleague = db.create_digital_colleague(&crate::database::CreateDigitalColleagueInput {
                name: "报价整理".into(),
                expert_id: "fox-debugger".into(),
                objective: "整理报价".into(),
                project_id: None,
                project_root: None,
                knowledge_references: Vec::new(),
                max_runs_per_day: 2,
                max_tokens_per_day: 10_000,
                max_duration_ms: 60_000,
                max_output_tokens: 1_024,
                max_tool_calls: 8,
                companion_id: Some(id.clone()),
            }).unwrap();
            db.with_connection(|c| {
                c.execute("INSERT INTO runs(id,conversation_id,status,model,last_seq,created_at)
                    VALUES('task-run',?1,'completed','model',1,1)", [&colleague.conversation_id])?;
                c.execute("INSERT INTO messages(id,conversation_id,role,kind,content,status,ordinal,created_at,updated_at)
                    VALUES('task-message',?1,'assistant','text','整理完成','completed',1,1,1)", [&colleague.conversation_id])?;
                Ok(())
            }).unwrap();
            let before = on_db(&db, |c| plan(c, &data, id)).unwrap();
            assert_eq!(before.shared_memories, 1);
            assert_eq!(before.workspace_files, 2);
            assert_eq!(before.tasks.len(), 1);
            assert_eq!(before.tasks[0].colleague_id, colleague.id);
            assert_eq!(before.tasks[0].conversation_id, colleague.conversation_id);
            assert_eq!((before.tasks[0].runs, before.tasks[0].messages), (1, 1));
            assert!(collect(&db, &data, id, "outdated")
                .unwrap_err()
                .contains("plan_changed"));
            collect(&db, &data, id, &hash(&before).unwrap()).unwrap();
            if pause_after_file {
                fs::remove_dir_all(&notes).unwrap();
                purge_rows(&db, &before).unwrap();
                remove_file_entry(
                    &db,
                    &data,
                    &before,
                    &attachment.canonicalize().unwrap(),
                    "attachment",
                )
                .unwrap();
                assert!(!attachment.exists());
                assert!(db.companion_profile(id).is_err());
            }
            drop(db);
            let db = Database::open(data.join("fox.db")).unwrap();
            let result = finish(&db, &data, id, |_| Ok(())).unwrap();
            assert_eq!(result["removedFiles"], 4);
            assert_eq!(result["keptFiles"], 2);
            let purged: (i64, i64, i64) = db.with_connection(|c| c.query_row(
                "SELECT (SELECT count(*) FROM conversations WHERE id=?1),
                        (SELECT count(*) FROM runs WHERE id='task-run'),
                        (SELECT count(*) FROM digital_colleagues WHERE id=?2)",
                params![colleague.conversation_id, colleague.id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))).unwrap();
            assert_eq!(purged, (0, 0, 0), "删除联系人一并移除后台任务的执行会话、运行与任务本身");
            let memories: (String,i64) = db.with_connection(|c| c.query_row(
                "SELECT source_label,(SELECT count(*) FROM memory_entities WHERE id='unresolved') FROM memory_entities WHERE id='saved'",
                [],|r| Ok((r.get(0)?,r.get(1)?)))).unwrap();
            assert_eq!(memories, (a.profile.display_name.clone(), 0));
            assert_eq!(fs::read(&retained).unwrap(), b"final quote");
            assert_eq!(fs::read(&imported).unwrap(), b"original quote");
            assert!(shared.is_file());
            for path in [&attachment, &preview, &compute, &backup, &notes] {
                assert!(!path.exists(), "{}", path.display());
            }
            assert!(db.companion_profile(id).is_err());
            assert!(db.companion_profile(&other.profile.id).is_ok());
            assert_eq!(
                finish(&db, &data, id, |_| panic!("完成后不再清理会话")).unwrap(),
                result
            );
            drop(db);
            fs::remove_dir_all(&data).unwrap();
        }
    }
}
