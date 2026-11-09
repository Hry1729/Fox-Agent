//! C05: profile-page file actions — list, open and save-as for one contact's
//! provided files and generated outputs.
//!
//! Every action re-verifies the K2 ownership chain in the repository layer
//! before any filesystem work; the request's contact id alone grants nothing.
//! Artifacts reuse the gateway's root/integrity checks, attachments are
//! confined to their own conversation's host directory, and save-as always
//! asks the user for the destination through the native dialog.

use crate::{
    app_state::AppState,
    artifact_gateway::{self, ArtifactGatewayError},
    companion_workspace,
    database::{ApiResponse, AttachmentRecord, CompanionFile, CompanionFileEntry},
};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};
use tauri::State;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CompanionFilesRequest {
    companion_id: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CompanionFiles {
    provided: Vec<CompanionFileEntry>,
    generated: Vec<CompanionFileEntry>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CompanionFileRequest {
    companion_id: String,
    kind: String,
    file_id: String,
}

fn file_error(error: ArtifactGatewayError) -> ApiResponse<()> {
    ApiResponse::failure(error.code, error.message, error.retryable)
}

/// Attachments live in the host-private per-conversation directory. The same
/// symlink / regular-file / canonical-root rules as the artifact gateway apply;
/// a missing file is an explicit state, never a silent success.
fn attachment_path(data_dir: &Path, record: &AttachmentRecord) -> Result<PathBuf, ArtifactGatewayError> {
    let missing = || {
        ArtifactGatewayError::new("companion.file_missing", "文件已不存在，可能已被移动或删除", false)
    };
    let root = data_dir
        .join("attachments")
        .join(&record.conversation_id)
        .canonicalize()
        .map_err(|_| missing())?;
    let raw = Path::new(record.storage_path.trim());
    if !raw.is_absolute() {
        return Err(missing());
    }
    let link = fs::symlink_metadata(raw).map_err(|_| missing())?;
    if link.file_type().is_symlink() || !link.is_file() {
        return Err(missing());
    }
    let canonical = raw.canonicalize().map_err(|_| missing())?;
    if !canonical.starts_with(&root) {
        return Err(ArtifactGatewayError::new(
            "companion.file_not_found",
            "附件文件超出该联系人的目录范围",
            false,
        ));
    }
    Ok(canonical)
}

#[tauri::command]
pub(crate) fn companion_files_list(
    state: State<'_, AppState>,
    request: CompanionFilesRequest,
) -> ApiResponse<CompanionFiles> {
    match state.database.companion_files_list(&request.companion_id) {
        Ok(entries) => {
            let (provided, generated): (Vec<_>, Vec<_>) =
                entries.into_iter().partition(|entry| entry.kind == "attachment");
            ApiResponse::success(CompanionFiles { provided, generated })
        }
        Err(error) => ApiResponse::failure("companion.file_list_failed", error, true),
    }
}

#[tauri::command]
pub(crate) fn companion_file_open(
    state: State<'_, AppState>,
    request: CompanionFileRequest,
) -> ApiResponse<()> {
    let file = match state
        .database
        .companion_file_resolve(&request.companion_id, &request.kind, &request.file_id)
    {
        Ok(file) => file,
        Err(error) => return ApiResponse::failure("companion.file_not_found", error, false),
    };
    match file {
        CompanionFile::Attachment(record) => {
            let path = match attachment_path(&state.data_dir, &record) {
                Ok(path) => path,
                Err(error) => return file_error(error),
            };
            match crate::commands::open_downloaded_file(&path, false) {
                Ok(()) => ApiResponse::success(()),
                Err(error) => ApiResponse::failure("companion.file_open_failed", error, true),
            }
        }
        CompanionFile::Artifact(record) => {
            // The gateway re-checks the conversation scope, roots and integrity.
            match artifact_gateway::action(
                &state,
                &artifact_gateway::ArtifactActionRequest {
                    conversation_id: record.conversation_id.clone(),
                    artifact_id: record.id.clone(),
                    action: "open".into(),
                    application_id: None,
                },
            ) {
                Ok(_) => ApiResponse::success(()),
                Err(error) => file_error(error),
            }
        }
    }
}

#[tauri::command]
pub(crate) async fn companion_file_save(
    state: State<'_, AppState>,
    request: CompanionFileRequest,
) -> Result<ApiResponse<Option<PathBuf>>, String> {
    let database = state.database.clone();
    let data_dir = state.data_dir.clone();
    let result: Result<Option<PathBuf>, ArtifactGatewayError> = tokio::task::spawn_blocking(move || {
        let file = database
            .companion_file_resolve(&request.companion_id, &request.kind, &request.file_id)
            .map_err(|error| ArtifactGatewayError::new("companion.file_not_found", error, false))?;
        let resolve = |file: &CompanionFile| -> Result<PathBuf, ArtifactGatewayError> {
            match file {
                CompanionFile::Attachment(record) => attachment_path(&data_dir, record),
                CompanionFile::Artifact(record) => artifact_gateway::validated_path(
                    &database,
                    &data_dir,
                    &record.conversation_id,
                    &record.id,
                ),
            }
        };
        let source = resolve(&file)?;
        #[cfg(windows)]
        let destination = crate::commands::pick_save_file_windows(file.name())
            .map_err(|error| ArtifactGatewayError::new("companion.export_failed", error, true))?;
        #[cfg(not(windows))]
        let destination: Option<PathBuf> = {
            let _ = source;
            return Err(ArtifactGatewayError::new(
                "companion.export_failed",
                "此平台尚无系统保存对话框",
                false,
            ));
        };
        if let Some(destination) = &destination {
            // Re-resolve after the dialog, like the workspace export does.
            let source = resolve(&file)?;
            companion_workspace::export_copy(&source, destination)
                .map_err(|error| ArtifactGatewayError::new("companion.export_failed", error, true))?;
        }
        Ok(destination)
    })
    .await
    .map_err(|error| error.to_string())?;
    Ok(match result {
        Ok(path) => ApiResponse::success(path),
        Err(error) => ApiResponse::failure(error.code, error.message, error.retryable),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::Database;

    /// 验收：缺失文件明确状态；附件路径越过会话目录即拒绝。 These run against a
    /// real database and real files, without the Tauri command shell.
    #[test]
    fn attachment_paths_stay_inside_their_conversation_directory() {
        let root = std::env::temp_dir().join(format!("fox-c05-paths-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let db = Database::open(root.join("fox.db")).unwrap();
        let view = db.add_companion(&root, "fox-debugger").unwrap();
        let chat = view.conversation_id.clone();
        let dir = root.join("attachments").join(&chat);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("att-1.txt");
        fs::write(&path, b"hello").unwrap();
        let record = AttachmentRecord {
            id: "att-1".into(),
            conversation_id: chat.clone(),
            message_id: Some("msg-1".into()),
            display_name: "你好.txt".into(),
            storage_path: path.to_string_lossy().into_owned(),
            media_type: None,
            byte_size: 5,
            sha256: None,
            status: "ready".into(),
            created_at: 1,
        };
        assert_eq!(attachment_path(&root, &record).unwrap(), path.canonicalize().unwrap());

        // Missing file: explicit state, not a panic or a silent ok.
        fs::remove_file(&path).unwrap();
        let error = attachment_path(&root, &record).unwrap_err();
        assert_eq!(error.code, "companion.file_missing");

        // A path pointing at another conversation's directory is refused even
        // if the file exists.
        let other = root.join("attachments").join("other-chat");
        fs::create_dir_all(&other).unwrap();
        let foreign = other.join("att-2.txt");
        fs::write(&foreign, b"other").unwrap();
        let mut forged = record.clone();
        forged.storage_path = foreign.to_string_lossy().into_owned();
        assert_eq!(attachment_path(&root, &forged).unwrap_err().code, "companion.file_not_found");

        // Symlinks are refused like the artifact gateway does.
        #[cfg(unix)]
        {
            let link = dir.join("link.txt");
            std::os::unix::fs::symlink(&foreign, &link).unwrap();
            let mut linked = record.clone();
            linked.storage_path = link.to_string_lossy().into_owned();
            assert_eq!(attachment_path(&root, &linked).unwrap_err().code, "companion.file_missing");
        }
        drop(db);
        let _ = fs::remove_dir_all(&root);
    }
}