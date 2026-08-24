use crate::database::{ApiResponse, Database};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    ffi::{OsStr, OsString},
    fs,
    path::{Component, Path, PathBuf, Prefix},
};

pub const LOCAL_KNOWLEDGE_STORAGE_PATH_KEY: &str = "local_knowledge_storage_path";

#[derive(Debug, Clone)]
pub(crate) struct StorageMigrationError {
    pub(crate) code: &'static str,
    pub(crate) message: String,
    pub(crate) retryable: bool,
}

impl StorageMigrationError {
    pub(crate) fn new(code: &'static str, message: impl Into<String>, retryable: bool) -> Self {
        Self {
            code,
            message: message.into(),
            retryable,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StorageMigration {
    pub id: String,
    pub source_path: String,
    pub destination_path: String,
    pub status: String,
    pub stage: String,
    pub progress: i64,
    pub last_sequence: i64,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub restart_required: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageMigrationStartRequest {
    pub destination_path: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageMigrationIdRequest {
    pub id: String,
}

pub(crate) fn default_storage_root(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join("local-knowledge")
}

pub(crate) fn resolve_startup_root(app_data_dir: &Path, configured_path: Option<&str>) -> PathBuf {
    let default_root = default_storage_root(app_data_dir);
    let Some(configured_path) = configured_path.map(str::trim) else {
        return default_root;
    };
    if configured_path.is_empty() {
        return default_root;
    }
    let configured = PathBuf::from(configured_path);
    if normalize_storage_path(&configured, None).is_ok() {
        configured
    } else {
        default_root
    }
}

pub(crate) fn normalize_storage_path(
    path: &Path,
    source: Option<&Path>,
) -> Result<PathBuf, StorageMigrationError> {
    if path.as_os_str().is_empty() || !path.is_absolute() {
        return Err(StorageMigrationError::new(
            "local_knowledge.storage_migration_invalid_destination",
            "存储目录必须是绝对路径",
            false,
        ));
    }
    validate_path_components(path)?;
    if path.file_name().is_none() {
        return Err(StorageMigrationError::new(
            "local_knowledge.storage_migration_dangerous_path",
            "不能使用磁盘根目录作为知识库存储目录",
            false,
        ));
    }

    let normalized = strip_windows_verbatim_prefix(canonicalize_existing_prefix(path)?);
    if let Some(source) = source {
        let normalized_source =
            strip_windows_verbatim_prefix(canonicalize_existing_prefix(source)?);
        if same_path(&normalized_source, &normalized)
            || path_starts_with(&normalized, &normalized_source)
            || path_starts_with(&normalized_source, &normalized)
        {
            return Err(StorageMigrationError::new(
                "local_knowledge.storage_migration_path_conflict",
                "新旧知识库目录不能相同，也不能互为子目录",
                false,
            ));
        }
    }
    Ok(normalized)
}

pub(crate) fn validate_current_storage_root(path: &Path) -> Result<PathBuf, StorageMigrationError> {
    let normalized = normalize_storage_path(path, None)?;
    let metadata = fs::symlink_metadata(&normalized).map_err(|error| {
        StorageMigrationError::new(
            "local_knowledge.storage_migration_source_unavailable",
            format!("当前知识库存储目录不可用：{error}"),
            true,
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(StorageMigrationError::new(
            "local_knowledge.storage_migration_source_unavailable",
            "当前知识库存储路径不是普通目录",
            false,
        ));
    }
    Ok(normalized)
}

pub(crate) fn validate_destination_storage_root(
    source: &Path,
    destination: &Path,
) -> Result<PathBuf, StorageMigrationError> {
    let source = validate_current_storage_root(source)?;
    let destination = normalize_storage_path(destination, Some(&source))?;
    if let Ok(metadata) = fs::symlink_metadata(&destination) {
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(StorageMigrationError::new(
                "local_knowledge.storage_migration_destination_invalid",
                "目标存储路径必须是普通目录",
                false,
            ));
        }
        if fs::read_dir(&destination)
            .map_err(|error| {
                StorageMigrationError::new(
                    "local_knowledge.storage_migration_destination_unreadable",
                    format!("无法检查目标存储目录：{error}"),
                    true,
                )
            })?
            .next()
            .is_some()
        {
            return Err(StorageMigrationError::new(
                "local_knowledge.storage_migration_destination_exists",
                "目标存储目录必须不存在或为空目录",
                false,
            ));
        }
    }
    Ok(destination)
}

pub(crate) fn create_staging_path(
    destination: &Path,
    migration_id: &str,
) -> Result<PathBuf, StorageMigrationError> {
    let parent = destination.parent().ok_or_else(|| {
        StorageMigrationError::new(
            "local_knowledge.storage_migration_invalid_destination",
            "目标存储目录缺少有效父目录",
            false,
        )
    })?;
    fs::create_dir_all(parent).map_err(|error| {
        StorageMigrationError::new(
            "local_knowledge.storage_migration_staging_failed",
            format!("无法创建迁移临时目录的父目录：{error}"),
            true,
        )
    })?;
    let name = destination
        .file_name()
        .and_then(OsStr::to_str)
        .ok_or_else(|| {
            StorageMigrationError::new(
                "local_knowledge.storage_migration_invalid_destination",
                "目标存储目录名称不可用",
                false,
            )
        })?;
    Ok(parent.join(format!(".{name}.fox-staging-{migration_id}")))
}

pub(crate) fn copy_managed_directory(
    source: &Path,
    staging: &Path,
) -> Result<(), StorageMigrationError> {
    if staging.exists() {
        return Err(StorageMigrationError::new(
            "local_knowledge.storage_migration_staging_exists",
            "迁移临时目录已经存在，拒绝覆盖",
            false,
        ));
    }
    fs::create_dir_all(staging).map_err(|error| {
        StorageMigrationError::new(
            "local_knowledge.storage_migration_copy_failed",
            format!("无法创建迁移临时目录：{error}"),
            true,
        )
    })?;
    copy_directory_entries(source, staging).map_err(|error| {
        StorageMigrationError::new("local_knowledge.storage_migration_copy_failed", error, true)
    })
}

pub(crate) fn verify_managed_directory(
    source: &Path,
    staging: &Path,
) -> Result<(), StorageMigrationError> {
    for directory in ["models", "indexes", "knowledge-bases", "staging"] {
        let path = staging.join(directory);
        let metadata = fs::symlink_metadata(&path).map_err(|error| {
            StorageMigrationError::new(
                "local_knowledge.storage_migration_verify_failed",
                format!("迁移结果缺少关键目录 {directory}：{error}"),
                true,
            )
        })?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(StorageMigrationError::new(
                "local_knowledge.storage_migration_verify_failed",
                format!("迁移结果中的关键目录无效：{directory}"),
                false,
            ));
        }
    }
    let source_database = source.join("knowledge.db");
    let staging_database = staging.join("knowledge.db");
    let source_hash = sha256_file(&source_database)?;
    let staging_hash = sha256_file(&staging_database)?;
    if source_hash != staging_hash {
        return Err(StorageMigrationError::new(
            "local_knowledge.storage_migration_verify_failed",
            "迁移后的 knowledge.db 哈希校验失败",
            true,
        ));
    }
    let connection = Connection::open(&staging_database).map_err(|error| {
        StorageMigrationError::new(
            "local_knowledge.storage_migration_verify_failed",
            format!("无法打开迁移后的 knowledge.db：{error}"),
            true,
        )
    })?;
    let integrity: String = connection
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .map_err(|error| {
            StorageMigrationError::new(
                "local_knowledge.storage_migration_verify_failed",
                format!("迁移后的 knowledge.db 完整性检查失败：{error}"),
                true,
            )
        })?;
    if integrity != "ok" {
        return Err(StorageMigrationError::new(
            "local_knowledge.storage_migration_verify_failed",
            format!("迁移后的 knowledge.db 完整性检查返回：{integrity}"),
            true,
        ));
    }
    Ok(())
}

pub(crate) fn commit_staging(
    staging: &Path,
    destination: &Path,
) -> Result<(), StorageMigrationError> {
    if let Ok(metadata) = fs::symlink_metadata(destination) {
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(StorageMigrationError::new(
                "local_knowledge.storage_migration_destination_invalid",
                "目标目录在提交时不再是普通目录",
                false,
            ));
        }
        if fs::read_dir(destination)
            .map_err(|error| {
                StorageMigrationError::new(
                    "local_knowledge.storage_migration_destination_unreadable",
                    format!("无法检查目标目录：{error}"),
                    true,
                )
            })?
            .next()
            .is_some()
        {
            return Err(StorageMigrationError::new(
                "local_knowledge.storage_migration_destination_exists",
                "目标目录在提交前已被写入，拒绝覆盖",
                false,
            ));
        }
        fs::remove_dir(destination).map_err(|error| {
            StorageMigrationError::new(
                "local_knowledge.storage_migration_switch_failed",
                format!("无法移除已验证的空目标目录：{error}"),
                true,
            )
        })?;
    }
    fs::rename(staging, destination).map_err(|error| {
        StorageMigrationError::new(
            "local_knowledge.storage_migration_switch_failed",
            format!("无法原子切换迁移后的存储目录：{error}"),
            true,
        )
    })
}

pub(crate) fn rollback_committed_destination(
    destination: &Path,
    rollback_path: &Path,
) -> Result<(), StorageMigrationError> {
    if !is_verified_staging_path(rollback_path, destination) {
        return Err(StorageMigrationError::new(
            "local_knowledge.storage_migration_cleanup_failed",
            "回滚路径未通过临时目录校验",
            false,
        ));
    }
    if rollback_path.exists() {
        return Err(StorageMigrationError::new(
            "local_knowledge.storage_migration_cleanup_failed",
            "回滚临时目录已经存在，拒绝覆盖",
            false,
        ));
    }
    fs::rename(destination, rollback_path).map_err(|error| {
        StorageMigrationError::new(
            "local_knowledge.storage_migration_cleanup_failed",
            format!("无法把已提交目录移回回滚临时目录：{error}"),
            true,
        )
    })
}

pub(crate) fn cleanup_staging(
    staging: &Path,
    destination: &Path,
) -> Result<(), StorageMigrationError> {
    if !staging.exists() {
        return Ok(());
    }
    if !is_verified_staging_path(staging, destination) {
        return Err(StorageMigrationError::new(
            "local_knowledge.storage_migration_cleanup_failed",
            "清理路径未通过临时目录校验，已跳过递归删除",
            false,
        ));
    }
    let metadata = fs::symlink_metadata(staging).map_err(|error| {
        StorageMigrationError::new(
            "local_knowledge.storage_migration_cleanup_failed",
            format!("无法读取临时目录：{error}"),
            true,
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(StorageMigrationError::new(
            "local_knowledge.storage_migration_cleanup_failed",
            "临时目录不是普通目录，已跳过递归删除",
            false,
        ));
    }
    fs::remove_dir_all(staging).map_err(|error| {
        StorageMigrationError::new(
            "local_knowledge.storage_migration_cleanup_failed",
            format!("清理迁移临时目录失败：{error}"),
            true,
        )
    })
}

pub(crate) fn pick_storage_directory() -> Result<Option<String>, String> {
    #[cfg(windows)]
    {
        return std::thread::spawn(pick_storage_directory_windows)
            .join()
            .map_err(|_| "系统文件夹选择器意外退出".to_owned())?
            .and_then(|path| {
                path.map(|path| {
                    validate_current_storage_root(Path::new(&path))
                        .map(|path| path.to_string_lossy().into_owned())
                        .map_err(|error| error.message)
                })
                .transpose()
            });
    }
    #[cfg(not(windows))]
    {
        Err("当前平台暂不支持系统文件夹选择器".to_owned())
    }
}

fn copy_directory_entries(source: &Path, destination: &Path) -> Result<(), String> {
    for entry in fs::read_dir(source).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let source_path = entry.path();
        let name = entry.file_name();
        validate_path_component(&name).map_err(|error| error.message)?;
        let destination_path = destination.join(&name);
        let metadata = fs::symlink_metadata(&source_path).map_err(|error| error.to_string())?;
        if metadata.file_type().is_symlink() {
            return Err(format!(
                "受管目录包含不支持的符号链接：{}",
                source_path.display()
            ));
        }
        if metadata.is_dir() {
            fs::create_dir(&destination_path).map_err(|error| error.to_string())?;
            copy_directory_entries(&source_path, &destination_path)?;
        } else if metadata.is_file() {
            fs::copy(&source_path, &destination_path).map_err(|error| error.to_string())?;
        } else {
            return Err(format!(
                "受管目录包含无法复制的文件类型：{}",
                source_path.display()
            ));
        }
    }
    Ok(())
}

fn sha256_file(path: &Path) -> Result<String, StorageMigrationError> {
    let mut file = fs::File::open(path).map_err(|error| {
        StorageMigrationError::new(
            "local_knowledge.storage_migration_verify_failed",
            format!("无法读取 {}：{error}", path.display()),
            true,
        )
    })?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 128 * 1024];
    loop {
        let count = std::io::Read::read(&mut file, &mut buffer).map_err(|error| {
            StorageMigrationError::new(
                "local_knowledge.storage_migration_verify_failed",
                format!("读取 {} 失败：{error}", path.display()),
                true,
            )
        })?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn canonicalize_existing_prefix(path: &Path) -> Result<PathBuf, StorageMigrationError> {
    let mut cursor = path.to_path_buf();
    let mut suffix: Vec<OsString> = Vec::new();
    loop {
        match fs::symlink_metadata(&cursor) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() {
                    return Err(StorageMigrationError::new(
                        "local_knowledge.storage_migration_dangerous_path",
                        format!("路径包含不受支持的符号链接：{}", cursor.display()),
                        false,
                    ));
                }
                let mut canonical = cursor.canonicalize().map_err(|error| {
                    StorageMigrationError::new(
                        "local_knowledge.storage_migration_invalid_destination",
                        format!("无法解析存储路径：{error}"),
                        false,
                    )
                })?;
                for component in suffix.iter().rev() {
                    canonical.push(component);
                }
                return Ok(canonical);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let name = cursor.file_name().ok_or_else(|| {
                    StorageMigrationError::new(
                        "local_knowledge.storage_migration_invalid_destination",
                        "存储路径的父目录不存在",
                        false,
                    )
                })?;
                suffix.push(name.to_os_string());
                if !cursor.pop() {
                    return Err(StorageMigrationError::new(
                        "local_knowledge.storage_migration_invalid_destination",
                        "无法解析存储路径的父目录",
                        false,
                    ));
                }
            }
            Err(error) => {
                return Err(StorageMigrationError::new(
                    "local_knowledge.storage_migration_invalid_destination",
                    format!("无法读取存储路径：{error}"),
                    true,
                ));
            }
        }
    }
}

fn validate_path_components(path: &Path) -> Result<(), StorageMigrationError> {
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => match prefix.kind() {
                Prefix::Disk(_) => {}
                Prefix::UNC(_, _)
                | Prefix::Verbatim(_)
                | Prefix::VerbatimUNC(_, _)
                | Prefix::VerbatimDisk(_)
                | Prefix::DeviceNS(_) => {
                    return Err(StorageMigrationError::new(
                        "local_knowledge.storage_migration_dangerous_path",
                        "不支持 UNC、设备或 verbatim 存储路径",
                        false,
                    ));
                }
            },
            Component::RootDir => {}
            Component::CurDir | Component::ParentDir => {
                return Err(StorageMigrationError::new(
                    "local_knowledge.storage_migration_dangerous_path",
                    "存储路径不能包含相对路径遍历组件",
                    false,
                ));
            }
            Component::Normal(value) => validate_path_component(value)?,
        }
    }
    Ok(())
}

fn validate_path_component(component: &OsStr) -> Result<(), StorageMigrationError> {
    let value = component.to_str().ok_or_else(|| {
        StorageMigrationError::new(
            "local_knowledge.storage_migration_dangerous_path",
            "存储路径包含无法处理的字符",
            false,
        )
    })?;
    if value.is_empty()
        || value.chars().any(char::is_control)
        || value.chars().any(|character| {
            matches!(
                character,
                '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'
            )
        })
        || value.ends_with([' ', '.'])
        || is_windows_device_name(value)
    {
        return Err(StorageMigrationError::new(
            "local_knowledge.storage_migration_dangerous_path",
            "存储路径包含不安全的 Windows 文件名或路径组件",
            false,
        ));
    }
    Ok(())
}

fn is_windows_device_name(value: &str) -> bool {
    let stem = value
        .split('.')
        .next()
        .unwrap_or_default()
        .trim_end_matches([' ', '.'])
        .to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.as_bytes()[3].is_ascii_digit()
            && stem.as_bytes()[3] != b'0')
}

fn same_path(left: &Path, right: &Path) -> bool {
    #[cfg(windows)]
    {
        left.to_string_lossy()
            .eq_ignore_ascii_case(&right.to_string_lossy())
    }
    #[cfg(not(windows))]
    {
        left == right
    }
}

fn path_starts_with(path: &Path, prefix: &Path) -> bool {
    let mut path_components = path.components();
    prefix.components().all(|prefix_component| {
        path_components.next().is_some_and(|path_component| {
            #[cfg(windows)]
            {
                path_component
                    .as_os_str()
                    .to_string_lossy()
                    .eq_ignore_ascii_case(&prefix_component.as_os_str().to_string_lossy())
            }
            #[cfg(not(windows))]
            {
                path_component == prefix_component
            }
        })
    })
}

fn strip_windows_verbatim_prefix(path: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        let value = path.to_string_lossy();
        if let Some(stripped) = value.strip_prefix("\\\\?\\") {
            return PathBuf::from(stripped);
        }
    }
    path
}

fn is_verified_staging_path(staging: &Path, destination: &Path) -> bool {
    let Some(parent) = destination.parent() else {
        return false;
    };
    if staging.parent() != Some(parent) {
        return false;
    }
    let Some(name) = staging.file_name().and_then(OsStr::to_str) else {
        return false;
    };
    let Some(destination_name) = destination.file_name().and_then(OsStr::to_str) else {
        return false;
    };
    name.starts_with(&format!(".{destination_name}.fox-staging-"))
        && name.len() > destination_name.len() + ".fox-staging-".len() + 1
}

#[tauri::command]
pub fn local_knowledge_storage_directory_pick() -> ApiResponse<Option<String>> {
    match pick_storage_directory() {
        Ok(path) => ApiResponse::success(path),
        Err(error) => ApiResponse::failure(
            "local_knowledge.storage_directory_picker_failed",
            error,
            true,
        ),
    }
}

pub(crate) fn database_storage_path(database: &Database) -> Result<Option<String>, String> {
    database.app_setting(LOCAL_KNOWLEDGE_STORAGE_PATH_KEY)
}

#[cfg(windows)]
fn pick_storage_directory_windows() -> Result<Option<String>, String> {
    use windows::core::HRESULT;
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_INPROC_SERVER,
        COINIT_APARTMENTTHREADED,
    };
    use windows::Win32::UI::Shell::{
        FileOpenDialog, IFileOpenDialog, FOS_FORCEFILESYSTEM, FOS_PICKFOLDERS, SIGDN_FILESYSPATH,
    };

    struct ComGuard;
    impl Drop for ComGuard {
        fn drop(&mut self) {
            unsafe { CoUninitialize() };
        }
    }

    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED)
            .ok()
            .map_err(|error| format!("无法初始化系统文件夹选择器：{error}"))?;
        let _guard = ComGuard;
        let dialog: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)
            .map_err(|error| format!("无法打开系统文件夹选择器：{error}"))?;
        dialog
            .SetOptions(
                dialog.GetOptions().map_err(|error| error.to_string())?
                    | FOS_PICKFOLDERS
                    | FOS_FORCEFILESYSTEM,
            )
            .map_err(|error| format!("无法配置系统文件夹选择器：{error}"))?;
        if let Err(error) = dialog.Show(None) {
            const ERROR_CANCELLED: HRESULT = HRESULT(0x800704C7u32 as i32);
            if error.code() == ERROR_CANCELLED {
                return Ok(None);
            }
            return Err(format!("系统文件夹选择器失败：{error}"));
        }
        let item = dialog
            .GetResult()
            .map_err(|error| format!("无法读取所选文件夹：{error}"))?;
        let raw_path = item
            .GetDisplayName(SIGDN_FILESYSPATH)
            .map_err(|error| format!("无法读取所选文件夹路径：{error}"))?;
        let path = raw_path
            .to_string()
            .map_err(|error| format!("所选文件夹路径不是有效的 Unicode：{error}"));
        CoTaskMemFree(Some(raw_path.0.cast()));
        path.map(Some)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use uuid::Uuid;

    #[test]
    fn startup_uses_valid_configured_path_and_falls_back_for_invalid_path() {
        let app_data = PathBuf::from(if cfg!(windows) {
            r"C:\fox-data"
        } else {
            "/tmp/fox-data"
        });
        let configured = if cfg!(windows) {
            r"C:\fox-custom\knowledge"
        } else {
            "/tmp/fox-custom/knowledge"
        };
        assert_eq!(
            resolve_startup_root(&app_data, Some(configured)),
            PathBuf::from(configured)
        );
        assert_eq!(
            resolve_startup_root(&app_data, Some("relative/path")),
            default_storage_root(&app_data)
        );
    }

    #[test]
    fn destination_rejects_same_path_and_nested_paths() {
        let root = std::env::temp_dir().join(format!("fox-storage-path-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let source = root.join("source");
        fs::create_dir_all(&source).unwrap();
        assert!(validate_destination_storage_root(&source, &source).is_err());
        assert!(validate_destination_storage_root(&source, &source.join("nested")).is_err());
        assert!(validate_destination_storage_root(&source.join("nested"), &source).is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
