//! Conversation-scoped authorization and immutable input snapshots for JS computation.
use crate::database::Database;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

pub(crate) fn conversation_compute_root(sessions_dir: &Path, conversation_id: &str) -> PathBuf {
    sessions_dir
        .join("attachment-compute")
        .join(hex::encode(Sha256::digest(conversation_id.as_bytes())))
}

fn regular_directory(path: &Path) -> Result<(), String> {
    let m = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if m.file_attributes() & 0x400 != 0 {
            return Err("Compute workspace must not use a reparse point".into());
        }
    }
    if !m.is_dir() || m.file_type().is_symlink() {
        return Err("Compute workspace must be a regular directory".into());
    }
    Ok(())
}

pub(crate) fn authorized_artifact_root(
    sessions_dir: &Path,
    conversation_id: &str,
) -> Result<Option<PathBuf>, String> {
    let candidate = conversation_compute_root(sessions_dir, conversation_id);
    if !candidate.exists() {
        return Ok(None);
    }
    regular_directory(sessions_dir)?;
    regular_directory(&sessions_dir.join("attachment-compute"))?;
    regular_directory(&candidate)?;
    let expected = fs::canonicalize(sessions_dir)
        .map_err(|e| e.to_string())?
        .join("attachment-compute")
        .join(hex::encode(Sha256::digest(conversation_id.as_bytes())));
    let canonical = fs::canonicalize(candidate).map_err(|e| e.to_string())?;
    if canonical != expected {
        return Err("Compute artifact root escaped its conversation".into());
    }
    Ok(Some(canonical))
}

/// Create each fixed/hashed component separately and reject links before descending.
pub(crate) fn safe_workspace(
    sessions_dir: &Path,
    conversation_id: &str,
    run_id: &str,
) -> Result<PathBuf, String> {
    regular_directory(sessions_dir)?;
    let mut path = fs::canonicalize(sessions_dir).map_err(|e| e.to_string())?;
    for component in [
        "attachment-compute".to_owned(),
        hex::encode(Sha256::digest(conversation_id.as_bytes())),
        hex::encode(Sha256::digest(run_id.as_bytes())),
        uuid::Uuid::new_v4().to_string(),
    ] {
        path.push(component);
        match fs::create_dir(&path) {
            Ok(()) => (),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
            Err(e) => return Err(e.to_string()),
        }
        regular_directory(&path)?;
    }
    Ok(path)
}

pub(crate) fn execute(
    database: &Database,
    attachments_dir: &Path,
    sessions_dir: &Path,
    conversation_id: &str,
    run_id: &str,
    input: &Value,
    cancelled: impl Fn() -> bool + 'static,
) -> Result<Value, String> {
    let empty = Vec::new();
    let ids = input
        .get("attachmentIds")
        .map(|v| v.as_array().ok_or("attachmentIds must be an array"))
        .transpose()?
        .unwrap_or(&empty);
    let artifacts = input
        .get("artifactIds")
        .map(|v| v.as_array().ok_or("artifactIds must be an array"))
        .transpose()?
        .unwrap_or(&empty);
    if ids.len() + artifacts.len() > 8 {
        return Err(
            "Select at most eight conversation attachments or computed artifacts".into(),
        );
    }
    if input["code"]
        .as_str()
        .is_none_or(|code| code.trim().is_empty() || code.len() > 128 * 1024)
    {
        return Err("code must be nonempty JavaScript within 128 KiB".into());
    }
    let storage = fs::canonicalize(attachments_dir).map_err(|e| e.to_string())?;
    let mut snapshots = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut total = 0;
    // Validate every input before creating an output workspace.
    for value in ids {
        if cancelled() {
            return Err("Attachment computation cancelled".into());
        }
        let id = value
            .as_str()
            .filter(|id| !id.is_empty())
            .ok_or("Invalid attachment ID")?;
        if !seen.insert(id) {
            return Err("Duplicate attachment ID".into());
        }
        let a = database
            .attachment_for_conversation(conversation_id, id)?
            .ok_or("Attachment was not found in this conversation")?;
        let path = fs::canonicalize(&a.storage_path).map_err(|e| e.to_string())?;
        let metadata = fs::metadata(&path).map_err(|e| e.to_string())?;
        if !path.starts_with(&storage)
            || !metadata.is_file()
            || metadata.len() > 5 * 1024 * 1024
            || a.byte_size < 0
        {
            return Err("Attachment is outside storage or exceeds the 5 MiB file limit".into());
        }
        let bytes = fs::read(path).map_err(|e| e.to_string())?;
        total += bytes.len();
        if total > 20 * 1024 * 1024 {
            return Err("Compute inputs exceed 20 MiB".into());
        }
        if let Some(expected) = a.sha256.as_deref().filter(|s| !s.is_empty()) {
            let expected = expected.strip_prefix("sha256:").unwrap_or(expected);
            if !expected.eq_ignore_ascii_case(&hex::encode(Sha256::digest(&bytes))) {
                return Err("Attachment integrity check failed".into());
            }
        }
        let extension = Path::new(&a.display_name)
            .extension()
            .and_then(|x| x.to_str())
            .unwrap_or("txt")
            .to_ascii_lowercase();
        if !matches!(
            extension.as_str(),
            "xlsx" | "xls" | "xlsb" | "xlsm" | "ods" | "csv" | "tsv" | "json" | "txt"
        ) {
            return Err(
                "Computable inputs are XLSX, XLS, XLSB, XLSM, ODS, CSV, TSV, JSON and TXT".into(),
            );
        }
        snapshots.push((id.to_owned(), extension, bytes));
    }
    if !artifacts.is_empty() {
        let root = authorized_artifact_root(sessions_dir, conversation_id)?
            .ok_or("No computed files exist in this conversation")?;
        for value in artifacts {
            if cancelled() {
                return Err("Attachment computation cancelled".into());
            }
            let id = value
                .as_str()
                .filter(|id| !id.is_empty())
                .ok_or("Invalid artifact ID")?;
            if !seen.insert(id) {
                return Err("Duplicate input ID".into());
            }
            let artifact = database
                .artifact_for_conversation(conversation_id, id)?
                .ok_or("Computed artifact was not found in this conversation")?;
            if artifact.byte_size > 16 * 1024 * 1024 {
                return Err("Computed input exceeds 16 MiB".into());
            }
            let path = crate::artifact_gateway::validate_record(&artifact, &[root.clone()])
                .map_err(|e| e.message)?;
            let bytes = fs::read(path).map_err(|e| e.to_string())?;
            total += bytes.len();
            if total > 20 * 1024 * 1024 {
                return Err("Compute inputs exceed 20 MiB".into());
            }
            let extension = Path::new(&artifact.display_name)
                .extension()
                .and_then(|s| s.to_str())
                .unwrap_or("txt")
                .to_ascii_lowercase();
            if !matches!(
                extension.as_str(),
                "json" | "csv" | "tsv" | "txt" | "md" | "svg" | "html" | "htm" | "xml"
            ) {
                return Err("Computed input is not a supported text or data file".into());
            }
            snapshots.push((id.to_owned(), extension, bytes));
        }
    }
    let workspace = safe_workspace(sessions_dir, conversation_id, run_id)?;
    let input_root = workspace.join("inputs");
    fs::create_dir(&input_root).map_err(|e| e.to_string())?;
    let output_root = workspace.join("outputs");
    fs::create_dir(&output_root).map_err(|e| e.to_string())?;
    let mut paths = Vec::new();
    for (id, extension, bytes) in snapshots {
        let path = input_root.join(format!(
            "{}.{}",
            hex::encode(Sha256::digest(id.as_bytes())),
            extension
        ));
        let mut file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
            .map_err(|e| e.to_string())?;
        file.write_all(&bytes).map_err(|e| e.to_string())?;
        paths.push((id, path));
    }
    let mut normalized = input.clone();
    normalized["attachmentIds"] = json!(paths.iter().map(|(id, _)| id).collect::<Vec<_>>());
    let mut result = crate::data_compute::execute(&paths, &output_root, &normalized, cancelled)?;
    if let Some(files) = result["files"].as_array_mut() {
        for file in files {
            if let Some(path) = file["path"].as_str() {
                file["id"] = json!(Database::computed_artifact_id(path));
            }
        }
    }
    result["computedBy"] = json!("fox-quickjs");
    result["attachmentIds"] = json!(ids);
    Ok(result)
}
