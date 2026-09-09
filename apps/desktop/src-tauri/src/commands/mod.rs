mod kernel_reconciliation;
pub use kernel_reconciliation::*;
use crate::model_service::{
    api_key_configured, clear_api_key, get_api_key, normalize_model_base_url, normalize_model_id,
    set_api_key,
};
use crate::yuxi::{
    access_token_configured, clear_access_token, get_access_token, normalize_base_url,
    set_access_token,
};
use crate::{
    app_state::AppState,
    database::{
        AgentIdRequest, AgentRecord, ApiResponse, AttachmentRecord, CancelRunRequest,
        CleanupDataRequest, ConversationDetail, ConversationExpertBindRequest,
        ConversationExpertBinding, ConversationExpertBindingError, ConversationForkRequest,
        ConversationHistoryPage, ConversationHistoryRequest, ConversationIdRequest,
        ConversationSearchRequest, ConversationSummary, CopyAgentRequest,
        CreateConversationRequest, CreateMemoryRequest, DeleteKnowledgeDocumentActivityItemRequest,
        EvaluationRunSummary, GoalIdRequest, GraphQueryRequest, KnowledgeBaseRecord,
        KnowledgeBindingInput, KnowledgeDetailRecord, KnowledgeDocumentActivity,
        KnowledgeDocumentAnnotation, KnowledgeDocumentBookmark, KnowledgeDocumentDownloadRequest,
        KnowledgeDocumentDownloadResult, KnowledgeDocumentRangeRequest,
        KnowledgeDocumentReadingState, KnowledgeDocumentRequest, KnowledgeDocumentSourceMetadata,
        KnowledgeDownloadCancelRequest, KnowledgeDownloadProgress, KnowledgeIdRequest,
        KnowledgePreviewCacheAcquireRequest, KnowledgePreviewCacheCancelRequest,
        KnowledgePreviewCacheEntry, KnowledgePreviewCacheLease, KnowledgePreviewCacheLimitRequest,
        KnowledgePreviewCacheOpenRequest, KnowledgePreviewCacheReadRequest,
        KnowledgePreviewCacheReleaseRequest, KnowledgeQueryRequest, KnowledgeReference,
        KnowledgeReferenceBindingRecord, LifecycleHookIdRequest, LifecycleHookRecord,
        McpServerIdRequest, McpServerRecord, MemoryEntityRecord, MemoryIdRequest,
        MemoryListRequest, MemoryMutationRequest, MemoryRecallListRequest, MemoryRecallRecord,
        MemoryRevisionRecord, ModelConnectionTest, ModelProviderIdRequest, ModelProviderRecord,
        ModelServiceRecord, ObservabilityStatistics, OpenDownloadedFileRequest, ProjectFileEntry,
        ProjectFilePreview, ProjectFileReadRequest, ProjectFilesRequest, ProjectIdRequest,
        ProjectRecord, ProviderModelRecord, RecordUiMetricRequest, RenameConversationRequest,
        ResolveApprovalRequest, ResolveMemoryConflictRequest, RestoreBackupRequest,
        ResumeYuxiRunRequest, RewindRunRequest, RuntimeInitialization, SaveAgentRequest,
        SaveAttachmentsRequest, SaveKnowledgeDocumentAnnotationRequest,
        SaveKnowledgeDocumentBookmarkRequest, SaveKnowledgeDocumentReadingStateRequest,
        SaveLifecycleHookRequest, SaveMcpServerRequest, SaveModelProviderRequest,
        SaveModelServiceRequest, SaveUserProfileRequest, SaveYuxiServiceRequest,
        SetGoalRunningRequest, SetGoalRunningResult, SetMemoryEnabledRequest,
        SetSkillEnabledRequest, SkillRecord, StartRunRequest, StartRunResult,
        TestModelServiceRequest, TestYuxiServiceRequest, UpdateConversationPermissionRequest,
        UpdateConversationPinnedRequest, UpdateLifecycleHookEnabledRequest,
        UpdateMcpServerEnabledRequest, UpdateMemoryRequest, UpdateProjectPermissionRequest,
        UsageStatistics, UserProfileRecord, YuxiAgentRecord, YuxiConnectionTest, YuxiLoginRequest,
        YuxiModelRecord, YuxiServiceRecord, YuxiUserRecord,
    },
    runtime_host::{RuntimeDiagnostics, RuntimeStatus},
    work_mode_gate::{self, ResolveWorkModeConfirmationRequest, WorkModeDecision},
};
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Component, Path, PathBuf},
    sync::atomic::Ordering,
    time::{Duration, Instant, UNIX_EPOCH},
};
use tauri::{AppHandle, Emitter, State};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use uuid::Uuid;

pub(crate) mod plugin_center;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct KnowledgeReferenceInput {
    #[serde(flatten)]
    reference: KnowledgeReference,
    #[serde(default)]
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SetKnowledgeBindingsWireRequest {
    pub conversation_id: String,
    #[serde(default)]
    pub knowledge_bases: Option<Vec<KnowledgeBindingInput>>,
    #[serde(default)]
    pub knowledge_references: Option<Vec<KnowledgeReferenceInput>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CancelChildRunRequest {
    pub parent_conversation_id: String,
    pub child_run_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct KnowledgeBindingsSetResponse {
    bindings: Vec<crate::database::KnowledgeBindingRecord>,
    knowledge_references: Vec<KnowledgeReference>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProjectFileActionRequest {
    conversation_id: String,
    path: String,
    action: String,
    application_id: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProjectFileActionResponse {
    action: String,
    completed: bool,
    applications: Vec<crate::artifact_gateway::ArtifactApplication>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ConversationLoadResponse {
    #[serde(flatten)]
    detail: ConversationDetail,
    knowledge_references: Vec<KnowledgeReference>,
    kernel_snapshot: Option<fox_engine_protocol::KernelRunSnapshot>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvePlanRevisionRequest {
    pub conversation_id: String,
    pub plan_revision_id: String,
    pub decision: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenExternalUrlRequest {
    pub url: String,
}

fn validated_external_url(value: &str) -> Result<String, String> {
    let value = value.trim();
    let parsed = url::Url::parse(value).map_err(|_| "链接地址无效".to_owned())?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("只允许使用 HTTP 或 HTTPS 链接".to_owned());
    }
    Ok(parsed.into())
}

#[tauri::command]
pub fn external_url_open(request: OpenExternalUrlRequest) -> ApiResponse<bool> {
    let url = match validated_external_url(&request.url) {
        Ok(url) => url,
        Err(error) => return ApiResponse::failure("external_url.invalid", error, false),
    };

    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows::core::{w, PCWSTR};
        use windows::Win32::UI::Shell::ShellExecuteW;
        use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

        let wide = std::ffi::OsStr::new(&url)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        let result = unsafe {
            ShellExecuteW(
                None,
                w!("open"),
                PCWSTR(wide.as_ptr()),
                None,
                None,
                SW_SHOWNORMAL,
            )
        };
        if result.0 as isize <= 32 {
            return ApiResponse::failure(
                "external_url.open_failed",
                format!(
                    "无法调用系统默认浏览器（ShellExecuteW={}）",
                    result.0 as isize
                ),
                true,
            );
        }
        return ApiResponse::success(true);
    }

    #[cfg(not(windows))]
    ApiResponse::failure(
        "external_url.unsupported",
        "当前平台暂不支持调用系统默认浏览器",
        false,
    )
}

#[cfg(windows)]
fn pick_project_folder_windows() -> Result<Option<String>, String> {
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

#[tauri::command]
pub fn project_folder_pick() -> ApiResponse<Option<String>> {
    #[cfg(windows)]
    {
        return match std::thread::spawn(pick_project_folder_windows).join() {
            Ok(Ok(path)) => ApiResponse::success(path),
            Ok(Err(error)) => ApiResponse::failure("project.folder_picker_failed", error, true),
            Err(_) => ApiResponse::failure(
                "project.folder_picker_failed",
                "系统文件夹选择器意外退出",
                true,
            ),
        };
    }

    #[cfg(not(windows))]
    ApiResponse::failure(
        "project.folder_picker_unsupported",
        "当前平台暂不支持系统文件夹选择器",
        false,
    )
}

#[cfg(windows)]
fn pick_save_file_windows(filename: &str) -> Result<Option<PathBuf>, String> {
    use windows::core::{HRESULT, HSTRING};
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_INPROC_SERVER,
        COINIT_APARTMENTTHREADED,
    };
    use windows::Win32::UI::Shell::{
        FileSaveDialog, IFileSaveDialog, FOS_FORCEFILESYSTEM, FOS_OVERWRITEPROMPT,
        SIGDN_FILESYSPATH,
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
            .map_err(|error| format!("无法初始化系统保存对话框：{error}"))?;
        let _guard = ComGuard;
        let dialog: IFileSaveDialog = CoCreateInstance(&FileSaveDialog, None, CLSCTX_INPROC_SERVER)
            .map_err(|error| format!("无法打开系统保存对话框：{error}"))?;
        dialog
            .SetOptions(
                dialog.GetOptions().map_err(|error| error.to_string())?
                    | FOS_FORCEFILESYSTEM
                    | FOS_OVERWRITEPROMPT,
            )
            .map_err(|error| format!("无法配置系统保存对话框：{error}"))?;
        dialog
            .SetFileName(&HSTRING::from(filename))
            .map_err(|error| format!("无法设置默认文件名：{error}"))?;

        if let Err(error) = dialog.Show(None) {
            const ERROR_CANCELLED: HRESULT = HRESULT(0x800704C7u32 as i32);
            if error.code() == ERROR_CANCELLED {
                return Ok(None);
            }
            return Err(format!("系统保存对话框失败：{error}"));
        }

        let item = dialog
            .GetResult()
            .map_err(|error| format!("无法读取保存路径：{error}"))?;
        let raw_path = item
            .GetDisplayName(SIGDN_FILESYSPATH)
            .map_err(|error| format!("无法读取保存路径：{error}"))?;
        let path = raw_path
            .to_string()
            .map(PathBuf::from)
            .map_err(|error| format!("保存路径不是有效的 Unicode：{error}"));
        CoTaskMemFree(Some(raw_path.0.cast()));
        path.map(Some)
    }
}

fn safe_download_filename(value: &str, fallback: &str) -> String {
    let cleaned = value
        .chars()
        .map(|character| match character {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            character if character.is_control() => '_',
            character => character,
        })
        .collect::<String>()
        .trim_matches([' ', '.'])
        .to_owned();
    if cleaned.is_empty() {
        fallback.to_owned()
    } else {
        cleaned
    }
}

const PROJECT_FILE_LIMIT: usize = 2_000;
const PROJECT_FILE_MAX_DEPTH: usize = 8;
const PROJECT_PREVIEW_LIMIT: usize = 512 * 1024;

fn ignored_project_directory(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        ".git"
            | ".idea"
            | ".next"
            | ".nuxt"
            | ".turbo"
            | ".venv"
            | ".vscode"
            | "__pycache__"
            | "build"
            | "coverage"
            | "dist"
            | "node_modules"
            | "target"
            | "venv"
    )
}

fn project_root(state: &AppState, conversation_id: &str) -> Result<PathBuf, String> {
    let root = state
        .database
        .conversation_project_root(conversation_id)?
        .ok_or_else(|| "当前会话尚未绑定项目文件夹".to_owned())?;
    let root = PathBuf::from(root)
        .canonicalize()
        .map_err(|error| format!("无法访问项目文件夹：{error}"))?;
    if !root.is_dir() {
        return Err("当前项目文件夹不可用".to_owned());
    }
    Ok(root)
}

fn safe_project_target(root: &Path, relative: &str) -> Result<PathBuf, String> {
    let relative = Path::new(relative);
    if relative.is_absolute()
        || relative.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err("文件路径必须位于当前项目内".to_owned());
    }
    let target = root
        .join(relative)
        .canonicalize()
        .map_err(|error| format!("无法访问文件：{error}"))?;
    if !target.starts_with(root) {
        return Err("文件路径超出当前项目范围".to_owned());
    }
    Ok(target)
}

#[cfg(test)]
mod project_file_tests {
    use super::*;

    #[test]
    fn rejects_parent_and_absolute_project_paths() {
        let root = std::env::temp_dir();
        assert!(safe_project_target(&root, "../outside.txt").is_err());
        assert!(
            safe_project_target(&root, root.join("outside.txt").to_string_lossy().as_ref())
                .is_err()
        );
    }

    #[test]
    fn skips_large_generated_directories() {
        assert!(ignored_project_directory("node_modules"));
        assert!(ignored_project_directory("TARGET"));
        assert!(!ignored_project_directory("src"));
    }

    #[test]
    fn sanitizes_download_filenames_without_losing_unicode() {
        assert_eq!(
            safe_download_filename("报告:2026?.docx", "fallback"),
            "报告_2026_.docx"
        );
        assert_eq!(
            safe_download_filename(" .. ", "fallback.bin"),
            "fallback.bin"
        );
        assert_eq!(
            safe_download_filename("folder\\file.pdf", "fallback"),
            "folder_file.pdf"
        );
    }
}

fn relative_project_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn collect_project_files(
    root: &Path,
    directory: &Path,
    depth: usize,
    entries: &mut Vec<ProjectFileEntry>,
) -> Result<(), String> {
    if depth > PROJECT_FILE_MAX_DEPTH || entries.len() >= PROJECT_FILE_LIMIT {
        return Ok(());
    }
    let mut children = fs::read_dir(directory)
        .map_err(|error| format!("无法读取项目目录：{error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("无法读取项目目录项：{error}"))?;
    children.sort_by_key(|entry| entry.file_name().to_string_lossy().to_ascii_lowercase());

    for child in children {
        if entries.len() >= PROJECT_FILE_LIMIT {
            break;
        }
        let name = child.file_name().to_string_lossy().into_owned();
        let metadata = fs::symlink_metadata(child.path())
            .map_err(|error| format!("无法读取文件信息：{error}"))?;
        if metadata.file_type().is_symlink()
            || (metadata.is_dir() && ignored_project_directory(&name))
        {
            continue;
        }
        let path = child.path();
        let relative = relative_project_path(root, &path);
        let parent = path
            .parent()
            .map(|value| relative_project_path(root, value))
            .unwrap_or_default();
        let modified_at = metadata
            .modified()
            .ok()
            .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
            .map(|value| value.as_millis() as i64);
        entries.push(ProjectFileEntry {
            path: relative,
            name,
            parent,
            is_directory: metadata.is_dir(),
            byte_size: metadata.len().min(i64::MAX as u64) as i64,
            modified_at,
        });
        if metadata.is_dir() {
            collect_project_files(root, &path, depth + 1, entries)?;
        }
    }
    Ok(())
}

#[tauri::command]
pub fn usage_statistics(state: State<'_, AppState>) -> ApiResponse<UsageStatistics> {
    match state.database.usage_statistics() {
        Ok(statistics) => ApiResponse::success(statistics),
        Err(error) => storage_error(error),
    }
}

#[tauri::command]
pub fn observability_statistics(
    state: State<'_, AppState>,
) -> ApiResponse<ObservabilityStatistics> {
    match state.database.observability_statistics() {
        Ok(statistics) => ApiResponse::success(statistics),
        Err(error) => storage_error(error),
    }
}

#[tauri::command]
pub fn run_ui_metric_record(
    state: State<'_, AppState>,
    request: RecordUiMetricRequest,
) -> ApiResponse<()> {
    match state
        .database
        .record_ui_metric(&request.run_id, &request.metric, request.duration_ms)
    {
        Ok(()) => ApiResponse::success(()),
        Err(error) => ApiResponse::failure("observability.ui_metric_rejected", error, false),
    }
}

#[tauri::command]
pub fn offline_evaluation_run(state: State<'_, AppState>) -> ApiResponse<EvaluationRunSummary> {
    match state.runtime_host.run_offline_evaluations() {
        Ok(report) => ApiResponse::success(report),
        Err(error) => ApiResponse::failure("evaluation.offline_failed", error, true),
    }
}

#[tauri::command]
pub fn user_profile_get(state: State<'_, AppState>) -> ApiResponse<Option<UserProfileRecord>> {
    match state.database.user_profile() {
        Ok(profile) => ApiResponse::success(profile),
        Err(error) => storage_error(error),
    }
}

#[tauri::command]
pub fn user_profile_save(
    state: State<'_, AppState>,
    request: SaveUserProfileRequest,
) -> ApiResponse<UserProfileRecord> {
    let name = request.name.trim();
    let avatar = request.avatar.trim();
    if name.is_empty() || name.chars().count() > 32 {
        return ApiResponse::failure("profile.invalid_name", "用户名需为 1 至 32 个字符", false);
    }
    if !avatar.starts_with("/avatars/defaults/")
        || avatar.contains("..")
        || !avatar.to_ascii_lowercase().ends_with(".png")
    {
        return ApiResponse::failure("profile.invalid_avatar", "请选择 Fox 内置头像", false);
    }
    let profile = UserProfileRecord {
        name: name.to_owned(),
        avatar: avatar.to_owned(),
    };
    match state.database.save_user_profile(&profile) {
        Ok(profile) => ApiResponse::success(profile),
        Err(error) => storage_error(error),
    }
}

fn memory_error<T: Serialize>(error: String) -> ApiResponse<T> {
    let code = error
        .split_whitespace()
        .find(|part| part.starts_with("memory."))
        .unwrap_or("memory.operation_failed")
        .trim_matches(|character: char| {
            !character.is_ascii_alphanumeric() && character != '.' && character != '_'
        })
        .to_owned();
    ApiResponse::failure(&code, error, false)
}

#[tauri::command]
pub fn memories_list(
    state: State<'_, AppState>,
    request: MemoryListRequest,
) -> ApiResponse<Vec<MemoryEntityRecord>> {
    state
        .database
        .list_memories(&request)
        .map(ApiResponse::success)
        .unwrap_or_else(memory_error)
}

#[tauri::command]
pub fn memory_create(
    state: State<'_, AppState>,
    request: CreateMemoryRequest,
) -> ApiResponse<MemoryEntityRecord> {
    state
        .database
        .create_user_memory(&request)
        .map(ApiResponse::success)
        .unwrap_or_else(memory_error)
}

#[tauri::command]
pub fn memory_confirm(
    state: State<'_, AppState>,
    request: MemoryMutationRequest,
) -> ApiResponse<MemoryEntityRecord> {
    state
        .database
        .confirm_memory(&request.memory_id, request.expected_version)
        .map(ApiResponse::success)
        .unwrap_or_else(memory_error)
}

#[tauri::command]
pub fn memory_update(
    state: State<'_, AppState>,
    request: UpdateMemoryRequest,
) -> ApiResponse<MemoryEntityRecord> {
    state
        .database
        .update_memory(&request)
        .map(ApiResponse::success)
        .unwrap_or_else(memory_error)
}

#[tauri::command]
pub fn memory_set_enabled(
    state: State<'_, AppState>,
    request: SetMemoryEnabledRequest,
) -> ApiResponse<MemoryEntityRecord> {
    state
        .database
        .set_memory_enabled(&request)
        .map(ApiResponse::success)
        .unwrap_or_else(memory_error)
}

#[tauri::command]
pub fn memory_delete(
    state: State<'_, AppState>,
    request: MemoryIdRequest,
) -> ApiResponse<MemoryEntityRecord> {
    state
        .database
        .delete_memory(&request.memory_id)
        .map(ApiResponse::success)
        .unwrap_or_else(memory_error)
}

#[tauri::command]
pub fn memory_conflict_resolve(
    state: State<'_, AppState>,
    request: ResolveMemoryConflictRequest,
) -> ApiResponse<MemoryEntityRecord> {
    state
        .database
        .resolve_memory_conflict(&request)
        .map(ApiResponse::success)
        .unwrap_or_else(memory_error)
}

#[tauri::command]
pub fn memory_revisions_list(
    state: State<'_, AppState>,
    request: MemoryIdRequest,
) -> ApiResponse<Vec<MemoryRevisionRecord>> {
    state
        .database
        .list_memory_revisions(&request.memory_id)
        .map(ApiResponse::success)
        .unwrap_or_else(memory_error)
}

#[tauri::command]
pub fn memory_recalls_list(
    state: State<'_, AppState>,
    request: MemoryRecallListRequest,
) -> ApiResponse<Vec<MemoryRecallRecord>> {
    state
        .database
        .list_memory_recalls(&request.memory_id, request.limit.unwrap_or(50))
        .map(ApiResponse::success)
        .unwrap_or_else(memory_error)
}

#[tauri::command]
pub fn project_folder_open(
    state: State<'_, AppState>,
    request: ProjectFilesRequest,
) -> ApiResponse<bool> {
    let root = match project_root(&state, &request.conversation_id) {
        Ok(root) => root,
        Err(error) => return ApiResponse::failure("project.root_unavailable", error, false),
    };
    match open_project_folder(&root) {
        Ok(()) => ApiResponse::success(true),
        Err(error) => ApiResponse::failure("project.folder_open_failed", error, true),
    }
}

#[tauri::command]
pub fn project_files_list(
    state: State<'_, AppState>,
    request: ProjectFilesRequest,
) -> ApiResponse<Vec<ProjectFileEntry>> {
    let root = match project_root(&state, &request.conversation_id) {
        Ok(root) => root,
        Err(error) => return ApiResponse::failure("project.root_unavailable", error, false),
    };
    let mut entries = Vec::new();
    match collect_project_files(&root, &root, 0, &mut entries) {
        Ok(()) => ApiResponse::success(entries),
        Err(error) => ApiResponse::failure("project.files_failed", error, true),
    }
}

#[tauri::command]
pub fn project_file_read(
    state: State<'_, AppState>,
    request: ProjectFileReadRequest,
) -> ApiResponse<ProjectFilePreview> {
    let root = match project_root(&state, &request.conversation_id) {
        Ok(root) => root,
        Err(error) => return ApiResponse::failure("project.root_unavailable", error, false),
    };
    let target = match safe_project_target(&root, &request.path) {
        Ok(target) => target,
        Err(error) => return ApiResponse::failure("project.path_denied", error, false),
    };
    if !target.is_file() {
        return ApiResponse::failure("project.file_unavailable", "所选路径不是文件", false);
    }
    let bytes = match fs::read(&target) {
        Ok(bytes) => bytes,
        Err(error) => {
            return ApiResponse::failure(
                "project.file_read_failed",
                format!("无法读取文件：{error}"),
                true,
            )
        }
    };
    let truncated = bytes.len() > PROJECT_PREVIEW_LIMIT;
    let preview = &bytes[..bytes.len().min(PROJECT_PREVIEW_LIMIT)];
    if preview.contains(&0) {
        return ApiResponse::failure(
            "project.binary_preview_unsupported",
            "该文件不是可预览的文本文件",
            false,
        );
    }
    ApiResponse::success(ProjectFilePreview {
        path: relative_project_path(&root, &target),
        name: target
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
        content: String::from_utf8_lossy(preview).into_owned(),
        byte_size: bytes.len().min(i64::MAX as usize) as i64,
        truncated,
    })
}

#[tauri::command]
pub fn project_file_action(
    state: State<'_, AppState>,
    request: ProjectFileActionRequest,
) -> ApiResponse<ProjectFileActionResponse> {
    let root = match project_root(&state, &request.conversation_id) {
        Ok(root) => root,
        Err(error) => return ApiResponse::failure("project.root_unavailable", error, false),
    };
    let target = match safe_project_target(&root, &request.path) {
        Ok(target) => target,
        Err(error) => return ApiResponse::failure("project.path_denied", error, false),
    };
    if !target.is_file() {
        return ApiResponse::failure("project.file_unavailable", "所选路径不是文件", false);
    }
    match crate::artifact_gateway::system_file_action(
        &target,
        &request.action,
        request.application_id.as_deref(),
    ) {
        Ok(applications) => ApiResponse::success(ProjectFileActionResponse {
            action: request.action.trim().to_ascii_lowercase(),
            completed: true,
            applications,
        }),
        Err(error) => ApiResponse::failure(error.code, error.message, error.retryable),
    }
}

/// Inspect a persisted artifact through the Host-owned file gateway.  The
/// gateway performs the conversation-scoped database lookup and all path,
/// status and integrity checks before returning metadata or preview content.
#[tauri::command]
pub fn artifact_inspect(
    state: State<'_, AppState>,
    request: crate::artifact_gateway::ArtifactRequest,
) -> ApiResponse<crate::artifact_gateway::ArtifactInspectResponse> {
    crate::artifact_gateway::inspect_api(&state, &request)
}

/// Perform an artifact OS action (preview, open, reveal or copy path).  The
/// renderer only sends an action name; it never supplies a path to the Host.
#[tauri::command]
pub fn artifact_action(
    state: State<'_, AppState>,
    request: crate::artifact_gateway::ArtifactActionRequest,
) -> ApiResponse<crate::artifact_gateway::ArtifactActionResponse> {
    crate::artifact_gateway::action_api(&state, &request)
}

fn normalize_project_root(value: Option<&str>) -> Result<Option<String>, String> {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    let path = std::path::Path::new(value)
        .canonicalize()
        .map_err(|error| format!("无法访问项目文件夹：{error}"))?;
    if !path.is_dir() {
        return Err("项目路径必须是文件夹".to_owned());
    }
    Ok(Some(path.to_string_lossy().into_owned()))
}

#[tauri::command]
pub fn runtime_initialize(state: State<'_, AppState>) -> ApiResponse<RuntimeInitialization> {
    ApiResponse::success(RuntimeInitialization {
        database_ready: true,
        runtime_available: state.runtime_host.runtime_available(),
        default_agent_id: state.database.default_agent_id().to_owned(),
    })
}

#[tauri::command]
pub fn runtime_status(state: State<'_, AppState>) -> ApiResponse<RuntimeStatus> {
    ApiResponse::success(state.runtime_host.status())
}

#[tauri::command]
pub fn runtime_diagnostics(state: State<'_, AppState>) -> ApiResponse<RuntimeDiagnostics> {
    ApiResponse::success(state.runtime_host.diagnostics())
}

#[tauri::command]
pub fn diagnostics_export(
    state: State<'_, AppState>,
) -> ApiResponse<crate::maintenance::MaintenanceResult> {
    crate::maintenance::create_diagnostics(&state)
        .map(ApiResponse::success)
        .unwrap_or_else(|error| ApiResponse::failure("diagnostics.export_failed", error, true))
}

#[tauri::command]
pub fn diagnose_work_state(
    state: State<'_, AppState>,
    request: crate::work_diagnostics::WorkStateRequest,
) -> ApiResponse<crate::work_diagnostics::WorkStateDiagnosticReport> {
    crate::work_diagnostics::diagnose_work_state(&state.database, &request.conversation_id)
        .map(ApiResponse::success)
        .unwrap_or_else(|error| ApiResponse::failure("work_state.diagnosis_failed", error, false))
}

#[tauri::command]
pub fn export_work_trace(
    state: State<'_, AppState>,
    request: crate::work_diagnostics::WorkStateRequest,
) -> ApiResponse<crate::maintenance::MaintenanceResult> {
    crate::work_diagnostics::export_work_trace(
        &state.database,
        &state.data_dir,
        &request.conversation_id,
    )
    .map(ApiResponse::success)
    .unwrap_or_else(|error| ApiResponse::failure("work_trace.export_failed", error, false))
}

#[tauri::command]
pub fn backup_create(
    state: State<'_, AppState>,
) -> ApiResponse<crate::maintenance::MaintenanceResult> {
    crate::maintenance::create_backup(&state)
        .map(ApiResponse::success)
        .unwrap_or_else(|error| ApiResponse::failure("backup.create_failed", error, true))
}

#[tauri::command]
pub fn backup_restore(
    state: State<'_, AppState>,
    request: RestoreBackupRequest,
) -> ApiResponse<crate::maintenance::MaintenanceResult> {
    let path = std::path::PathBuf::from(request.backup_path.trim());
    if !path.is_file() {
        return ApiResponse::failure("backup.not_found", "备份文件不存在", false);
    }
    crate::maintenance::queue_restore(&state.data_dir, &path)
        .map(ApiResponse::success)
        .unwrap_or_else(|error| ApiResponse::failure("backup.restore_failed", error, false))
}

#[tauri::command]
pub fn data_cleanup(
    state: State<'_, AppState>,
    request: CleanupDataRequest,
) -> ApiResponse<crate::maintenance::MaintenanceResult> {
    crate::maintenance::cleanup(&state, request.runtime_session_days.unwrap_or(30))
        .map(ApiResponse::success)
        .unwrap_or_else(|error| ApiResponse::failure("maintenance.cleanup_failed", error, true))
}

#[tauri::command]
pub fn knowledge_preview_cache_statistics(
    state: State<'_, AppState>,
) -> ApiResponse<crate::database::KnowledgePreviewCacheStatistics> {
    let result = crate::maintenance::configured_knowledge_preview_cache_limit(&state.database)
        .and_then(|limit| state.database.knowledge_preview_cache_statistics(limit));
    result.map(ApiResponse::success).unwrap_or_else(|error| {
        ApiResponse::failure("knowledge.preview_cache_statistics_failed", error, true)
    })
}

#[tauri::command]
pub fn knowledge_preview_cache_limit_set(
    state: State<'_, AppState>,
    request: KnowledgePreviewCacheLimitRequest,
) -> ApiResponse<crate::database::KnowledgePreviewCacheStatistics> {
    if !(crate::maintenance::KNOWLEDGE_PREVIEW_CACHE_LIMIT_MIN
        ..=crate::maintenance::KNOWLEDGE_PREVIEW_CACHE_LIMIT_MAX)
        .contains(&request.limit_bytes)
    {
        return ApiResponse::failure(
            "knowledge.preview_cache_limit_invalid",
            "预览缓存上限必须在 250 MB 到 5 GB 之间",
            false,
        );
    }
    let result = state
        .database
        .set_knowledge_preview_cache_limit(request.limit_bytes)
        .and_then(|_| crate::maintenance::cleanup_knowledge_preview_cache(&state))
        .and_then(|_| {
            state
                .database
                .knowledge_preview_cache_statistics(request.limit_bytes)
        });
    result.map(ApiResponse::success).unwrap_or_else(|error| {
        ApiResponse::failure("knowledge.preview_cache_limit_save_failed", error, true)
    })
}

#[tauri::command]
pub fn knowledge_preview_cache_clear(
    state: State<'_, AppState>,
) -> ApiResponse<crate::maintenance::MaintenanceResult> {
    crate::maintenance::clear_unused_knowledge_preview_cache(&state)
        .map(ApiResponse::success)
        .unwrap_or_else(|error| {
            ApiResponse::failure("knowledge.preview_cache_clear_failed", error, true)
        })
}

#[tauri::command]
pub fn agents_list(state: State<'_, AppState>) -> ApiResponse<Vec<AgentRecord>> {
    match state.database.list_agents() {
        Ok(agents) => ApiResponse::success(agents),
        Err(error) => storage_error(error),
    }
}

#[tauri::command]
pub fn agent_save(
    state: State<'_, AppState>,
    request: SaveAgentRequest,
) -> ApiResponse<AgentRecord> {
    state
        .database
        .save_agent(&request)
        .map(ApiResponse::success)
        .unwrap_or_else(storage_error)
}

#[tauri::command]
pub fn agent_copy(
    state: State<'_, AppState>,
    request: CopyAgentRequest,
) -> ApiResponse<AgentRecord> {
    state
        .database
        .copy_agent(&request.agent_id, request.name.as_deref())
        .map(ApiResponse::success)
        .unwrap_or_else(storage_error)
}

#[tauri::command]
pub fn agent_delete(state: State<'_, AppState>, request: AgentIdRequest) -> ApiResponse<bool> {
    state
        .database
        .delete_agent(&request.agent_id)
        .map(ApiResponse::success)
        .unwrap_or_else(storage_error)
}

#[tauri::command]
pub fn skills_list(
    state: State<'_, AppState>,
    request: AgentIdRequest,
) -> ApiResponse<Vec<SkillRecord>> {
    let enabled = match state.database.enabled_agent_skills(&request.agent_id) {
        Ok(value) => value,
        Err(error) => return storage_error(error),
    };
    match crate::skills::scan_skills(&state.skills_dir, &enabled) {
        Ok(skills) => ApiResponse::success(skills.into_iter().map(|skill| skill.record).collect()),
        Err(error) => ApiResponse::failure("skills.scan_failed", error, true),
    }
}

#[tauri::command]
pub fn skill_set_enabled(
    state: State<'_, AppState>,
    request: SetSkillEnabledRequest,
) -> ApiResponse<Vec<SkillRecord>> {
    let enabled = match state.database.enabled_agent_skills(&request.agent_id) {
        Ok(value) => value,
        Err(error) => return storage_error(error),
    };
    let skills = match crate::skills::scan_skills(&state.skills_dir, &enabled) {
        Ok(value) => value,
        Err(error) => return ApiResponse::failure("skills.scan_failed", error, true),
    };
    let Some(skill) = skills
        .iter()
        .find(|skill| skill.record.id == request.skill_id)
    else {
        return ApiResponse::failure("skills.not_found", "Skill 不存在", false);
    };
    if request.enabled && !skill.record.valid {
        return ApiResponse::failure(
            "skills.invalid",
            skill
                .record
                .validation_error
                .clone()
                .unwrap_or_else(|| "Skill 校验失败".to_owned()),
            false,
        );
    }
    let enabled = match state.database.set_agent_skill_enabled(
        &request.agent_id,
        &request.skill_id,
        request.enabled,
    ) {
        Ok(value) => value,
        Err(error) => return storage_error(error),
    };
    match crate::skills::scan_skills(&state.skills_dir, &enabled) {
        Ok(skills) => ApiResponse::success(skills.into_iter().map(|skill| skill.record).collect()),
        Err(error) => ApiResponse::failure("skills.scan_failed", error, true),
    }
}

#[tauri::command]
pub fn mcp_servers_list(state: State<'_, AppState>) -> ApiResponse<Vec<McpServerRecord>> {
    match state.database.list_mcp_servers() {
        Ok(mut servers) => {
            for server in &mut servers {
                server.credential_configured = crate::mcp::environment_configured(&server.id);
            }
            ApiResponse::success(servers)
        }
        Err(error) => storage_error(error),
    }
}

#[tauri::command]
pub fn mcp_server_save(
    state: State<'_, AppState>,
    request: SaveMcpServerRequest,
) -> ApiResponse<McpServerRecord> {
    let name = request.name.trim();
    let command = request.command.trim();
    let transport = request.transport.trim();
    if name.is_empty() {
        return ApiResponse::failure("mcp.invalid_config", "名称不能为空", false);
    }
    if !matches!(transport, "stdio" | "streamable_http" | "openapi") {
        return ApiResponse::failure("mcp.invalid_transport", "扩展传输类型无效", false);
    }
    if transport == "stdio" && command.is_empty() {
        return ApiResponse::failure("mcp.invalid_config", "stdio MCP 的命令不能为空", false);
    }
    let endpoint_url = request
        .endpoint_url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if transport == "streamable_http" && endpoint_url.is_none() {
        return ApiResponse::failure("mcp.invalid_endpoint", "HTTP MCP 的端点不能为空", false);
    }
    if let Some(endpoint_url) = endpoint_url {
        if let Err(error) = crate::mcp::parse_endpoint(Some(endpoint_url), "扩展源") {
            return ApiResponse::failure("mcp.invalid_endpoint", error, false);
        }
    }
    let definition = request
        .definition
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if transport == "openapi" && definition.is_none() {
        return ApiResponse::failure("mcp.invalid_openapi", "OpenAPI 定义不能为空", false);
    }
    if definition.is_some_and(|value| value.len() > 2 * 1024 * 1024) {
        return ApiResponse::failure("mcp.invalid_openapi", "OpenAPI 定义超过 2 MB 上限", false);
    }
    if request.args.len() > 64 || request.args.iter().any(|arg| arg.len() > 4096) {
        return ApiResponse::failure("mcp.invalid_args", "MCP 参数数量或长度超过限制", false);
    }
    let id = request
        .id
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    if request.clear_environment {
        if let Err(error) = crate::mcp::clear_environment(&id) {
            return ApiResponse::failure("mcp.credential_delete_failed", error, true);
        }
    } else if !request.environment.is_empty() {
        if request.environment.len() > 64
            || request
                .environment
                .iter()
                .any(|(key, value)| key.trim().is_empty() || key.len() > 128 || value.len() > 8192)
        {
            return ApiResponse::failure("mcp.invalid_environment", "MCP 环境变量超过限制", false);
        }
        if let Err(error) = crate::mcp::save_environment(&id, &request.environment) {
            return ApiResponse::failure("mcp.credential_save_failed", error, true);
        }
    }
    crate::mcp::invalidate_connection(&id);
    match state.database.save_mcp_server(
        &id,
        name,
        if transport == "stdio" { command } else { "" },
        if transport == "stdio" {
            &request.args
        } else {
            &[]
        },
        transport,
        endpoint_url,
        definition,
    ) {
        Ok(mut server) => {
            server.credential_configured = crate::mcp::environment_configured(&server.id);
            ApiResponse::success(server)
        }
        Err(error) => storage_error(error),
    }
}

#[tauri::command]
pub fn mcp_server_test(
    state: State<'_, AppState>,
    request: McpServerIdRequest,
) -> ApiResponse<Value> {
    let server = match state.database.get_mcp_server(&request.server_id) {
        Ok(Some(server)) => server,
        Ok(None) => return ApiResponse::failure("mcp.not_found", "MCP Server 不存在", false),
        Err(error) => return storage_error(error),
    };
    let started = std::time::Instant::now();
    match crate::mcp::list_tools(&server) {
        Ok(tools) => {
            let _ = state.database.record_mcp_health(
                &server.id,
                "connected",
                None,
                Some(started.elapsed().as_millis() as i64),
                Some(tools.len() as i64),
            );
            ApiResponse::success(json!({
                "toolCount": tools.len(),
                "tools": tools,
                "latencyMs": started.elapsed().as_millis() as i64,
                "transport": server.transport,
            }))
        }
        Err(error) => {
            let _ = state.database.record_mcp_health(
                &server.id,
                "unavailable",
                Some(&error),
                Some(started.elapsed().as_millis() as i64),
                None,
            );
            ApiResponse::failure("mcp.connection_failed", error, true)
        }
    }
}

#[tauri::command]
pub fn mcp_server_set_enabled(
    state: State<'_, AppState>,
    request: UpdateMcpServerEnabledRequest,
) -> ApiResponse<McpServerRecord> {
    match state
        .database
        .set_mcp_server_enabled(&request.server_id, request.enabled)
    {
        Ok(Some(mut server)) => {
            crate::mcp::invalidate_connection(&server.id);
            server.credential_configured = crate::mcp::environment_configured(&server.id);
            ApiResponse::success(server)
        }
        Ok(None) => ApiResponse::failure("mcp.not_found", "MCP Server 不存在", false),
        Err(error) => storage_error(error),
    }
}

#[tauri::command]
pub fn mcp_server_delete(
    state: State<'_, AppState>,
    request: McpServerIdRequest,
) -> ApiResponse<bool> {
    let _ = crate::mcp::clear_environment(&request.server_id);
    crate::mcp::invalidate_connection(&request.server_id);
    match state.database.delete_mcp_server(&request.server_id) {
        Ok(deleted) => ApiResponse::success(deleted),
        Err(error) => storage_error(error),
    }
}

#[tauri::command]
pub fn lifecycle_hooks_list(state: State<'_, AppState>) -> ApiResponse<Vec<LifecycleHookRecord>> {
    match state.database.list_lifecycle_hooks() {
        Ok(hooks) => ApiResponse::success(hooks),
        Err(error) => storage_error(error),
    }
}

#[tauri::command]
pub fn lifecycle_hook_save(
    state: State<'_, AppState>,
    request: SaveLifecycleHookRequest,
) -> ApiResponse<LifecycleHookRecord> {
    let name = request.name.trim();
    let matcher = request.matcher.trim();
    let reason = request.reason.trim();
    if name.is_empty() || matcher.is_empty() {
        return ApiResponse::failure("hook.invalid", "Hook 名称和匹配器不能为空", false);
    }
    if !matches!(
        request.event.as_str(),
        "before_run" | "before_tool" | "after_tool" | "after_run"
    ) {
        return ApiResponse::failure("hook.invalid_event", "Hook 事件无效", false);
    }
    if !matches!(
        request.action.as_str(),
        "block" | "require_approval" | "annotate"
    ) {
        return ApiResponse::failure("hook.invalid_action", "Hook 动作无效", false);
    }
    if request.event != "before_tool"
        && matches!(request.action.as_str(), "block" | "require_approval")
    {
        return ApiResponse::failure(
            "hook.invalid_action",
            "block 和 require_approval 仅可用于 before_tool",
            false,
        );
    }
    if matcher.len() > 512 || reason.len() > 4096 || !(0..=1000).contains(&request.priority) {
        return ApiResponse::failure("hook.invalid", "Hook 配置超过限制", false);
    }
    let id = request
        .id
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    match state.database.save_lifecycle_hook(
        &id,
        name,
        &request.event,
        matcher,
        &request.action,
        reason,
        request.enabled,
        request.priority,
    ) {
        Ok(hook) => ApiResponse::success(hook),
        Err(error) => storage_error(error),
    }
}

#[tauri::command]
pub fn lifecycle_hook_set_enabled(
    state: State<'_, AppState>,
    request: UpdateLifecycleHookEnabledRequest,
) -> ApiResponse<LifecycleHookRecord> {
    match state
        .database
        .set_lifecycle_hook_enabled(&request.hook_id, request.enabled)
    {
        Ok(Some(hook)) => ApiResponse::success(hook),
        Ok(None) => ApiResponse::failure("hook.not_found", "Hook 不存在", false),
        Err(error) => storage_error(error),
    }
}

#[tauri::command]
pub fn lifecycle_hook_delete(
    state: State<'_, AppState>,
    request: LifecycleHookIdRequest,
) -> ApiResponse<bool> {
    match state.database.delete_lifecycle_hook(&request.hook_id) {
        Ok(deleted) => ApiResponse::success(deleted),
        Err(error) => storage_error(error),
    }
}

#[tauri::command]
pub fn conversations_list(state: State<'_, AppState>) -> ApiResponse<Vec<ConversationSummary>> {
    match state.database.list_conversations() {
        Ok(conversations) => ApiResponse::success(conversations),
        Err(error) => storage_error(error),
    }
}

#[tauri::command]
pub fn conversations_archived_list(
    state: State<'_, AppState>,
) -> ApiResponse<Vec<ConversationSummary>> {
    match state.database.list_archived_conversations() {
        Ok(conversations) => ApiResponse::success(conversations),
        Err(error) => storage_error(error),
    }
}

#[tauri::command]
pub fn conversations_trashed_list(
    state: State<'_, AppState>,
) -> ApiResponse<Vec<ConversationSummary>> {
    match state.database.list_trashed_conversations() {
        Ok(conversations) => ApiResponse::success(conversations),
        Err(error) => storage_error(error),
    }
}

#[tauri::command]
pub fn projects_list(state: State<'_, AppState>) -> ApiResponse<Vec<ProjectRecord>> {
    match state.database.list_projects() {
        Ok(projects) => ApiResponse::success(projects),
        Err(error) => storage_error(error),
    }
}

#[tauri::command]
pub fn project_permission_update(
    state: State<'_, AppState>,
    request: UpdateProjectPermissionRequest,
) -> ApiResponse<ProjectRecord> {
    match state
        .database
        .update_project_permission_mode(&request.project_id, &request.permission_mode)
    {
        Ok(Some(project)) => ApiResponse::success(project),
        Ok(None) => ApiResponse::failure("project.not_found", "未找到项目", false),
        Err(error) => ApiResponse::failure("project.invalid_permission_mode", error, false),
    }
}

#[tauri::command]
pub fn conversation_permission_update(
    state: State<'_, AppState>,
    request: UpdateConversationPermissionRequest,
) -> ApiResponse<ConversationSummary> {
    match state
        .database
        .update_conversation_permission_mode(&request.conversation_id, &request.permission_mode)
    {
        Ok(Some(conversation)) => ApiResponse::success(conversation),
        Ok(None) => ApiResponse::failure("conversation.not_found", "未找到对话", false),
        Err(error) => ApiResponse::failure("conversation.invalid_permission_mode", error, false),
    }
}

#[tauri::command]
pub fn project_delete(state: State<'_, AppState>, request: ProjectIdRequest) -> ApiResponse<bool> {
    match state.database.delete_project(&request.project_id) {
        Ok(deleted) => ApiResponse::success(deleted),
        Err(error) => storage_error(error),
    }
}

#[tauri::command]
pub async fn conversation_create(
    state: State<'_, AppState>,
    request: CreateConversationRequest,
) -> Result<ApiResponse<ConversationSummary>, String> {
    let project_root = match normalize_project_root(request.project_root.as_deref()) {
        Ok(project_root) => project_root,
        Err(error) => {
            return Ok(ApiResponse::failure(
                "conversation.invalid_project_root",
                error,
                false,
            ))
        }
    };
    let permission_mode = request.permission_mode.as_deref().unwrap_or("read_only");
    if !matches!(permission_mode, "read_only" | "ask" | "allow") {
        return Ok(ApiResponse::failure(
            "project.invalid_permission_mode",
            "项目权限模式不合法",
            false,
        ));
    }
    match state.database.agent_kind(&request.agent_id) {
        Ok(Some(kind)) if kind == "assistant" => {}
        Ok(Some(_)) => {
            return Ok(ApiResponse::failure(
                "conversation.assistant_required",
                "基础会话必须选择助手类型智能体",
                false,
            ))
        }
        Ok(None) => {
            return Ok(ApiResponse::failure(
                "agent.not_found",
                "未找到智能体",
                false,
            ))
        }
        Err(error) => return Ok(storage_error(error)),
    }
    if let Some(expert_id) = request.expert_id.as_deref() {
        match state.database.get_agent(expert_id) {
            Ok(Some(expert))
                if expert.agent_kind == "expert"
                    && expert.invocation_mode == "inline"
                    && expert.visibility == "expert_center"
                    && (expert.runtime_type != "yuxi" || expert.available) => {}
            Ok(Some(expert))
                if expert.agent_kind != "expert"
                    || expert.invocation_mode != "inline"
                    || expert.visibility != "expert_center" =>
            {
                return Ok(ApiResponse::failure(
                    "conversation.expert_invalid_role",
                    "会话专家必须选择可内联调用的专家类型智能体",
                    false,
                ))
            }
            Ok(Some(_)) => {
                return Ok(ApiResponse::failure(
                    "conversation.expert_remote_unavailable",
                    "远程专家当前不可用",
                    true,
                ))
            }
            Ok(None) => {
                return Ok(ApiResponse::failure(
                    "expert.not_found",
                    "未找到专家",
                    false,
                ))
            }
            Err(error) => return Ok(storage_error(error)),
        }
    }
    let runtime = match state.database.agent_runtime(&request.agent_id) {
        Ok(Some(runtime)) => runtime,
        Ok(None) => {
            return Ok(ApiResponse::failure(
                "agent.not_found",
                "未找到智能体",
                false,
            ))
        }
        Err(error) => return Ok(storage_error(error)),
    };
    let conversation = match state.database.create_conversation(
        &request.agent_id,
        request.title.as_deref(),
        project_root.as_deref(),
        Some(permission_mode),
    ) {
        Ok(conversation) => conversation,
        Err(error) => {
            return Ok(ApiResponse::failure(
                "conversation.create_failed",
                error,
                false,
            ))
        }
    };
    if let Some(expert_id) = request.expert_id.as_deref() {
        if let Err(error) = state.database.bind_conversation_expert(
            &conversation.id,
            expert_id,
            "conversation_create",
        ) {
            let _ = state.database.delete_conversation(&conversation.id);
            return Ok(conversation_expert_error(error));
        }
    }
    if runtime.0 == "yuxi" {
        let Some(remote_agent_id) = runtime.1 else {
            let _ = state.database.delete_conversation(&conversation.id);
            return Ok(ApiResponse::failure(
                "yuxi.agent_mapping_missing",
                "远程智能体映射缺失",
                false,
            ));
        };
        let service = match state.database.get_yuxi_service() {
            Ok(Some(service)) => service,
            Ok(None) => {
                let _ = state.database.delete_conversation(&conversation.id);
                return Ok(ApiResponse::failure(
                    "yuxi.not_configured",
                    "请先配置知识库服务",
                    false,
                ));
            }
            Err(error) => return Ok(storage_error(error)),
        };
        let Some(token) = get_access_token(&service.base_url) else {
            let _ = state.database.delete_conversation(&conversation.id);
            return Ok(ApiResponse::failure(
                "yuxi.not_authenticated",
                "请先登录知识库",
                false,
            ));
        };
        let remote_thread = match state
            .yuxi_client
            .create_thread(
                &service.base_url,
                &token,
                &remote_agent_id,
                &conversation.title,
            )
            .await
        {
            Ok(thread) => thread,
            Err(error) => {
                let _ = state.database.delete_conversation(&conversation.id);
                return Ok(ApiResponse::failure(
                    "yuxi.thread_create_failed",
                    error,
                    true,
                ));
            }
        };
        if let Err(error) = state.database.ensure_external_runtime_session(
            &conversation.id,
            "yuxi",
            &remote_thread,
            None,
        ) {
            let _ = state.database.delete_conversation(&conversation.id);
            return Ok(storage_error(error));
        }
    }
    Ok(ApiResponse::success(conversation))
}

#[tauri::command]
pub fn conversation_expert_bind(
    state: State<'_, AppState>,
    request: ConversationExpertBindRequest,
) -> ApiResponse<ConversationExpertBinding> {
    match state.database.bind_conversation_expert(
        &request.conversation_id,
        &request.expert_id,
        "manual",
    ) {
        Ok(binding) => ApiResponse::success(binding),
        Err(error) => conversation_expert_error(error),
    }
}

#[tauri::command]
pub fn conversation_expert_remove(
    state: State<'_, AppState>,
    request: ConversationIdRequest,
) -> ApiResponse<ConversationExpertBinding> {
    match state
        .database
        .remove_conversation_expert(&request.conversation_id)
    {
        Ok(Some(binding)) => ApiResponse::success(binding),
        Ok(None) => ApiResponse::failure(
            "conversation.expert_not_bound",
            "当前会话没有活动专家",
            false,
        ),
        Err(error) => conversation_expert_error(error),
    }
}

#[tauri::command]
pub async fn conversation_rename(
    state: State<'_, AppState>,
    request: RenameConversationRequest,
) -> Result<ApiResponse<ConversationSummary>, String> {
    let title = request.title.trim();
    if title.is_empty() {
        return Ok(ApiResponse::failure(
            "conversation.invalid_title",
            "对话标题不能为空",
            false,
        ));
    }
    let runtime = match state
        .database
        .conversation_runtime(&request.conversation_id)
    {
        Ok(runtime) => runtime,
        Err(error) => return Ok(storage_error(error)),
    };
    if runtime.runtime_type == "yuxi" {
        let service = match state.database.get_yuxi_service() {
            Ok(Some(value)) => value,
            Ok(None) => {
                return Ok(ApiResponse::failure(
                    "yuxi.not_configured",
                    "请先配置知识库服务",
                    false,
                ))
            }
            Err(error) => return Ok(storage_error(error)),
        };
        let token = match get_access_token(&service.base_url) {
            Some(value) => value,
            None => {
                return Ok(ApiResponse::failure(
                    "yuxi.not_authenticated",
                    "请先登录知识库",
                    false,
                ))
            }
        };
        let thread_id = match runtime.remote_thread_id {
            Some(value) => value,
            None => {
                return Ok(ApiResponse::failure(
                    "yuxi.thread_missing",
                    "远程会话映射缺失",
                    false,
                ))
            }
        };
        if let Err(error) = state
            .yuxi_client
            .rename_thread(&service.base_url, &token, &thread_id, title)
            .await
        {
            return Ok(ApiResponse::failure(
                "yuxi.thread_rename_failed",
                error,
                true,
            ));
        }
    }
    match state
        .database
        .rename_conversation(&request.conversation_id, title)
    {
        Ok(Some(conversation)) => Ok(ApiResponse::success(conversation)),
        Ok(None) => Ok(ApiResponse::failure(
            "conversation.not_found",
            "未找到对话",
            false,
        )),
        Err(error) => Ok(storage_error(error)),
    }
}

#[tauri::command]
pub fn conversation_pin(
    state: State<'_, AppState>,
    request: UpdateConversationPinnedRequest,
) -> ApiResponse<ConversationSummary> {
    match state
        .database
        .set_conversation_pinned(&request.conversation_id, request.pinned)
    {
        Ok(Some(conversation)) => ApiResponse::success(conversation),
        Ok(None) => ApiResponse::failure("conversation.not_found", "未找到对话", false),
        Err(error) => storage_error(error),
    }
}

#[tauri::command]
pub fn conversation_archive(
    state: State<'_, AppState>,
    request: ConversationIdRequest,
) -> ApiResponse<ConversationSummary> {
    match state
        .database
        .conversation_has_active_run(&request.conversation_id)
    {
        Ok(true) => {
            return ApiResponse::failure(
                "conversation.run_active",
                "请先停止当前生成，再归档对话",
                false,
            )
        }
        Ok(false) => {}
        Err(error) => return storage_error(error),
    }
    match state
        .database
        .archive_conversation(&request.conversation_id)
    {
        Ok(Some(conversation)) => ApiResponse::success(conversation),
        Ok(None) => ApiResponse::failure("conversation.not_found", "未找到对话", false),
        Err(error) => storage_error(error),
    }
}

#[tauri::command]
pub fn conversation_unarchive(
    state: State<'_, AppState>,
    request: ConversationIdRequest,
) -> ApiResponse<ConversationSummary> {
    match state
        .database
        .unarchive_conversation(&request.conversation_id)
    {
        Ok(Some(conversation)) => ApiResponse::success(conversation),
        Ok(None) => ApiResponse::failure("conversation.not_found", "未找到可恢复的归档对话", false),
        Err(error) => storage_error(error),
    }
}

#[tauri::command]
pub fn conversation_restore(
    state: State<'_, AppState>,
    request: ConversationIdRequest,
) -> ApiResponse<ConversationSummary> {
    match state
        .database
        .restore_trashed_conversation(&request.conversation_id)
    {
        Ok(Some(conversation)) => ApiResponse::success(conversation),
        Ok(None) => {
            ApiResponse::failure("conversation.not_found", "未找到可恢复的回收站对话", false)
        }
        Err(error) => storage_error(error),
    }
}

#[tauri::command]
pub fn conversation_fork(
    state: State<'_, AppState>,
    request: ConversationForkRequest,
) -> ApiResponse<ConversationSummary> {
    match state
        .database
        .conversation_has_active_run(&request.conversation_id)
    {
        Ok(true) => {
            return ApiResponse::failure(
                "conversation.run_active",
                "请等待当前生成结束，再从消息创建分支",
                false,
            )
        }
        Ok(false) => {}
        Err(error) => return storage_error(error),
    }
    match state
        .database
        .conversation_runtime(&request.conversation_id)
    {
        Ok(runtime) if runtime.runtime_type == "yuxi" => {
            return ApiResponse::failure(
                "conversation.fork_remote_unsupported",
                "远程会话暂不支持可靠复制上下文，请在本地助手会话中使用 Fork",
                false,
            )
        }
        Ok(_) => {}
        Err(error) => return ApiResponse::failure("conversation.not_found", error, false),
    }
    match state.database.fork_conversation(
        &request.conversation_id,
        &request.message_id,
        request.title.as_deref(),
    ) {
        Ok(Some(conversation)) => ApiResponse::success(conversation),
        Ok(None) => ApiResponse::failure(
            "conversation.fork_point_not_found",
            "未找到指定的会话或消息",
            false,
        ),
        Err(error) => ApiResponse::failure("conversation.fork_failed", error, false),
    }
}

#[tauri::command]
pub fn conversation_load(
    state: State<'_, AppState>,
    request: ConversationIdRequest,
) -> ApiResponse<ConversationLoadResponse> {
    // Loading a Yuxi conversation is also a reconciliation point. This lets a
    // run that completed remotely after a transient Worker/SSE outage replay
    // its missing events into Fox without requiring a new login or app restart.
    state
        .yuxi_runtime
        .recover_conversation_detached(&request.conversation_id);
    match state.database.load_conversation(&request.conversation_id) {
        Ok(detail) => {
            let kernel_snapshot = match detail.last_run.as_ref() {
                Some(run) => match state.database.kernel_conversation_snapshot(
                    &request.conversation_id, &run.id,
                ) {
                    Ok(snapshot) => snapshot,
                    Err(error) => return storage_error(error),
                },
                None => None,
            };
            match state
                .database
                .conversation_knowledge_references(&request.conversation_id)
            {
                Ok(knowledge_references) => ApiResponse::success(ConversationLoadResponse {
                    detail,
                    knowledge_references,
                    kernel_snapshot,
                }),
                Err(error) => storage_error(error),
            }
        }
        Err(error) => ApiResponse::failure("conversation.not_found", error, false),
    }
}

#[tauri::command]
pub fn conversation_history(
    state: State<'_, AppState>,
    request: ConversationHistoryRequest,
) -> ApiResponse<ConversationHistoryPage> {
    match state.database.load_conversation_history(
        &request.conversation_id,
        request.before_ordinal,
        request.limit.unwrap_or(100),
    ) {
        Ok(page) => ApiResponse::success(page),
        Err(error) => ApiResponse::failure("conversation.history_failed", error, true),
    }
}

#[tauri::command]
pub fn conversations_search(
    state: State<'_, AppState>,
    request: ConversationSearchRequest,
) -> ApiResponse<Vec<ConversationSummary>> {
    match state
        .database
        .search_conversations(&request.query, request.limit.unwrap_or(50))
    {
        Ok(records) => ApiResponse::success(records),
        Err(error) => ApiResponse::failure("conversation.search_failed", error, true),
    }
}

#[tauri::command]
pub async fn conversation_delete(
    state: State<'_, AppState>,
    request: ConversationIdRequest,
) -> Result<ApiResponse<bool>, String> {
    match state
        .database
        .conversation_has_active_run(&request.conversation_id)
    {
        Ok(true) => {
            return Ok(ApiResponse::failure(
                "conversation.run_active",
                "请先停止当前生成，再删除对话",
                false,
            ))
        }
        Ok(false) => {}
        Err(error) => return Ok(storage_error(error)),
    }
    match state.database.trash_conversation(&request.conversation_id) {
        Ok(Some(_)) => Ok(ApiResponse::success(true)),
        Ok(None) => Ok(ApiResponse::failure(
            "conversation.not_found",
            "未找到可移入回收站的对话",
            false,
        )),
        Err(error) => Ok(storage_error(error)),
    }
}

#[tauri::command]
pub async fn conversation_purge(
    state: State<'_, AppState>,
    request: ConversationIdRequest,
) -> Result<ApiResponse<bool>, String> {
    match state
        .database
        .conversation_is_trashed(&request.conversation_id)
    {
        Ok(true) => {}
        Ok(false) => {
            return Ok(ApiResponse::failure(
                "conversation.purge_requires_trash",
                "请先将对话移入回收站，再执行永久删除",
                false,
            ))
        }
        Err(error) => return Ok(ApiResponse::failure("conversation.not_found", error, false)),
    }
    match state
        .database
        .conversation_has_active_run(&request.conversation_id)
    {
        Ok(true) => {
            return Ok(ApiResponse::failure(
                "conversation.run_active",
                "请先停止当前生成，再永久删除对话",
                false,
            ))
        }
        Ok(false) => {}
        Err(error) => return Ok(storage_error(error)),
    }
    if let Ok(runtime) = state
        .database
        .conversation_runtime(&request.conversation_id)
    {
        if runtime.runtime_type == "yuxi" {
            if let (Ok(Some(service)), Some(thread_id)) =
                (state.database.get_yuxi_service(), runtime.remote_thread_id)
            {
                if let Some(token) = get_access_token(&service.base_url) {
                    if let Err(error) = state
                        .yuxi_client
                        .delete_thread(&service.base_url, &token, &thread_id)
                        .await
                    {
                        return Ok(ApiResponse::failure(
                            "yuxi.thread_delete_failed",
                            error,
                            true,
                        ));
                    }
                }
            }
        } else if let Err(error) = state
            .runtime_host
            .remove_conversation(&request.conversation_id)
        {
            return Ok(ApiResponse::failure("runtime.stop_failed", error, true));
        }
    }
    let managed_paths = match state
        .database
        .conversation_internal_paths(&request.conversation_id)
    {
        Ok(paths) => paths,
        Err(error) => return Ok(storage_error(error)),
    };
    match state
        .database
        .purge_trashed_conversation(&request.conversation_id)
    {
        Ok(deleted) => {
            if deleted {
                for path in managed_paths {
                    if let Err(error) = remove_managed_file(&state.data_dir, &path) {
                        return Ok(ApiResponse::failure(
                            "conversation.file_cleanup_failed",
                            format!("对话已永久删除，但内部文件清理失败：{error}"),
                            true,
                        ));
                    }
                }
            }
            Ok(ApiResponse::success(deleted))
        }
        Err(error) => Ok(storage_error(error)),
    }
}

fn remove_managed_file(data_dir: &std::path::Path, path: &std::path::Path) -> Result<(), String> {
    if !path.exists() {
        return Ok(());
    }
    let root = data_dir
        .canonicalize()
        .map_err(|error| format!("无法校验 Fox 数据目录：{error}"))?;
    let path = path
        .canonicalize()
        .map_err(|error| format!("无法校验待清理文件：{error}"))?;
    if !path.starts_with(root) || !path.is_file() {
        return Ok(());
    }
    std::fs::remove_file(path).map_err(|error| error.to_string())
}

#[cfg(test)]
mod command_tests {
    use super::{
        conversation_expert_error, downloaded_file_can_open_directly, knowledge_preview_cache_key,
        knowledge_preview_cache_path, managed_preview_cache_file, remove_managed_file,
        validated_external_url, ApiResponse, ConversationExpertBindingError,
        SetKnowledgeBindingsWireRequest,
    };
    use crate::database::KnowledgeReference;
    use serde_json::json;
    use std::path::Path;
    use uuid::Uuid;

    #[test]
    fn kernel_cancel_command_routes_before_legacy_writes_and_never_falls_back() {
        let path = std::env::temp_dir().join(format!("fox-command-cancel-{}.db", Uuid::new_v4()));
        let db = crate::database::Database::open(path).unwrap();
        let conversation = db.create_conversation("fox-general", Some("cancel routing"), None, None).unwrap();
        let run = db.create_run(&conversation.id, "cancel test", None).unwrap().run;
        assert_eq!(super::cancel_kernel_owned_run(&db, &run.id, || panic!("unbound Legacy Run must not route to Kernel")).unwrap(), None);
        db.freeze_kernel_run_control(&run.id, "legacy", fox_engine_protocol::TimeBudgets::default()).unwrap();
        assert!(db.mark_run_cancelling(&run.id).is_err());
        assert_eq!(super::cancel_kernel_owned_run(&db, &run.id, || db.queue_kernel_host_command(&run.id, None)).unwrap(), Some(true));
        assert_eq!(super::cancel_kernel_owned_run(&db, &run.id, || db.queue_kernel_host_command(&run.id, None)).unwrap(), Some(false));
        assert_eq!(db.pending_kernel_host_commands(&run.id).unwrap().len(), 1);
        assert_eq!(super::cancel_kernel_owned_run(&db, &run.id, || Err("owner unavailable".into())).unwrap_err(), "owner unavailable");
        let detail = db.load_conversation(&conversation.id).unwrap();
        assert_eq!(detail.last_run.unwrap().status, "queued");
    }

    #[test]
    fn external_url_validation_only_allows_http_and_https() {
        assert_eq!(
            validated_external_url(" https://example.com/path ").unwrap(),
            "https://example.com/path"
        );
        assert_eq!(
            validated_external_url("http://example.com").unwrap(),
            "http://example.com/"
        );
        assert!(validated_external_url("javascript:alert(1)").is_err());
        assert!(validated_external_url("file:///C:/Windows/System32").is_err());
        assert!(validated_external_url("not a url").is_err());
    }

    #[test]
    fn expert_binding_errors_keep_stable_command_codes() {
        for (error, expected_code, retryable) in [
            (
                ConversationExpertBindingError::ExpertNotFound,
                "expert.not_found",
                false,
            ),
            (
                ConversationExpertBindingError::InvalidRole,
                "conversation.expert_invalid_role",
                false,
            ),
            (
                ConversationExpertBindingError::RemoteUnavailable,
                "conversation.expert_remote_unavailable",
                true,
            ),
            (
                ConversationExpertBindingError::LockedArchived,
                "conversation.expert_locked",
                false,
            ),
            (
                ConversationExpertBindingError::LockedByMessages,
                "conversation.expert_locked",
                false,
            ),
            (
                ConversationExpertBindingError::LockedByActiveRun,
                "conversation.expert_locked",
                false,
            ),
        ] {
            let response: ApiResponse<bool> = conversation_expert_error(error);
            let api_error = response.error.expect("structured expert error");
            assert_eq!(api_error.code, expected_code);
            assert_eq!(api_error.retryable, retryable);
        }
    }

    #[test]
    fn conversation_cleanup_removes_only_fox_managed_files() {
        let root = std::env::temp_dir().join(format!("fox-delete-test-{}", Uuid::new_v4()));
        let data = root.join("data");
        let project = root.join("project");
        std::fs::create_dir_all(data.join("attachments")).unwrap();
        std::fs::create_dir_all(&project).unwrap();
        let managed = data.join("attachments/managed.txt");
        let exported = project.join("exported.txt");
        std::fs::write(&managed, b"managed").unwrap();
        std::fs::write(&exported, b"exported").unwrap();
        remove_managed_file(&data, &managed).unwrap();
        remove_managed_file(&data, &exported).unwrap();
        assert!(!managed.exists());
        assert!(exported.exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn preview_cache_key_is_stable_and_revision_sensitive() {
        let first = knowledge_preview_cache_key("kb-1", "file-1", "revision-a");
        let repeated = knowledge_preview_cache_key("kb-1", "file-1", "revision-a");
        let changed = knowledge_preview_cache_key("kb-1", "file-1", "revision-b");

        assert_eq!(first, repeated);
        assert_ne!(first, changed);
        assert_eq!(first.len(), 64);
    }

    #[test]
    fn preview_cache_paths_cannot_escape_or_use_part_files() {
        let root = std::env::temp_dir().join(format!("fox-preview-path-test-{}", Uuid::new_v4()));
        let cache = root.join("cache");
        std::fs::create_dir_all(&cache).unwrap();
        let valid = cache.join("valid.cache");
        let typed = cache.join("valid.cache.docx");
        let partial = cache.join("pending.part");
        let outside = root.join("outside.cache");
        std::fs::write(&valid, b"valid").unwrap();
        std::fs::write(&typed, b"typed").unwrap();
        std::fs::write(&partial, b"partial").unwrap();
        std::fs::write(&outside, b"outside").unwrap();

        assert!(managed_preview_cache_file(&valid, &cache).is_some());
        assert!(managed_preview_cache_file(&typed, &cache).is_some());
        assert!(managed_preview_cache_file(&partial, &cache).is_none());
        assert!(managed_preview_cache_file(&outside, &cache).is_none());

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn preview_cache_path_preserves_only_safe_file_extensions() {
        let root = Path::new("preview-cache");
        let key = "b".repeat(64);
        assert_eq!(
            knowledge_preview_cache_path(root, &key, Some("报告.DOCX")),
            root.join(format!("{key}.cache.docx"))
        );
        assert_eq!(
            knowledge_preview_cache_path(root, &key, Some("archive.tar.gz")),
            root.join(format!("{key}.cache.gz"))
        );
        assert_eq!(
            knowledge_preview_cache_path(root, &key, Some("unsafe.bad-ext!")),
            root.join(format!("{key}.cache"))
        );
    }

    #[test]
    fn direct_open_allows_documents_and_images() {
        for filename in [
            "manual.PDF",
            "report.docx",
            "sheet.xlsx",
            "slides.pptx",
            "notes.md",
            "data.csv",
            "photo.webp",
        ] {
            assert!(
                downloaded_file_can_open_directly(Path::new(filename)),
                "expected {filename} to be directly openable"
            );
        }
    }

    #[test]
    fn direct_open_rejects_executables_scripts_and_unknown_files() {
        for filename in [
            "installer.exe",
            "package.msi",
            "run.cmd",
            "run.ps1",
            "macro.docm",
            "script.py",
            "page.html",
            "archive.zip",
            "unknown.bin",
            "README",
        ] {
            assert!(
                !downloaded_file_can_open_directly(Path::new(filename)),
                "expected {filename} to require reveal-only handling"
            );
        }
    }

    #[test]
    fn knowledge_binding_request_distinguishes_legacy_and_v2_fields() {
        let legacy: SetKnowledgeBindingsWireRequest = serde_json::from_value(json!({
            "conversationId": "conversation-1",
            "knowledgeBases": [{ "id": "remote-1", "name": "远程资料" }]
        }))
        .expect("legacy request should decode");
        assert!(legacy.knowledge_references.is_none());
        assert_eq!(legacy.knowledge_bases.as_ref().map(Vec::len), Some(1));

        let v2: SetKnowledgeBindingsWireRequest = serde_json::from_value(json!({
            "conversationId": "conversation-1",
            "knowledgeReferences": [{
                "source": "local",
                "providerKey": "local",
                "id": "local-1",
                "name": "本地资料"
            }]
        }))
        .expect("v2 request should decode");
        assert!(v2.knowledge_bases.is_none());
        let item = v2
            .knowledge_references
            .as_ref()
            .and_then(|items| items.first())
            .expect("one reference");
        assert_eq!(item.reference, KnowledgeReference::local("local-1"));
        assert_eq!(item.name.as_deref(), Some("本地资料"));
    }
}

#[tauri::command]
pub fn run_start(
    app: AppHandle,
    state: State<'_, AppState>,
    request: StartRunRequest,
) -> ApiResponse<StartRunResult> {
    let runtime_text = request
        .runtime_text
        .clone()
        .unwrap_or_else(|| request.text.clone());
    let runtime = match state
        .database
        .conversation_runtime(&request.conversation_id)
    {
        Ok(runtime) => runtime,
        Err(error) => return ApiResponse::failure("runtime.resolve_failed", error, false),
    };
    if runtime.runtime_type != "yuxi" && !request.attachment_ids.is_empty() {
        let mut pending_attachments = Vec::with_capacity(request.attachment_ids.len());
        for attachment_id in &request.attachment_ids {
            let attachment = match state
                .database
                .attachment_for_conversation(&request.conversation_id, attachment_id)
            {
                Ok(Some(attachment)) if attachment.message_id.is_none() => attachment,
                Ok(_) => {
                    return ApiResponse::failure(
                        "attachment.invalid_binding",
                        "附件不存在、已发送或不属于当前会话",
                        false,
                    )
                }
                Err(error) => return storage_error(error),
            };
            pending_attachments.push(attachment);
        }
        if let Err(error) = state
            .runtime_host
            .validate_run_attachments(&request.conversation_id, &pending_attachments)
        {
            return ApiResponse::failure("attachment.image_invalid", error, false);
        }
    }
    let started = match state.database.create_run(
        &request.conversation_id,
        &request.text,
        request.model.as_deref(),
    ) {
        Ok(started) => started,
        Err(error) => return ApiResponse::failure("run.create_failed", error, false),
    };

    let attachments = match state.database.bind_attachments_to_message(
        &request.conversation_id,
        &started.user_message.id,
        &request.attachment_ids,
    ) {
        Ok(attachments) => attachments,
        Err(error) => {
            let _ =
                state
                    .database
                    .mark_run_failed(&started.run.id, "attachment.bind_failed", &error);
            return ApiResponse::failure("attachment.bind_failed", error, false);
        }
    };

    dispatch_started_run(
        &app,
        &state,
        StartRunResult {
            attachments,
            ..started
        },
        runtime_text,
        true,
    )
}

fn dispatch_started_run(
    app: &AppHandle,
    state: &AppState,
    started: StartRunResult,
    runtime_text: String,
    apply_work_gate: bool,
) -> ApiResponse<StartRunResult> {
    let runtime = match state
        .database
        .conversation_runtime(&started.run.conversation_id)
    {
        Ok(runtime) => runtime,
        Err(error) => return ApiResponse::failure("runtime.resolve_failed", error, false),
    };
    let attachments = started.attachments.clone();
    if apply_work_gate {
        let applied_gate = match work_mode_gate::apply_user_request_with_intent(
            &state.database,
            &started.run.conversation_id,
            &started.run.id,
            &runtime_text,
            &started.user_message.content,
        ) {
            Ok(applied) => applied,
            Err(error) => {
                let _ = state.database.mark_run_failed(
                    &started.run.id,
                    "work_mode.gate_failed",
                    &error,
                );
                return ApiResponse::failure("work_mode.gate_failed", error, true);
            }
        };
        if applied_gate.evaluation.decision == WorkModeDecision::RequestConfirmation {
            if let Some(goal) = applied_gate
                .goal
                .filter(|goal| goal.status == crate::database::GoalStatus::Proposed)
            {
                let started = StartRunResult {
                    attachments,
                    ..started
                };
                return match state.database.save_pending_work_mode_dispatch(
                    &goal.id,
                    &started,
                    &runtime_text,
                ) {
                    Ok(pending) => {
                        if let Some(event) = applied_gate.event {
                            let _ = app.emit("fox://work-event", event);
                        }
                        ApiResponse::success(pending)
                    }
                    Err(error) => {
                        let _ = state.database.mark_run_failed(
                            &started.run.id,
                            "work_mode.confirmation_failed",
                            &error,
                        );
                        ApiResponse::failure("work_mode.confirmation_failed", error, true)
                    }
                };
            }
        }
        if let Some(event) = applied_gate.event {
            let _ = app.emit("fox://work-event", event);
        }
    }

    if runtime.runtime_type == "yuxi" {
        state
            .yuxi_runtime
            .start_run_detached(started.clone(), runtime_text, attachments.clone())
    } else {
        state
            .runtime_host
            .start_run_detached(started.clone(), runtime_text, attachments.clone())
    }
    ApiResponse::success(StartRunResult {
        attachments,
        ..started
    })
}

#[tauri::command]
pub async fn run_rewind(
    app: AppHandle,
    state: State<'_, AppState>,
    request: RewindRunRequest,
) -> Result<ApiResponse<StartRunResult>, String> {
    let mut runtime_text = request
        .runtime_text
        .clone()
        .unwrap_or_else(|| request.text.clone());
    let runtime = match state
        .database
        .conversation_runtime(&request.conversation_id)
    {
        Ok(runtime) => runtime,
        Err(error) => return Ok(ApiResponse::failure("runtime.resolve_failed", error, false)),
    };
    match state
        .database
        .conversation_has_active_run(&request.conversation_id)
    {
        Ok(true) => {
            return Ok(ApiResponse::failure(
                "run.rewind_active",
                "请先停止当前生成，再重新编辑消息",
                false,
            ))
        }
        Ok(false) => {}
        Err(error) => return Ok(storage_error(error)),
    }

    let mut replacement_thread: Option<(String, String, String, String)> = None;
    if runtime.runtime_type == "yuxi" {
        let prior_messages = match state.database.runtime_prompt_context_before_message(
            &request.conversation_id,
            &request.message_id,
            80,
        ) {
            Ok(messages) => messages,
            Err(error) => return Ok(storage_error(error)),
        };
        if !prior_messages.is_empty() {
            let context = prior_messages
                .into_iter()
                .map(|message| {
                    let speaker = if message.role == "assistant" {
                        "助手"
                    } else {
                        "用户"
                    };
                    format!("{speaker}：{}", message.content)
                })
                .collect::<Vec<_>>()
                .join("\n\n");
            runtime_text = format!(
                "以下是本次重新编辑之前仍然保留的对话上下文：\n\n{context}\n\n用户当前消息：\n{runtime_text}"
            );
        }
        let Some(remote_agent_id) = runtime.remote_agent_id.as_deref() else {
            return Ok(ApiResponse::failure(
                "yuxi.agent_mapping_missing",
                "远程智能体映射缺失",
                false,
            ));
        };
        let service = match state.database.get_yuxi_service() {
            Ok(Some(service)) => service,
            Ok(None) => {
                return Ok(ApiResponse::failure(
                    "yuxi.not_configured",
                    "请先配置知识库服务",
                    false,
                ))
            }
            Err(error) => return Ok(storage_error(error)),
        };
        let Some(token) = get_access_token(&service.base_url) else {
            return Ok(ApiResponse::failure(
                "yuxi.not_authenticated",
                "请先登录知识库",
                false,
            ));
        };
        let title = match state.database.load_conversation(&request.conversation_id) {
            Ok(detail) => detail.conversation.title,
            Err(error) => return Ok(storage_error(error)),
        };
        let new_thread = match state
            .yuxi_client
            .create_thread(&service.base_url, &token, remote_agent_id, &title)
            .await
        {
            Ok(thread) => thread,
            Err(error) => {
                return Ok(ApiResponse::failure(
                    "yuxi.thread_create_failed",
                    error,
                    true,
                ))
            }
        };
        replacement_thread = Some((
            service.base_url,
            token,
            runtime.remote_thread_id.unwrap_or_default(),
            new_thread,
        ));
    } else if let Err(error) = state
        .runtime_host
        .remove_conversation(&request.conversation_id)
    {
        return Ok(ApiResponse::failure("runtime.reset_failed", error, true));
    }

    let started = match state.database.rewind_run(
        &request.conversation_id,
        &request.message_id,
        &request.text,
        request.model.as_deref(),
    ) {
        Ok(started) => started,
        Err(error) => {
            if let Some((base_url, token, _, new_thread)) = replacement_thread.as_ref() {
                let _ = state
                    .yuxi_client
                    .delete_thread(base_url, token, new_thread)
                    .await;
            }
            return Ok(ApiResponse::failure("run.rewind_failed", error, false));
        }
    };

    if runtime.runtime_type != "yuxi" && !started.attachments.is_empty() {
        let attachment_note = started
            .attachments
            .iter()
            .map(|attachment| format!("- {}: {}", attachment.id, attachment.display_name))
            .collect::<Vec<_>>()
            .join("\n");
        runtime_text.push_str(&format!(
            "\n\nFox attachments available through read_attachment:\n{attachment_note}"
        ));
    }

    if let Some((base_url, token, old_thread, new_thread)) = replacement_thread {
        if let Err(error) = state.database.ensure_external_runtime_session(
            &request.conversation_id,
            "yuxi",
            &new_thread,
            None,
        ) {
            let _ =
                state
                    .database
                    .mark_run_failed(&started.run.id, "yuxi.thread_bind_failed", &error);
            return Ok(ApiResponse::failure("yuxi.thread_bind_failed", error, true));
        }
        if !old_thread.is_empty() && old_thread != new_thread {
            let _ = state
                .yuxi_client
                .delete_thread(&base_url, &token, &old_thread)
                .await;
        }
    }

    Ok(dispatch_started_run(
        &app,
        &state,
        started,
        runtime_text,
        true,
    ))
}

#[tauri::command]
pub fn run_resume(
    state: State<'_, AppState>,
    request: ResumeYuxiRunRequest,
) -> ApiResponse<StartRunResult> {
    if request.text.trim().is_empty() || !request.answers.is_object() {
        return ApiResponse::failure("yuxi.resume_invalid", "请先完成需要补充的事项", false);
    }
    let runtime = match state
        .database
        .conversation_runtime(&request.conversation_id)
    {
        Ok(runtime) if runtime.runtime_type == "yuxi" => runtime,
        Ok(_) => {
            return ApiResponse::failure(
                "yuxi.resume_invalid",
                "当前会话不是知识库智能体会话",
                false,
            )
        }
        Err(error) => return ApiResponse::failure("runtime.resolve_failed", error, false),
    };
    let parent_external_run_id = match state.database.external_run(&request.parent_run_id) {
        Ok(Some(run))
            if run.runtime_type == "yuxi" && run.conversation_id == request.conversation_id =>
        {
            match run.external_run_id {
                Some(id) => id,
                None => {
                    return ApiResponse::failure(
                        "yuxi.resume_missing_parent",
                        "找不到需要恢复的远程任务",
                        false,
                    )
                }
            }
        }
        Ok(_) => {
            return ApiResponse::failure(
                "yuxi.resume_missing_parent",
                "找不到需要恢复的远程任务",
                false,
            )
        }
        Err(error) => return storage_error(error),
    };
    let started = match state.database.create_resumed_run(
        &request.conversation_id,
        &request.parent_run_id,
        &request.text,
    ) {
        Ok(started) => started,
        Err(error) => return ApiResponse::failure("yuxi.resume_invalid", error, false),
    };
    state.yuxi_runtime.resume_interrupted_run_detached(
        started.clone(),
        runtime,
        parent_external_run_id,
        request.answers,
    );
    ApiResponse::success(started)
}

#[tauri::command]
pub async fn run_cancel(
    state: State<'_, AppState>,
    request: CancelRunRequest,
) -> Result<ApiResponse<bool>, String> {
    // Frozen authority must be selected before touching Legacy read models.
    match cancel_kernel_owned_run(&state.database, &request.run_id, || state.runtime_host.cancel_managed_run(&request.run_id)) {
        Ok(Some(cancelled)) => return Ok(ApiResponse::success(cancelled)),
        Ok(None) => {},
        Err(error) => return Ok(ApiResponse::failure("runtime.cancel_failed", error, true)),
    }
    let conversation = match state.database.mark_run_cancelling(&request.run_id) {
        Ok(conversation) => conversation,
        Err(error) => return Ok(storage_error(error)),
    };
    if conversation.is_none() {
        return Ok(ApiResponse::success(false));
    }
    let is_yuxi = state
        .database
        .external_run(&request.run_id)
        .ok()
        .flatten()
        .is_some_and(|run| run.runtime_type == "yuxi");
    let cancelled = if is_yuxi {
        state.yuxi_runtime.cancel_run(&request.run_id).await
    } else {
        state.runtime_host.cancel_run(&request.run_id)
    };
    match cancelled {
        Ok(true) => Ok(ApiResponse::success(true)),
        Ok(false) => {
            let message = "Runtime 未找到正在执行的任务";
            let _ = state.database.mark_run_interrupted(
                &request.run_id,
                "runtime.cancel_not_active",
                message,
            );
            Ok(ApiResponse::failure(
                "runtime.cancel_not_active",
                message,
                true,
            ))
        }
        Err(error) => {
            let _ = state.database.mark_run_interrupted(
                &request.run_id,
                "runtime.cancel_failed",
                &error,
            );
            Ok(ApiResponse::failure("runtime.cancel_failed", error, true))
        }
    }
}

fn cancel_kernel_owned_run(
    database: &crate::database::Database,
    run_id: &str,
    cancel: impl FnOnce() -> Result<bool, String>,
) -> Result<Option<bool>, String> {
    if database.run_control_binding(run_id)?.is_some_and(|binding|
        binding.authority == fox_engine_protocol::ExecutionAuthority::Authoritative) {
        cancel().map(Some)
    } else {
        Ok(None)
    }
}

#[tauri::command]
pub fn child_run_cancel_by_user(
    state: State<'_, AppState>,
    request: CancelChildRunRequest,
) -> ApiResponse<bool> {
    let child = match state.database.child_run(&request.child_run_id) {
        Ok(Some(child)) => child,
        Ok(None) => return ApiResponse::success(false),
        Err(error) => return storage_error(error),
    };
    let belongs_to_conversation = match state
        .database
        .child_runs_for_conversation(&request.parent_conversation_id)
    {
        Ok(children) => children
            .iter()
            .any(|candidate| candidate.child_run_id == child.child_run_id),
        Err(error) => return storage_error(error),
    };
    if !belongs_to_conversation {
        return ApiResponse::failure(
            "child_run.parent_mismatch",
            "这个子任务不属于当前会话，Fox 已拒绝取消操作",
            false,
        );
    }
    if matches!(
        child.status.as_str(),
        "completed" | "failed" | "cancelled" | "interrupted"
    ) {
        return ApiResponse::success(false);
    }
    match state
        .database
        .graph_attempt_id_for_child_run(&request.child_run_id)
    {
        Ok(Some(_)) => {
            return ApiResponse::failure(
                "graph.readonly_dedicated_cancel_required",
                "这个子任务由工作图管理，请从对应的工作图节点取消，避免破坏图状态",
                false,
            )
        }
        Ok(None) => {}
        Err(error) => return storage_error(error),
    }
    match state.runtime_host.cancel_managed_run(&request.child_run_id) {
        Ok(cancelled) => ApiResponse::success(cancelled),
        Err(error) => ApiResponse::failure("child_run.cancel_failed", error, true),
    }
}

#[tauri::command]
pub fn attachments_save(
    state: State<'_, AppState>,
    request: SaveAttachmentsRequest,
) -> ApiResponse<Vec<AttachmentRecord>> {
    let directory = state
        .data_dir
        .join("attachments")
        .join(&request.conversation_id);
    if let Err(error) = std::fs::create_dir_all(&directory) {
        return ApiResponse::failure("attachment.directory_failed", error.to_string(), true);
    }
    let mut records = Vec::new();
    for file in request.files {
        let Some((header, encoded)) = file.data_url.split_once(',') else {
            return ApiResponse::failure("attachment.invalid_data", "附件数据格式无效", false);
        };
        if !header.starts_with("data:") {
            return ApiResponse::failure("attachment.invalid_data", "附件数据格式无效", false);
        }
        let bytes = match base64::engine::general_purpose::STANDARD.decode(encoded) {
            Ok(value) => value,
            Err(error) => {
                return ApiResponse::failure("attachment.decode_failed", error.to_string(), false)
            }
        };
        if bytes.len() > 20 * 1024 * 1024 {
            return ApiResponse::failure("attachment.too_large", "单个附件不能超过 20 MB", false);
        }
        let id = Uuid::new_v4().to_string();
        let filename_path = PathBuf::from(&file.filename);
        let extension = filename_path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("bin");
        let path = directory.join(format!("{id}.{extension}"));
        if let Err(error) = std::fs::write(&path, &bytes) {
            return ApiResponse::failure("attachment.write_failed", error.to_string(), true);
        }
        let hash = format!("{:x}", Sha256::digest(&bytes));
        records.push(AttachmentRecord {
            id,
            conversation_id: request.conversation_id.clone(),
            message_id: request.message_id.clone(),
            display_name: file.filename,
            storage_path: path.to_string_lossy().into_owned(),
            media_type: file.media_type,
            byte_size: bytes.len() as i64,
            sha256: Some(hash),
            status: "ready".to_owned(),
            created_at: crate::database::now_ms(),
        });
    }
    match state.database.add_attachments(&records) {
        Ok(()) => ApiResponse::success(records),
        Err(error) => storage_error(error),
    }
}

#[tauri::command]
pub fn knowledge_bindings_set(
    state: State<'_, AppState>,
    request: SetKnowledgeBindingsWireRequest,
) -> ApiResponse<KnowledgeBindingsSetResponse> {
    if let Some(references) = request.knowledge_references {
        let items = references
            .into_iter()
            .map(|KnowledgeReferenceInput { reference, name }| {
                let name = name
                    .filter(|value| !value.trim().is_empty())
                    .unwrap_or_else(|| reference.id.clone());
                (reference, name)
            })
            .collect::<Vec<_>>();
        return match state
            .database
            .set_knowledge_references(&request.conversation_id, &items)
        {
            Ok(records) => ApiResponse::success(KnowledgeBindingsSetResponse {
                bindings: legacy_knowledge_bindings(&records),
                knowledge_references: records.into_iter().map(|record| record.reference).collect(),
            }),
            Err(error) => storage_error(error),
        };
    }

    let items = request
        .knowledge_bases
        .unwrap_or_default()
        .into_iter()
        .map(|item| (item.id, item.name))
        .collect::<Vec<_>>();
    match state
        .database
        .set_knowledge_bindings(&request.conversation_id, &items)
    {
        Ok(bindings) => match state
            .database
            .conversation_knowledge_references(&request.conversation_id)
        {
            Ok(knowledge_references) => ApiResponse::success(KnowledgeBindingsSetResponse {
                bindings,
                knowledge_references,
            }),
            Err(error) => storage_error(error),
        },
        Err(error) => storage_error(error),
    }
}

fn legacy_knowledge_bindings(
    records: &[KnowledgeReferenceBindingRecord],
) -> Vec<crate::database::KnowledgeBindingRecord> {
    records
        .iter()
        .filter_map(|record| {
            let connection_id = record.reference.connection_id.clone()?;
            Some(crate::database::KnowledgeBindingRecord {
                conversation_id: record.conversation_id.clone(),
                service_connection_id: connection_id,
                knowledge_base_id: record.reference.id.clone(),
                knowledge_base_name: record.knowledge_base_name.clone(),
                enabled: record.enabled,
                created_at: record.created_at,
                updated_at: record.updated_at,
            })
        })
        .collect()
}

#[tauri::command]
pub fn approval_resolve(
    state: State<'_, AppState>,
    request: ResolveApprovalRequest,
) -> ApiResponse<bool> {
    match state
        .runtime_host
        .resolve_approval(&request.approval_id, request.decision)
    {
        Ok(resolved) => ApiResponse::success(resolved),
        Err(error) => ApiResponse::failure("approval.resolve_failed", error, true),
    }
}

#[tauri::command]
pub fn plan_revision_resolve(
    app: AppHandle,
    state: State<'_, AppState>,
    request: ResolvePlanRevisionRequest,
) -> ApiResponse<crate::database::PlanRevisionRecord> {
    match state.database.resolve_plan_revision(
        &request.conversation_id,
        &request.plan_revision_id,
        &request.decision,
    ) {
        Ok(plan_revision) => {
            let event_type = if plan_revision.status == "approved" {
                "plan.approved"
            } else {
                "plan.rejected"
            };
            if let Ok(event) = state.database.append_work_event(
                event_type,
                &request.conversation_id,
                Some(&plan_revision.goal_id),
                None,
                None,
                json!({ "planRevision": plan_revision }),
            ) {
                let _ = app.emit("fox://work-event", event);
            }
            ApiResponse::success(plan_revision)
        }
        Err(error) => ApiResponse::failure("plan_revision.resolve_failed", error, false),
    }
}

#[tauri::command]
pub fn work_mode_confirmation_resolve(
    app: AppHandle,
    state: State<'_, AppState>,
    request: ResolveWorkModeConfirmationRequest,
) -> ApiResponse<crate::database::GoalRecord> {
    let pending_dispatch = match state.database.pending_work_mode_dispatch(&request.goal_id) {
        Ok(pending) => pending,
        Err(error) => {
            return ApiResponse::failure("work_mode.dispatch_restore_failed", error, true)
        }
    };
    match work_mode_gate::resolve_confirmation(&state.database, &request) {
        Ok((goal, event)) => {
            let _ = app.emit("fox://work-event", event);
            if request.approved && pending_dispatch.is_some() {
                let pending = match state
                    .database
                    .release_pending_work_mode_dispatch(&request.goal_id)
                {
                    Ok(pending) => pending,
                    Err(error) => {
                        return ApiResponse::failure(
                            "work_mode.dispatch_restore_failed",
                            error,
                            true,
                        )
                    }
                };
                let runtime = match state
                    .database
                    .conversation_runtime(&request.conversation_id)
                {
                    Ok(runtime) => runtime,
                    Err(error) => {
                        let _ = state.database.mark_run_failed(
                            &pending.started.run.id,
                            "runtime.resolve_failed",
                            &error,
                        );
                        return ApiResponse::failure("runtime.resolve_failed", error, false);
                    }
                };
                let attachments = pending.started.attachments.clone();
                if runtime.runtime_type == "yuxi" {
                    state.yuxi_runtime.start_run_detached(
                        pending.started,
                        pending.runtime_text,
                        attachments,
                    );
                } else {
                    state.runtime_host.start_run_detached(
                        pending.started,
                        pending.runtime_text,
                        attachments,
                    );
                }
            } else if !request.approved && pending_dispatch.is_some() {
                if let Err(error) = state
                    .database
                    .reject_pending_work_mode_dispatch(&request.goal_id)
                {
                    return ApiResponse::failure("work_mode.reject_failed", error, true);
                }
            }
            ApiResponse::success(goal)
        }
        Err(error) => ApiResponse::failure("work_mode.confirmation_failed", error, false),
    }
}

#[tauri::command]
pub fn goal_delete(state: State<'_, AppState>, request: GoalIdRequest) -> ApiResponse<bool> {
    let goal = match state.database.goals().get(&request.goal_id) {
        Ok(Some(goal)) => goal,
        Ok(None) => return ApiResponse::success(false),
        Err(error) => return storage_error(error.to_string()),
    };
    if goal.conversation_id != request.conversation_id {
        return ApiResponse::failure("goal.not_found", "未找到目标", false);
    }

    if let Err(error) = state
        .database
        .reject_pending_work_mode_dispatch(&request.goal_id)
    {
        return storage_error(error);
    }
    match state
        .database
        .conversation_has_active_run(&request.conversation_id)
    {
        Ok(true) => {
            return ApiResponse::failure("goal.run_active", "请先停止当前生成，再删除目标", false)
        }
        Ok(false) => {}
        Err(error) => return storage_error(error),
    }

    match state
        .database
        .goals()
        .delete(&request.goal_id, &request.conversation_id)
    {
        Ok(deleted) => ApiResponse::success(deleted),
        Err(error) => storage_error(error.to_string()),
    }
}

#[tauri::command]
pub async fn goal_running_set(
    app: AppHandle,
    state: State<'_, AppState>,
    request: SetGoalRunningRequest,
) -> Result<ApiResponse<SetGoalRunningResult>, String> {
    let current = match state.database.goals().get(&request.goal_id) {
        Ok(Some(goal)) if goal.conversation_id == request.conversation_id => goal,
        Ok(_) => return Ok(ApiResponse::failure("goal.not_found", "未找到目标", false)),
        Err(error) => return Ok(storage_error(error.to_string())),
    };
    let active_run_id = if request.running {
        None
    } else {
        match state.database.active_run_id(&request.conversation_id) {
            Ok(run_id) => run_id,
            Err(error) => return Ok(storage_error(error)),
        }
    };
    let result = if request.running {
        state
            .database
            .goals()
            .activate(&request.goal_id, request.expected_version)
    } else {
        state.database.goals().block(
            &request.goal_id,
            "Paused by user".to_owned(),
            request.expected_version,
        )
    };
    let goal = match result {
        Ok(goal) => goal,
        Err(error) => {
            return Ok(ApiResponse::failure(
                "goal.status_update_failed",
                error.to_string(),
                false,
            ))
        }
    };
    let event_type = if request.running {
        "goal.activated"
    } else {
        "goal.blocked"
    };
    if let Ok(event) = state.database.append_work_event(
        event_type,
        &current.conversation_id,
        Some(&goal.id),
        None,
        None,
        json!({
            "goal": goal.clone(),
            "source": "user",
            "paused": !request.running,
        }),
    ) {
        let _ = app.emit("fox://work-event", event);
    }
    if let Some(run_id) = active_run_id {
        let kernel_cancelled = match cancel_kernel_owned_run(&state.database, &run_id, || state.runtime_host.cancel_managed_run(&run_id)) {
            Ok(result) => result.is_some(),
            Err(error) => return Ok(ApiResponse::failure("runtime.cancel_failed", error, true)),
        };
        if !kernel_cancelled {
        let marked = match state.database.mark_run_cancelling(&run_id) {
            Ok(marked) => marked.is_some(),
            Err(error) => return Ok(storage_error(error)),
        };
        if marked {
            let is_yuxi = state
                .database
                .external_run(&run_id)
                .ok()
                .flatten()
                .is_some_and(|run| run.runtime_type == "yuxi");
            let cancelled = if is_yuxi {
                state.yuxi_runtime.cancel_run(&run_id).await
            } else {
                state.runtime_host.cancel_run(&run_id)
            };
            match cancelled {
                Ok(true) => {}
                Ok(false) => {
                    let _ = state.database.mark_run_interrupted(
                        &run_id,
                        "runtime.cancel_not_active",
                        "Runtime 未找到正在执行的任务",
                    );
                }
                Err(error) => {
                    let _ = state.database.mark_run_interrupted(
                        &run_id,
                        "runtime.cancel_failed",
                        &error,
                    );
                    return Ok(ApiResponse::failure("runtime.cancel_failed", error, true));
                }
            }
        }
        }
    }

    if !request.running {
        return Ok(ApiResponse::success(SetGoalRunningResult {
            goal,
            started_run: None,
        }));
    }

    let runtime_text = format!(
        "继续执行当前目标《{}》。读取当前工作快照，从尚未完成的任务继续，不要重复已经完成的工作。",
        goal.title
    );
    let started = match state
        .database
        .create_goal_continuation_run(&request.conversation_id, &runtime_text)
    {
        Ok(started) => started,
        Err(error) => {
            let _ = state.database.goals().block(
                &goal.id,
                "Failed to resume goal".to_owned(),
                goal.version,
            );
            return Ok(ApiResponse::failure("goal.resume_failed", error, true));
        }
    };
    let dispatched = dispatch_started_run(&app, &state, started, runtime_text, false);
    if !dispatched.ok {
        let _ = state.database.goals().block(
            &goal.id,
            "Failed to resume goal".to_owned(),
            goal.version,
        );
        return Ok(ApiResponse {
            ok: false,
            data: None,
            error: dispatched.error,
        });
    }
    Ok(ApiResponse::success(SetGoalRunningResult {
        goal,
        started_run: dispatched.data,
    }))
}

fn storage_error<T: serde::Serialize>(error: String) -> ApiResponse<T> {
    ApiResponse::failure("storage.operation_failed", error, true)
}

fn conversation_expert_error<T: serde::Serialize>(
    error: ConversationExpertBindingError,
) -> ApiResponse<T> {
    ApiResponse::failure(error.code(), error.to_string(), error.retryable())
}

#[tauri::command]
pub fn yuxi_service_get(state: State<'_, AppState>) -> ApiResponse<Option<YuxiServiceRecord>> {
    match state.database.get_yuxi_service() {
        Ok(service) => ApiResponse::success(service.map(|mut service| {
            service.credential_configured = access_token_configured(&service.base_url);
            service
        })),
        Err(error) => storage_error(error),
    }
}

#[tauri::command]
pub fn yuxi_service_save(
    state: State<'_, AppState>,
    request: SaveYuxiServiceRequest,
) -> ApiResponse<YuxiServiceRecord> {
    let name = request.name.trim();
    if name.is_empty() {
        return ApiResponse::failure("yuxi.invalid_name", "请输入服务名称", false);
    }
    let base_url = match normalize_base_url(&request.base_url) {
        Ok(url) => url,
        Err(error) => return ApiResponse::failure("yuxi.invalid_url", error, false),
    };
    let previous_base_url = match state.database.get_yuxi_service() {
        Ok(service) => service.map(|service| service.base_url),
        Err(error) => return storage_error(error),
    };
    if request.clear_access_token {
        if let Err(error) = clear_access_token(&base_url) {
            return ApiResponse::failure("yuxi.credential_delete_failed", error, true);
        }
    } else if let Some(token) = request
        .access_token
        .as_deref()
        .map(str::trim)
        .filter(|token| !token.is_empty())
    {
        if let Err(error) = set_access_token(&base_url, token) {
            return ApiResponse::failure("yuxi.credential_save_failed", error, true);
        }
    }
    match state.database.save_yuxi_service(name, &base_url) {
        Ok(mut service) => {
            if previous_base_url
                .as_deref()
                .is_some_and(|url| url != base_url)
            {
                if let Some(previous_base_url) = previous_base_url {
                    if let Err(error) = clear_access_token(&previous_base_url) {
                        return ApiResponse::failure(
                            "yuxi.old_credential_delete_failed",
                            format!("知识库服务已保存，但旧地址的访问凭证清理失败：{error}"),
                            true,
                        );
                    }
                }
            }
            service.credential_configured = access_token_configured(&service.base_url);
            ApiResponse::success(service)
        }
        Err(error) => storage_error(error),
    }
}

#[tauri::command]
pub async fn yuxi_service_test(
    state: State<'_, AppState>,
    request: TestYuxiServiceRequest,
) -> Result<ApiResponse<YuxiConnectionTest>, String> {
    let base_url = match request.base_url {
        Some(url) => url,
        None => match state.database.get_yuxi_service() {
            Ok(Some(service)) => service.base_url,
            Ok(None) => {
                return Ok(ApiResponse::failure(
                    "yuxi.not_configured",
                    "请先配置知识库服务",
                    false,
                ))
            }
            Err(error) => return Ok(storage_error(error)),
        },
    };
    let normalized_url = match normalize_base_url(&base_url) {
        Ok(url) => url,
        Err(error) => return Ok(ApiResponse::failure("yuxi.invalid_url", error, false)),
    };
    let access_token = request
        .access_token
        .map(|token| token.trim().to_owned())
        .filter(|token| !token.is_empty())
        .or_else(|| get_access_token(&normalized_url));
    match state
        .yuxi_client
        .test(&normalized_url, access_token.as_deref())
        .await
    {
        Ok(result) => {
            let _ = state.database.record_yuxi_connection_test(&result);
            Ok(ApiResponse::success(result))
        }
        Err(error) => {
            let _ = state
                .database
                .record_yuxi_connection_failure(&normalized_url);
            Ok(ApiResponse::failure("yuxi.connection_failed", error, true))
        }
    }
}

fn configured_yuxi(state: &AppState) -> Result<(YuxiServiceRecord, String), String> {
    let service = state
        .database
        .get_yuxi_service()?
        .ok_or_else(|| "请先配置知识库服务".to_owned())?;
    let token = get_access_token(&service.base_url).ok_or_else(|| "请先登录知识库".to_owned())?;
    Ok((service, token))
}

#[tauri::command]
pub async fn yuxi_login(
    state: State<'_, AppState>,
    request: YuxiLoginRequest,
) -> Result<ApiResponse<YuxiUserRecord>, String> {
    let service = match state.database.get_yuxi_service() {
        Ok(Some(service)) => service,
        Ok(None) => {
            return Ok(ApiResponse::failure(
                "yuxi.not_configured",
                "请先配置知识库服务",
                false,
            ))
        }
        Err(error) => return Ok(storage_error(error)),
    };
    match state
        .yuxi_client
        .login(&service.base_url, &request.username, &request.password)
        .await
    {
        Ok((token, user)) => {
            if let Err(error) = set_access_token(&service.base_url, &token) {
                return Ok(ApiResponse::failure(
                    "yuxi.credential_save_failed",
                    error,
                    true,
                ));
            }
            match state
                .yuxi_client
                .list_agents(&service.base_url, &token)
                .await
            {
                Ok(agents) => {
                    let _ = state.database.upsert_yuxi_agents(&agents);
                }
                Err(error) => {
                    return Ok(ApiResponse::failure("yuxi.agent_sync_failed", error, true))
                }
            }
            state.yuxi_runtime.recover_pending_runs_detached();
            Ok(ApiResponse::success(user))
        }
        Err(error) => Ok(ApiResponse::failure("yuxi.login_failed", error, false)),
    }
}

#[tauri::command]
pub async fn yuxi_current_user(
    state: State<'_, AppState>,
) -> Result<ApiResponse<YuxiUserRecord>, String> {
    let (service, token) = match configured_yuxi(&state) {
        Ok(value) => value,
        Err(error) => return Ok(ApiResponse::failure("yuxi.not_authenticated", error, false)),
    };
    match state
        .yuxi_client
        .current_user(&service.base_url, &token)
        .await
    {
        Ok(user) => {
            state.yuxi_runtime.recover_pending_runs_detached();
            Ok(ApiResponse::success(user))
        }
        Err(error) => Ok(ApiResponse::failure("yuxi.user_failed", error, true)),
    }
}

#[tauri::command]
pub async fn yuxi_agents_sync(
    state: State<'_, AppState>,
) -> Result<ApiResponse<Vec<YuxiAgentRecord>>, String> {
    let (service, token) = match configured_yuxi(&state) {
        Ok(value) => value,
        Err(error) => {
            let _ = state.database.mark_yuxi_agents_unavailable();
            return Ok(ApiResponse::failure("yuxi.not_authenticated", error, false));
        }
    };
    match tokio::time::timeout(
        Duration::from_secs(3),
        state.yuxi_client.list_agents(&service.base_url, &token),
    )
    .await
    {
        Ok(Ok(agents)) => match state.database.upsert_yuxi_agents(&agents) {
            Ok(()) => Ok(ApiResponse::success(agents)),
            Err(error) => Ok(storage_error(error)),
        },
        Ok(Err(error)) => {
            let cache_error = state.database.mark_yuxi_agents_unavailable().err();
            let message = cache_error.map_or(error.clone(), |cache_error| {
                format!("{error}；本地缓存状态更新失败：{cache_error}")
            });
            Ok(ApiResponse::failure(
                "yuxi.agent_sync_failed",
                message,
                true,
            ))
        }
        Err(_) => {
            let cache_error = state.database.mark_yuxi_agents_unavailable().err();
            let message = cache_error.map_or_else(
                || "知识库专家同步超时，已继续使用本地缓存".to_owned(),
                |cache_error| format!("知识库专家同步超时；本地缓存状态更新失败：{cache_error}"),
            );
            Ok(ApiResponse::failure(
                "yuxi.agent_sync_timeout",
                message,
                true,
            ))
        }
    }
}

#[tauri::command]
pub async fn yuxi_models_list(
    state: State<'_, AppState>,
) -> Result<ApiResponse<Vec<YuxiModelRecord>>, String> {
    let (service, token) = match configured_yuxi(&state) {
        Ok(value) => value,
        Err(error) => return Ok(ApiResponse::failure("yuxi.not_authenticated", error, false)),
    };
    match state
        .yuxi_client
        .list_models(&service.base_url, &token)
        .await
    {
        Ok(models) => Ok(ApiResponse::success(models)),
        Err(error) => Ok(ApiResponse::failure("yuxi.models_failed", error, true)),
    }
}

#[tauri::command]
pub async fn knowledge_bases_list(
    state: State<'_, AppState>,
) -> Result<ApiResponse<Vec<KnowledgeBaseRecord>>, String> {
    let (service, token) = match configured_yuxi(&state) {
        Ok(value) => value,
        Err(error) => return Ok(ApiResponse::failure("yuxi.not_authenticated", error, false)),
    };
    match state
        .yuxi_client
        .list_knowledge_bases(&service.base_url, &token)
        .await
    {
        Ok(records) => Ok(ApiResponse::success(records)),
        Err(error) => Ok(ApiResponse::failure("knowledge.list_failed", error, true)),
    }
}

#[tauri::command]
pub async fn knowledge_detail(
    state: State<'_, AppState>,
    request: KnowledgeIdRequest,
) -> Result<ApiResponse<KnowledgeDetailRecord>, String> {
    let (service, token) = match configured_yuxi(&state) {
        Ok(value) => value,
        Err(error) => return Ok(ApiResponse::failure("yuxi.not_authenticated", error, false)),
    };
    match state
        .yuxi_client
        .knowledge_detail(&service.base_url, &token, &request.knowledge_base_id)
        .await
    {
        Ok(record) => Ok(ApiResponse::success(record)),
        Err(error) => Ok(ApiResponse::failure("knowledge.detail_failed", error, true)),
    }
}

#[tauri::command]
pub async fn knowledge_document_content(
    state: State<'_, AppState>,
    request: KnowledgeDocumentRequest,
) -> Result<ApiResponse<Value>, String> {
    let (service, token) = match configured_yuxi(&state) {
        Ok(value) => value,
        Err(error) => return Ok(ApiResponse::failure("yuxi.not_authenticated", error, false)),
    };
    match state
        .yuxi_client
        .knowledge_document_content(
            &service.base_url,
            &token,
            &request.knowledge_base_id,
            &request.document_id,
        )
        .await
    {
        Ok(value) => Ok(ApiResponse::success(value)),
        Err(error) => Ok(ApiResponse::failure(
            "knowledge.document_failed",
            error,
            true,
        )),
    }
}

const MAX_DOCUMENT_ACTIVITY_TEXT: usize = 8 * 1024;
const MAX_DOCUMENT_NOTE_TEXT: usize = 32 * 1024;

#[tauri::command]
pub fn knowledge_document_activity_get(
    state: State<'_, AppState>,
    request: KnowledgeDocumentRequest,
) -> ApiResponse<KnowledgeDocumentActivity> {
    match validate_document_identity(&request.knowledge_base_id, &request.document_id).and_then(
        |(knowledge_base_id, document_id)| {
            state
                .database
                .knowledge_document_activity(knowledge_base_id, document_id)
        },
    ) {
        Ok(activity) => ApiResponse::success(activity),
        Err(error) => ApiResponse::failure("knowledge.activity_load_failed", error, false),
    }
}

#[tauri::command]
pub fn knowledge_document_reading_state_save(
    state: State<'_, AppState>,
    request: SaveKnowledgeDocumentReadingStateRequest,
) -> ApiResponse<KnowledgeDocumentReadingState> {
    let result = (|| {
        let (knowledge_base_id, document_id) =
            validate_document_identity(&request.knowledge_base_id, &request.document_id)?;
        if !(1..=1_000_000).contains(&request.page) {
            return Err("文档页码超出允许范围".to_owned());
        }
        if !request.scroll_offset.is_finite()
            || !(0.0..=1_000_000_000.0).contains(&request.scroll_offset)
        {
            return Err("文档滚动位置无效".to_owned());
        }
        if request
            .zoom
            .is_some_and(|zoom| !zoom.is_finite() || !(0.1..=10.0).contains(&zoom))
        {
            return Err("文档缩放比例无效".to_owned());
        }
        let state_record = KnowledgeDocumentReadingState {
            knowledge_base_id: knowledge_base_id.to_owned(),
            document_id: document_id.to_owned(),
            source_revision: bounded_optional_text(request.source_revision, 512)?,
            page: request.page,
            scroll_offset: request.scroll_offset,
            zoom: request.zoom,
            updated_at: crate::database::now_ms(),
        };
        state
            .database
            .save_knowledge_document_reading_state(&state_record)
    })();
    result.map(ApiResponse::success).unwrap_or_else(|error| {
        ApiResponse::failure("knowledge.reading_state_save_failed", error, false)
    })
}

#[tauri::command]
pub fn knowledge_document_bookmark_save(
    state: State<'_, AppState>,
    request: SaveKnowledgeDocumentBookmarkRequest,
) -> ApiResponse<KnowledgeDocumentBookmark> {
    let result = (|| {
        let (knowledge_base_id, document_id) =
            validate_document_identity(&request.knowledge_base_id, &request.document_id)?;
        if !(1..=1_000_000).contains(&request.page) {
            return Err("书签页码超出允许范围".to_owned());
        }
        let now = crate::database::now_ms();
        let bookmark = KnowledgeDocumentBookmark {
            id: normalized_activity_id(request.id)?,
            knowledge_base_id: knowledge_base_id.to_owned(),
            document_id: document_id.to_owned(),
            source_revision: bounded_optional_text(request.source_revision, 512)?,
            page: request.page,
            anchor: bounded_text(
                request.anchor.unwrap_or_default(),
                MAX_DOCUMENT_ACTIVITY_TEXT,
            )?,
            excerpt: bounded_text(
                request.excerpt.unwrap_or_default(),
                MAX_DOCUMENT_ACTIVITY_TEXT,
            )?,
            label: bounded_text(request.label.unwrap_or_default(), 512)?,
            created_at: now,
            updated_at: now,
        };
        state.database.save_knowledge_document_bookmark(&bookmark)
    })();
    result.map(ApiResponse::success).unwrap_or_else(|error| {
        ApiResponse::failure("knowledge.bookmark_save_failed", error, false)
    })
}

#[tauri::command]
pub fn knowledge_document_bookmark_delete(
    state: State<'_, AppState>,
    request: DeleteKnowledgeDocumentActivityItemRequest,
) -> ApiResponse<bool> {
    delete_document_activity_item(&state, request, true)
}

#[tauri::command]
pub fn knowledge_document_annotation_save(
    state: State<'_, AppState>,
    request: SaveKnowledgeDocumentAnnotationRequest,
) -> ApiResponse<KnowledgeDocumentAnnotation> {
    let result = (|| {
        let (knowledge_base_id, document_id) =
            validate_document_identity(&request.knowledge_base_id, &request.document_id)?;
        if !(1..=1_000_000).contains(&request.page) {
            return Err("批注页码超出允许范围".to_owned());
        }
        let annotation_type = request.annotation_type.unwrap_or_else(|| "note".to_owned());
        if annotation_type != "note" && annotation_type != "highlight" {
            return Err("不支持的批注类型".to_owned());
        }
        let color = request.color.unwrap_or_else(|| "blue".to_owned());
        if !["blue", "yellow", "green", "red"].contains(&color.as_str()) {
            return Err("不支持的批注颜色".to_owned());
        }
        let now = crate::database::now_ms();
        let annotation = KnowledgeDocumentAnnotation {
            id: normalized_activity_id(request.id)?,
            knowledge_base_id: knowledge_base_id.to_owned(),
            document_id: document_id.to_owned(),
            source_revision: bounded_optional_text(request.source_revision, 512)?,
            annotation_type,
            page: request.page,
            anchor: bounded_text(
                request.anchor.unwrap_or_default(),
                MAX_DOCUMENT_ACTIVITY_TEXT,
            )?,
            excerpt: bounded_text(
                request.excerpt.unwrap_or_default(),
                MAX_DOCUMENT_ACTIVITY_TEXT,
            )?,
            note: bounded_text(request.note, MAX_DOCUMENT_NOTE_TEXT)?,
            color,
            created_at: now,
            updated_at: now,
        };
        state
            .database
            .save_knowledge_document_annotation(&annotation)
    })();
    result.map(ApiResponse::success).unwrap_or_else(|error| {
        ApiResponse::failure("knowledge.annotation_save_failed", error, false)
    })
}

#[tauri::command]
pub fn knowledge_document_annotation_delete(
    state: State<'_, AppState>,
    request: DeleteKnowledgeDocumentActivityItemRequest,
) -> ApiResponse<bool> {
    delete_document_activity_item(&state, request, false)
}

fn delete_document_activity_item(
    state: &State<'_, AppState>,
    request: DeleteKnowledgeDocumentActivityItemRequest,
    bookmark: bool,
) -> ApiResponse<bool> {
    let result = (|| {
        let (knowledge_base_id, document_id) =
            validate_document_identity(&request.knowledge_base_id, &request.document_id)?;
        let id = request.id.trim();
        if Uuid::parse_str(id).is_err() {
            return Err("文档活动标识无效".to_owned());
        }
        if bookmark {
            state
                .database
                .delete_knowledge_document_bookmark(knowledge_base_id, document_id, id)
        } else {
            state
                .database
                .delete_knowledge_document_annotation(knowledge_base_id, document_id, id)
        }
    })();
    result.map(ApiResponse::success).unwrap_or_else(|error| {
        ApiResponse::failure(
            if bookmark {
                "knowledge.bookmark_delete_failed"
            } else {
                "knowledge.annotation_delete_failed"
            },
            error,
            false,
        )
    })
}

fn validate_document_identity<'a>(
    knowledge_base_id: &'a str,
    document_id: &'a str,
) -> Result<(&'a str, &'a str), String> {
    let knowledge_base_id = knowledge_base_id.trim();
    let document_id = document_id.trim();
    if knowledge_base_id.is_empty() || knowledge_base_id.len() > 512 {
        return Err("知识库标识无效".to_owned());
    }
    if document_id.is_empty() || document_id.len() > 512 {
        return Err("文档标识无效".to_owned());
    }
    Ok((knowledge_base_id, document_id))
}

fn normalized_activity_id(id: Option<String>) -> Result<String, String> {
    match id
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
    {
        Some(id) if Uuid::parse_str(&id).is_ok() => Ok(id),
        Some(_) => Err("文档活动标识无效".to_owned()),
        None => Ok(Uuid::new_v4().to_string()),
    }
}

fn bounded_optional_text(value: Option<String>, limit: usize) -> Result<Option<String>, String> {
    value.map(|value| bounded_text(value, limit)).transpose()
}

fn bounded_text(value: String, limit: usize) -> Result<String, String> {
    let value = value.trim().to_owned();
    if value.len() > limit {
        return Err(format!("文档活动文本不能超过 {limit} 字节"));
    }
    Ok(value)
}

#[tauri::command]
pub async fn knowledge_document_source_metadata(
    state: State<'_, AppState>,
    request: KnowledgeDocumentRequest,
) -> Result<ApiResponse<KnowledgeDocumentSourceMetadata>, String> {
    let (service, token) = match configured_yuxi(&state) {
        Ok(value) => value,
        Err(error) => return Ok(ApiResponse::failure("yuxi.not_authenticated", error, false)),
    };
    match state
        .yuxi_client
        .knowledge_document_source_metadata(
            &service.base_url,
            &token,
            &request.knowledge_base_id,
            &request.document_id,
        )
        .await
    {
        Ok(metadata) => Ok(ApiResponse::success(metadata)),
        Err(error) => Ok(ApiResponse::failure(
            "knowledge.source_metadata_failed",
            error,
            true,
        )),
    }
}

#[tauri::command]
pub async fn knowledge_document_range(
    state: State<'_, AppState>,
    request: KnowledgeDocumentRangeRequest,
) -> Result<tauri::ipc::Response, String> {
    let (service, token) = configured_yuxi(&state)?;
    let bytes = state
        .yuxi_client
        .knowledge_document_range(
            &service.base_url,
            &token,
            &request.knowledge_base_id,
            &request.document_id,
            request.start,
            request.end,
            request.source_revision.trim(),
        )
        .await?;
    Ok(tauri::ipc::Response::new(bytes))
}

const KNOWLEDGE_PREVIEW_RANGE_BYTES: u64 = 4 * 1024 * 1024;
const KNOWLEDGE_PREVIEW_MAX_FILE_BYTES: u64 = 500 * 1024 * 1024;
const KNOWLEDGE_PREVIEW_VARIANT: &str = "original-v1";

fn knowledge_preview_cache_key(
    knowledge_base_id: &str,
    document_id: &str,
    source_revision: &str,
) -> String {
    let identity = serde_json::json!({
        "knowledgeBaseId": knowledge_base_id,
        "documentId": document_id,
        "sourceRevision": source_revision,
        "variant": KNOWLEDGE_PREVIEW_VARIANT,
    });
    hex::encode(Sha256::digest(identity.to_string().as_bytes()))
}

fn knowledge_preview_cache_dir(state: &AppState) -> PathBuf {
    state.data_dir.join("cache").join("knowledge-preview")
}

fn knowledge_preview_cache_path(
    cache_dir: &Path,
    cache_key: &str,
    filename: Option<&str>,
) -> PathBuf {
    let extension = filename
        .and_then(|value| Path::new(value).extension())
        .and_then(|value| value.to_str())
        .map(str::trim)
        .filter(|value| {
            !value.is_empty()
                && value.len() <= 16
                && value
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric())
        })
        .map(str::to_ascii_lowercase);
    match extension {
        Some(extension) => cache_dir.join(format!("{cache_key}.cache.{extension}")),
        None => cache_dir.join(format!("{cache_key}.cache")),
    }
}

fn managed_preview_cache_name(path: &Path) -> bool {
    let Some(filename) = path.file_name().and_then(|value| value.to_str()) else {
        return false;
    };
    let Some((cache_key, suffix)) = filename.split_once(".cache") else {
        return false;
    };
    !cache_key.is_empty()
        && cache_key.len() <= 128
        && cache_key
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
        && (suffix.is_empty()
            || suffix.strip_prefix('.').is_some_and(|extension| {
                !extension.is_empty()
                    && extension.len() <= 16
                    && extension
                        .chars()
                        .all(|character| character.is_ascii_alphanumeric())
            }))
}

fn valid_cached_preview(
    entry: &KnowledgePreviewCacheEntry,
    cache_dir: &Path,
    expected_size: u64,
) -> bool {
    if entry.byte_size != expected_size {
        return false;
    }
    managed_preview_cache_file(Path::new(&entry.storage_path), cache_dir).is_some_and(|path| {
        std::fs::metadata(path)
            .is_ok_and(|metadata| metadata.is_file() && metadata.len() == expected_size)
    })
}

fn managed_preview_cache_file(path: &Path, cache_dir: &Path) -> Option<PathBuf> {
    if !managed_preview_cache_name(path) {
        return None;
    }
    let root = cache_dir.canonicalize().ok()?;
    let path = path.canonicalize().ok()?;
    (path.starts_with(root) && path.is_file()).then_some(path)
}

struct KnowledgePreviewOperationGuard<'a> {
    state: &'a AppState,
    operation_id: String,
}

impl Drop for KnowledgePreviewOperationGuard<'_> {
    fn drop(&mut self) {
        let _ = self
            .state
            .finish_knowledge_preview_operation(&self.operation_id);
    }
}

#[tauri::command]
pub fn knowledge_preview_cache_operation_register(
    state: State<'_, AppState>,
    request: KnowledgePreviewCacheCancelRequest,
) -> ApiResponse<bool> {
    let operation_id = request.operation_id.trim();
    if operation_id.is_empty() {
        return ApiResponse::failure(
            "knowledge.preview_invalid_operation",
            "知识库文件预览缺少有效的操作标识",
            false,
        );
    }
    match state.register_knowledge_preview_operation(operation_id) {
        Ok(true) => ApiResponse::success(true),
        Ok(false) => ApiResponse::failure(
            "knowledge.preview_operation_exists",
            "知识库文件预览操作已存在，请重试",
            false,
        ),
        Err(error) => ApiResponse::failure("knowledge.preview_register_failed", error, true),
    }
}

#[tauri::command]
pub fn knowledge_preview_cache_operation_finish(
    state: State<'_, AppState>,
    request: KnowledgePreviewCacheCancelRequest,
) -> ApiResponse<bool> {
    let operation_id = request.operation_id.trim();
    if operation_id.is_empty() {
        return ApiResponse::failure(
            "knowledge.preview_invalid_operation",
            "知识库文件预览缺少有效的操作标识",
            false,
        );
    }
    state
        .finish_knowledge_preview_operation(operation_id)
        .map(ApiResponse::success)
        .unwrap_or_else(|error| {
            ApiResponse::failure("knowledge.preview_finish_failed", error, true)
        })
}

#[tauri::command]
pub async fn knowledge_preview_cache_acquire(
    state: State<'_, AppState>,
    request: KnowledgePreviewCacheAcquireRequest,
) -> Result<ApiResponse<KnowledgePreviewCacheLease>, String> {
    let operation_id = request.operation_id.trim();
    if operation_id.is_empty() {
        return Ok(ApiResponse::failure(
            "knowledge.preview_invalid_operation",
            "知识库文件预览缺少有效的操作标识",
            false,
        ));
    }
    let cancelled = match state.knowledge_preview_operation(operation_id)? {
        Some(cancelled) => cancelled,
        None => {
            return Ok(ApiResponse::failure(
                "knowledge.preview_operation_not_registered",
                "知识库文件预览操作尚未注册，请重试",
                true,
            ))
        }
    };
    let _operation_guard = KnowledgePreviewOperationGuard {
        state: state.inner(),
        operation_id: operation_id.to_owned(),
    };
    if cancelled.load(Ordering::Acquire) {
        return Ok(ApiResponse::failure(
            "knowledge.preview_cancelled",
            "预览已取消",
            false,
        ));
    }
    let knowledge_base_id = request.knowledge_base_id.trim();
    let document_id = request.document_id.trim();
    let source_revision = request.source_revision.trim();
    if knowledge_base_id.is_empty() || document_id.is_empty() || source_revision.is_empty() {
        return Ok(ApiResponse::failure(
            "knowledge.preview_invalid_identity",
            "知识库文件预览缺少有效的文件标识或源版本",
            false,
        ));
    }
    if request.size > KNOWLEDGE_PREVIEW_MAX_FILE_BYTES {
        let filename = request
            .filename
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("该文件");
        return Ok(ApiResponse::failure(
            "knowledge.preview_file_too_large",
            format!("{filename} 超过 Fox 本地预览缓存上限，请下载后查看"),
            false,
        ));
    }

    let cache_key = knowledge_preview_cache_key(knowledge_base_id, document_id, source_revision);
    let operation_lock = state.knowledge_preview_lock(&cache_key)?;
    let _operation = operation_lock.lock().await;
    if cancelled.load(Ordering::Acquire) {
        return Ok(ApiResponse::failure(
            "knowledge.preview_cancelled",
            "预览已取消",
            false,
        ));
    }
    let cache_dir = knowledge_preview_cache_dir(&state);
    fs::create_dir_all(&cache_dir).map_err(|error| error.to_string())?;
    let final_path =
        knowledge_preview_cache_path(&cache_dir, &cache_key, request.filename.as_deref());

    if let Some(entry) = state.database.knowledge_preview_cache_entry(&cache_key)? {
        let uses_current_filename = Path::new(&entry.storage_path) == final_path;
        if valid_cached_preview(&entry, &cache_dir, request.size) && uses_current_filename {
            if !state
                .database
                .acquire_knowledge_preview_cache_lease(&cache_key)?
            {
                return Ok(ApiResponse::failure(
                    "knowledge.preview_cache_missing",
                    "预览缓存记录已失效，请重试",
                    true,
                ));
            }
            return Ok(ApiResponse::success(KnowledgePreviewCacheLease {
                cache_key,
                media_type: entry.media_type.unwrap_or(request.media_type),
                size: entry.byte_size,
                cache_hit: true,
            }));
        }
        if entry.lease_count > 0 {
            return Ok(ApiResponse::failure(
                "knowledge.preview_cache_busy",
                "正在使用的预览缓存不完整，请关闭当前预览后重试",
                true,
            ));
        }
        if state
            .database
            .remove_knowledge_preview_cache_entry(&cache_key)?
        {
            let path = PathBuf::from(entry.storage_path);
            if let Some(path) = managed_preview_cache_file(&path, &cache_dir) {
                let _ = fs::remove_file(path);
            }
        }
    }

    let (service, token) = match configured_yuxi(&state) {
        Ok(value) => value,
        Err(error) => return Ok(ApiResponse::failure("yuxi.not_authenticated", error, false)),
    };
    if cancelled.load(Ordering::Acquire) {
        return Ok(ApiResponse::failure(
            "knowledge.preview_cancelled",
            "预览已取消",
            false,
        ));
    }
    let part_path = cache_dir.join(format!("{cache_key}.{}.part", Uuid::new_v4()));
    let write_result: Result<(), String> = async {
        let mut file = tokio::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&part_path)
            .await
            .map_err(|error| format!("无法创建预览缓存：{error}"))?;
        let mut start = 0_u64;
        while start < request.size {
            if cancelled.load(Ordering::Acquire) {
                return Err("预览已取消".to_owned());
            }
            let end = request
                .size
                .saturating_sub(1)
                .min(start + KNOWLEDGE_PREVIEW_RANGE_BYTES - 1);
            let bytes = state
                .yuxi_client
                .knowledge_document_range(
                    &service.base_url,
                    &token,
                    knowledge_base_id,
                    document_id,
                    start,
                    end,
                    source_revision,
                )
                .await?;
            if cancelled.load(Ordering::Acquire) {
                return Err("预览已取消".to_owned());
            }
            file.write_all(&bytes)
                .await
                .map_err(|error| format!("写入预览缓存失败：{error}"))?;
            start = end + 1;
        }
        file.flush()
            .await
            .map_err(|error| format!("刷新预览缓存失败：{error}"))?;
        file.sync_all()
            .await
            .map_err(|error| format!("同步预览缓存失败：{error}"))?;
        drop(file);
        replace_download_file(&part_path, &final_path)?;
        Ok(())
    }
    .await;
    if let Err(error) = write_result {
        let _ = tokio::fs::remove_file(&part_path).await;
        if error == "预览已取消" {
            return Ok(ApiResponse::failure(
                "knowledge.preview_cancelled",
                error,
                false,
            ));
        }
        return Ok(ApiResponse::failure(
            "knowledge.preview_cache_write_failed",
            error,
            true,
        ));
    }

    let now = crate::database::now_ms();
    state
        .database
        .upsert_knowledge_preview_cache_entry(&KnowledgePreviewCacheEntry {
            cache_key: cache_key.clone(),
            knowledge_base_id: knowledge_base_id.to_owned(),
            document_id: document_id.to_owned(),
            source_revision: source_revision.to_owned(),
            variant: KNOWLEDGE_PREVIEW_VARIANT.to_owned(),
            storage_path: final_path.to_string_lossy().into_owned(),
            media_type: Some(request.media_type.clone()),
            byte_size: request.size,
            lease_count: 0,
            created_at: now,
            last_accessed_at: now,
        })?;
    if !state
        .database
        .acquire_knowledge_preview_cache_lease(&cache_key)?
    {
        return Ok(ApiResponse::failure(
            "knowledge.preview_cache_missing",
            "预览缓存写入后未能建立租约，请重试",
            true,
        ));
    }
    let _ = crate::maintenance::cleanup_knowledge_preview_cache(&state);
    Ok(ApiResponse::success(KnowledgePreviewCacheLease {
        cache_key,
        media_type: request.media_type,
        size: request.size,
        cache_hit: false,
    }))
}

#[tauri::command]
pub fn knowledge_preview_cache_cancel(
    state: State<'_, AppState>,
    request: KnowledgePreviewCacheCancelRequest,
) -> ApiResponse<bool> {
    let operation_id = request.operation_id.trim();
    if operation_id.is_empty() {
        return ApiResponse::failure(
            "knowledge.preview_invalid_operation",
            "知识库文件预览缺少有效的操作标识",
            false,
        );
    }
    state
        .cancel_knowledge_preview(operation_id)
        .map(ApiResponse::success)
        .unwrap_or_else(|error| {
            ApiResponse::failure("knowledge.preview_cancel_failed", error, true)
        })
}

#[tauri::command]
pub async fn knowledge_preview_cache_read(
    state: State<'_, AppState>,
    request: KnowledgePreviewCacheReadRequest,
) -> Result<tauri::ipc::Response, String> {
    if request.end < request.start {
        return Err("预览缓存范围结束位置不能小于开始位置".to_owned());
    }
    let requested = request
        .end
        .checked_sub(request.start)
        .and_then(|value| value.checked_add(1))
        .ok_or_else(|| "预览缓存读取范围过大".to_owned())?;
    if requested > KNOWLEDGE_PREVIEW_RANGE_BYTES {
        return Err(format!(
            "单次预览缓存读取不能超过 {KNOWLEDGE_PREVIEW_RANGE_BYTES} 字节"
        ));
    }
    let entry = state
        .database
        .knowledge_preview_cache_entry(request.cache_key.trim())?
        .ok_or_else(|| "预览缓存不存在或已被清理".to_owned())?;
    if entry.lease_count == 0 {
        return Err("预览缓存租约已释放".to_owned());
    }
    if request.end >= entry.byte_size {
        return Err("预览缓存读取范围超出文件大小".to_owned());
    }
    let cache_dir = knowledge_preview_cache_dir(&state);
    let path = managed_preview_cache_file(Path::new(&entry.storage_path), &cache_dir)
        .ok_or_else(|| "预览缓存文件路径无效".to_owned())?;
    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|error| format!("无法打开预览缓存：{error}"))?;
    file.seek(std::io::SeekFrom::Start(request.start))
        .await
        .map_err(|error| format!("无法定位预览缓存：{error}"))?;
    let mut bytes = vec![0_u8; requested as usize];
    file.read_exact(&mut bytes)
        .await
        .map_err(|error| format!("读取预览缓存失败：{error}"))?;
    state
        .database
        .touch_knowledge_preview_cache_entry(&entry.cache_key, crate::database::now_ms())?;
    Ok(tauri::ipc::Response::new(bytes))
}

#[tauri::command]
pub fn knowledge_preview_cache_release(
    state: State<'_, AppState>,
    request: KnowledgePreviewCacheReleaseRequest,
) -> ApiResponse<bool> {
    match state
        .database
        .release_knowledge_preview_cache_lease(request.cache_key.trim())
    {
        Ok(released) => {
            if released {
                let _ = crate::maintenance::cleanup_knowledge_preview_cache(&state);
            }
            ApiResponse::success(released)
        }
        Err(error) => ApiResponse::failure("knowledge.preview_release_failed", error, true),
    }
}

#[tauri::command]
pub fn knowledge_preview_cache_open(
    state: State<'_, AppState>,
    request: KnowledgePreviewCacheOpenRequest,
) -> ApiResponse<bool> {
    let cache_key = request.cache_key.trim();
    if cache_key.is_empty() {
        return ApiResponse::failure(
            "knowledge.preview_cache_key_missing",
            "知识库预览缓存标识不能为空",
            false,
        );
    }
    let entry = match state.database.knowledge_preview_cache_entry(cache_key) {
        Ok(Some(entry)) => entry,
        Ok(None) => {
            return ApiResponse::failure(
                "knowledge.preview_cache_missing",
                "预览缓存不存在或已被清理，请重新打开文档后再试",
                true,
            )
        }
        Err(error) => {
            return ApiResponse::failure("knowledge.preview_cache_open_failed", error, false)
        }
    };
    let cache_dir = knowledge_preview_cache_dir(&state);
    let Some(path) = managed_preview_cache_file(Path::new(&entry.storage_path), &cache_dir) else {
        return ApiResponse::failure(
            "knowledge.preview_cache_path_invalid",
            "预览缓存文件无效，请重新打开文档后再试",
            true,
        );
    };
    if !downloaded_file_can_open_directly(&path) {
        return ApiResponse::failure(
            "knowledge.preview_cache_type_blocked",
            "出于安全考虑，该文件类型不能直接使用本机应用打开，请下载原文件后查看",
            false,
        );
    }
    let added_external_lease = match state.register_externally_opened_preview_cache(cache_key) {
        Ok(value) => value,
        Err(error) => {
            return ApiResponse::failure("knowledge.preview_cache_open_failed", error, false)
        }
    };
    if added_external_lease {
        match state
            .database
            .acquire_knowledge_preview_cache_lease(cache_key)
        {
            Ok(true) => {}
            Ok(false) => {
                state.unregister_externally_opened_preview_cache(cache_key);
                return ApiResponse::failure(
                    "knowledge.preview_cache_missing",
                    "预览缓存已失效，请重新打开文档后再试",
                    true,
                );
            }
            Err(error) => {
                state.unregister_externally_opened_preview_cache(cache_key);
                return ApiResponse::failure("knowledge.preview_cache_open_failed", error, false);
            }
        }
    }
    match open_downloaded_file(&path, false) {
        Ok(()) => {
            let _ = state
                .database
                .touch_knowledge_preview_cache_entry(cache_key, crate::database::now_ms());
            ApiResponse::success(true)
        }
        Err(error) => {
            if added_external_lease {
                state.unregister_externally_opened_preview_cache(cache_key);
                let _ = state
                    .database
                    .release_knowledge_preview_cache_lease(cache_key);
            }
            ApiResponse::failure("knowledge.preview_cache_open_failed", error, false)
        }
    }
}

#[tauri::command]
pub async fn knowledge_document_download(
    app: AppHandle,
    state: State<'_, AppState>,
    request: KnowledgeDocumentDownloadRequest,
) -> Result<ApiResponse<Option<KnowledgeDocumentDownloadResult>>, String> {
    let download_id = request.download_id.trim().to_owned();
    if download_id.is_empty() {
        return Ok(ApiResponse::failure(
            "knowledge.invalid_download_id",
            "下载任务 ID 不能为空",
            false,
        ));
    }
    state.clear_knowledge_download_cancel(&download_id);
    let (service, token) = match configured_yuxi(&state) {
        Ok(value) => value,
        Err(error) => {
            state.clear_knowledge_download_cancel(&download_id);
            return Ok(ApiResponse::failure("yuxi.not_authenticated", error, false));
        }
    };
    let filename = safe_download_filename(&request.filename, "knowledge-file");

    #[cfg(windows)]
    let destination = match tokio::task::spawn_blocking({
        let filename = filename.clone();
        move || pick_save_file_windows(&filename)
    })
    .await
    {
        Ok(Ok(path)) => path,
        Ok(Err(error)) => {
            state.clear_knowledge_download_cancel(&download_id);
            return Ok(ApiResponse::failure(
                "knowledge.save_dialog_failed",
                error,
                true,
            ));
        }
        Err(error) => {
            state.clear_knowledge_download_cancel(&download_id);
            return Ok(ApiResponse::failure(
                "knowledge.save_dialog_failed",
                format!("系统保存对话框意外退出：{error}"),
                true,
            ));
        }
    };

    #[cfg(not(windows))]
    let destination: Option<PathBuf> = None;

    let Some(destination) = destination else {
        state.clear_knowledge_download_cancel(&download_id);
        return Ok(ApiResponse::success(None));
    };
    let parent = match destination.parent() {
        Some(parent) => parent,
        None => {
            state.clear_knowledge_download_cancel(&download_id);
            return Ok(ApiResponse::failure(
                "knowledge.invalid_download_path",
                "所选下载路径无效",
                false,
            ));
        }
    };
    if !parent.exists() {
        state.clear_knowledge_download_cancel(&download_id);
        return Ok(ApiResponse::failure(
            "knowledge.download_directory_missing",
            "所选下载目录不存在",
            false,
        ));
    }

    let temporary = parent.join(format!(".{}.{}.part", filename, Uuid::new_v4()));
    let mut response = match state
        .yuxi_client
        .knowledge_document_download(
            &service.base_url,
            &token,
            &request.knowledge_base_id,
            &request.document_id,
        )
        .await
    {
        Ok(response) => response,
        Err(error) => {
            state.clear_knowledge_download_cancel(&download_id);
            return Ok(ApiResponse::failure(
                "knowledge.document_download_failed",
                error,
                true,
            ));
        }
    };
    let media_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_owned();
    let expected_length = response.content_length();
    let emit_progress = |bytes_written: u64, status: &str| {
        let _ = app.emit(
            "fox://knowledge-download-progress",
            KnowledgeDownloadProgress {
                download_id: download_id.clone(),
                document_id: request.document_id.clone(),
                bytes_written,
                total_bytes: expected_length,
                status: status.to_owned(),
            },
        );
    };
    emit_progress(0, "downloading");

    let download_result = async {
        let mut file = tokio::fs::File::create(&temporary)
            .await
            .map_err(|error| format!("无法创建临时下载文件：{error}"))?;
        let mut bytes_written = 0_u64;
        let mut last_progress_bytes = 0_u64;
        let mut last_progress_at = Instant::now();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|error| format!("下载知识库文件失败：{error}"))?
        {
            if state.knowledge_download_cancelled(&download_id)? {
                return Err("下载已取消".to_owned());
            }
            file.write_all(&chunk)
                .await
                .map_err(|error| format!("写入下载文件失败：{error}"))?;
            bytes_written += chunk.len() as u64;
            if bytes_written.saturating_sub(last_progress_bytes) >= 256 * 1024
                || last_progress_at.elapsed() >= Duration::from_millis(100)
            {
                emit_progress(bytes_written, "downloading");
                last_progress_bytes = bytes_written;
                last_progress_at = Instant::now();
            }
        }
        if state.knowledge_download_cancelled(&download_id)? {
            return Err("下载已取消".to_owned());
        }
        file.flush()
            .await
            .map_err(|error| format!("刷新下载文件失败：{error}"))?;
        file.sync_all()
            .await
            .map_err(|error| format!("同步下载文件失败：{error}"))?;
        drop(file);
        if let Some(expected) = expected_length {
            if expected != bytes_written {
                return Err(format!(
                    "下载内容不完整：预期 {expected} 字节，实际 {bytes_written} 字节"
                ));
            }
        }
        replace_download_file(&temporary, &destination)?;
        Ok(bytes_written)
    }
    .await;

    match download_result {
        Ok(bytes_written) => {
            emit_progress(bytes_written, "completed");
            state.clear_knowledge_download_cancel(&download_id);
            let saved_filename = destination
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or(&filename)
                .to_owned();
            state.register_downloaded_file(&destination)?;
            let can_open_directly = downloaded_file_can_open_directly(&destination);
            Ok(ApiResponse::success(Some(
                KnowledgeDocumentDownloadResult {
                    path: destination.to_string_lossy().into_owned(),
                    filename: saved_filename,
                    media_type,
                    bytes_written,
                    can_open_directly,
                },
            )))
        }
        Err(error) => {
            let _ = tokio::fs::remove_file(&temporary).await;
            let cancelled = error == "下载已取消";
            emit_progress(0, if cancelled { "cancelled" } else { "failed" });
            state.clear_knowledge_download_cancel(&download_id);
            if cancelled {
                return Ok(ApiResponse::success(None));
            }
            Ok(ApiResponse::failure(
                "knowledge.document_download_failed",
                error,
                true,
            ))
        }
    }
}

#[tauri::command]
pub fn knowledge_document_download_cancel(
    state: State<'_, AppState>,
    request: KnowledgeDownloadCancelRequest,
) -> ApiResponse<bool> {
    match state.cancel_knowledge_download(request.download_id.trim()) {
        Ok(()) => ApiResponse::success(true),
        Err(error) => ApiResponse::failure("knowledge.download_cancel_failed", error, true),
    }
}

#[tauri::command]
pub fn downloaded_file_open(
    state: State<'_, AppState>,
    request: OpenDownloadedFileRequest,
) -> ApiResponse<bool> {
    let path = Path::new(request.path.trim());
    match state.is_registered_downloaded_file(path) {
        Ok(true) => {}
        Ok(false) => {
            return ApiResponse::failure(
                "knowledge.download_not_registered",
                "只能打开本次 Fox 运行期间成功下载的文件",
                false,
            )
        }
        Err(error) => return ApiResponse::failure("knowledge.download_open_failed", error, false),
    }
    match open_downloaded_file(path, request.reveal) {
        Ok(()) => ApiResponse::success(true),
        Err(error) => ApiResponse::failure("knowledge.download_open_failed", error, false),
    }
}

#[cfg(windows)]
fn open_project_folder(path: &Path) -> Result<(), String> {
    let path = path
        .canonicalize()
        .map_err(|error| format!("项目文件夹不存在或无法访问：{error}"))?;
    if !path.is_dir() {
        return Err("项目路径不是文件夹".to_owned());
    }
    std::process::Command::new("explorer.exe")
        .arg(path)
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("系统无法打开项目文件夹：{error}"))
}

#[cfg(not(windows))]
fn open_project_folder(_path: &Path) -> Result<(), String> {
    Err("当前平台暂不支持打开项目文件夹".to_owned())
}

fn downloaded_file_can_open_directly(path: &Path) -> bool {
    let Some(extension) = path.extension().and_then(|value| value.to_str()) else {
        return false;
    };
    matches!(
        extension.to_ascii_lowercase().as_str(),
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

#[cfg(windows)]
pub(crate) fn open_downloaded_file(path: &Path, reveal: bool) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    if !path.is_absolute() {
        return Err("只能打开绝对路径中的已下载文件".to_owned());
    }
    let path = path
        .canonicalize()
        .map_err(|error| format!("下载文件不存在或无法访问：{error}"))?;
    if !path.is_file() {
        return Err("下载目标不是普通文件".to_owned());
    }
    if !reveal && !downloaded_file_can_open_directly(&path) {
        return Err("出于安全考虑，该文件类型只能在所在文件夹中查看".to_owned());
    }

    let operation = "open"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let (target, parameters) = if reveal {
        let target = "explorer.exe"
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        let parameters = format!("/select,\"{}\"", path.display())
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        (target, Some(parameters))
    } else {
        (
            path.as_os_str()
                .encode_wide()
                .chain(std::iter::once(0))
                .collect::<Vec<_>>(),
            None,
        )
    };
    let result = unsafe {
        ShellExecuteW(
            None,
            PCWSTR(operation.as_ptr()),
            PCWSTR(target.as_ptr()),
            parameters
                .as_ref()
                .map_or(PCWSTR::null(), |value| PCWSTR(value.as_ptr())),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    if result.0 as isize <= 32 {
        return Err(format!(
            "系统无法打开下载文件（错误代码 {}）",
            result.0 as isize
        ));
    }
    Ok(())
}

#[cfg(not(windows))]
pub(crate) fn open_downloaded_file(_path: &Path, _reveal: bool) -> Result<(), String> {
    Err("当前平台暂不支持此操作".to_owned())
}

#[cfg(windows)]
fn replace_download_file(source: &Path, destination: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::GetLastError;
    use windows::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };

    let source = source
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    unsafe {
        MoveFileExW(
            PCWSTR(source.as_ptr()),
            PCWSTR(destination.as_ptr()),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
        .map_err(|_| format!("无法完成下载文件：Windows 错误 {:?}", GetLastError()))
    }
}

#[cfg(not(windows))]
fn replace_download_file(source: &Path, destination: &Path) -> Result<(), String> {
    std::fs::rename(source, destination).map_err(|error| format!("无法完成下载文件：{error}"))
}

#[tauri::command]
pub async fn knowledge_query(
    state: State<'_, AppState>,
    request: KnowledgeQueryRequest,
) -> Result<ApiResponse<Value>, String> {
    let query = request.query.trim();
    if query.is_empty() {
        return Ok(ApiResponse::failure(
            "knowledge.invalid_query",
            "检索内容不能为空",
            false,
        ));
    }
    let (service, token) = match configured_yuxi(&state) {
        Ok(value) => value,
        Err(error) => return Ok(ApiResponse::failure("yuxi.not_authenticated", error, false)),
    };
    match state
        .yuxi_client
        .query_knowledge(&service.base_url, &token, &request.knowledge_base_id, query)
        .await
    {
        Ok(value) => Ok(ApiResponse::success(value)),
        Err(error) => Ok(ApiResponse::failure("knowledge.query_failed", error, true)),
    }
}

#[tauri::command]
pub async fn knowledge_graph(
    state: State<'_, AppState>,
    request: GraphQueryRequest,
) -> Result<ApiResponse<Value>, String> {
    let (service, token) = match configured_yuxi(&state) {
        Ok(value) => value,
        Err(error) => return Ok(ApiResponse::failure("yuxi.not_authenticated", error, false)),
    };
    match state
        .yuxi_client
        .graph_subgraph(
            &service.base_url,
            &token,
            &request.knowledge_base_id,
            request.keyword.as_deref().unwrap_or(""),
            request.max_depth.unwrap_or(2).clamp(1, 5),
            request.max_nodes.unwrap_or(100).clamp(1, 500),
        )
        .await
    {
        Ok(value) => Ok(ApiResponse::success(value)),
        Err(error) => Ok(ApiResponse::failure("knowledge.graph_failed", error, true)),
    }
}

#[tauri::command]
pub async fn knowledge_graph_labels(
    state: State<'_, AppState>,
    request: KnowledgeIdRequest,
) -> Result<ApiResponse<Value>, String> {
    let (service, token) = match configured_yuxi(&state) {
        Ok(value) => value,
        Err(error) => return Ok(ApiResponse::failure("yuxi.not_authenticated", error, false)),
    };
    match state
        .yuxi_client
        .graph_labels(&service.base_url, &token, &request.knowledge_base_id)
        .await
    {
        Ok(value) => Ok(ApiResponse::success(value)),
        Err(error) => Ok(ApiResponse::failure(
            "knowledge.graph_labels_failed",
            error,
            true,
        )),
    }
}

#[tauri::command]
pub async fn knowledge_graph_stats(
    state: State<'_, AppState>,
    request: KnowledgeIdRequest,
) -> Result<ApiResponse<Value>, String> {
    let (service, token) = match configured_yuxi(&state) {
        Ok(value) => value,
        Err(error) => return Ok(ApiResponse::failure("yuxi.not_authenticated", error, false)),
    };
    match state
        .yuxi_client
        .graph_stats(&service.base_url, &token, &request.knowledge_base_id)
        .await
    {
        Ok(value) => Ok(ApiResponse::success(value)),
        Err(error) => Ok(ApiResponse::failure(
            "knowledge.graph_stats_failed",
            error,
            true,
        )),
    }
}

#[tauri::command]
pub fn model_service_get(state: State<'_, AppState>) -> ApiResponse<Option<ModelServiceRecord>> {
    match state.database.get_model_service() {
        Ok(service) => ApiResponse::success(service.map(|mut service| {
            service.credential_configured = api_key_configured(&service.base_url);
            service
        })),
        Err(error) => storage_error(error),
    }
}

#[tauri::command]
pub fn model_providers_list(state: State<'_, AppState>) -> ApiResponse<Vec<ModelProviderRecord>> {
    match state.database.list_model_providers() {
        Ok(providers) => ApiResponse::success(
            providers
                .into_iter()
                .map(|mut provider| {
                    provider.credential_configured = api_key_configured(&provider.base_url);
                    provider
                })
                .collect(),
        ),
        Err(error) => storage_error(error),
    }
}

#[tauri::command]
pub fn model_provider_save(
    state: State<'_, AppState>,
    request: SaveModelProviderRequest,
) -> ApiResponse<ModelProviderRecord> {
    let name = request.name.trim();
    if name.is_empty() || request.models.is_empty() {
        return ApiResponse::failure(
            "model.invalid_provider",
            "供应商名称和至少一个模型不能为空",
            false,
        );
    }
    if !matches!(
        request.api_type.as_str(),
        "openai-completions" | "anthropic-messages"
    ) {
        return ApiResponse::failure("model.unsupported_api", "不支持此 API 协议", false);
    }
    let base_url = match normalize_model_base_url(&request.base_url) {
        Ok(url) => url,
        Err(error) => return ApiResponse::failure("model.invalid_url", error, false),
    };
    const PROVIDER_ICONS: &[&str] = &[
        "alibaba-cloud.svg",
        "alibaba.svg",
        "deepseek.svg",
        "minimax.svg",
        "modelscope.svg",
        "moonshot.svg",
        "openai.svg",
        "opencode.svg",
        "openrouter.svg",
        "siliconflow.svg",
        "xiaomi.svg",
        "zai.svg",
        "zhipu.svg",
    ];
    let icon = request
        .icon
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    if icon
        .as_deref()
        .is_some_and(|value| !PROVIDER_ICONS.contains(&value))
    {
        return ApiResponse::failure("model.invalid_provider_icon", "不支持此供应商图标", false);
    }
    let provider_id = request.id.unwrap_or_else(|| Uuid::new_v4().to_string());
    let mut models = Vec::new();
    for (index, model) in request.models.into_iter().enumerate() {
        let model_id = normalize_model_id(&base_url, &model.model_id);
        if model_id.is_empty()
            || !(1_024..=4_000_000).contains(&model.context_window)
            || !(256..=262_144).contains(&model.max_output_tokens)
        {
            return ApiResponse::failure(
                "model.invalid_model",
                "模型 ID、上下文或输出长度不合法",
                false,
            );
        }
        models.push(ProviderModelRecord {
            id: model.id.unwrap_or_else(|| Uuid::new_v4().to_string()),
            model_id: model_id.clone(),
            display_name: if model.display_name.trim().is_empty() {
                model_id.clone()
            } else {
                model.display_name.trim().to_owned()
            },
            context_window: model.context_window,
            max_output_tokens: model.max_output_tokens,
            supports_image_input: model.supports_image_input,
            is_default: model.is_default || index == 0 && !request.is_default,
        });
    }
    if !models.iter().any(|model| model.is_default) {
        if let Some(model) = models.first_mut() {
            model.is_default = true;
        }
    }
    if request.clear_api_key {
        if let Err(error) = clear_api_key(&base_url) {
            return ApiResponse::failure("model.credential_delete_failed", error, true);
        }
    } else if let Some(api_key) = request
        .api_key
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        if let Err(error) = set_api_key(&base_url, api_key) {
            return ApiResponse::failure("model.credential_save_failed", error, true);
        }
    }
    let provider = ModelProviderRecord {
        id: provider_id,
        name: name.to_owned(),
        icon,
        base_url: base_url.clone(),
        api_type: request.api_type.clone(),
        enabled: true,
        is_default: request.is_default,
        credential_configured: false,
        connection_type: String::new(),
        last_status: "unknown".to_owned(),
        last_latency_ms: None,
        last_checked_at: None,
        models,
        created_at: 0,
        updated_at: 0,
    };
    let saved = match state.database.save_model_provider(&provider) {
        Ok(saved) => saved,
        Err(error) => return storage_error(error),
    };
    if saved.is_default {
        if let Some(model) = saved
            .models
            .iter()
            .find(|model| model.is_default)
            .or_else(|| saved.models.first())
        {
            let runtime_record = ModelServiceRecord {
                name: saved.name.clone(),
                base_url: saved.base_url.clone(),
                model_id: model.model_id.clone(),
                api_type: saved.api_type.clone(),
                context_window: model.context_window,
                max_output_tokens: model.max_output_tokens,
                supports_image_input: model.supports_image_input,
                enabled: true,
                credential_configured: false,
                connection_type: saved.connection_type.clone(),
                last_status: saved.last_status.clone(),
                last_latency_ms: saved.last_latency_ms,
                last_checked_at: saved.last_checked_at,
                created_at: 0,
                updated_at: 0,
            };
            if let Err(error) = state.database.save_model_service(&runtime_record) {
                return storage_error(error);
            }
            if let Err(error) = state.runtime_host.reload_configuration() {
                return ApiResponse::failure("model.runtime_busy", error, true);
            }
        }
    }
    let mut saved = saved;
    saved.credential_configured = api_key_configured(&saved.base_url);
    ApiResponse::success(saved)
}

#[tauri::command]
pub fn model_provider_delete(
    state: State<'_, AppState>,
    request: ModelProviderIdRequest,
) -> ApiResponse<bool> {
    let provider = state
        .database
        .list_model_providers()
        .ok()
        .and_then(|items| {
            items
                .into_iter()
                .find(|item| item.id == request.provider_id)
        });
    let Some(provider) = provider else {
        return ApiResponse::success(false);
    };
    if provider.is_default {
        return ApiResponse::failure(
            "model.default_provider",
            "请先将其他供应商设为默认，再删除当前供应商",
            false,
        );
    }
    match state.database.delete_model_provider(&request.provider_id) {
        Ok(deleted) => ApiResponse::success(deleted),
        Err(error) => storage_error(error),
    }
}

#[tauri::command]
pub fn model_service_save(
    state: State<'_, AppState>,
    request: SaveModelServiceRequest,
) -> ApiResponse<ModelServiceRecord> {
    let name = request.name.trim();
    let raw_model_id = request.model_id.trim();
    if name.is_empty() || raw_model_id.is_empty() {
        return ApiResponse::failure("model.invalid_config", "服务名称和模型 ID 不能为空", false);
    }
    if !matches!(
        request.api_type.as_str(),
        "openai-completions" | "anthropic-messages"
    ) {
        return ApiResponse::failure(
            "model.unsupported_api",
            "仅支持 OpenAI-compatible Chat Completions 或 Anthropic Messages",
            false,
        );
    }
    if !(1_024..=4_000_000).contains(&request.context_window)
        || !(256..=262_144).contains(&request.max_output_tokens)
    {
        return ApiResponse::failure("model.invalid_limits", "模型上下文或输出长度不合法", false);
    }
    let base_url = match normalize_model_base_url(&request.base_url) {
        Ok(url) => url,
        Err(error) => return ApiResponse::failure("model.invalid_url", error, false),
    };
    let model_id = normalize_model_id(&base_url, raw_model_id);
    if let Err(error) = state.runtime_host.reload_configuration() {
        return ApiResponse::failure("model.runtime_busy", error, true);
    }
    if request.clear_api_key {
        if let Err(error) = clear_api_key(&base_url) {
            return ApiResponse::failure("model.credential_delete_failed", error, true);
        }
    } else if let Some(api_key) = request
        .api_key
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        if let Err(error) = set_api_key(&base_url, api_key) {
            return ApiResponse::failure("model.credential_save_failed", error, true);
        }
    }
    let record = ModelServiceRecord {
        name: name.to_owned(),
        base_url,
        model_id,
        api_type: request.api_type,
        context_window: request.context_window,
        max_output_tokens: request.max_output_tokens,
        supports_image_input: request.supports_image_input,
        enabled: true,
        credential_configured: false,
        connection_type: String::new(),
        last_status: "unknown".to_owned(),
        last_latency_ms: None,
        last_checked_at: None,
        created_at: 0,
        updated_at: 0,
    };
    match state.database.save_model_service(&record) {
        Ok(mut service) => {
            service.credential_configured = api_key_configured(&service.base_url);
            ApiResponse::success(service)
        }
        Err(error) => storage_error(error),
    }
}

#[tauri::command]
pub async fn model_service_test(
    state: State<'_, AppState>,
    request: TestModelServiceRequest,
) -> Result<ApiResponse<ModelConnectionTest>, String> {
    let (base_url, api_key, api_type, model_id) = match request.base_url {
        Some(base_url) => {
            let normalized = match normalize_model_base_url(&base_url) {
                Ok(url) => url,
                Err(error) => return Ok(ApiResponse::failure("model.invalid_url", error, false)),
            };
            let key = request
                .api_key
                .filter(|value| !value.trim().is_empty())
                .or_else(|| get_api_key(&normalized));
            (
                normalized,
                key,
                request
                    .api_type
                    .unwrap_or_else(|| "openai-completions".to_owned()),
                request.model_id.unwrap_or_default(),
            )
        }
        None => match state.database.get_model_service() {
            Ok(Some(service)) => {
                let key = get_api_key(&service.base_url);
                (service.base_url, key, service.api_type, service.model_id)
            }
            Ok(None) => {
                return Ok(ApiResponse::failure(
                    "model.not_configured",
                    "请先配置模型服务",
                    false,
                ))
            }
            Err(error) => return Ok(storage_error(error)),
        },
    };
    match state
        .model_service_client
        .test(&base_url, api_key.as_deref(), &api_type, &model_id)
        .await
    {
        Ok(result) => {
            let _ = state.database.record_model_connection_test(&result);
            Ok(ApiResponse::success(result))
        }
        Err(error) => {
            let _ = state.database.record_model_connection_failure(&base_url);
            Ok(ApiResponse::failure("model.connection_failed", error, true))
        }
    }
}
