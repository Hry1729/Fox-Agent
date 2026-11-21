use super::{
    project_root, relative_project_path, safe_project_target, ApiResponse, AppState,
    ProjectFileEntry, ProjectFilesRequest,
};
use ignore::WalkBuilder;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashSet, VecDeque},
    fs,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};
use tauri::State;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectFilesResponse {
    entries: Vec<ProjectFileEntry>,
    truncated: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChildrenRequest {
    conversation_id: String,
    path: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchRequest {
    conversation_id: String,
    query: String,
    limit: usize,
}

#[tauri::command]
pub async fn project_files_children(
    state: State<'_, AppState>,
    request: ChildrenRequest,
) -> Result<ApiResponse<ProjectFilesResponse>, String> {
    let root = match project_root(&state, &request.conversation_id) {
        Ok(root) => root,
        Err(error) => {
            return Ok(ApiResponse::failure(
                "project.root_unavailable",
                error,
                false,
            ))
        }
    };
    Ok(scan_response(
        tauri::async_runtime::spawn_blocking(move || {
            safe_project_target(&root, &request.path)?;
            let directory = root.join(&request.path);
            Ok(ProjectFilesResponse {
                entries: children(&root, &directory),
                truncated: false,
            })
        })
        .await,
    ))
}

#[tauri::command]
pub async fn project_files_search(
    state: State<'_, AppState>,
    request: SearchRequest,
) -> Result<ApiResponse<ProjectFilesResponse>, String> {
    let root = match project_root(&state, &request.conversation_id) {
        Ok(root) => root,
        Err(error) => {
            return Ok(ApiResponse::failure(
                "project.root_unavailable",
                error,
                false,
            ))
        }
    };
    Ok(scan_response(
        tauri::async_runtime::spawn_blocking(move || {
            Ok(scan(
                &root,
                Some(&request.query.trim().to_lowercase()),
                request.limit.clamp(1, 2000),
            ))
        })
        .await,
    ))
}

#[tauri::command]
pub async fn project_files_list(
    state: State<'_, AppState>,
    request: ProjectFilesRequest,
) -> Result<ApiResponse<ProjectFilesResponse>, String> {
    let root = match project_root(&state, &request.conversation_id) {
        Ok(root) => root,
        Err(error) => {
            return Ok(ApiResponse::failure(
                "project.root_unavailable",
                error,
                false,
            ))
        }
    };
    Ok(scan_response(
        tauri::async_runtime::spawn_blocking(move || Ok(scan(&root, None, 2000))).await,
    ))
}

fn scan_response(
    result: Result<Result<ProjectFilesResponse, String>, tauri::Error>,
) -> ApiResponse<ProjectFilesResponse> {
    match result {
        Ok(Ok(response)) => ApiResponse::success(response),
        Ok(Err(error)) => ApiResponse::failure("project.files_failed", error, true),
        Err(error) => ApiResponse::failure("project.files_failed", error.to_string(), true),
    }
}

fn excluded(path: &Path) -> bool {
    let name = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_ascii_lowercase();
    matches!(
        name.as_str(),
        ".git"
            | "node_modules"
            | ".pnpm-store"
            | ".uv-cache"
            | ".venv"
            | "__pycache__"
            | ".next"
            | ".nuxt"
            | ".turbo"
    ) || (name == "target"
        && path
            .parent()
            .is_some_and(|parent| parent.join("Cargo.toml").is_file()))
}

fn children(root: &Path, directory: &Path) -> Vec<ProjectFileEntry> {
    let mut builder = WalkBuilder::new(directory);
    builder
        .max_depth(Some(1))
        .hidden(false)
        .require_git(false)
        .git_global(false)
        .filter_entry(|entry| entry.depth() == 0 || !excluded(entry.path()));
    let mut entries: Vec<_> = builder
        .build()
        .filter_map(Result::ok)
        .filter(|entry| entry.depth() == 1)
        .filter_map(|entry| {
            let path = entry.path();
            let metadata = fs::metadata(path).ok()?;
            let is_directory = metadata.is_dir();
            Some(ProjectFileEntry {
                path: relative_project_path(root, path),
                name: entry.file_name().to_string_lossy().into_owned(),
                parent: relative_project_path(root, directory),
                is_directory,
                // Links remain visible; expansion retains the existing project boundary.
                has_children: is_directory
                    && path.canonicalize().is_ok_and(|p| p.starts_with(root))
                    && fs::read_dir(path).is_ok_and(|mut items| items.any(|item| item.is_ok())),
                byte_size: metadata.len().min(i64::MAX as u64) as i64,
                modified_at: metadata
                    .modified()
                    .ok()
                    .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                    .map(|time| time.as_millis() as i64),
            })
        })
        .collect();
    entries.sort_by_key(|entry| (!entry.is_directory, entry.name.to_lowercase()));
    entries
}

fn scan(root: &Path, query: Option<&str>, limit: usize) -> ProjectFilesResponse {
    let mut response = ProjectFilesResponse {
        entries: Vec::new(),
        truncated: false,
    };
    let mut queue = VecDeque::from([root.to_path_buf()]);
    let mut visited = HashSet::<PathBuf>::new();
    while let Some(directory) = queue.pop_front() {
        let Ok(canonical) = directory.canonicalize() else {
            continue;
        };
        if !canonical.starts_with(root) || !visited.insert(canonical) {
            continue;
        }
        for entry in children(root, &directory) {
            if entry.is_directory && entry.has_children {
                queue.push_back(root.join(&entry.path));
            }
            if query.is_some_and(|query| {
                entry.is_directory || !entry.path.to_lowercase().contains(query)
            }) {
                continue;
            }
            // Keep the whole top layer in the compatibility list, even above its soft limit.
            if response.entries.len() >= limit && (query.is_some() || directory != root) {
                response.truncated = true;
                return response;
            }
            response.entries.push(entry);
        }
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Project(PathBuf);
    impl Project {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("fox-file-tree-{}", uuid::Uuid::new_v4()));
            fs::create_dir_all(&path).unwrap();
            Self(path.canonicalize().unwrap())
        }
        fn file(&self, path: &str) {
            let path = self.0.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, "test").unwrap();
        }
    }
    impl Drop for Project {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn top_level_survives_three_thousand_dot_files_and_list_is_breadth_first() {
        let project = Project::new();
        for i in 0..3000 {
            project.file(&format!(".cache-{i}"));
        }
        project.file("Fox/src/main.ts");
        project.file("docs/readme.md");
        project.file("readme.md");
        let layer = children(&project.0, &project.0);
        assert_eq!(layer.len(), 3003);
        assert_eq!(layer[0].name, "docs");
        assert!(layer
            .iter()
            .any(|entry| entry.path == "Fox" && entry.has_children));
        assert!(!layer.iter().any(|entry| entry.path == "Fox/src"));
        let list = scan(&project.0, None, 2000);
        assert!(list.truncated);
        assert!(list.entries.iter().any(|entry| entry.path == "readme.md"));
        assert!(list.entries.iter().any(|entry| entry.path == "docs"));
    }

    #[test]
    fn ignore_rules_apply_to_children_and_deep_search_without_hiding_source_directories() {
        let project = Project::new();
        fs::write(project.0.join(".gitignore"), "ignored/\n").unwrap();
        fs::write(project.0.join(".ignore"), "private/\n").unwrap();
        for name in [
            "ignored",
            "private",
            "node_modules",
            ".pnpm-store",
            ".uv-cache",
            "build",
            "dist",
            "target",
        ] {
            project.file(&format!("{name}/match.txt"));
        }
        project.file("rust/Cargo.toml");
        project.file("rust/target/match.txt");
        project.file("deep/a/b/c/d/e/f/g/h/i/match.txt");
        fs::write(project.0.join("deep/.gitignore"), "excluded/\n").unwrap();
        project.file("deep/excluded/match.txt");
        let found = scan(&project.0, Some("match"), 100);
        assert!(!found.truncated);
        assert_eq!(found.entries.len(), 4);
        assert!(found
            .entries
            .iter()
            .any(|entry| entry.path.starts_with("deep/a/")));
        assert!(found
            .entries
            .iter()
            .any(|entry| entry.path == "build/match.txt"));
        assert!(found
            .entries
            .iter()
            .any(|entry| entry.path == "dist/match.txt"));
        assert!(found
            .entries
            .iter()
            .any(|entry| entry.path == "target/match.txt"));
        assert!(scan(&project.0, Some("match"), 1).truncated);
    }

    #[test]
    fn unreadable_or_missing_child_does_not_fail_the_scan() {
        let project = Project::new();
        project.file("readable/file.txt");
        assert!(children(&project.0, &project.0.join("missing")).is_empty());
        assert_eq!(scan(&project.0, Some("file.txt"), 100).entries.len(), 1);
    }

    #[cfg(windows)]
    #[test]
    fn permission_denied_directory_does_not_hide_other_children() {
        use std::process::Command;
        let project = Project::new();
        project.file("denied/secret.txt");
        project.file("visible.txt");
        let denied = project.0.join("denied");
        let account = "*S-1-1-0";
        let set = Command::new("icacls")
            .arg(&denied)
            .args(["/deny", &format!("{account}:(OI)(CI)(RD)"), "/Q"])
            .output()
            .unwrap();
        assert!(
            set.status.success(),
            "{}",
            String::from_utf8_lossy(&set.stderr)
        );
        let inaccessible = fs::read_dir(&denied).is_err();
        let response = scan(&project.0, None, 100);
        let restore = Command::new("icacls")
            .arg(&denied)
            .args(["/remove:d", &account, "/Q"])
            .output()
            .unwrap();
        assert!(restore.status.success());
        assert!(
            inaccessible,
            "The fixture must actually deny directory listing"
        );
        assert!(response
            .entries
            .iter()
            .any(|entry| entry.path == "visible.txt"));
    }

    #[cfg(unix)]
    #[test]
    fn permission_denied_directory_does_not_hide_other_children() {
        use std::os::unix::fs::PermissionsExt;
        let project = Project::new();
        project.file("denied/secret.txt");
        project.file("visible.txt");
        fs::set_permissions(project.0.join("denied"), fs::Permissions::from_mode(0)).unwrap();
        let response = scan(&project.0, None, 100);
        fs::set_permissions(project.0.join("denied"), fs::Permissions::from_mode(0o700)).unwrap();
        assert!(response
            .entries
            .iter()
            .any(|entry| entry.path == "visible.txt"));
    }
}
