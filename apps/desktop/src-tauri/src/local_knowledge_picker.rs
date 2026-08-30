use crate::database::ApiResponse;
use serde::Serialize;
use std::{
    ffi::OsStr,
    fs,
    path::{Component, Path, PathBuf, Prefix},
};

pub(crate) const MAX_PICKED_FILE_SIZE: u64 = 256 * 1024 * 1024;
const SUPPORTED_EXTENSIONS: &[&str] = &["txt", "md", "pdf", "docx", "pptx", "xlsx"];
const FILE_DIALOG_FILTER: &str = "*.txt;*.md;*.pdf;*.docx;*.pptx;*.xlsx";
const MAX_PICKED_FOLDER_FILES: usize = 5_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PickedLocalKnowledgeFile {
    pub source_path: String,
    pub name: String,
    pub mime_type: String,
    pub size_bytes: u64,
    pub relative_path: String,
}

fn extension_for_file_name(file_name: &str) -> Option<String> {
    Path::new(file_name)
        .extension()
        .and_then(OsStr::to_str)
        .map(str::to_ascii_lowercase)
        .filter(|extension| !extension.is_empty())
}

pub(crate) fn mime_type_for_file_name(file_name: &str) -> Option<&'static str> {
    match extension_for_file_name(file_name)?.as_str() {
        "txt" => Some("text/plain"),
        "md" => Some("text/markdown"),
        "pdf" => Some("application/pdf"),
        "docx" => Some("application/vnd.openxmlformats-officedocument.wordprocessingml.document"),
        "pptx" => Some("application/vnd.openxmlformats-officedocument.presentationml.presentation"),
        "xlsx" => Some("application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"),
        _ => None,
    }
}

pub(crate) fn is_supported_file_name(file_name: &str) -> bool {
    extension_for_file_name(file_name)
        .as_deref()
        .is_some_and(|extension| SUPPORTED_EXTENSIONS.contains(&extension))
}

pub(crate) fn validate_path_component(component: &OsStr) -> Result<(), String> {
    let value = component
        .to_str()
        .ok_or_else(|| "所选路径包含无法处理的字符".to_owned())?;
    if value.is_empty()
        || value == "."
        || value == ".."
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
        return Err("所选路径包含不安全的 Windows 文件名或路径组件".to_owned());
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

fn validate_source_path(path: &Path) -> Result<(), String> {
    if path.as_os_str().is_empty() || !path.is_absolute() {
        return Err("所选文件路径必须是绝对路径".to_owned());
    }

    for component in path.components() {
        match component {
            Component::Prefix(prefix) => match prefix.kind() {
                Prefix::Disk(_) => {}
                Prefix::UNC(_, _)
                | Prefix::Verbatim(_)
                | Prefix::VerbatimUNC(_, _)
                | Prefix::VerbatimDisk(_)
                | Prefix::DeviceNS(_) => {
                    return Err("不支持 UNC、设备或 verbatim 文件路径".to_owned());
                }
            },
            Component::RootDir => {}
            Component::CurDir | Component::ParentDir => {
                return Err("所选文件路径不能包含相对路径遍历组件".to_owned());
            }
            Component::Normal(value) => validate_path_component(value)?,
        }
    }
    Ok(())
}

fn validate_picked_file(path: &Path) -> Result<PickedLocalKnowledgeFile, String> {
    validate_source_path(path)?;

    let name = path
        .file_name()
        .and_then(OsStr::to_str)
        .ok_or_else(|| "无法读取所选文件名".to_owned())?;
    if !is_supported_file_name(name) {
        return Err(format!(
            "不支持导入此文件类型：{name}（支持格式：{FILE_DIALOG_FILTER}）"
        ));
    }
    let mime_type =
        mime_type_for_file_name(name).ok_or_else(|| format!("不支持导入此文件类型：{name}"))?;
    let metadata =
        fs::symlink_metadata(path).map_err(|error| format!("无法读取所选文件信息：{error}"))?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
        return Err("所选路径不是普通文件".to_owned());
    }
    if metadata.len() > MAX_PICKED_FILE_SIZE {
        return Err(format!("所选文件超过 256 MB 限制：{name}"));
    }

    let source_path = path
        .to_str()
        .ok_or_else(|| "所选文件路径不是有效的 Unicode".to_owned())?
        .to_owned();
    Ok(PickedLocalKnowledgeFile {
        source_path,
        name: name.to_owned(),
        mime_type: mime_type.to_owned(),
        size_bytes: metadata.len(),
        relative_path: name.to_owned(),
    })
}

fn collect_folder_files(root: &Path) -> Result<Vec<PickedLocalKnowledgeFile>, String> {
    validate_source_path(root)?;
    let metadata =
        fs::symlink_metadata(root).map_err(|error| format!("无法读取所选文件夹：{error}"))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err("所选路径不是普通文件夹".to_owned());
    }
    let root_name = root
        .file_name()
        .and_then(OsStr::to_str)
        .ok_or_else(|| "无法读取所选文件夹名称".to_owned())?;
    validate_path_component(OsStr::new(root_name))?;

    let mut pending = vec![root.to_path_buf()];
    let mut files = Vec::new();
    while let Some(directory) = pending.pop() {
        let entries = fs::read_dir(&directory)
            .map_err(|error| format!("无法读取文件夹 {}：{error}", directory.display()))?;
        for entry in entries {
            let entry = entry.map_err(|error| format!("无法读取文件夹条目：{error}"))?;
            let path: PathBuf = entry.path();
            let metadata = fs::symlink_metadata(&path)
                .map_err(|error| format!("无法读取 {}：{error}", path.display()))?;
            if metadata.file_type().is_symlink() {
                continue;
            }
            if metadata.is_dir() {
                pending.push(path);
                continue;
            }
            if !metadata.is_file() {
                continue;
            }
            let Some(name) = path.file_name().and_then(OsStr::to_str) else {
                return Err("文件夹中包含无法读取名称的文件".to_owned());
            };
            if !is_supported_file_name(name) {
                continue;
            }
            if files.len() >= MAX_PICKED_FOLDER_FILES {
                return Err(format!("单次最多导入 {MAX_PICKED_FOLDER_FILES} 个文件"));
            }
            let mut picked = validate_picked_file(&path)?;
            let nested = path
                .strip_prefix(root)
                .map_err(|_| "无法计算文件相对路径".to_owned())?
                .to_string_lossy()
                .replace('\\', "/");
            picked.relative_path = format!("{root_name}/{nested}");
            files.push(picked);
        }
    }
    files.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    Ok(files)
}

#[cfg(windows)]
fn pick_files_windows() -> Result<Vec<PickedLocalKnowledgeFile>, String> {
    use windows::core::HRESULT;
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_INPROC_SERVER,
        COINIT_APARTMENTTHREADED,
    };
    use windows::Win32::UI::Shell::{
        FileOpenDialog, IFileOpenDialog, FOS_ALLOWMULTISELECT, FOS_FILEMUSTEXIST,
        FOS_FORCEFILESYSTEM, SIGDN_FILESYSPATH,
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
            .map_err(|error| format!("无法初始化系统文件选择器：{error}"))?;
        let _guard = ComGuard;
        let dialog: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)
            .map_err(|error| format!("无法打开系统文件选择器：{error}"))?;
        dialog
            .SetOptions(
                dialog.GetOptions().map_err(|error| error.to_string())?
                    | FOS_ALLOWMULTISELECT
                    | FOS_FILEMUSTEXIST
                    | FOS_FORCEFILESYSTEM,
            )
            .map_err(|error| format!("无法配置系统文件选择器：{error}"))?;

        if let Err(error) = dialog.Show(None) {
            const ERROR_CANCELLED: HRESULT = HRESULT(0x800704C7u32 as i32);
            if error.code() == ERROR_CANCELLED {
                return Ok(Vec::new());
            }
            return Err(format!("系统文件选择器失败：{error}"));
        }

        let items = dialog
            .GetResults()
            .map_err(|error| format!("无法读取所选文件：{error}"))?;
        let count = items
            .GetCount()
            .map_err(|error| format!("无法读取所选文件数量：{error}"))?;
        let mut files = Vec::with_capacity(count as usize);
        for index in 0..count {
            let item = items
                .GetItemAt(index)
                .map_err(|error| format!("无法读取第 {} 个所选文件：{error}", index + 1))?;
            let raw_path = item
                .GetDisplayName(SIGDN_FILESYSPATH)
                .map_err(|error| format!("无法读取第 {} 个所选文件路径：{error}", index + 1))?;
            let path = raw_path.to_string();
            CoTaskMemFree(Some(raw_path.0.cast()));
            let path = path.map_err(|error| {
                format!("第 {} 个所选文件路径不是有效的 Unicode：{error}", index + 1)
            })?;
            files.push(
                validate_picked_file(Path::new(&path))
                    .map_err(|error| format!("第 {} 个所选文件无效：{error}", index + 1))?,
            );
        }
        Ok(files)
    }
}

#[tauri::command]
pub fn local_knowledge_import_files_pick() -> ApiResponse<Vec<PickedLocalKnowledgeFile>> {
    #[cfg(windows)]
    {
        return match std::thread::spawn(pick_files_windows).join() {
            Ok(Ok(files)) => ApiResponse::success(files),
            Ok(Err(error)) => {
                ApiResponse::failure("local_knowledge.file_picker_failed", error, true)
            }
            Err(_) => ApiResponse::failure(
                "local_knowledge.file_picker_failed",
                "系统文件选择器意外退出",
                true,
            ),
        };
    }

    #[cfg(not(windows))]
    ApiResponse::failure(
        "local_knowledge.file_picker_unsupported",
        "当前平台暂不支持系统文件选择器",
        false,
    )
}

#[tauri::command]
pub fn local_knowledge_import_folder_pick() -> ApiResponse<Vec<PickedLocalKnowledgeFile>> {
    match crate::local_knowledge_storage::pick_storage_directory() {
        Ok(Some(path)) => match collect_folder_files(Path::new(&path)) {
            Ok(files) => ApiResponse::success(files),
            Err(error) => {
                ApiResponse::failure("local_knowledge.import_folder_invalid", error, false)
            }
        },
        Ok(None) => ApiResponse::success(Vec::new()),
        Err(error) => {
            ApiResponse::failure("local_knowledge.import_folder_picker_failed", error, true)
        }
    }
}

#[tauri::command]
pub fn local_knowledge_source_folder_pick() -> ApiResponse<Option<String>> {
    match crate::local_knowledge_storage::pick_storage_directory() {
        Ok(path) => ApiResponse::success(path),
        Err(error) => {
            ApiResponse::failure("local_knowledge.source_folder_picker_failed", error, true)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_filter_is_case_insensitive_and_rejects_unknown_files() {
        assert!(is_supported_file_name("报告.PDF"));
        assert!(is_supported_file_name("notes.Md"));
        assert!(!is_supported_file_name("archive.zip"));
        assert!(!is_supported_file_name("README"));
        assert!(!is_supported_file_name(".pdf"));
    }

    #[test]
    fn mime_types_match_the_import_pipeline() {
        assert_eq!(mime_type_for_file_name("notes.txt"), Some("text/plain"));
        assert_eq!(mime_type_for_file_name("notes.md"), Some("text/markdown"));
        assert_eq!(
            mime_type_for_file_name("manual.docx"),
            Some("application/vnd.openxmlformats-officedocument.wordprocessingml.document")
        );
        assert_eq!(
            mime_type_for_file_name("slides.pptx"),
            Some("application/vnd.openxmlformats-officedocument.presentationml.presentation")
        );
        assert_eq!(
            mime_type_for_file_name("table.xlsx"),
            Some("application/vnd.openxmlformats-officedocument.spreadsheetml.sheet")
        );
        assert_eq!(mime_type_for_file_name("image.png"), None);
    }

    #[test]
    fn dialog_filter_covers_only_supported_extensions() {
        assert_eq!(SUPPORTED_EXTENSIONS.len(), 6);
        for extension in SUPPORTED_EXTENSIONS {
            assert!(FILE_DIALOG_FILTER.contains(&format!("*.{extension}")));
        }
        assert!(!FILE_DIALOG_FILTER.contains("*.*"));
    }

    #[test]
    fn path_validation_rejects_traversal_and_device_names() {
        assert!(validate_path_component(OsStr::new(".. ")).is_err());
        assert!(validate_path_component(OsStr::new("CON.txt")).is_err());
        assert!(validate_path_component(OsStr::new("safe-name.txt")).is_ok());
    }

    #[test]
    fn relative_paths_are_not_accepted_as_picker_results() {
        assert!(validate_source_path(Path::new("documents/manual.pdf")).is_err());
    }
}
