use crate::app_state::AppState;
use crate::data_paths::{companion_notes_dir, companion_workspace};
use crate::database::{ApiResponse, Database};
use std::{
    ffi::OsString,
    fs::{self, File},
    io::{self, ErrorKind, Read},
    path::{Component, Path, PathBuf},
};
use tauri::State;

fn profile_workspace(
    database: &Database,
    data_dir: &Path,
    id: &str,
) -> Result<(PathBuf, String), String> {
    if !id.starts_with("cmp_") || Path::new(id).file_name().and_then(|s| s.to_str()) != Some(id) {
        return Err("联系人 ID 无效".into());
    }
    let (stored, state): (String, String) = database.with_connection(|db| db.query_row(
        "SELECT workspace_dir,workspace_state FROM companion_profiles WHERE id=?1 AND deleting_at IS NULL",
        [id], |row| Ok((row.get(0)?, row.get(1)?)),
    ))?;
    let expected = companion_workspace(data_dir, id);
    if Path::new(&stored) != expected {
        return Err("联系人的工作目录记录与身份不一致".into());
    }
    Ok((expected, state))
}

// 顺序检查每级目录。先跟随联接再建子目录，会在其他联系人的目录里创建文件。
fn checked_dir(data_dir: &Path, path: &Path, create: bool) -> Result<PathBuf, String> {
    let mut current = data_dir.canonicalize().map_err(|e| e.to_string())?;
    let suffix = path.strip_prefix(data_dir).map_err(|e| e.to_string())?;
    for part in suffix.components() {
        let Component::Normal(name) = part else {
            return Err("无效的联系人目录".into());
        };
        current.push(name);
        if create {
            match fs::create_dir(&current) {
                Ok(()) => {}
                Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
                Err(e) => return Err(format!("无法创建目录 {}：{e}", current.display())),
            }
        }
        if !current.is_dir() || current.canonicalize().map_err(|e| e.to_string())? != current {
            return Err(format!("目录不可用或指向其他位置：{}", current.display()));
        }
    }
    Ok(current)
}

fn record_state(
    database: &Database,
    id: &str,
    state: &str,
    error: Option<&str>,
) -> Result<(), String> {
    database.with_connection(|db| {
        db.execute("UPDATE companion_profiles SET workspace_state=?2, workspace_error=?3 WHERE id=?1 AND deleting_at IS NULL",
            rusqlite::params![id, state, error])?;
        Ok(())
    })
}

pub(crate) fn repair(database: &Database, data_dir: &Path, id: &str) -> Result<PathBuf, String> {
    let (path, _) = profile_workspace(database, data_dir, id)?;
    let result = (|| {
        checked_dir(
            data_dir,
            &companion_notes_dir(data_dir, id).join("topics"),
            true,
        )?;
        checked_dir(data_dir, &path.join("收到的文件"), true)?;
        checked_dir(data_dir, &path, false)
    })();
    match &result {
        Ok(_) => record_state(database, id, "ready", None)?,
        Err(error) => record_state(database, id, "failed", Some(error))?,
    }
    result
}

fn ready_root(database: &Database, data_dir: &Path, id: &str) -> Result<PathBuf, String> {
    let (path, state) = profile_workspace(database, data_dir, id)?;
    if state != "ready" {
        return Err("companion.workspace_unavailable：请在联系人资料页重新创建工作目录。".into());
    }
    match checked_dir(data_dir, &path, false) {
        Ok(root) => Ok(root),
        Err(error) => {
            record_state(database, id, "missing", Some(&error))?;
            Err(format!(
                "companion.workspace_unavailable：{error}。请重新创建工作目录。"
            ))
        }
    }
}

pub(crate) fn require_for_chat(
    database: &Database,
    data_dir: &Path,
    conversation: &str,
) -> Result<(), String> {
    if let Some(id) = database.conversation_companion_id(conversation)? {
        ready_root(database, data_dir, &id)?;
    }
    Ok(())
}

pub(crate) fn allows_tool(tool: &str) -> bool {
    matches!(
        tool,
        "read"
            | "ls"
            | "find"
            | "grep"
            | "present_files"
            | "write_file"
            | "edit_file"
            | "memory_search"
            | "memory_write"
            | "note_read"
            | "note_search"
            | "note_organize"
    )
}

pub(crate) fn check_run_tool(
    database: &Database,
    data_dir: &Path,
    run: &str,
    tool: &str,
    input: Option<&serde_json::Value>,
    suggest_import: Option<&dyn Fn(&str, &Path)>,
) -> Result<(), (&'static str, String)> {
    let profile: Option<(String, String)> = database.with_connection(|db| {
        use rusqlite::OptionalExtension;
        db.query_row("SELECT c.companion_id,c.project_root FROM runs r JOIN conversations c ON c.id=r.conversation_id
            WHERE r.id=?1 AND c.companion_id IS NOT NULL", [run], |row| Ok((row.get(0)?, row.get(1)?))).optional()
    }).map_err(|e| ("companion.workspace_unavailable", e))?;
    let Some((id, project_root)) = profile else {
        return Ok(());
    };
    if !allows_tool(tool) {
        return Err((
            "companion.tool_not_available",
            format!("私聊不可使用工具 {tool}"),
        ));
    }
    let root =
        ready_root(database, data_dir, &id).map_err(|e| ("companion.workspace_unavailable", e))?;
    if Path::new(&project_root) != companion_workspace(data_dir, &id) {
        return Err((
            "companion.workspace_unavailable",
            "私聊的项目目录与联系人不符".into(),
        ));
    }
    if let Some(input) = input.filter(|_| {
        matches!(
            tool,
            "read" | "grep" | "find" | "ls" | "write_file" | "edit_file" | "present_files"
        )
    }) {
        let direct = input
            .get("path")
            .and_then(|v| v.as_str())
            .filter(|_| tool != "present_files");
        let files = input["files"]
            .as_array()
            .filter(|_| tool == "present_files")
            .into_iter()
            .flatten()
            .filter_map(|file| file["path"].as_str());
        for path in direct.into_iter().chain(files) {
            if crate::tool_host::reject_network_or_device_path(path).is_err() {
                continue;
            }
            if let Ok(target) = root.join(path).canonicalize() {
                if !target.starts_with(&root) && target.is_file() {
                    if let Some(emit) = suggest_import {
                        emit(&id, &target);
                    }
                    return Err((
                        "companion.path_blocked",
                        "私聊只可访问本联系人工作目录中的文件".into(),
                    ));
                }
            }
        }
    }
    if let Some(input) = input.filter(|_| matches!(tool, "write_file" | "edit_file")) {
        let path = input["path"]
            .as_str()
            .ok_or(("companion.path_blocked", "缺少文件路径".into()))?;
        crate::runtime_host::approval_policy::check_hard_root(&root, path)
            .map_err(|e| ("companion.path_blocked", e))?;
    }
    Ok(())
}

fn import_one(inbox: &Path, source: &Path) -> Result<PathBuf, String> {
    let name = source.file_name().ok_or("缺少文件名")?;
    let reader =
        File::open(source).map_err(|e| format!("无法打开 {}：{e}", source.display()))?;
    if !reader.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("请选择普通文件".into());
    }
    copy_to_inbox(inbox, Path::new(name), reader)
}

pub(crate) fn copy_to_inbox(inbox: &Path, filename: &Path, mut reader: impl Read) -> Result<PathBuf, String> {
    let name = filename.as_os_str();
    for index in 0u64.. {
        let mut candidate = OsString::from(name);
        if index > 0 {
            candidate = filename.file_stem().unwrap_or(name).to_os_string();
            candidate.push(format!(" ({index})"));
            if let Some(extension) = filename.extension() {
                candidate.push(".");
                candidate.push(extension);
            }
        }
        let target = inbox.join(candidate);
        let mut writer = match File::create_new(&target) {
            Ok(file) => file,
            Err(e) if e.kind() == ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.to_string()),
        };
        if let Err(e) = io::copy(&mut reader, &mut writer) {
            drop(writer);
            let _ = fs::remove_file(&target);
            return Err(format!("复制失败：{e}"));
        }
        return Ok(target);
    }
    unreachable!()
}

pub(crate) fn inbox(database: &Database, data_dir: &Path, id: &str) -> Result<PathBuf, String> {
    ready_root(database, data_dir, id)?;
    checked_dir(data_dir, &companion_workspace(data_dir, id).join("收到的文件"), true)
}

fn import_files(
    database: &Database,
    data_dir: &Path,
    id: &str,
    sources: &[PathBuf],
) -> Result<Vec<PathBuf>, String> {
    let inbox = inbox(database, data_dir, id)?;
    let mut imported = Vec::new();
    for source in sources {
        match import_one(&inbox, source) {
            Ok(path) => imported.push(path),
            Err(e) => {
                for path in imported {
                    fs::remove_file(path)
                        .map_err(|cleanup| format!("{e}；清理已复制文件失败：{cleanup}"))?;
                }
                return Err(e);
            }
        }
    }
    Ok(imported)
}

fn export_source(root: &Path, path: &str) -> Result<PathBuf, String> {
    crate::tool_host::resolve_present_target(root, path).map_err(|error| match error {
        crate::tool_host::PresentTargetError::NotAuthorized(message)
        | crate::tool_host::PresentTargetError::NotFound(message)
        | crate::tool_host::PresentTargetError::NotAFile(message) => message,
    })
}

fn export_copy(source: &Path, destination: &Path) -> Result<(), String> {
    if destination.canonicalize().ok().as_deref() == Some(source) {
        return Err("请选择另一个文件路径".into());
    }
    let staging = destination.with_file_name(format!(".fox-export-{}.tmp", uuid::Uuid::new_v4()));
    let mut reader = File::open(source).map_err(|e| e.to_string())?;
    let mut writer = File::create_new(&staging).map_err(|e| e.to_string())?;
    let copied = io::copy(&mut reader, &mut writer).and_then(|_| writer.sync_all());
    drop(writer);
    let result = copied
        .map_err(|e| e.to_string())
        .and_then(|_| crate::commands::replace_download_file(&staging, destination));
    if result.is_err() {
        let _ = fs::remove_file(staging);
    }
    result
}

#[tauri::command]
pub(crate) fn companion_workspace_repair(
    state: State<'_, AppState>,
    companion_id: String,
) -> ApiResponse<PathBuf> {
    match repair(&state.database, &state.data_dir, &companion_id) {
        Ok(path) => ApiResponse::success(path),
        Err(e) => ApiResponse::failure("companion.workspace_unavailable", e, true),
    }
}

#[tauri::command]
pub(crate) async fn companion_import_files(
    state: State<'_, AppState>,
    companion_id: String,
    paths: Option<Vec<PathBuf>>,
) -> Result<ApiResponse<Vec<PathBuf>>, String> {
    let database = state.database.clone();
    let data_dir = state.data_dir.clone();
    let result = tokio::task::spawn_blocking(move || {
        let paths = match paths {
            Some(paths) => paths,
            #[cfg(windows)]
            None => crate::local_knowledge_picker::pick_file_paths_windows()?,
            #[cfg(not(windows))]
            None => return Err("此平台尚无系统文件选择框".into()),
        };
        import_files(&database, &data_dir, &companion_id, &paths)
    })
    .await
    .map_err(|e| e.to_string())?;
    Ok(match result {
        Ok(paths) => ApiResponse::success(paths),
        Err(e) => ApiResponse::failure("companion.import_failed", e, true),
    })
}

#[tauri::command]
pub(crate) async fn companion_export_file(
    state: State<'_, AppState>,
    companion_id: String,
    path: String,
) -> Result<ApiResponse<Option<PathBuf>>, String> {
    let database = state.database.clone();
    let data_dir = state.data_dir.clone();
    let result: Result<Option<PathBuf>, String> = tokio::task::spawn_blocking(move || {
        let root = ready_root(&database, &data_dir, &companion_id)?;
        let source = export_source(&root, &path)?;
        #[cfg(windows)]
        let destination = crate::commands::pick_save_file_windows(
            &source.file_name().ok_or("缺少文件名")?.to_string_lossy(),
        )?;
        #[cfg(not(windows))]
        let destination: Option<PathBuf> = return Err("此平台尚无系统保存对话框".into());
        if let Some(destination) = &destination {
            export_copy(&export_source(&root, &path)?, destination)?;
        }
        Ok(destination)
    })
    .await
    .map_err(|e| e.to_string())?;
    Ok(match result {
        Ok(path) => ApiResponse::success(path),
        Err(e) => ApiResponse::failure("companion.export_failed", e, true),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn companion_workspace_copies_repairs_and_blocks_other_contacts_before_approval() {
        let data =
            std::env::temp_dir().join(format!("fox-contact-boundary-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&data).unwrap();
        let db = Database::open(data.join("fox.db")).unwrap();
        let a = db.add_companion(&data, "fox-debugger").unwrap();
        let b = db.add_companion(&data, "fox-reviewer").unwrap();
        let a_id = &a.profile.id;
        let a_root = repair(&db, &data, a_id).unwrap();
        let b_root = repair(&db, &data, &b.profile.id).unwrap();
        let source = data.join("报价.txt");
        fs::write(&source, b"original").unwrap();
        let a_file = import_files(&db, &data, a_id, std::slice::from_ref(&source))
            .unwrap()
            .remove(0);
        let b_file = import_files(&db, &data, &b.profile.id, std::slice::from_ref(&source))
            .unwrap()
            .remove(0);
        let duplicate = import_files(&db, &data, a_id, std::slice::from_ref(&source))
            .unwrap()
            .remove(0);
        assert_ne!(a_file, duplicate);
        fs::write(&a_file, b"revised").unwrap();
        assert_eq!(fs::read(&source).unwrap(), b"original");
        assert_eq!(fs::read(&b_file).unwrap(), b"original");
        let exported = data.join("导出.txt");
        export_copy(
            &export_source(&a_root, a_file.to_str().unwrap()).unwrap(),
            &exported,
        )
        .unwrap();
        assert_eq!(fs::read(&exported).unwrap(), b"revised");
        assert!(export_copy(&a_file, &a_file).is_err());
        assert!(export_source(&a_root, b_file.to_str().unwrap()).is_err());

        let run = db
            .create_run(&a.conversation_id, "检查文件", None)
            .unwrap()
            .run
            .id;
        let suggested = std::cell::RefCell::new(None);
        let on_import = |id: &str, path: &Path| {
            *suggested.borrow_mut() = Some((id.to_owned(), path.to_owned()));
        };
        let refusal = check_run_tool(
            &db,
            &data,
            &run,
            "read",
            Some(&json!({"path":b_file})),
            Some(&on_import),
        )
        .unwrap_err();
        assert_eq!(refusal.0, "companion.path_blocked");
        assert_eq!(
            suggested.into_inner(),
            Some((a_id.to_owned(), b_file.clone()))
        );
        for mode in ["read_only", "ask", "allow"] {
            db.with_connection(|c| {
                c.execute(
                    "UPDATE conversations SET permission_mode=?2 WHERE id=?1",
                    rusqlite::params![a.conversation_id, mode],
                )
            })
            .unwrap();
            for tool in ["write_file", "edit_file"] {
                for path in [
                    b_file.to_str().unwrap(),
                    "../notes/private.md",
                    ".fox/config",
                    ".git",
                    ".git/config",
                ] {
                    assert_eq!(
                        check_run_tool(&db, &data, &run, tool, Some(&json!({"path":path})), None)
                            .unwrap_err()
                            .0,
                        "companion.path_blocked",
                        "{mode} {tool} {path}"
                    );
                }
                check_run_tool(
                    &db,
                    &data,
                    &run,
                    tool,
                    Some(&json!({"path":"报告.txt"})),
                    None,
                )
                .unwrap();
            }
        }
        let repo = data.join("outside-repo");
        fs::create_dir(&repo).unwrap();
        assert!(std::process::Command::new("git")
            .args(["init", "--quiet"])
            .arg(&repo)
            .status()
            .unwrap()
            .success());
        let git_pointer = format!("gitdir: {}\n", repo.join(".git").display());
        fs::write(a_root.join(".git"), &git_pointer).unwrap();
        let external_pointer = data.join(".git");
        fs::write(&external_pointer, git_pointer).unwrap();
        assert_eq!(
            import_files(&db, &data, a_id, &[external_pointer])
                .unwrap()
                .len(),
            1
        );
        for tool in [
            "git_read",
            "sqlite_read",
            "run_command",
            "call_mcp_tool",
            "http_request",
            "attachment_compute",
        ] {
            assert_eq!(
                check_run_tool(&db, &data, &run, tool, Some(&json!({"path":b_root})), None)
                    .unwrap_err()
                    .0,
                "companion.tool_not_available"
            );
        }
        let ordinary = db
            .create_conversation("fox-general", Some("普通对话"), None, None)
            .unwrap();
        let ordinary_run = db
            .create_run(&ordinary.id, "查看仓库", None)
            .unwrap()
            .run
            .id;
        check_run_tool(
            &db,
            &data,
            &ordinary_run,
            "git_read",
            Some(&json!({})),
            None,
        )
        .unwrap();
        fs::remove_dir_all(&a_root).unwrap();
        assert!(require_for_chat(&db, &data, &a.conversation_id).is_err());
        assert_eq!(
            db.companion_profile(a_id).unwrap().profile.workspace_state,
            "missing"
        );
        assert!(!a_root.exists());
        assert_eq!(repair(&db, &data, a_id).unwrap(), a_root);
        require_for_chat(&db, &data, &a.conversation_id).unwrap();
        drop(db);
        fs::remove_dir_all(&data).unwrap();
    }
}
