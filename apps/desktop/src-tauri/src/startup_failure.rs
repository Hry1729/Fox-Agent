//! Readable startup failure reporting.
//!
//! An error returned from the tauri setup hook becomes
//! `panic!("Failed to setup app: …")` inside tauri's event loop. In a release
//! build (`windows_subsystem = "windows"`) that is a silent exit: no window,
//! no console. Recoverable startup problems are degraded in place (kernel run
//! locks, knowledge storage, preview cache, missing agent rows); whatever is
//! left is reported here in Chinese, with the data path, before exiting.

use std::path::Path;

#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) const DIALOG_TITLE: &str = "Fox 无法启动";

/// Message for a database that could not be opened or migrated. Names the
/// database file and the automatic-backup directory; never suggests deleting
/// anything.
pub(crate) fn database_open_message(database: &Path, backups: &Path, error: &str) -> String {
    format!(
        "无法打开 Fox 数据库：{}\n原因：{}\n启动已停止，数据库文件没有被删除。请先复制备份该文件；升级前的自动备份位于：{}。如需协助，请把本提示和启动日志发给维护人员。",
        database.display(),
        error.trim(),
        backups.display(),
    )
}

/// Final message for any setup error. Errors from `database_open_message`
/// already carry the path; others get the data directory appended.
pub(crate) fn setup_failure_message(data_dir: Option<&Path>, error: &str) -> String {
    let error = error.trim();
    match data_dir {
        Some(dir) if !error.contains(&dir.display().to_string()) => {
            format!("Fox 启动失败：{error}\n数据目录：{}", dir.display())
        }
        _ => format!("Fox 启动失败：{error}"),
    }
}

/// Log the message and, on Windows, show it in a native message box (the
/// webview is not available yet).
pub(crate) fn report(message: &str) {
    eprintln!("fox.startup error setup_failed message={}", message.replace('\n', " | "));
    #[cfg(windows)]
    {
        use windows::core::HSTRING;
        use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};
        unsafe {
            MessageBoxW(
                None,
                &HSTRING::from(message),
                &HSTRING::from(DIALOG_TITLE),
                MB_OK | MB_ICONERROR,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::Database;

    #[test]
    fn unreadable_database_reports_chinese_message_with_path() {
        let root = std::env::temp_dir().join(format!("fox-startup-corrupt-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("fox.db");
        std::fs::write(&path, vec![0x5au8; 8192]).unwrap();
        let error = match Database::open_with_migration_backup(path.clone(), &root.join("backups")) {
            Ok(_) => panic!("a garbage file must not open as a database"),
            Err(error) => error,
        };
        let message = setup_failure_message(
            Some(&root),
            &database_open_message(&path, &root.join("backups"), &error),
        );
        assert!(message.contains("无法打开 Fox 数据库"), "{message}");
        assert!(message.contains(&path.display().to_string()), "{message}");
        assert!(!message.contains("Query is not read-only"), "{message}");
        assert_eq!(std::fs::read(&path).unwrap(), vec![0x5au8; 8192], "the file is left untouched");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn setup_failure_message_names_data_dir_once() {
        let dir = Path::new("/data/fox");
        assert_eq!(setup_failure_message(Some(dir), "x"), "Fox 启动失败：x\n数据目录：/data/fox");
        assert_eq!(
            setup_failure_message(Some(dir), "无法打开 Fox 数据库：/data/fox/fox.db"),
            "Fox 启动失败：无法打开 Fox 数据库：/data/fox/fox.db"
        );
    }
}
