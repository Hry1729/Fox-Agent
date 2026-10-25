//! Data-directory paths that must be directories, and error text that names
//! them. A broken data directory (e.g. `runtime-sessions/kernel-run-locks`
//! replaced by a 1-byte file) used to surface only as
//! `run.create_failed: unable to open database file`; these helpers put the
//! concrete path (and "应为目录但是文件") into the message instead.
use std::path::{Path, PathBuf};

/// Directories under the data root that every Fox run relies on.
pub(crate) fn run_directories(data_dir: &Path) -> Vec<PathBuf> {
    vec![
        data_dir.join("runtime-sessions"),
        data_dir.join("runtime-sessions").join("kernel-run-locks"),
        data_dir.join("attachments"),
        data_dir.join("cache"),
        data_dir.join("cache").join("knowledge-preview"),
    ]
}

/// 「<路径> 应为目录但是文件」 when `path` exists and is not a directory.
pub(crate) fn not_a_directory(path: &Path) -> Option<String> {
    let metadata = std::fs::metadata(path).ok()?;
    (!metadata.is_dir()).then(|| format!("{} 应为目录但是文件", path.display()))
}

/// `create_dir_all` with the path in the error, and the explicit
/// "should be a directory but is a file" hint when that is the cause.
pub(crate) fn ensure_directory(path: &Path, what: &str) -> Result<(), String> {
    if let Some(problem) = not_a_directory(path) {
        return Err(format!("{what}：{problem}"));
    }
    std::fs::create_dir_all(path).map_err(|error| match first_file_ancestor(path) {
        Some(problem) => format!("{what}：{problem}"),
        None => format!("{what} {}：{error}", path.display()),
    })
}

/// The first existing ancestor of `path` that is a file, as a hint.
fn first_file_ancestor(path: &Path) -> Option<String> {
    path.ancestors().skip(1).find_map(not_a_directory)
}

/// Text for a failed run creation. Names the main database and every run
/// directory that is a file instead of a directory, then the raw error.
pub(crate) fn run_create_failure(data_dir: &Path, error: &str) -> String {
    let problems: Vec<String> = run_directories(data_dir)
        .iter()
        .filter_map(|path| not_a_directory(path))
        .collect();
    let database = data_dir.join("fox.db");
    if problems.is_empty() {
        format!("无法写入数据库 {}：{error}", database.display())
    } else {
        format!(
            "{}（数据库 {}：{error}）",
            problems.join("；"),
            database.display()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("fox-data-paths-{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn data_paths_name_a_lock_directory_that_is_a_file() {
        let root = temp_root("lock-file");
        let sessions = root.join("runtime-sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        let locks = sessions.join("kernel-run-locks");
        std::fs::write(&locks, b"x").unwrap();
        let error = ensure_directory(&locks, "Kernel 锁目录").unwrap_err();
        assert_eq!(error, format!("Kernel 锁目录：{} 应为目录但是文件", locks.display()));
        // A file higher up (cache replaced by a file) is named too.
        std::fs::write(root.join("cache"), b"x").unwrap();
        let preview = root.join("cache").join("knowledge-preview");
        let error = ensure_directory(&preview, "预览缓存目录").unwrap_err();
        assert!(error.contains(&format!("{} 应为目录但是文件", root.join("cache").display())), "{error}");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn data_paths_run_create_failure_lists_paths_and_database() {
        let root = temp_root("run-create");
        std::fs::create_dir_all(root.join("runtime-sessions")).unwrap();
        std::fs::write(root.join("runtime-sessions").join("kernel-run-locks"), b"x").unwrap();
        std::fs::create_dir_all(root.join("cache")).unwrap();
        std::fs::write(root.join("cache").join("knowledge-preview"), b"x").unwrap();
        let text = run_create_failure(&root, "unable to open database file");
        assert!(text.contains(&format!("{} 应为目录但是文件", root.join("runtime-sessions").join("kernel-run-locks").display())), "{text}");
        assert!(text.contains(&format!("{} 应为目录但是文件", root.join("cache").join("knowledge-preview").display())), "{text}");
        assert!(text.contains(&root.join("fox.db").display().to_string()), "{text}");
        assert!(text.contains("unable to open database file"));
        let healthy = temp_root("run-create-ok");
        assert_eq!(
            run_create_failure(&healthy, "disk I/O error"),
            format!("无法写入数据库 {}：disk I/O error", healthy.join("fox.db").display())
        );
        assert!(ensure_directory(&healthy.join("a").join("b"), "目录").is_ok());
        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_dir_all(healthy);
    }
}
