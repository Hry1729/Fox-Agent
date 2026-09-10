//! Host-owned access to persisted conversation artifacts.
//!
//! Artifact rows contain paths that originated in a tool result.  The
//! renderer must therefore never turn that path into a file-system operation
//! by itself.  This module keeps the lookup scoped to a conversation and
//! validates the current file, its root and its recorded integrity before any
//! preview or OS action is performed.

use crate::{
    app_state::AppState,
    commands,
    database::{ApiResponse, ArtifactRecord},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

pub(crate) const MAX_ARTIFACT_PREVIEW_BYTES: usize = 512 * 1024;
/// Bound integrity verification without applying the much smaller inline
/// preview limit to ordinary images, PDFs and Office documents. SHA-256 is
/// streamed, so validation never loads the whole file into memory.
pub(crate) const MAX_ARTIFACT_FILE_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ArtifactRequest {
    pub conversation_id: String,
    pub artifact_id: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ArtifactActionRequest {
    pub conversation_id: String,
    pub artifact_id: String,
    pub action: String,
    pub application_id: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct ArtifactGatewayError {
    pub code: &'static str,
    pub message: String,
    pub retryable: bool,
}

impl ArtifactGatewayError {
    fn new(code: &'static str, message: impl Into<String>, retryable: bool) -> Self {
        Self {
            code,
            message: message.into(),
            retryable,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ArtifactGatewayArtifact {
    pub id: String,
    pub conversation_id: String,
    pub run_id: Option<String>,
    pub display_name: String,
    pub artifact_type: String,
    pub media_type: Option<String>,
    pub byte_size: i64,
    pub sha256: Option<String>,
    pub status: String,
    pub created_at: i64,
    pub updated_at: i64,
}

impl From<&ArtifactRecord> for ArtifactGatewayArtifact {
    fn from(record: &ArtifactRecord) -> Self {
        Self {
            id: record.id.clone(),
            conversation_id: record.conversation_id.clone(),
            run_id: record.run_id.clone(),
            display_name: record.display_name.clone(),
            artifact_type: record.artifact_type.clone(),
            media_type: record.media_type.clone(),
            byte_size: record.byte_size,
            sha256: record.sha256.clone(),
            status: record.status.clone(),
            created_at: record.created_at,
            updated_at: record.updated_at,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ArtifactCapabilities {
    pub preview: bool,
    pub open: bool,
    pub reveal: bool,
    pub copy_path: bool,
    pub open_with: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ArtifactApplication {
    pub id: String,
    pub label: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ArtifactPreview {
    /// One of `text`, `markdown`, `html`, `image`, `pdf` or `unknown`.
    pub kind: String,
    pub available: bool,
    pub content: Option<String>,
    pub truncated: bool,
    pub byte_size: i64,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ArtifactInspectResponse {
    pub artifact: ArtifactGatewayArtifact,
    pub capabilities: ArtifactCapabilities,
    pub applications: Vec<ArtifactApplication>,
    pub preview: ArtifactPreview,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ArtifactActionResponse {
    pub action: String,
    pub completed: bool,
    pub artifact: ArtifactGatewayArtifact,
    pub capabilities: ArtifactCapabilities,
    pub applications: Vec<ArtifactApplication>,
    pub preview: ArtifactPreview,
}

pub(crate) fn inspect(
    state: &AppState,
    request: &ArtifactRequest,
) -> Result<ArtifactInspectResponse, ArtifactGatewayError> {
    let record = load_record(state, &request.conversation_id, &request.artifact_id)?;
    let roots = authorized_roots(state, &request.conversation_id)?;
    let path = validate_record(&record, &roots)?;
    Ok(inspect_record(&record, &path))
}

pub(crate) fn action(
    state: &AppState,
    request: &ArtifactActionRequest,
) -> Result<ArtifactActionResponse, ArtifactGatewayError> {
    let action = request.action.trim().to_ascii_lowercase();
    if !matches!(
        action.as_str(),
        "preview" | "open" | "open_with" | "reveal" | "copy_path"
    ) {
        return Err(ArtifactGatewayError::new(
            "artifact.action_unsupported",
            "不支持该产物操作",
            false,
        ));
    }

    let record = load_record(state, &request.conversation_id, &request.artifact_id)?;
    let roots = authorized_roots(state, &request.conversation_id)?;
    let path = validate_record(&record, &roots)?;
    let inspected = inspect_record(&record, &path);

    match action.as_str() {
        "preview" => {}
        "open" => {
            if !inspected.capabilities.open {
                return Err(ArtifactGatewayError::new(
                    "artifact.open_unsupported",
                    "该产物类型不支持直接打开，请先在文件夹中查看",
                    false,
                ));
            }
            commands::open_downloaded_file(&path, false)
                .map_err(|error| ArtifactGatewayError::new("artifact.open_failed", error, true))?;
        }
        "open_with" => {
            let application_id = request
                .application_id
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    ArtifactGatewayError::new(
                        "artifact.application_required",
                        "请选择一个可用的编程软件",
                        false,
                    )
                })?;
            open_with_application(application_id, &path)?;
        }
        "reveal" => {
            commands::open_downloaded_file(&path, true).map_err(|error| {
                ArtifactGatewayError::new("artifact.reveal_failed", error, true)
            })?;
        }
        "copy_path" => copy_path_to_clipboard(&path)?,
        _ => unreachable!("action was checked above"),
    }

    Ok(ArtifactActionResponse {
        action,
        completed: true,
        artifact: inspected.artifact,
        capabilities: inspected.capabilities,
        applications: inspected.applications,
        preview: inspected.preview,
    })
}

pub(crate) fn inspect_api(
    state: &AppState,
    request: &ArtifactRequest,
) -> ApiResponse<ArtifactInspectResponse> {
    match inspect(state, request) {
        Ok(response) => ApiResponse::success(response),
        Err(error) => ApiResponse::failure(error.code, error.message, error.retryable),
    }
}

pub(crate) fn action_api(
    state: &AppState,
    request: &ArtifactActionRequest,
) -> ApiResponse<ArtifactActionResponse> {
    match action(state, request) {
        Ok(response) => ApiResponse::success(response),
        Err(error) => ApiResponse::failure(error.code, error.message, error.retryable),
    }
}

pub(crate) fn system_file_applications() -> Vec<ArtifactApplication> {
    detected_applications()
        .into_iter()
        .map(|application| ArtifactApplication {
            id: application.id.to_owned(),
            label: application.label.to_owned(),
        })
        .collect()
}

pub(crate) fn system_file_action(
    path: &Path,
    action: &str,
    application_id: Option<&str>,
) -> Result<Vec<ArtifactApplication>, ArtifactGatewayError> {
    let action = action.trim().to_ascii_lowercase();
    if !matches!(
        action.as_str(),
        "inspect" | "open" | "open_with" | "reveal" | "copy_path"
    ) {
        return Err(ArtifactGatewayError::new(
            "project.file_action_unsupported",
            "不支持该文件操作",
            false,
        ));
    }
    match action.as_str() {
        "inspect" => {}
        "open" => commands::open_downloaded_file(path, false)
            .map_err(|error| ArtifactGatewayError::new("project.file_open_failed", error, true))?,
        "open_with" => {
            let application_id = application_id
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    ArtifactGatewayError::new(
                        "project.application_required",
                        "请选择一个可用的编程软件",
                        false,
                    )
                })?;
            open_with_application(application_id, path)?;
        }
        "reveal" => commands::open_downloaded_file(path, true).map_err(|error| {
            ArtifactGatewayError::new("project.file_reveal_failed", error, true)
        })?,
        "copy_path" => copy_path_to_clipboard(path)?,
        _ => unreachable!("action was checked above"),
    }
    Ok(system_file_applications())
}

fn load_record(
    state: &AppState,
    conversation_id: &str,
    artifact_id: &str,
) -> Result<ArtifactRecord, ArtifactGatewayError> {
    state
        .database
        .artifact_for_conversation(conversation_id, artifact_id)
        .map_err(|error| {
            ArtifactGatewayError::new(
                "artifact.database_error",
                format!("无法读取产物记录：{error}"),
                true,
            )
        })?
        .ok_or_else(|| {
            ArtifactGatewayError::new("artifact.not_found", "找不到当前会话中的产物", false)
        })
}

fn authorized_roots(
    state: &AppState,
    conversation_id: &str,
) -> Result<Vec<PathBuf>, ArtifactGatewayError> {
    let mut roots = Vec::new();
    if let Some(project_root) = state
        .database
        .conversation_project_root(conversation_id)
        .map_err(|error| {
            ArtifactGatewayError::new(
                "artifact.database_error",
                format!("无法读取会话项目根：{error}"),
                true,
            )
        })?
    {
        let root = PathBuf::from(project_root)
            .canonicalize()
            .map_err(|error| {
                ArtifactGatewayError::new(
                    "artifact.root_unavailable",
                    format!("会话项目根不可用：{error}"),
                    false,
                )
            })?;
        if root.is_dir() {
            roots.push(root);
        }
    }

    if let Some(root)=crate::runtime_host::attachment_compute::authorized_artifact_root(&state.data_dir.join("runtime-sessions"),conversation_id)
        .map_err(|e|ArtifactGatewayError::new("artifact.path_denied",e,false))? { roots.push(root); }
    if roots.is_empty() {
        return Err(ArtifactGatewayError::new(
            "artifact.root_unavailable",
            "当前会话没有可用的产物文件根目录",
            false,
        ));
    }
    Ok(roots)
}

/// Resolve an artifact path against already-canonical roots.  Keeping this
/// helper independent makes the traversal and symlink boundary testable
/// without constructing a Tauri `AppState`.
pub(crate) fn resolve_artifact_path(
    record: &ArtifactRecord,
    roots: &[PathBuf],
) -> Result<PathBuf, ArtifactGatewayError> {
    let raw = Path::new(record.storage_path.trim());
    if raw.as_os_str().is_empty() || !raw.is_absolute() {
        return Err(ArtifactGatewayError::new(
            "artifact.path_denied",
            "产物路径必须是绝对路径",
            false,
        ));
    }
    if roots.is_empty() {
        return Err(ArtifactGatewayError::new(
            "artifact.path_denied",
            "没有可用的产物文件根目录",
            false,
        ));
    }

    let link_metadata = fs::symlink_metadata(raw).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            ArtifactGatewayError::new("artifact.file_missing", "产物文件不存在", false)
        } else {
            ArtifactGatewayError::new(
                "artifact.file_unavailable",
                format!("无法访问产物文件：{error}"),
                true,
            )
        }
    })?;
    if link_metadata.file_type().is_symlink() {
        return Err(ArtifactGatewayError::new(
            "artifact.path_denied",
            "不允许通过符号链接访问产物",
            false,
        ));
    }
    if !link_metadata.is_file() {
        return Err(ArtifactGatewayError::new(
            "artifact.not_regular_file",
            "产物路径不是普通文件",
            false,
        ));
    }

    let canonical = raw.canonicalize().map_err(|error| {
        ArtifactGatewayError::new(
            "artifact.file_unavailable",
            format!("无法解析产物文件：{error}"),
            true,
        )
    })?;
    if !roots.iter().any(|root| canonical.starts_with(root)) {
        return Err(ArtifactGatewayError::new(
            "artifact.path_denied",
            "产物文件超出当前会话允许的目录范围",
            false,
        ));
    }
    let canonical_metadata = fs::metadata(&canonical).map_err(|error| {
        ArtifactGatewayError::new(
            "artifact.file_unavailable",
            format!("无法读取产物文件属性：{error}"),
            true,
        )
    })?;
    if !canonical_metadata.is_file() {
        return Err(ArtifactGatewayError::new(
            "artifact.not_regular_file",
            "产物路径不是普通文件",
            false,
        ));
    }
    Ok(canonical)
}

pub(crate) fn validate_record(
    record: &ArtifactRecord,
    roots: &[PathBuf],
) -> Result<PathBuf, ArtifactGatewayError> {
    let status = record.status.trim().to_ascii_lowercase();
    if !matches!(status.as_str(), "ready" | "completed") {
        return Err(ArtifactGatewayError::new(
            "artifact.status_unavailable",
            "该产物当前不可访问",
            false,
        ));
    }
    if record.byte_size < 0 {
        return Err(ArtifactGatewayError::new(
            "artifact.integrity_mismatch",
            "产物记录的大小无效",
            false,
        ));
    }

    let path = resolve_artifact_path(record, roots)?;
    let actual_size = fs::metadata(&path)
        .map_err(|error| {
            ArtifactGatewayError::new(
                "artifact.file_unavailable",
                format!("无法读取产物文件属性：{error}"),
                true,
            )
        })?
        .len();
    if actual_size > MAX_ARTIFACT_FILE_BYTES {
        return Err(ArtifactGatewayError::new(
            "artifact.file_too_large",
            "产物文件超过 Fox 允许的访问大小",
            false,
        ));
    }
    if actual_size != record.byte_size as u64 {
        return Err(ArtifactGatewayError::new(
            "artifact.integrity_mismatch",
            "产物文件大小已发生变化",
            false,
        ));
    }
    if let Some(expected) = record
        .sha256
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let actual = sha256_file(&path)?;
        if !actual.eq_ignore_ascii_case(expected) {
            return Err(ArtifactGatewayError::new(
                "artifact.integrity_mismatch",
                "产物文件校验和已发生变化",
                false,
            ));
        }
    }
    Ok(path)
}

fn sha256_file(path: &Path) -> Result<String, ArtifactGatewayError> {
    let mut file = File::open(path).map_err(|error| {
        ArtifactGatewayError::new(
            "artifact.file_unavailable",
            format!("无法读取产物文件：{error}"),
            true,
        )
    })?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 32 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|error| {
            ArtifactGatewayError::new(
                "artifact.file_unavailable",
                format!("无法读取产物文件：{error}"),
                true,
            )
        })?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn inspect_record(record: &ArtifactRecord, path: &Path) -> ArtifactInspectResponse {
    let preview = build_preview(record, path);
    let direct_open = can_open_directly(path);
    let applications = detected_applications();
    ArtifactInspectResponse {
        artifact: ArtifactGatewayArtifact::from(record),
        capabilities: ArtifactCapabilities {
            preview: preview.available,
            open: direct_open,
            reveal: true,
            copy_path: cfg!(windows),
            open_with: !applications.is_empty(),
        },
        applications: applications
            .iter()
            .map(|application| ArtifactApplication {
                id: application.id.to_owned(),
                label: application.label.to_owned(),
            })
            .collect(),
        preview,
    }
}

fn build_preview(record: &ArtifactRecord, path: &Path) -> ArtifactPreview {
    let kind = preview_kind(record, path);
    let byte_size = fs::metadata(path)
        .map(|metadata| metadata.len().min(i64::MAX as u64) as i64)
        .unwrap_or(record.byte_size);
    if !matches!(kind.as_str(), "text" | "markdown" | "html") {
        let reason = match kind.as_str() {
            "image" => "图片预览将在系统查看器中打开".to_owned(),
            "pdf" => "PDF 预览将在系统查看器中打开".to_owned(),
            _ => "该文件类型没有内置文本预览".to_owned(),
        };
        return ArtifactPreview {
            kind,
            available: false,
            content: None,
            truncated: false,
            byte_size,
            reason: Some(reason),
        };
    }

    let mut bytes = Vec::with_capacity(MAX_ARTIFACT_PREVIEW_BYTES.min(byte_size as usize) + 1);
    let read_result = File::open(path).and_then(|mut file| {
        Read::by_ref(&mut file)
            .take((MAX_ARTIFACT_PREVIEW_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
    });
    let Ok(_) = read_result else {
        return ArtifactPreview {
            kind,
            available: false,
            content: None,
            truncated: false,
            byte_size,
            reason: Some("无法读取文本预览".to_owned()),
        };
    };
    let truncated = bytes.len() > MAX_ARTIFACT_PREVIEW_BYTES;
    if truncated {
        bytes.truncate(MAX_ARTIFACT_PREVIEW_BYTES);
    }
    if bytes.contains(&0) {
        return ArtifactPreview {
            kind: "unknown".to_owned(),
            available: false,
            content: None,
            truncated: false,
            byte_size,
            reason: Some("该文件包含二进制内容，暂不提供内置预览".to_owned()),
        };
    }
    match String::from_utf8(bytes) {
        Ok(content) => ArtifactPreview {
            kind,
            available: true,
            content: Some(content),
            truncated,
            byte_size,
            reason: None,
        },
        Err(_) => ArtifactPreview {
            kind: "unknown".to_owned(),
            available: false,
            content: None,
            truncated: false,
            byte_size,
            reason: Some("该文件不是有效的 UTF-8 文本".to_owned()),
        },
    }
}

fn preview_kind(record: &ArtifactRecord, path: &Path) -> String {
    let media_type = record
        .media_type
        .as_deref()
        .unwrap_or_default()
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    if media_type == "text/html" || media_type == "application/xhtml+xml" {
        return "html".to_owned();
    }
    if media_type == "text/markdown" || media_type == "text/x-markdown" {
        return "markdown".to_owned();
    }
    if media_type.starts_with("text/")
        || matches!(
            media_type.as_str(),
            "application/json"
                | "application/xml"
                | "text/yaml"
                | "application/yaml"
                | "application/toml"
        )
    {
        return "text".to_owned();
    }
    if media_type == "application/pdf" {
        return "pdf".to_owned();
    }
    if media_type.starts_with("image/") {
        return "image".to_owned();
    }
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "md" | "markdown" => "markdown".to_owned(),
        "html" | "htm" => "html".to_owned(),
        "txt" | "text" | "log" | "json" | "xml" | "yaml" | "yml" | "toml" | "csv" | "tsv"
        | "rs" | "ts" | "tsx" | "js" | "jsx" | "py" | "go" | "java" | "css" | "sql" => {
            "text".to_owned()
        }
        "pdf" => "pdf".to_owned(),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "tif" | "tiff" => "image".to_owned(),
        _ => "unknown".to_owned(),
    }
}

fn can_open_directly(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str(),
        "pdf"
            | "doc"
            | "docx"
            | "xls"
            | "xlsx"
            | "ppt"
            | "pptx"
            | "odt"
            | "ods"
            | "odp"
            | "rtf"
            | "txt"
            | "md"
            | "markdown"
            | "csv"
            | "tsv"
            | "json"
            | "xml"
            | "yaml"
            | "yml"
            | "toml"
            | "html"
            | "htm"
            | "png"
            | "jpg"
            | "jpeg"
            | "gif"
            | "webp"
            | "bmp"
            | "tif"
            | "tiff"
    )
}

#[derive(Debug, Clone)]
struct DetectedApplication {
    id: &'static str,
    label: &'static str,
    executable: PathBuf,
}

#[cfg(windows)]
fn detected_applications() -> Vec<DetectedApplication> {
    let local_app_data = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
    let program_files = std::env::var_os("ProgramFiles").map(PathBuf::from);
    let program_files_x86 = std::env::var_os("ProgramFiles(x86)").map(PathBuf::from);
    let mut applications = Vec::new();
    let mut add =
        |id: &'static str, label: &'static str, executable_names: &[&str], paths: Vec<PathBuf>| {
            let executable = paths
                .into_iter()
                .find(|path| path.is_file())
                .or_else(|| find_on_path(executable_names));
            if let Some(executable) = executable {
                applications.push(DetectedApplication {
                    id,
                    label,
                    executable,
                });
            }
        };

    add(
        "vscode",
        "Visual Studio Code",
        &["Code.exe"],
        [
            local_app_data
                .as_ref()
                .map(|root| root.join("Programs/Microsoft VS Code/Code.exe")),
            program_files
                .as_ref()
                .map(|root| root.join("Microsoft VS Code/Code.exe")),
        ]
        .into_iter()
        .flatten()
        .collect(),
    );
    add(
        "cursor",
        "Cursor",
        &["Cursor.exe"],
        [
            local_app_data
                .as_ref()
                .map(|root| root.join("Programs/cursor/Cursor.exe")),
            program_files
                .as_ref()
                .map(|root| root.join("Cursor/Cursor.exe")),
        ]
        .into_iter()
        .flatten()
        .collect(),
    );
    add(
        "windsurf",
        "Windsurf",
        &["Windsurf.exe"],
        [
            local_app_data
                .as_ref()
                .map(|root| root.join("Programs/Windsurf/Windsurf.exe")),
            program_files
                .as_ref()
                .map(|root| root.join("Windsurf/Windsurf.exe")),
        ]
        .into_iter()
        .flatten()
        .collect(),
    );
    add(
        "zed",
        "Zed",
        &["zed.exe", "Zed.exe"],
        [local_app_data
            .as_ref()
            .map(|root| root.join("Programs/Zed/Zed.exe"))]
        .into_iter()
        .flatten()
        .collect(),
    );
    add(
        "sublime_text",
        "Sublime Text",
        &["sublime_text.exe"],
        [
            program_files
                .as_ref()
                .map(|root| root.join("Sublime Text/sublime_text.exe")),
            program_files_x86
                .as_ref()
                .map(|root| root.join("Sublime Text/sublime_text.exe")),
        ]
        .into_iter()
        .flatten()
        .collect(),
    );
    add(
        "notepad_plus_plus",
        "Notepad++",
        &["notepad++.exe"],
        [
            program_files
                .as_ref()
                .map(|root| root.join("Notepad++/notepad++.exe")),
            program_files_x86
                .as_ref()
                .map(|root| root.join("Notepad++/notepad++.exe")),
        ]
        .into_iter()
        .flatten()
        .collect(),
    );
    for (id, label, executable) in [
        ("intellij_idea", "IntelliJ IDEA", "idea64.exe"),
        ("pycharm", "PyCharm", "pycharm64.exe"),
        ("webstorm", "WebStorm", "webstorm64.exe"),
        ("rustrover", "RustRover", "rustrover64.exe"),
    ] {
        add(id, label, &[executable], Vec::new());
    }
    applications
}

#[cfg(windows)]
fn find_on_path(executable_names: &[&str]) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for directory in std::env::split_paths(&path) {
        for executable_name in executable_names {
            let candidate = directory.join(executable_name);
            if candidate.is_file() {
                return Some(candidate);
            }
            if let Some(parent) = directory.parent() {
                let candidate = parent.join(executable_name);
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
    }
    None
}

#[cfg(not(windows))]
fn detected_applications() -> Vec<DetectedApplication> {
    Vec::new()
}

#[cfg(windows)]
fn open_with_application(application_id: &str, path: &Path) -> Result<(), ArtifactGatewayError> {
    let application = detected_applications()
        .into_iter()
        .find(|application| application.id == application_id)
        .ok_or_else(|| {
            ArtifactGatewayError::new(
                "artifact.application_unavailable",
                "选择的编程软件当前不可用",
                false,
            )
        })?;
    Command::new(&application.executable)
        .arg(path)
        .spawn()
        .map(|_| ())
        .map_err(|error| {
            ArtifactGatewayError::new(
                "artifact.open_with_failed",
                format!("无法使用 {} 打开文件：{error}", application.label),
                true,
            )
        })
}

#[cfg(not(windows))]
fn open_with_application(_application_id: &str, _path: &Path) -> Result<(), ArtifactGatewayError> {
    Err(ArtifactGatewayError::new(
        "artifact.application_unavailable",
        "当前平台暂不支持选择编程软件打开",
        false,
    ))
}

#[cfg(windows)]
fn copy_path_to_clipboard(path: &Path) -> Result<(), ArtifactGatewayError> {
    let mut child = Command::new("clip.exe")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| {
            ArtifactGatewayError::new(
                "artifact.copy_path_failed",
                format!("无法调用系统剪贴板：{error}"),
                true,
            )
        })?;
    let mut stdin = child.stdin.take().ok_or_else(|| {
        ArtifactGatewayError::new("artifact.copy_path_failed", "无法写入系统剪贴板", true)
    })?;
    stdin
        .write_all(path.to_string_lossy().as_bytes())
        .map_err(|error| {
            ArtifactGatewayError::new(
                "artifact.copy_path_failed",
                format!("无法写入系统剪贴板：{error}"),
                true,
            )
        })?;
    drop(stdin);
    let status = child.wait().map_err(|error| {
        ArtifactGatewayError::new(
            "artifact.copy_path_failed",
            format!("系统剪贴板操作失败：{error}"),
            true,
        )
    })?;
    if !status.success() {
        return Err(ArtifactGatewayError::new(
            "artifact.copy_path_failed",
            "系统剪贴板拒绝了该路径",
            true,
        ));
    }
    Ok(())
}

#[cfg(not(windows))]
fn copy_path_to_clipboard(_path: &Path) -> Result<(), ArtifactGatewayError> {
    Err(ArtifactGatewayError::new(
        "artifact.copy_path_unsupported",
        "当前平台暂不支持复制产物路径",
        false,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};
    use uuid::Uuid;

    fn fixture_dir() -> PathBuf {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("fox-artifact-gateway-{suffix}"));
        fs::create_dir_all(&path).expect("fixture directory");
        path
    }

    fn artifact(path: &Path) -> ArtifactRecord {
        let bytes = fs::read(path).expect("fixture file");
        ArtifactRecord {
            id: "artifact-1".to_owned(),
            conversation_id: "conversation-1".to_owned(),
            run_id: None,
            display_name: "result.md".to_owned(),
            artifact_type: "file".to_owned(),
            storage_path: path.to_string_lossy().into_owned(),
            media_type: Some("text/markdown".to_owned()),
            byte_size: bytes.len() as i64,
            sha256: Some(hex::encode(Sha256::digest(&bytes))),
            status: "ready".to_owned(),
            created_at: 1,
            updated_at: 1,
        }
    }

    #[test]
    fn resolves_only_regular_files_inside_authorized_root() {
        let root = fixture_dir();
        let file = root.join("result.md");
        fs::write(&file, "# result").expect("write fixture");
        let record = artifact(&file);
        assert_eq!(
            resolve_artifact_path(&record, &[root.canonicalize().unwrap()]).unwrap(),
            file.canonicalize().unwrap()
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_missing_and_outside_paths() {
        let root = fixture_dir();
        let placeholder = root.join("placeholder.md");
        fs::write(&placeholder, "placeholder").expect("placeholder fixture");
        let missing = root.join("missing.md");
        let mut record = artifact(&placeholder);
        record.storage_path = missing.to_string_lossy().into_owned();
        assert_eq!(
            resolve_artifact_path(&record, &[root.canonicalize().unwrap()])
                .unwrap_err()
                .code,
            "artifact.file_missing"
        );

        let outside = std::env::temp_dir().join(format!("fox-artifact-outside-{}", Uuid::new_v4()));
        fs::write(&outside, "outside").expect("outside fixture");
        record.storage_path = outside.to_string_lossy().into_owned();
        assert_eq!(
            resolve_artifact_path(&record, &[root.canonicalize().unwrap()])
                .unwrap_err()
                .code,
            "artifact.path_denied"
        );
        let _ = fs::remove_file(outside);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_directories_and_invalid_relative_paths() {
        let root = fixture_dir();
        let placeholder = root.join("placeholder.md");
        fs::write(&placeholder, "placeholder").expect("placeholder fixture");
        let mut record = artifact(&placeholder);
        record.storage_path = root.to_string_lossy().into_owned();
        assert_eq!(
            resolve_artifact_path(&record, &[root.canonicalize().unwrap()])
                .unwrap_err()
                .code,
            "artifact.not_regular_file"
        );
        record.storage_path = "relative.md".to_owned();
        assert_eq!(
            resolve_artifact_path(&record, &[root.canonicalize().unwrap()])
                .unwrap_err()
                .code,
            "artifact.path_denied"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_even_when_target_is_inside_root() {
        let root = fixture_dir();
        let target = root.join("target.md");
        let link = root.join("link.md");
        fs::write(&target, "target").expect("target fixture");
        std::os::unix::fs::symlink(&target, &link).expect("symlink fixture");
        let mut record = artifact(&target);
        record.storage_path = link.to_string_lossy().into_owned();
        assert_eq!(
            resolve_artifact_path(&record, &[root.canonicalize().unwrap()])
                .unwrap_err()
                .code,
            "artifact.path_denied"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn reports_text_html_image_pdf_and_unknown_preview_kinds() {
        let root = fixture_dir();
        for (name, media_type, expected) in [
            ("result.md", Some("text/markdown"), "markdown"),
            ("result.html", Some("text/html"), "html"),
            ("result.png", Some("image/png"), "image"),
            ("result.pdf", Some("application/pdf"), "pdf"),
            ("result.bin", Some("application/octet-stream"), "unknown"),
        ] {
            let path = root.join(name);
            let bytes: &[u8] = if expected == "markdown" {
                b"hello"
            } else {
                b"x"
            };
            fs::write(&path, bytes).expect("preview fixture");
            let mut record = artifact(&path);
            record.media_type = media_type.map(str::to_owned);
            record.display_name = name.to_owned();
            record.byte_size = 1;
            record.sha256 = None;
            let response = inspect_record(&record, &path);
            assert_eq!(response.preview.kind, expected);
        }
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_stale_size_hash_and_non_ready_records() {
        let root = fixture_dir();
        let path = root.join("result.md");
        fs::write(&path, "stable").expect("fixture file");
        let roots = vec![root.canonicalize().unwrap()];

        let mut stale_size = artifact(&path);
        stale_size.byte_size += 1;
        assert_eq!(
            validate_record(&stale_size, &roots).unwrap_err().code,
            "artifact.integrity_mismatch"
        );

        let mut stale_hash = artifact(&path);
        stale_hash.sha256 = Some("00".repeat(32));
        assert_eq!(
            validate_record(&stale_hash, &roots).unwrap_err().code,
            "artifact.integrity_mismatch"
        );

        let mut pending = artifact(&path);
        pending.status = "pending".to_owned();
        assert_eq!(
            validate_record(&pending, &roots).unwrap_err().code,
            "artifact.status_unavailable"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_files_larger_than_gateway_limit() {
        let root = fixture_dir();
        let path = root.join("large.bin");
        let placeholder = root.join("placeholder.md");
        fs::write(&placeholder, "placeholder").expect("placeholder fixture");
        let file = File::create(&path).expect("large fixture");
        file.set_len(MAX_ARTIFACT_FILE_BYTES + 1)
            .expect("sparse fixture");
        let mut record = artifact(&placeholder);
        record.storage_path = path.to_string_lossy().into_owned();
        record.display_name = "large.bin".to_owned();
        record.byte_size = (MAX_ARTIFACT_FILE_BYTES + 1) as i64;
        record.sha256 = None;
        assert_eq!(
            validate_record(&record, &[root.canonicalize().unwrap()])
                .unwrap_err()
                .code,
            "artifact.file_too_large"
        );
        let _ = fs::remove_dir_all(root);
    }
}
