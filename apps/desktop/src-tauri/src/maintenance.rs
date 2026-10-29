use crate::app_state::AppState;
use crate::database::now_ms;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, BufWriter, Read, Write};
use std::path::{Component, Path, PathBuf};

const BACKUP_MAGIC: &str = "FOXBACKUP1";
const MAX_ENTRY_BYTES: u64 = 512 * 1024 * 1024;
const MAX_ARCHIVE_BYTES: u64 = 4 * 1024 * 1024 * 1024;
pub const KNOWLEDGE_PREVIEW_CACHE_LIMIT: u64 = 500 * 1024 * 1024;
pub const KNOWLEDGE_PREVIEW_CACHE_LIMIT_MIN: u64 = 250 * 1024 * 1024;
pub const KNOWLEDGE_PREVIEW_CACHE_LIMIT_MAX: u64 = 5 * 1024 * 1024 * 1024;
const PART_FILE_MAX_AGE: std::time::Duration = std::time::Duration::from_secs(24 * 60 * 60);

fn stale_part_file(modified: std::time::SystemTime, now: std::time::SystemTime) -> bool {
    now.duration_since(modified)
        .is_ok_and(|age| age > PART_FILE_MAX_AGE)
}

pub fn configured_knowledge_preview_cache_limit(
    database: &crate::database::Database,
) -> Result<u64, String> {
    Ok(database
        .knowledge_preview_cache_limit(KNOWLEDGE_PREVIEW_CACHE_LIMIT)?
        .clamp(
            KNOWLEDGE_PREVIEW_CACHE_LIMIT_MIN,
            KNOWLEDGE_PREVIEW_CACHE_LIMIT_MAX,
        ))
}

/// Startup recovery of the knowledge preview cache. Never fatal: a broken
/// `cache/knowledge-preview` (e.g. a 1-byte file in place of the directory)
/// must not abort the Tauri setup hook. The problem is logged as a warning
/// and returned; it surfaces with its path (「预览缓存目录：<路径> 应为目录但是文件」)
/// only when a preview actually uses the cache, like `kernel-run-locks`.
pub fn recover_knowledge_preview_cache_at_startup(state: &AppState) -> Option<String> {
    recover_knowledge_preview_cache_at_startup_at(&state.database, &state.data_dir)
}

fn recover_knowledge_preview_cache_at_startup_at(
    database: &crate::database::Database,
    data_dir: &Path,
) -> Option<String> {
    let result = database
        .reset_knowledge_preview_cache_leases()
        .and_then(|_| cleanup_knowledge_preview_cache_at(database, data_dir));
    match result {
        Ok(_) => None,
        Err(error) => {
            eprintln!("fox.knowledge_preview warn startup_recovery_deferred error={error}");
            Some(error)
        }
    }
}

pub fn cleanup_knowledge_preview_cache(state: &AppState) -> Result<(usize, u64), String> {
    cleanup_knowledge_preview_cache_at(&state.database, &state.data_dir)
}

fn cleanup_knowledge_preview_cache_at(
    database: &crate::database::Database,
    data_dir: &Path,
) -> Result<(usize, u64), String> {
    let cache_dir = data_dir.join("cache").join("knowledge-preview");
    crate::data_paths::ensure_directory(&cache_dir, "预览缓存目录")?;
    let canonical_cache_dir = cache_dir
        .canonicalize()
        .map_err(|error| error.to_string())?;
    let now = std::time::SystemTime::now();
    let mut files = 0usize;
    let mut bytes = 0u64;
    if let Ok(entries) = fs::read_dir(&cache_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            if metadata.is_file()
                && path
                    .extension()
                    .is_some_and(|extension| extension == "part")
                && metadata
                    .modified()
                    .ok()
                    .is_some_and(|modified| stale_part_file(modified, now))
                && fs::remove_file(path).is_ok()
            {
                bytes += metadata.len();
                files += 1;
            }
        }
    }
    let known_cache_paths = database
        .knowledge_preview_cache_storage_paths()?
        .into_iter()
        .filter_map(|path| PathBuf::from(path).canonicalize().ok())
        .collect::<std::collections::HashSet<_>>();
    if let Ok(entries) = fs::read_dir(&cache_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !managed_cache_filename(&path) {
                continue;
            }
            let Some(path) = canonical_managed_cache_file(&path, &canonical_cache_dir) else {
                continue;
            };
            if known_cache_paths.contains(&path) {
                continue;
            }
            let byte_size = path.metadata().map(|metadata| metadata.len()).unwrap_or(0);
            if fs::remove_file(path).is_ok() {
                files += 1;
                bytes = bytes.saturating_add(byte_size);
            }
        }
    }
    let limit = configured_knowledge_preview_cache_limit(database)?;
    let mut usage = database.knowledge_preview_cache_usage()?;
    if usage > limit {
        for (cache_key, storage_path, byte_size) in
            database.knowledge_preview_cache_lru_candidates()?
        {
            if usage <= limit {
                break;
            }
            let path = PathBuf::from(storage_path);
            if !database.remove_knowledge_preview_cache_entry(&cache_key)? {
                continue;
            }
            if canonical_managed_cache_file(&path, &canonical_cache_dir).is_some() {
                let _ = fs::remove_file(&path);
            }
            usage = usage.saturating_sub(byte_size);
            bytes += byte_size;
            files += 1;
        }
    }
    Ok((files, bytes))
}

pub fn clear_unused_knowledge_preview_cache(state: &AppState) -> Result<MaintenanceResult, String> {
    clear_unused_knowledge_preview_cache_at(&state.database, &state.data_dir)
}

fn clear_unused_knowledge_preview_cache_at(
    database: &crate::database::Database,
    data_dir: &Path,
) -> Result<MaintenanceResult, String> {
    let cache_dir = data_dir.join("cache").join("knowledge-preview");
    crate::data_paths::ensure_directory(&cache_dir, "预览缓存目录")?;
    let canonical_cache_dir = cache_dir
        .canonicalize()
        .map_err(|error| error.to_string())?;
    let mut files = 0usize;
    let mut bytes = 0u64;

    for (cache_key, storage_path, byte_size) in database.knowledge_preview_cache_lru_candidates()? {
        if !database.remove_knowledge_preview_cache_entry(&cache_key)? {
            continue;
        }
        let path = PathBuf::from(storage_path);
        match canonical_managed_cache_file(&path, &canonical_cache_dir) {
            Some(path) => {
                fs::remove_file(path).map_err(|error| {
                    format!("清理预览缓存文件失败，稍后启动 Fox 时会再次处理：{error}")
                })?;
                files += 1;
                bytes = bytes.saturating_add(byte_size);
            }
            None if !path.exists() => {
                files += 1;
                bytes = bytes.saturating_add(byte_size);
            }
            None => {}
        }
    }

    Ok(MaintenanceResult {
        path: None,
        message: if files == 0 {
            "没有可清理的预览缓存；正在使用的文件已保留".to_owned()
        } else {
            format!("已清理 {files} 个未使用的预览缓存文件")
        },
        files,
        bytes,
        restart_required: false,
    })
}

fn canonical_managed_cache_file(path: &Path, canonical_cache_dir: &Path) -> Option<PathBuf> {
    if !managed_cache_filename(path) {
        return None;
    }
    let path = path.canonicalize().ok()?;
    (path.starts_with(canonical_cache_dir) && path.is_file()).then_some(path)
}

fn managed_cache_filename(path: &Path) -> bool {
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

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MaintenanceResult {
    pub path: Option<String>,
    pub message: String,
    pub files: usize,
    pub bytes: u64,
    pub restart_required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BackupManifest {
    format_version: u32,
    created_at: i64,
    fox_version: String,
    schema_version: i64,
    runtime_sessions_best_effort: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source_data_dir: Option<String>,
    entries: Vec<BackupEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BackupEntry {
    path: String,
    byte_size: u64,
    sha256: String,
}

pub fn create_diagnostics(state: &AppState) -> Result<MaintenanceResult, String> {
    let directory = state.data_dir.join("exports");
    fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    let path = directory.join(format!("fox-diagnostics-{}.json", now_ms()));
    let runtime = state.runtime_host.diagnostics();
    let yuxi = state.database.get_yuxi_service()?;
    let model = state.database.get_model_service()?;
    let mcp = state.database.list_mcp_servers()?;
    let report = diagnostic_report(
        serde_json::to_value(runtime).map_err(|error| error.to_string())?,
        state.database.schema_version()?,
        model,
        yuxi,
        mcp,
    );
    let bytes = crate::canonical_json::to_vec_pretty(&report).map_err(|error| error.to_string())?;
    fs::write(&path, &bytes).map_err(|error| error.to_string())?;
    Ok(MaintenanceResult {
        path: Some(path.to_string_lossy().into_owned()),
        message: "脱敏诊断包已导出".to_owned(),
        files: 1,
        bytes: bytes.len() as u64,
        restart_required: false,
    })
}

pub fn create_backup(state: &AppState) -> Result<MaintenanceResult, String> {
    let directory = state.data_dir.join("backups");
    fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    let staging = directory.join(format!(".staging-{}", now_ms()));
    fs::create_dir_all(&staging).map_err(|error| error.to_string())?;
    let db_snapshot = staging.join("fox.db");
    state.database.backup_to(&db_snapshot)?;
    let mut sources = vec![(db_snapshot.clone(), "fox.db".to_owned())];
    collect_files(
        &state.data_dir.join("attachments"),
        "attachments",
        &mut sources,
    )?;
    collect_files(
        &state.data_dir.join("runtime-sessions"),
        "runtime-sessions",
        &mut sources,
    )?;
    collect_files(&state.data_dir.join("skills"), "skills", &mut sources)?;
    let entries = sources
        .iter()
        .map(|(path, relative)| backup_entry(path, relative))
        .collect::<Result<Vec<_>, _>>()?;
    let manifest = BackupManifest {
        format_version: 1,
        created_at: now_ms(),
        fox_version: env!("CARGO_PKG_VERSION").to_owned(),
        schema_version: state.database.schema_version()?,
        runtime_sessions_best_effort: true,
        source_data_dir: Some(state.data_dir.to_string_lossy().into_owned()),
        entries,
    };
    let backup_path = directory.join(format!("fox-backup-{}.foxbackup", now_ms()));
    write_archive(&backup_path, &manifest, &sources)?;
    let metadata = fs::metadata(&backup_path).map_err(|error| error.to_string())?;
    let _ = fs::remove_dir_all(staging);
    Ok(MaintenanceResult {
        path: Some(backup_path.to_string_lossy().into_owned()),
        message: "Fox 核心数据已备份；Runtime Session 为尽力恢复，不保证跨版本兼容".to_owned(),
        files: manifest.entries.len(),
        bytes: metadata.len(),
        restart_required: false,
    })
}

pub fn queue_restore(data_dir: &Path, backup_path: &Path) -> Result<MaintenanceResult, String> {
    let (manifest, _) = read_archive(backup_path, None)?;
    if !manifest.entries.iter().any(|entry| entry.path == "fox.db") {
        return Err("备份缺少 fox.db".to_owned());
    }
    let pending = data_dir.join("restore-pending.foxbackup");
    fs::copy(backup_path, &pending).map_err(|error| error.to_string())?;
    Ok(MaintenanceResult {
        path: Some(pending.to_string_lossy().into_owned()),
        message: "备份已验证，将在下次启动 Fox 时恢复；Runtime Session 仅尽力兼容".to_owned(),
        files: manifest.entries.len(),
        bytes: fs::metadata(&pending)
            .map_err(|error| error.to_string())?
            .len(),
        restart_required: true,
    })
}

pub fn apply_pending_restore(data_dir: &Path) -> Result<(), String> {
    let pending = data_dir.join("restore-pending.foxbackup");
    if !pending.is_file() {
        return Ok(());
    }
    let staging = data_dir.join("restore-staging");
    let rollback = data_dir.join("restore-rollback");
    recover_restore_rollback(data_dir, &rollback)?;
    if staging.exists() {
        fs::remove_dir_all(&staging).map_err(|error| error.to_string())?;
    }
    fs::create_dir_all(&staging).map_err(|error| error.to_string())?;
    let (manifest, _) = read_archive(&pending, Some(&staging))?;
    let restored_db = staging.join("fox.db");
    validate_sqlite(&restored_db, manifest.schema_version)?;
    rebase_internal_paths(&restored_db, data_dir, manifest.source_data_dir.as_deref())?;
    fs::create_dir_all(&rollback).map_err(|error| error.to_string())?;
    let names = [
        "fox.db",
        "fox.db-wal",
        "fox.db-shm",
        "attachments",
        "runtime-sessions",
        "skills",
    ];
    for name in names {
        let target = data_dir.join(name);
        if target.exists() {
            fs::rename(&target, rollback.join(name))
                .map_err(|error| format!("无法准备恢复回滚：{error}"))?;
        }
    }
    let apply = (|| -> Result<(), String> {
        fs::rename(&restored_db, data_dir.join("fox.db")).map_err(|error| error.to_string())?;
        for directory in ["attachments", "runtime-sessions", "skills"] {
            let target = data_dir.join(directory);
            let source = staging.join(directory);
            if source.exists() {
                fs::rename(source, target).map_err(|error| error.to_string())?;
            } else {
                fs::create_dir_all(target).map_err(|error| error.to_string())?;
            }
        }
        Ok(())
    })();
    if let Err(error) = apply {
        for name in names {
            remove_path(&data_dir.join(name))?;
            let previous = rollback.join(name);
            if previous.exists() {
                fs::rename(previous, data_dir.join(name))
                    .map_err(|cause| format!("恢复失败且回滚失败：{cause}"))?;
            }
        }
        return Err(format!("恢复未应用，原数据已回滚：{error}"));
    }
    let _ = fs::remove_dir_all(staging);
    let _ = fs::remove_dir_all(rollback);
    fs::remove_file(pending).map_err(|error| error.to_string())
}

pub fn cleanup(state: &AppState, runtime_session_days: u64) -> Result<MaintenanceResult, String> {
    let mut files = 0usize;
    let mut bytes = 0u64;
    let cutoff = std::time::SystemTime::now().checked_sub(std::time::Duration::from_secs(
        runtime_session_days.clamp(1, 3650) * 86_400,
    ));
    if let Some(cutoff) = cutoff {
        let sessions = state.data_dir.join("runtime-sessions");
        if let Ok(entries) = fs::read_dir(sessions) {
            for entry in entries.flatten() {
                let path = entry.path();
                let Ok(metadata) = entry.metadata() else {
                    continue;
                };
                if metadata.is_file() && metadata.modified().ok().is_some_and(|time| time < cutoff)
                {
                    bytes += metadata.len();
                    files += 1;
                    let _ = fs::remove_file(path);
                }
            }
        }
    }
    let known = state
        .database
        .known_attachment_paths()?
        .into_iter()
        .filter_map(|path| fs::canonicalize(path).ok())
        .collect::<std::collections::HashSet<_>>();
    let attachments = state.data_dir.join("attachments");
    if let Ok(entries) = fs::read_dir(&attachments) {
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            if metadata.is_file()
                && fs::canonicalize(&path)
                    .ok()
                    .is_some_and(|path| !known.contains(&path))
            {
                bytes += metadata.len();
                files += 1;
                let _ = fs::remove_file(path);
            }
        }
    }
    let records = state.database.remove_failed_attachment_records()?;
    let (cache_files, cache_bytes) = cleanup_knowledge_preview_cache(state)?;
    files += cache_files;
    bytes += cache_bytes;
    Ok(MaintenanceResult {
        path: None,
        message: format!("已清理 {files} 个文件和 {records} 条失败附件记录"),
        files,
        bytes,
        restart_required: false,
    })
}

fn collect_files(
    root: &Path,
    prefix: &str,
    output: &mut Vec<(PathBuf, String)>,
) -> Result<(), String> {
    if !root.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(root).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let path = entry.path();
        if path.is_dir() {
            collect_files(
                &path,
                &format!("{prefix}/{}", entry.file_name().to_string_lossy()),
                output,
            )?;
        } else if path.is_file() {
            output.push((
                path,
                format!("{prefix}/{}", entry.file_name().to_string_lossy()),
            ));
        }
    }
    Ok(())
}

fn backup_entry(path: &Path, relative: &str) -> Result<BackupEntry, String> {
    let byte_size = fs::metadata(path).map_err(|error| error.to_string())?.len();
    if byte_size > MAX_ENTRY_BYTES {
        return Err(format!("备份条目超过 512 MB 限制：{relative}"));
    }
    let mut file = File::open(path).map_err(|error| error.to_string())?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(BackupEntry {
        path: relative.replace('\\', "/"),
        byte_size,
        sha256: hex::encode(hasher.finalize()),
    })
}

fn write_archive(
    path: &Path,
    manifest: &BackupManifest,
    sources: &[(PathBuf, String)],
) -> Result<(), String> {
    let mut writer = BufWriter::new(File::create(path).map_err(|error| error.to_string())?);
    writeln!(writer, "{BACKUP_MAGIC}").map_err(|error| error.to_string())?;
    let manifest_bytes = crate::canonical_json::to_vec(manifest).map_err(|error| error.to_string())?;
    writeln!(writer, "{}", manifest_bytes.len()).map_err(|error| error.to_string())?;
    writer
        .write_all(&manifest_bytes)
        .map_err(|error| error.to_string())?;
    writer.write_all(b"\n").map_err(|error| error.to_string())?;
    for (source, relative) in sources {
        let size = fs::metadata(source)
            .map_err(|error| error.to_string())?
            .len();
        writeln!(writer, "{}\t{}", relative.replace('\\', "/"), size)
            .map_err(|error| error.to_string())?;
        let mut file = File::open(source).map_err(|error| error.to_string())?;
        std::io::copy(&mut file, &mut writer).map_err(|error| error.to_string())?;
        writer.write_all(b"\n").map_err(|error| error.to_string())?;
    }
    writer.flush().map_err(|error| error.to_string())
}

fn read_archive(path: &Path, destination: Option<&Path>) -> Result<(BackupManifest, u64), String> {
    let mut reader = BufReader::new(File::open(path).map_err(|error| error.to_string())?);
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .map_err(|error| error.to_string())?;
    if line.trim() != BACKUP_MAGIC {
        return Err("不是有效的 Fox 备份".to_owned());
    }
    line.clear();
    reader
        .read_line(&mut line)
        .map_err(|error| error.to_string())?;
    let length = line
        .trim()
        .parse::<usize>()
        .map_err(|_| "备份清单长度无效".to_owned())?;
    if length > 4 * 1024 * 1024 {
        return Err("备份清单过大".to_owned());
    }
    let mut bytes = vec![0; length];
    reader
        .read_exact(&mut bytes)
        .map_err(|error| error.to_string())?;
    let mut newline = [0u8; 1];
    reader
        .read_exact(&mut newline)
        .map_err(|error| error.to_string())?;
    let manifest: BackupManifest =
        serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    if manifest.format_version != 1 {
        return Err("不支持的 Fox 备份版本".to_owned());
    }
    let mut seen = std::collections::HashSet::new();
    for expected in &manifest.entries {
        validate_relative(&expected.path)?;
        if !seen.insert(expected.path.clone()) {
            return Err("备份清单包含重复路径".to_owned());
        }
        if expected.byte_size > MAX_ENTRY_BYTES {
            return Err("备份条目超过 512 MB 限制".to_owned());
        }
    }
    if manifest
        .entries
        .iter()
        .try_fold(0u64, |total, entry| total.checked_add(entry.byte_size))
        .is_none_or(|total| total > MAX_ARCHIVE_BYTES)
    {
        return Err("备份内容超过 4 GB 限制".to_owned());
    }
    let mut total = 0u64;
    for expected in &manifest.entries {
        line.clear();
        reader
            .read_line(&mut line)
            .map_err(|error| error.to_string())?;
        let (name, size) = line.trim_end().split_once('\t').ok_or("备份条目头无效")?;
        let size = size
            .parse::<u64>()
            .map_err(|_| "备份条目大小无效".to_owned())?;
        if name != expected.path
            || size != expected.byte_size
            || size > MAX_ENTRY_BYTES
            || total.saturating_add(size) > MAX_ARCHIVE_BYTES
        {
            return Err("备份条目与清单不一致".to_owned());
        }
        let target = destination.map(|destination| destination.join(Path::new(&expected.path)));
        if let Some(parent) = target.as_ref().and_then(|target| target.parent()) {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        let mut output = target
            .as_ref()
            .map(File::create)
            .transpose()
            .map_err(|error| error.to_string())?;
        let mut remaining = size;
        let mut hasher = Sha256::new();
        let mut buffer = [0u8; 64 * 1024];
        while remaining > 0 {
            let count = usize::try_from(remaining.min(buffer.len() as u64)).unwrap_or(buffer.len());
            reader
                .read_exact(&mut buffer[..count])
                .map_err(|error| error.to_string())?;
            hasher.update(&buffer[..count]);
            if let Some(output) = output.as_mut() {
                output
                    .write_all(&buffer[..count])
                    .map_err(|error| error.to_string())?;
            }
            remaining -= count as u64;
        }
        reader
            .read_exact(&mut newline)
            .map_err(|error| error.to_string())?;
        if newline[0] != b'\n' || hex::encode(hasher.finalize()) != expected.sha256 {
            return Err(format!("备份条目校验失败：{}", expected.path));
        }
        total += size;
    }
    Ok((manifest, total))
}

fn validate_relative(path: &str) -> Result<(), String> {
    let path = Path::new(path);
    if path.is_absolute()
        || path.as_os_str().is_empty()
        || path.to_string_lossy().chars().any(char::is_control)
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err("备份包含不安全路径".to_owned());
    }
    Ok(())
}

fn recover_restore_rollback(data_dir: &Path, rollback: &Path) -> Result<(), String> {
    if !rollback.is_dir() {
        return Ok(());
    }
    for name in [
        "fox.db",
        "fox.db-wal",
        "fox.db-shm",
        "attachments",
        "runtime-sessions",
        "skills",
    ] {
        let previous = rollback.join(name);
        let target = data_dir.join(name);
        if previous.exists() && !target.exists() {
            fs::rename(previous, target)
                .map_err(|error| format!("无法恢复上次中断的回滚：{error}"))?;
        }
    }
    fs::remove_dir_all(rollback).map_err(|error| error.to_string())
}

fn remove_path(path: &Path) -> Result<(), String> {
    if !path.exists() {
        return Ok(());
    }
    if path.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
    .map_err(|error| error.to_string())
}

fn validate_sqlite(path: &Path, manifest_schema_version: i64) -> Result<(), String> {
    let actual = crate::database::Database::inspect_compatible_schema_version(path)?
        .filter(|version| *version > 0)
        .ok_or_else(|| "备份数据库缺少 Fox schema".to_owned())?;
    if actual != manifest_schema_version {
        return Err(format!(
            "备份清单数据库版本 {manifest_schema_version} 与实际数据库版本 {actual} 不一致"
        ));
    }
    let connection = rusqlite::Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| error.to_string())?;
    let ok: String = connection
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .map_err(|error| error.to_string())?;
    if ok != "ok" {
        return Err(format!("备份数据库完整性校验失败：{ok}"));
    }
    Ok(())
}

struct RestoreSessionPath {
    table: &'static str,
    rowid: i64,
    native: bool,
    snapshot: Option<String>,
    path: Option<String>,
    native_ref: Option<String>,
    session_id: String,
}

fn absolute_restore_path(value: &str) -> bool {
    let value=value.replace('\\',"/");
    value.starts_with('/') || matches!(value.as_bytes(), [drive,b':',b'/',..] if drive.is_ascii_alphabetic())
}

fn restore_relative(root: &Path, named: &str) -> Result<Option<PathBuf>, String> {
    if !absolute_restore_path(named) || named.contains('\0') || named.split(['/', '\\']).any(|part|part=="..") {
        return Err("备份会话路径必须是没有上级跳转的绝对路径".into());
    }
    // The shared path identity handles drive/UNC namespaces, case and segment
    // boundaries. Its API accepts descendants; the probe also admits root==cwd.
    let probe=format!("{}/.fox-restore-boundary",named.trim_end_matches(['/', '\\']));
    let Some(relative)=crate::runtime_host::delivery::project_relative_path_for_u1(&probe,root.to_str()) else {return Ok(None)};
    let mut relative=PathBuf::from(relative);
    relative.pop();
    Ok(Some(relative))
}

fn inferred_backup_root(named: &str, directory: &str) -> Option<PathBuf> {
    let unified=named.replace('\\',"/");
    let plain=match unified.strip_prefix("//?/").or_else(||unified.strip_prefix("//./")) {
        Some(rest) if rest.get(..4).is_some_and(|prefix|prefix.eq_ignore_ascii_case("unc/"))=>format!("//{}",&rest[4..]),
        Some(rest)=>rest.to_owned(),None=>unified,
    };
    let parts:Vec<_>=plain.split('/').collect();
    let at=parts.iter().rposition(|part|part.eq_ignore_ascii_case(directory))?;
    if at==0 || at+1>=parts.len() {return None;}
    let mut root=parts[..at].join("/");
    if root.ends_with(':') {root.push('/');}
    absolute_restore_path(&root).then(||PathBuf::from(root))
}

fn restored_native_header(bytes: &[u8], native_ref: Option<&str>, old_root: &Path, new_root: &Path) -> Result<Option<Vec<u8>>,String> {
    let end=bytes.iter().position(|byte|*byte==b'\n').unwrap_or(bytes.len());
    let line_end=if end>0&&bytes[end-1]==b'\r' {end-1} else {end};
    let mut header:Value=serde_json::from_slice(&bytes[..line_end]).map_err(|error|format!("备份 Pi 会话头损坏：{error}"))?;
    if header["type"]!="session" || !matches!(header["version"].as_i64(),Some(1..=3))
        || header["id"].as_str().is_none_or(str::is_empty) {
        return Err("备份 Pi 会话头类型或版本不受支持".into());
    }
    if native_ref.filter(|id|!id.is_empty()).is_some_and(|id|header["id"].as_str()!=Some(id)) {
        return Err("备份 Pi 会话身份与冻结映射不一致".into());
    }
    let Some(cwd)=header["cwd"].as_str() else {return Ok(None)};
    if !absolute_restore_path(cwd) {return Ok(None);}
    if restore_relative(new_root,cwd)?.is_some() {return Ok(None);}
    let Some(relative)=restore_relative(old_root,cwd)? else {return Ok(None)};
    let mapped=new_root.join(relative).to_string_lossy().into_owned();
    if cwd==mapped {return Ok(None);}
    header["cwd"]=json!(mapped);
    let mut rewritten=crate::canonical_json::to_vec(&header).map_err(|error|error.to_string())?;
    rewritten.extend_from_slice(&bytes[line_end..]);
    Ok(Some(rewritten))
}

fn restore_session_relative(old_root: &Path, new_root: &Path, named: &str) -> Result<PathBuf,String> {
    let relative=restore_relative(&old_root.join("runtime-sessions"),named)?
        .or(restore_relative(&new_root.join("runtime-sessions"),named)?);
    relative.filter(|path|!path.as_os_str().is_empty()).ok_or_else(||"备份路径不在原数据根的会话目录内".into())
}

fn verify_staged_session_parents(directory: &Path, relative: &Path) -> Result<(),String> {
    let mut current=directory.to_path_buf();
    for component in std::iter::once(None).chain(relative.parent().into_iter().flat_map(Path::components).map(Some)) {
        match component {Some(Component::Normal(part))=>current.push(part),None=>{},_=>return Err("恢复会话路径包含非相对目录".into())}
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir()=>return Err("恢复会话目录包含重定向或非目录路径".into()),
            Ok(_)=>{},Err(error) if error.kind()==std::io::ErrorKind::NotFound=>{},
            Err(error)=>return Err(error.to_string()),
        }
    }
    Ok(())
}

fn rewrite_staged_session_header(path: &Path, expected: &[u8], header: &[u8]) -> Result<(),String> {
    let temporary=path.with_file_name(format!(".fox-restore-{}.tmp",uuid::Uuid::new_v4()));
    let result=(|| {
        let mut source=BufReader::new(File::open(path).map_err(|error|error.to_string())?);
        let mut old_header=Vec::new();
        Read::by_ref(&mut source).take(1_048_577).read_until(b'\n',&mut old_header).map_err(|error|error.to_string())?;
        if old_header.len()>1_048_576 {return Err("备份 Pi 会话头过大".into());}
        if old_header!=expected {return Err("备份 Pi 会话在恢复校验后发生变化".into());}
        let mut output=fs::OpenOptions::new().write(true).create_new(true).open(&temporary).map_err(|error|error.to_string())?;
        output.write_all(header).map_err(|error|error.to_string())?;
        std::io::copy(&mut source,&mut output).map_err(|error|error.to_string())?;
        output.sync_all().map_err(|error|error.to_string())?;
        drop(output);drop(source);
        fs::rename(&temporary,path).map_err(|error|error.to_string())
    })();
    if result.is_err() {let _=fs::remove_file(&temporary);}
    result
}

fn rebase_internal_paths(database_path: &Path, data_dir: &Path, source_data_dir: Option<&str>) -> Result<(), String> {
    restore_relative(data_dir,&data_dir.to_string_lossy())?;
    let mut connection =
        rusqlite::Connection::open(database_path).map_err(|error| error.to_string())?;
    let mut paths=Vec::new();
    for table in ["runtime_sessions","runtime_session_generations"] {
        let exists:bool=connection.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",[table],|row|row.get(0))
            .map_err(|error|error.to_string())?;
        if !exists {continue;}
        let mut query=connection.prepare(&format!("PRAGMA table_info({table})")).map_err(|error|error.to_string())?;
        let columns=query.query_map([],|row|row.get::<_,String>(1)).map_err(|error|error.to_string())?
            .collect::<rusqlite::Result<Vec<_>>>().map_err(|error|error.to_string())?;
        let column=|name:&str,fallback:&str|if columns.iter().any(|column|column==name) {name.to_owned()} else {fallback.to_owned()};
        let sql=format!("SELECT rowid,session_path,{},{},{},runtime_session_id FROM {table} WHERE runtime_type='pi'",
            column("native_session_path","NULL"),column("native_session_ref","NULL"),column("format_version","1"));
        let mut query=connection.prepare(&sql).map_err(|error|error.to_string())?;
        let rows=query.query_map([],|row| {
            let format:i64=row.get(4)?;
            if !matches!(format,1|2) {return Err(rusqlite::Error::InvalidParameterName("不支持的备份会话格式".into()));}
            Ok(RestoreSessionPath {table,rowid:row.get(0)?,snapshot:row.get(1)?,path:row.get(2)?,native_ref:row.get(3)?,native:format==2,session_id:row.get(5)?})
        }).map_err(|error|error.to_string())?.collect::<rusqlite::Result<Vec<_>>>().map_err(|error|error.to_string())?;
        paths.extend(rows);
    }
    let mut inferred:Option<PathBuf>=None;
    let mut candidates=paths.iter().flat_map(|row|row.snapshot.iter().chain(row.path.iter())).map(|path|(path.as_str(),"runtime-sessions")).collect::<Vec<_>>();
    let mut query=connection.prepare("SELECT storage_path FROM attachments").map_err(|error|error.to_string())?;
    let attachments=query.query_map([],|row|row.get::<_,String>(0)).map_err(|error|error.to_string())?
        .collect::<rusqlite::Result<Vec<_>>>().map_err(|error|error.to_string())?;
    drop(query);
    candidates.extend(attachments.iter().map(|path|(path.as_str(),"attachments")));
    if source_data_dir.is_none() {
        for (path,directory) in candidates {
            let Some(root)=inferred_backup_root(path,directory) else {continue};
            if let Some(existing)=&inferred {
                if restore_relative(existing,&root.to_string_lossy())?!=Some(PathBuf::new()) {
                    return Err("旧备份的数据根不唯一，已拒绝猜测会话归属".into());
                }
            } else {inferred=Some(root);}
        }
    }
    let old_root=source_data_dir.map(PathBuf::from).or(inferred);
    if let Some(root)=&old_root {restore_relative(root,&root.to_string_lossy())?;}
    let staged_sessions=database_path.parent().ok_or("恢复数据库没有父目录")?.join("runtime-sessions");
    let new_sessions=data_dir.join("runtime-sessions");
    let mut mapped=Vec::new();
    let mut rewrites=std::collections::BTreeMap::new();
    for row in paths {
        if !row.native {
            let path=if row.table=="runtime_sessions" {
                let name=format!("{}.json",row.session_id);
                if name.split(['/', '\\']).count()!=1 || row.session_id.is_empty() || name.contains(':') {
                    return Err("旧快照的运行时身份不能用作会话文件名".into());
                }
                Some(new_sessions.join(name))
            } else {
                row.snapshot.as_deref().map(|path| {
                    let root=old_root.as_ref().ok_or("旧备份缺少会话数据根")?;
                    let relative=restore_session_relative(root,data_dir,path)?;
                    Ok::<_,String>(new_sessions.join(relative))
                }).transpose()?
            };
            mapped.push((row.table,row.rowid,false,path));
            continue;
        }
        let Some(path)=row.path.as_deref().filter(|path|!path.trim().is_empty()) else {continue};
        let root=old_root.as_ref().ok_or("旧备份缺少原生会话数据根")?;
        let relative=restore_session_relative(root,data_dir,path)?;
        verify_staged_session_parents(&staged_sessions,&relative)?;
        let copied=staged_sessions.join(&relative);
        match fs::symlink_metadata(&copied) {
            Ok(metadata)=> {
                if !metadata.is_file() || metadata.file_type().is_symlink() {return Err("备份原生会话不是普通文件".into());}
                let canonical=copied.canonicalize().map_err(|error|error.to_string())?;
                let directory=staged_sessions.canonicalize().map_err(|error|error.to_string())?;
                if !crate::runtime_host::artifact_store::is_inside(&directory,&canonical) {return Err("备份原生会话跳出了暂存目录".into());}
                let reader=BufReader::new(File::open(&copied).map_err(|error|error.to_string())?);
                let mut bytes=Vec::new();
                reader.take(1_048_577).read_until(b'\n',&mut bytes).map_err(|error|error.to_string())?;
                if bytes.len()>1_048_576 {return Err("备份 Pi 会话头过大".into());}
                if let Some(rewritten)=restored_native_header(&bytes,row.native_ref.as_deref(),root,data_dir)? {
                    if rewrites.get(&copied).is_some_and(|other:&(Vec<u8>,Vec<u8>)|other.1!=rewritten) {return Err("备份原生会话有冲突的归属".into());}
                    rewrites.insert(copied,(bytes,rewritten));
                }
            }
            Err(error) if error.kind()==std::io::ErrorKind::NotFound=>{},
            Err(error)=>return Err(error.to_string()),
        }
        mapped.push((row.table,row.rowid,true,Some(new_sessions.join(relative))));
    }
    let transaction=connection.transaction().map_err(|error|error.to_string())?;
    let mut statement = transaction
        .prepare("SELECT id, storage_path FROM attachments")
        .map_err(|error| error.to_string())?;
    let records = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|error| error.to_string())?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|error| error.to_string())?;
    drop(statement);
    for (id, old) in records {
        let name = Path::new(&old)
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or(&id);
        transaction
            .execute(
                "UPDATE attachments SET storage_path = ?2 WHERE id = ?1",
                rusqlite::params![
                    id,
                    data_dir.join("attachments").join(name).to_string_lossy()
                ],
            )
            .map_err(|error| error.to_string())?;
    }
    for (table,rowid,native,path) in mapped {
        let column=if native {"native_session_path"} else {"session_path"};
        transaction.execute(&format!("UPDATE {table} SET {column}=?2 WHERE rowid=?1"),
            rusqlite::params![rowid,path.map(|path|path.to_string_lossy().into_owned())]).map_err(|error|error.to_string())?;
    }
    for (path,(expected,bytes)) in rewrites {rewrite_staged_session_header(&path,&expected,&bytes)?;}
    transaction.commit().map_err(|error|error.to_string())
}

fn redact(value: &str) -> String {
    let mut result = value.to_owned();
    for key in [
        "authorization",
        "api_key",
        "apikey",
        "token",
        "password",
        "secret",
    ] {
        let lower = result.to_ascii_lowercase();
        if let Some(index) = lower.find(key) {
            result.replace_range(index.., "[REDACTED]");
        }
    }
    result
}

fn diagnostic_report(
    runtime: Value,
    schema_version: i64,
    model: Option<crate::database::ModelServiceRecord>,
    yuxi: Option<crate::database::YuxiServiceRecord>,
    mcp: Vec<crate::database::McpServerRecord>,
) -> Value {
    json!({
        "generatedAt": now_ms(),
        "foxVersion": env!("CARGO_PKG_VERSION"),
        "platform": std::env::consts::OS,
        "architecture": std::env::consts::ARCH,
        "database": { "schemaVersion": schema_version },
        "runtime": redact_value(runtime),
        "model": model.map(|item| json!({
            "name": item.name, "apiType": item.api_type, "connectionType": item.connection_type,
            "status": item.last_status, "credentialConfigured": item.credential_configured,
            "supportsImageInput": item.supports_image_input
        })),
        "yuxi": yuxi.map(|item| json!({
            "name": item.name, "connectionType": item.connection_type,
            "status": item.last_status, "version": item.last_version,
            "credentialConfigured": item.credential_configured
        })),
        "mcp": mcp.into_iter().map(|item| json!({
            "id": item.id, "name": item.name, "transport": item.transport,
            "enabled": item.enabled, "status": item.status,
            "lastLatencyMs": item.last_latency_ms, "toolCount": item.tool_count,
            "consecutiveFailures": item.consecutive_failures,
            "credentialConfigured": item.credential_configured,
            "lastError": item.last_error.map(|error| redact(&error))
        })).collect::<Vec<_>>(),
        "privacy": {
            "credentialsIncluded": false,
            "conversationContentIncluded": false,
            "projectFilesIncluded": false
        }
    })
}

fn redact_value(value: Value) -> Value {
    match value {
        Value::String(value) => Value::String(redact(&value)),
        Value::Array(values) => Value::Array(values.into_iter().map(redact_value).collect()),
        Value::Object(values) => Value::Object(
            values
                .into_iter()
                .map(|(key, value)| (key, redact_value(value)))
                .collect(),
        ),
        value => value,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn preview_cache_entry(
        cache_key: &str,
        storage_path: &Path,
        byte_size: u64,
        lease_count: u32,
    ) -> crate::database::KnowledgePreviewCacheEntry {
        crate::database::KnowledgePreviewCacheEntry {
            cache_key: cache_key.to_owned(),
            knowledge_base_id: "kb-1".to_owned(),
            document_id: format!("document-{cache_key}"),
            source_revision: "sha256:test".to_owned(),
            variant: "original".to_owned(),
            storage_path: storage_path.to_string_lossy().into_owned(),
            media_type: Some("application/octet-stream".to_owned()),
            byte_size,
            lease_count,
            created_at: 1,
            last_accessed_at: 1,
        }
    }

    #[test]
    fn stale_part_files_expire_after_twenty_four_hours() {
        let now =
            std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(10 * 24 * 60 * 60);
        assert!(stale_part_file(
            now - std::time::Duration::from_secs(25 * 60 * 60),
            now
        ));
        assert!(!stale_part_file(
            now - std::time::Duration::from_secs(23 * 60 * 60),
            now
        ));
        assert!(!stale_part_file(
            now + std::time::Duration::from_secs(60),
            now
        ));
    }

    #[test]
    fn preview_cache_file_in_place_of_directory_does_not_fail_startup() {
        let root = std::env::temp_dir().join(format!("fox-cache-startup-file-{}", Uuid::new_v4()));
        fs::create_dir_all(root.join("cache")).unwrap();
        let preview = root.join("cache").join("knowledge-preview");
        fs::write(&preview, b"x").unwrap();
        let database = crate::database::Database::open(root.join("fox.db")).unwrap();

        // Startup: deferred warning (with path), not an error that aborts setup.
        let warning = recover_knowledge_preview_cache_at_startup_at(&database, &root)
            .expect("broken cache directory is reported as a warning");
        assert_eq!(
            warning,
            format!("预览缓存目录：{} 应为目录但是文件", preview.display())
        );
        // The file is left untouched; nothing was deleted or replaced.
        assert!(preview.is_file());

        // Use: cleanup and manual clear fail with the path in the error.
        let error = cleanup_knowledge_preview_cache_at(&database, &root).unwrap_err();
        assert!(error.contains(&format!("{} 应为目录但是文件", preview.display())), "{error}");
        let error = clear_unused_knowledge_preview_cache_at(&database, &root).unwrap_err();
        assert!(error.contains("预览缓存目录："), "{error}");
        assert!(error.contains(&format!("{} 应为目录但是文件", preview.display())), "{error}");

        // A healthy (or missing) cache directory recovers silently.
        fs::remove_file(&preview).unwrap();
        assert!(recover_knowledge_preview_cache_at_startup_at(&database, &root).is_none());
        assert!(preview.is_dir());
        drop(database);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn cache_cleanup_paths_stay_inside_the_managed_directory() {
        let root = std::env::temp_dir().join(format!("fox-cache-cleanup-test-{}", Uuid::new_v4()));
        let cache = root.join("cache");
        fs::create_dir_all(&cache).unwrap();
        let canonical_cache = cache.canonicalize().unwrap();
        let managed = cache.join("managed.cache");
        let partial = cache.join("pending.part");
        let outside = root.join("outside.cache");
        fs::write(&managed, b"managed").unwrap();
        fs::write(&partial, b"partial").unwrap();
        fs::write(&outside, b"outside").unwrap();

        assert!(canonical_managed_cache_file(&managed, &canonical_cache).is_some());
        assert!(canonical_managed_cache_file(&partial, &canonical_cache).is_none());
        assert!(canonical_managed_cache_file(&outside, &canonical_cache).is_none());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn manual_preview_cache_clear_removes_idle_files_and_preserves_active_files() {
        let root = std::env::temp_dir().join(format!("fox-cache-clear-test-{}", Uuid::new_v4()));
        let cache = root.join("cache").join("knowledge-preview");
        fs::create_dir_all(&cache).unwrap();
        let idle = cache.join("idle.cache");
        let active = cache.join("active.cache");
        fs::write(&idle, b"idle").unwrap();
        fs::write(&active, b"active").unwrap();
        let database = crate::database::Database::open(root.join("fox.db")).unwrap();
        database
            .upsert_knowledge_preview_cache_entry(&preview_cache_entry("idle", &idle, 4, 0))
            .unwrap();
        database
            .upsert_knowledge_preview_cache_entry(&preview_cache_entry("active", &active, 6, 1))
            .unwrap();

        let result = clear_unused_knowledge_preview_cache_at(&database, &root).unwrap();

        assert_eq!(result.files, 1);
        assert_eq!(result.bytes, 4);
        assert!(!idle.exists());
        assert!(active.exists());
        assert!(database
            .knowledge_preview_cache_entry("idle")
            .unwrap()
            .is_none());
        assert!(database
            .knowledge_preview_cache_entry("active")
            .unwrap()
            .is_some());
        drop(database);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn preview_cache_maintenance_removes_orphans_without_touching_known_or_part_files() {
        let root = std::env::temp_dir().join(format!("fox-cache-orphan-test-{}", Uuid::new_v4()));
        let cache = root.join("cache").join("knowledge-preview");
        fs::create_dir_all(&cache).unwrap();
        let known = cache.join("known.cache");
        let orphan = cache.join("orphan.cache");
        let partial = cache.join("pending.part");
        fs::write(&known, b"known").unwrap();
        fs::write(&orphan, b"orphan").unwrap();
        fs::write(&partial, b"partial").unwrap();
        let database = crate::database::Database::open(root.join("fox.db")).unwrap();
        database
            .upsert_knowledge_preview_cache_entry(&preview_cache_entry("known", &known, 5, 1))
            .unwrap();

        let (files, bytes) = cleanup_knowledge_preview_cache_at(&database, &root).unwrap();

        assert_eq!(files, 1);
        assert_eq!(bytes, 6);
        assert!(known.exists());
        assert!(!orphan.exists());
        assert!(partial.exists());
        drop(database);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn configured_preview_cache_limit_evicts_idle_lru_and_preserves_active_entries() {
        let root = std::env::temp_dir().join(format!("fox-cache-limit-test-{}", Uuid::new_v4()));
        let cache = root.join("cache").join("knowledge-preview");
        fs::create_dir_all(&cache).unwrap();
        let idle = cache.join("idle.cache");
        let active = cache.join("active.cache");
        let idle_size = 256 * 1024 * 1024;
        File::create(&idle).unwrap().set_len(idle_size).unwrap();
        fs::write(&active, vec![0; 16]).unwrap();
        let database = crate::database::Database::open(root.join("fox.db")).unwrap();
        database
            .upsert_knowledge_preview_cache_entry(&preview_cache_entry("idle", &idle, idle_size, 0))
            .unwrap();
        database
            .upsert_knowledge_preview_cache_entry(&preview_cache_entry("active", &active, 16, 1))
            .unwrap();
        database
            .set_knowledge_preview_cache_limit(KNOWLEDGE_PREVIEW_CACHE_LIMIT_MIN)
            .unwrap();

        let (files, bytes) = cleanup_knowledge_preview_cache_at(&database, &root).unwrap();

        assert_eq!((files, bytes), (1, idle_size));
        assert!(!idle.exists());
        assert!(active.exists());
        assert!(database
            .knowledge_preview_cache_entry("active")
            .unwrap()
            .is_some());
        drop(database);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn configured_preview_cache_limit_clamps_invalid_persisted_values() {
        let root = std::env::temp_dir().join(format!("fox-cache-clamp-test-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let database = crate::database::Database::open(root.join("fox.db")).unwrap();

        database.set_knowledge_preview_cache_limit(1).unwrap();
        assert_eq!(
            configured_knowledge_preview_cache_limit(&database).unwrap(),
            KNOWLEDGE_PREVIEW_CACHE_LIMIT_MIN
        );
        database
            .set_knowledge_preview_cache_limit(u64::MAX)
            .unwrap();
        assert_eq!(
            configured_knowledge_preview_cache_limit(&database).unwrap(),
            KNOWLEDGE_PREVIEW_CACHE_LIMIT_MAX
        );

        drop(database);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn archive_round_trip_validates_hashes_and_paths() {
        let root = std::env::temp_dir().join(format!("fox-backup-test-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let source = root.join("fox.db");
        fs::write(&source, b"database").unwrap();
        let entry = backup_entry(&source, "fox.db").unwrap();
        let manifest = BackupManifest {
            format_version: 1,
            created_at: 1,
            fox_version: "test".to_owned(),
            schema_version: 1,
            runtime_sessions_best_effort: true,
            source_data_dir: None,
            entries: vec![entry],
        };
        let archive = root.join("backup.foxbackup");
        write_archive(&archive, &manifest, &[(source, "fox.db".to_owned())]).unwrap();
        let output = root.join("out");
        fs::create_dir_all(&output).unwrap();
        let (loaded, _) = read_archive(&archive, Some(&output)).unwrap();
        assert_eq!(loaded.entries.len(), 1);
        assert_eq!(fs::read(output.join("fox.db")).unwrap(), b"database");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_path_traversal() {
        assert!(validate_relative("../fox.db").is_err());
        assert!(validate_relative("C:/fox.db").is_err());
        assert!(validate_relative("attachments/file.txt").is_ok());
    }

    #[test]
    fn rejects_duplicate_and_truncated_archive_entries() {
        let root = std::env::temp_dir().join(format!("fox-corrupt-backup-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let source = root.join("fox.db");
        fs::write(&source, b"database").unwrap();
        let entry = backup_entry(&source, "fox.db").unwrap();
        let duplicate = BackupManifest {
            format_version: 1,
            created_at: 1,
            fox_version: "test".to_owned(),
            schema_version: 1,
            runtime_sessions_best_effort: true,
            source_data_dir: None,
            entries: vec![entry.clone(), entry],
        };
        let duplicate_archive = root.join("duplicate.foxbackup");
        write_archive(
            &duplicate_archive,
            &duplicate,
            &[
                (source.clone(), "fox.db".to_owned()),
                (source.clone(), "fox.db".to_owned()),
            ],
        )
        .unwrap();
        assert!(read_archive(&duplicate_archive, None)
            .unwrap_err()
            .contains("重复路径"));

        let valid = BackupManifest {
            format_version: 1,
            created_at: 1,
            fox_version: "test".to_owned(),
            schema_version: 1,
            runtime_sessions_best_effort: true,
            source_data_dir: None,
            entries: vec![backup_entry(&source, "fox.db").unwrap()],
        };
        let truncated = root.join("truncated.foxbackup");
        write_archive(&truncated, &valid, &[(source, "fox.db".to_owned())]).unwrap();
        let bytes = fs::read(&truncated).unwrap();
        fs::write(&truncated, &bytes[..bytes.len() - 3]).unwrap();
        assert!(read_archive(&truncated, None).is_err());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_declared_archive_sizes_before_extracting_entries() {
        let root = std::env::temp_dir().join(format!("fox-size-backup-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let archive = root.join("oversized.foxbackup");
        let manifest = BackupManifest {
            format_version: 1,
            created_at: 1,
            fox_version: "test".to_owned(),
            schema_version: 9,
            runtime_sessions_best_effort: true,
            source_data_dir: None,
            entries: (0..9)
                .map(|index| BackupEntry {
                    path: format!("attachments/{index}.bin"),
                    byte_size: MAX_ENTRY_BYTES,
                    sha256: "00".repeat(32),
                })
                .collect(),
        };
        let manifest_bytes = crate::canonical_json::to_vec(&manifest).unwrap();
        let mut writer = BufWriter::new(File::create(&archive).unwrap());
        writeln!(writer, "{BACKUP_MAGIC}").unwrap();
        writeln!(writer, "{}", manifest_bytes.len()).unwrap();
        writer.write_all(&manifest_bytes).unwrap();
        writer.write_all(b"\n").unwrap();
        writer.flush().unwrap();
        assert!(read_archive(&archive, None)
            .unwrap_err()
            .contains("超过 4 GB"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn redacts_secret_bearing_diagnostic_errors() {
        assert_eq!(
            redact("request failed token=very-secret trailing"),
            "request failed [REDACTED]"
        );
        assert_eq!(
            redact("ordinary connection error"),
            "ordinary connection error"
        );
    }

    #[test]
    fn diagnostic_report_excludes_credentials_conversations_and_project_contents() {
        let secret = "sk-super-secret-token";
        let conversation = "private conversation body";
        let project_content = "confidential project source";
        let report = diagnostic_report(
            json!({"state": "ready", "lastError": format!("token={secret}")}),
            9,
            None,
            None,
            vec![crate::database::McpServerRecord {
                id: "mcp-1".to_owned(),
                name: "test".to_owned(),
                command: "server".to_owned(),
                args: Vec::new(),
                transport: "stdio".to_owned(),
                endpoint_url: None,
                definition: None,
                enabled: true,
                status: "error".to_owned(),
                credential_configured: true,
                last_error: Some(format!("authorization bearer {secret}")),
                last_checked_at: None,
                last_latency_ms: None,
                tool_count: None,
                consecutive_failures: 0,
                created_at: 1,
                updated_at: 1,
            }],
        );
        let serialized = crate::canonical_json::to_string(&report).unwrap();
        assert!(!serialized.contains(secret));
        assert!(!serialized.contains(conversation));
        assert!(!serialized.contains(project_content));
        assert!(!serialized.contains("command"));
        assert_eq!(report["privacy"]["conversationContentIncluded"], false);
        assert_eq!(report["privacy"]["projectFilesIncluded"], false);
    }

    #[test]
    fn interrupted_restore_rollback_recovers_previous_data() {
        let root = std::env::temp_dir().join(format!("fox-rollback-test-{}", Uuid::new_v4()));
        let rollback = root.join("restore-rollback");
        fs::create_dir_all(rollback.join("attachments")).unwrap();
        fs::write(rollback.join("fox.db"), b"previous-database").unwrap();
        fs::write(
            rollback.join("attachments/previous.txt"),
            b"previous-attachment",
        )
        .unwrap();
        recover_restore_rollback(&root, &rollback).unwrap();
        assert_eq!(fs::read(root.join("fox.db")).unwrap(), b"previous-database");
        assert_eq!(
            fs::read(root.join("attachments/previous.txt")).unwrap(),
            b"previous-attachment"
        );
        assert!(!rollback.exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn restored_database_paths_are_rebased_to_the_new_data_directory() {
        let root = std::env::temp_dir().join(format!("fox-restore-test-{}", Uuid::new_v4()));
        let old = root.join("old");
        let new = root.join("new");
        fs::create_dir_all(old.join("attachments")).unwrap();
        fs::create_dir_all(new.join("attachments")).unwrap();
        fs::create_dir_all(new.join("runtime-sessions")).unwrap();
        let db_path = root.join("restored.db");
        let database = crate::database::Database::open(db_path.clone()).unwrap();
        let conversation = database
            .create_conversation("fox-general", None, None, None)
            .unwrap();
        database
            .add_attachments(&[crate::database::AttachmentRecord {
                id: "attachment-1".to_owned(),
                conversation_id: conversation.id.clone(),
                message_id: None,
                display_name: "notes.txt".to_owned(),
                storage_path: old
                    .join("attachments/notes.txt")
                    .to_string_lossy()
                    .into_owned(),
                media_type: Some("text/plain".to_owned()),
                byte_size: 5,
                sha256: None,
                status: "ready".to_owned(),
                created_at: 1,
            }])
            .unwrap();
        database
            .ensure_runtime_session(
                &conversation.id,
                "session-1",
                Some("0.1.0"),
                Some(
                    &old.join("runtime-sessions/session-1.json")
                        .to_string_lossy(),
                ),
            )
            .unwrap();
        drop(database);
        rebase_internal_paths(&db_path, &new, None).unwrap();
        let database = crate::database::Database::open(db_path.clone()).unwrap();
        assert_eq!(
            PathBuf::from(&database.known_attachment_paths().unwrap()[0]),
            new.join("attachments").join("notes.txt")
        );
        assert_eq!(
            database
                .get_runtime_session(&conversation.id)
                .unwrap()
                .unwrap()
                .session_path
                .as_deref(),
            Some(
                new.join("runtime-sessions")
                    .join("session-1.json")
                    .to_string_lossy()
                    .as_ref()
            )
        );
        drop(database);
        let _ = fs::remove_dir_all(root);
    }

    struct NativeRestoreFixture {
        root: PathBuf,
        old: PathBuf,
        new: PathBuf,
        conversation: String,
        allocated: String,
        current: PathBuf,
        retired: PathBuf,
        tail: Vec<u8>,
    }

    impl NativeRestoreFixture {
        fn new() -> Self {
            let parent=std::env::var_os("FOX_TEST_NATIVE_HOST_FIXTURE_ROOT").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
            let root=parent.join(format!("fox-native-restore-{}",Uuid::new_v4()));
            let old=root.join("old");let new=root.join("new");
            fs::create_dir_all(old.join("runtime-sessions")).unwrap();
            fs::create_dir_all(&new).unwrap();
            let database=crate::database::Database::open(old.join("fox.db")).unwrap();
            let conversation=database.create_conversation("fox-general",Some("native restore"),None,None).unwrap().id;
            let allocated=database.create_conversation("fox-general",Some("unwritten native restore"),None,None).unwrap().id;
            let current=old.join("runtime-sessions/current.jsonl");
            let retired=old.join("runtime-sessions/retired.jsonl");
            let tail=b"{\"type\":\"compaction\",\"id\":\"summary-1\",\"summary\":\"keep context\"}\r\n{ \"type\": \"custom\", \"data\": \"history bytes stay exact\" }\n".to_vec();
            for (path,id) in [(&current,"native-current"),(&retired,"native-retired")] {
                let mut bytes=crate::canonical_json::to_vec(&json!({"type":"session","version":3,"id":id,
                    "timestamp":"2026-10-28T00:00:00.000Z","cwd":old.join("runtime-sessions")})).unwrap();
                bytes.extend_from_slice(b"\r\n");bytes.extend_from_slice(&tail);
                fs::write(path,bytes).unwrap();
            }
            fs::write(old.join("runtime-sessions/legacy.json"),b"legacy snapshot").unwrap();
            database.ensure_native_runtime_session(&conversation,"runtime-current",Some("fixture"),&current.to_string_lossy(),
                Some("native-current"),3,crate::database::RuntimeSessionStatus::Ready).unwrap();
            database.ensure_native_runtime_session(&allocated,"runtime-unwritten",Some("fixture"),
                &old.join("runtime-sessions/unwritten.jsonl").to_string_lossy(),Some("native-unwritten"),1,
                crate::database::RuntimeSessionStatus::Allocated).unwrap();
            database.with_connection(|connection| {
                connection.execute("INSERT INTO runtime_session_generations(conversation_id,runtime_type,context_generation,runtime_session_id,
                    runtime_version,native_session_ref,native_session_path,format_version,status,retired_at)
                    VALUES(?1,'pi',1,'runtime-retired','fixture','native-retired',?2,2,'ready',1)",
                    rusqlite::params![conversation,retired.to_string_lossy()])?;
                connection.execute("INSERT INTO runtime_session_generations(conversation_id,runtime_type,context_generation,runtime_session_id,
                    runtime_version,session_path,format_version,status,retired_at)
                    VALUES(?1,'pi',2,'runtime-legacy','fixture',?2,1,'ready',2)",
                    rusqlite::params![conversation,old.join("runtime-sessions/legacy.json").to_string_lossy()])?;
                Ok(())
            }).unwrap();
            drop(database);
            println!("native restore fixture retained at {}",root.display());
            Self {root,old,new,conversation,allocated,current,retired,tail}
        }

        fn archive(&self, include_root: bool) -> PathBuf {
            self.archive_with_schema_version(include_root, crate::database::DATABASE_SCHEMA_VERSION)
        }

        fn archive_with_schema_version(&self, include_root: bool, schema_version: i64) -> PathBuf {
            let sources=vec![(self.old.join("fox.db"),"fox.db".to_owned()),
                (self.current.clone(),"runtime-sessions/current.jsonl".to_owned()),
                (self.retired.clone(),"runtime-sessions/retired.jsonl".to_owned()),
                (self.old.join("runtime-sessions/legacy.json"),"runtime-sessions/legacy.json".to_owned())];
            let manifest=BackupManifest {format_version:1,created_at:1,fox_version:"fixture".into(),
                schema_version,runtime_sessions_best_effort:true,
                source_data_dir:include_root.then(||self.old.to_string_lossy().into_owned()),
                entries:sources.iter().map(|(path,relative)|backup_entry(path,relative).unwrap()).collect()};
            let archive=self.root.join("native.foxbackup");
            write_archive(&archive,&manifest,&sources).unwrap();
            archive
        }

        fn header(&self, path: &Path, change: impl FnOnce(&mut Value)) {
            let bytes=fs::read(path).unwrap();
            let at=bytes.iter().position(|byte|*byte==b'\n').unwrap();
            let mut header:Value=serde_json::from_slice(&bytes[..at]).unwrap();
            change(&mut header);
            let mut updated=crate::canonical_json::to_vec(&header).unwrap();
            updated.extend_from_slice(b"\r\n");updated.extend_from_slice(&bytes[at+1..]);
            fs::write(path,updated).unwrap();
        }
    }

    fn assert_schema_restore_refusal_preserves_live_data(fixture: &NativeRestoreFixture, archive: &Path, expected: &str) {
        let live_db = fixture.new.join("fox.db");
        let database = crate::database::Database::open(live_db.clone()).unwrap();
        database.create_conversation("fox-general", Some("keep live conversation"), None, None).unwrap();
        drop(database);
        fs::create_dir_all(fixture.new.join("runtime-sessions")).unwrap();
        fs::create_dir_all(fixture.new.join("attachments")).unwrap();
        let live_session = fixture.new.join("runtime-sessions/live.jsonl");
        let live_attachment = fixture.new.join("attachments/live.txt");
        fs::write(&live_session, b"live session must stay exact").unwrap();
        fs::write(&live_attachment, b"live attachment must stay exact").unwrap();
        let before = fs::read(&live_db).unwrap();
        let modified = fs::metadata(&live_db).unwrap().modified().unwrap();
        let source_before = fs::read(fixture.old.join("fox.db")).unwrap();
        let source_session_before = fs::read(&fixture.current).unwrap();
        let archive_before = fs::read(archive).unwrap();
        queue_restore(&fixture.new, archive).unwrap();
        let pending = fixture.new.join("restore-pending.foxbackup");
        for _ in 0..2 {
            let error = apply_pending_restore(&fixture.new).unwrap_err();
            assert!(error.contains(expected), "{error}; fixture={}", fixture.root.display());
            assert_eq!(fs::read(&live_db).unwrap(), before);
            assert_eq!(fs::metadata(&live_db).unwrap().modified().unwrap(), modified);
            assert_eq!(fs::read(&live_session).unwrap(), b"live session must stay exact");
            assert_eq!(fs::read(&live_attachment).unwrap(), b"live attachment must stay exact");
            assert_eq!(fs::read(&pending).unwrap(), archive_before, "the original pending archive must remain retryable");
            assert_eq!(fs::read(archive).unwrap(), archive_before);
            assert_eq!(fs::read(fixture.old.join("fox.db")).unwrap(), source_before);
            assert_eq!(fs::read(&fixture.current).unwrap(), source_session_before);
            assert!(!fixture.new.join("restore-rollback").exists(), "refusal must precede live replacement");
            assert!(!fixture.new.join("fox.db-wal").exists());
            assert!(!fixture.new.join("fox.db-shm").exists());
        }
    }

    #[test]
    fn queued_future_schema_restore_preserves_live_database_sessions_and_pending_archive() {
        let fixture = NativeRestoreFixture::new();
        let connection = rusqlite::Connection::open(fixture.old.join("fox.db")).unwrap();
        connection.execute("INSERT INTO schema_migrations(version, applied_at) VALUES(?1, 1)",
            [crate::database::DATABASE_SCHEMA_VERSION + 1]).unwrap();
        drop(connection);
        // A supported manifest cannot hide a newer SQLite version.
        let archive = fixture.archive(true);
        assert_schema_restore_refusal_preserves_live_data(&fixture, &archive, "database.schema_too_new");
    }

    #[test]
    fn queued_schema_manifest_mismatch_preserves_live_database_sessions_and_pending_archive() {
        for declared in [crate::database::DATABASE_SCHEMA_VERSION - 1, crate::database::DATABASE_SCHEMA_VERSION + 1] {
            let fixture = NativeRestoreFixture::new();
            validate_sqlite(&fixture.old.join("fox.db"), crate::database::DATABASE_SCHEMA_VERSION).unwrap();
            let archive = fixture.archive_with_schema_version(true, declared);
            assert_schema_restore_refusal_preserves_live_data(&fixture, &archive, "与实际数据库版本");
        }
    }

    #[test]
    fn native_restore_keeps_live_retired_and_unwritten_generations_in_the_copy_and_is_repeatable() {
        for include_root in [false,true] {
            let fixture=NativeRestoreFixture::new();
            let original_db=fs::read(fixture.old.join("fox.db")).unwrap();
            let original_current=fs::read(&fixture.current).unwrap();
            let original_retired=fs::read(&fixture.retired).unwrap();
            let archive=fixture.archive(include_root);
            let mut restored_bytes=None;
            for _ in 0..2 {
                queue_restore(&fixture.new,&archive).unwrap();
                apply_pending_restore(&fixture.new).unwrap();
                let database=crate::database::Database::open(fixture.new.join("fox.db")).unwrap();
                let current=database.get_runtime_session(&fixture.conversation).unwrap().unwrap();
                let selected=current.native_path().unwrap();
                assert_eq!(selected,fixture.new.join("runtime-sessions/current.jsonl"));
                assert_eq!(current.native_session_ref.as_deref(),Some("native-current"));
                assert_eq!(current.runtime_session_id,"runtime-current");
                assert_eq!(current.context_generation,3);
                assert!(current.session_path.is_none());
                let bytes=fs::read(&selected).unwrap();
                let at=bytes.iter().position(|byte|*byte==b'\n').unwrap();
                assert_eq!(&bytes[at-1..at+1],b"\r\n");
                assert_eq!(&bytes[at+1..],fixture.tail.as_slice());
                let header:Value=serde_json::from_slice(&bytes[..at]).unwrap();
                assert_eq!(header["id"],"native-current");
                assert_eq!(PathBuf::from(header["cwd"].as_str().unwrap()),fixture.new.join("runtime-sessions"));
                if let Some(previous)=&restored_bytes {assert_eq!(&bytes,previous);} else {restored_bytes=Some(bytes);}
                let retired:(String,String)=database.with_connection(|connection|connection.query_row(
                    "SELECT native_session_path,native_session_ref FROM runtime_session_generations WHERE conversation_id=?1 AND context_generation=1",
                    [&fixture.conversation],|row|Ok((row.get(0)?,row.get(1)?)))).unwrap();
                assert_eq!(PathBuf::from(retired.0),fixture.new.join("runtime-sessions/retired.jsonl"));
                assert_eq!(retired.1,"native-retired");
                let legacy:String=database.with_connection(|connection|connection.query_row(
                    "SELECT session_path FROM runtime_session_generations WHERE conversation_id=?1 AND context_generation=2",
                    [&fixture.conversation],|row|row.get(0))).unwrap();
                assert_eq!(PathBuf::from(legacy),fixture.new.join("runtime-sessions/legacy.json"));
                let allocated=database.get_runtime_session(&fixture.allocated).unwrap().unwrap();
                assert_eq!(allocated.status,"allocated");
                assert_eq!(allocated.native_path(),Some(fixture.new.join("runtime-sessions/unwritten.jsonl")));
                assert!(!allocated.native_path().unwrap().exists());
                drop(database);
                assert_eq!(fs::read(fixture.old.join("fox.db")).unwrap(),original_db);
                assert_eq!(fs::read(&fixture.current).unwrap(),original_current);
                assert_eq!(fs::read(&fixture.retired).unwrap(),original_retired);
            }
            // The path a resume will receive names only the copied file.
            let copied=fixture.new.join("runtime-sessions/current.jsonl");
            fs::OpenOptions::new().append(true).open(copied).unwrap().write_all(b"{\"type\":\"custom\",\"id\":\"copy-only\",\"customType\":\"restore-test\",\"data\":\"append\"}\n").unwrap();
            assert_eq!(fs::read(&fixture.current).unwrap(),original_current);
            rebase_internal_paths(&fixture.new.join("fox.db"),&fixture.new,Some(&fixture.old.to_string_lossy())).unwrap();
            assert_eq!(fs::read(&fixture.current).unwrap(),original_current);
        }
    }

    #[test]
    fn native_restore_preserves_an_external_project_cwd_and_all_its_file_bytes() {
        let fixture=NativeRestoreFixture::new();
        let external=fixture.root.join("external-project");
        fixture.header(&fixture.current,|header|header["cwd"]=json!(external));
        let original=fs::read(&fixture.current).unwrap();
        let archive=fixture.archive(true);
        queue_restore(&fixture.new,&archive).unwrap();
        apply_pending_restore(&fixture.new).unwrap();
        assert_eq!(fs::read(fixture.new.join("runtime-sessions/current.jsonl")).unwrap(),original);
        assert_eq!(fs::read(&fixture.current).unwrap(),original);
    }

    #[test]
    fn native_restore_refuses_external_or_ambiguous_roots_before_replacing_live_data() {
        for include_root in [false,true] {
            let fixture=NativeRestoreFixture::new();
            let connection=rusqlite::Connection::open(fixture.old.join("fox.db")).unwrap();
            let outside=fixture.root.join("old-similar-prefix/runtime-sessions/current.jsonl");
            connection.execute("UPDATE runtime_sessions SET native_session_path=?2 WHERE conversation_id=?1",
                rusqlite::params![fixture.conversation,outside.to_string_lossy()]).unwrap();
            drop(connection);
            let sentinel=fixture.new.join("fox.db");
            fs::write(&sentinel,b"previous live database remains untouched").unwrap();
            let original=fs::read(&fixture.current).unwrap();
            let archive=fixture.archive(include_root);
            queue_restore(&fixture.new,&archive).unwrap();
            assert!(apply_pending_restore(&fixture.new).is_err());
            assert_eq!(fs::read(&sentinel).unwrap(),b"previous live database remains untouched");
            assert_eq!(fs::read(&fixture.current).unwrap(),original);
        }
    }

    #[test]
    fn native_restore_refuses_wrong_identity_and_unknown_session_versions() {
        for defect in ["identity","header-version","mapping-version"] {
            let fixture=NativeRestoreFixture::new();
            if defect=="mapping-version" {
                let connection=rusqlite::Connection::open(fixture.old.join("fox.db")).unwrap();
                connection.execute("UPDATE runtime_sessions SET format_version=999 WHERE conversation_id=?1",[&fixture.conversation]).unwrap();
            } else {
                fixture.header(&fixture.current,|header|if defect=="identity" {header["id"]=json!("a-different-native-session");}
                    else {header["version"]=json!(999);});
            }
            let sentinel=fixture.new.join("fox.db");fs::write(&sentinel,b"keep live data").unwrap();
            let archive=fixture.archive(true);
            queue_restore(&fixture.new,&archive).unwrap();
            let error=apply_pending_restore(&fixture.new).unwrap_err();
            assert!(error.contains("身份")||error.contains("版本")||error.contains("格式"),"{defect}: {error}");
            assert_eq!(fs::read(&sentinel).unwrap(),b"keep live data");
        }
    }

    #[test]
    fn native_restore_path_identity_folds_windows_spelling_without_prefix_or_traversal_escapes() {
        assert_eq!(restore_relative(Path::new(r"D:\Fox Data"),r"\\?\d:\FOX DATA\runtime-sessions\A.jsonl").unwrap(),
            Some(PathBuf::from("runtime-sessions/A.jsonl")));
        assert_eq!(restore_relative(Path::new(r"\\Server\Share\Fox"),r"\\?\UNC\server\share\FOX\runtime-sessions\A.jsonl").unwrap(),
            Some(PathBuf::from("runtime-sessions/A.jsonl")));
        assert!(restore_relative(Path::new(r"D:\Fox"),r"d:\fox-similar\runtime-sessions\A.jsonl").unwrap().is_none());
        assert!(restore_relative(Path::new(r"D:\Fox"),r"d:\fox\runtime-sessions\..\A.jsonl").is_err());
    }

    #[test]
    fn native_restore_optional_source_root_keeps_old_archives_and_rejects_future_archive_versions() {
        let fixture=NativeRestoreFixture::new();
        let archive=fixture.archive(false);
        let (mut manifest,_)=read_archive(&archive,None).unwrap();
        assert_eq!(manifest.format_version,1);
        assert!(manifest.source_data_dir.is_none());
        manifest.format_version=2;
        manifest.source_data_dir=Some(fixture.old.to_string_lossy().into_owned());
        let sources=manifest.entries.iter().map(|entry|(fixture.old.join(&entry.path),entry.path.clone())).collect::<Vec<_>>();
        let future=fixture.root.join("future.foxbackup");
        write_archive(&future,&manifest,&sources).unwrap();
        assert!(queue_restore(&fixture.new,&future).unwrap_err().contains("不支持的 Fox 备份版本"));
        assert!(!fixture.new.join("restore-pending.foxbackup").exists());
    }

    #[test]
    fn backup_restores_core_data_into_a_fresh_directory_without_credentials() {
        let root = std::env::temp_dir().join(format!("fox-e2e-restore-test-{}", Uuid::new_v4()));
        let old = root.join("old");
        let fresh = root.join("fresh");
        let extracted = root.join("extracted");
        for directory in [&old, &fresh, &extracted] {
            fs::create_dir_all(directory).unwrap();
        }
        fs::create_dir_all(old.join("attachments")).unwrap();
        fs::create_dir_all(old.join("runtime-sessions")).unwrap();
        fs::create_dir_all(old.join("skills/example")).unwrap();
        fs::create_dir_all(fresh.join("attachments")).unwrap();
        fs::create_dir_all(fresh.join("runtime-sessions")).unwrap();
        let db_path = old.join("fox.db");
        let database = crate::database::Database::open(db_path.clone()).unwrap();
        let conversation = database
            .create_conversation("fox-general", Some("可恢复会话"), None, None)
            .unwrap();
        database
            .create_run(&conversation.id, "恢复后的消息", None)
            .unwrap();
        let attachment = old.join("attachments/notes.txt");
        fs::write(&attachment, b"attachment body").unwrap();
        database
            .add_attachments(&[crate::database::AttachmentRecord {
                id: "attachment-restore".to_owned(),
                conversation_id: conversation.id.clone(),
                message_id: None,
                display_name: "notes.txt".to_owned(),
                storage_path: attachment.to_string_lossy().into_owned(),
                media_type: Some("text/plain".to_owned()),
                byte_size: 15,
                sha256: None,
                status: "ready".to_owned(),
                created_at: 1,
            }])
            .unwrap();
        database
            .ensure_runtime_session(
                &conversation.id,
                "session-restore",
                Some("0.1.0"),
                Some(
                    &old.join("runtime-sessions/session-restore.json")
                        .to_string_lossy(),
                ),
            )
            .unwrap();
        drop(database);
        fs::write(
            old.join("runtime-sessions/session-restore.json"),
            b"runtime session",
        )
        .unwrap();
        fs::write(
            old.join("skills/example/SKILL.md"),
            b"---\nname: example\ndescription: test\n---\ninstructions",
        )
        .unwrap();

        let sources = vec![
            (db_path.clone(), "fox.db".to_owned()),
            (attachment.clone(), "attachments/notes.txt".to_owned()),
            (
                old.join("runtime-sessions/session-restore.json"),
                "runtime-sessions/session-restore.json".to_owned(),
            ),
            (
                old.join("skills/example/SKILL.md"),
                "skills/example/SKILL.md".to_owned(),
            ),
        ];
        let manifest = BackupManifest {
            format_version: 1,
            created_at: 1,
            fox_version: "test".to_owned(),
            schema_version: 9,
            runtime_sessions_best_effort: true,
            source_data_dir: None,
            entries: sources
                .iter()
                .map(|(path, relative)| backup_entry(path, relative).unwrap())
                .collect(),
        };
        let archive = root.join("backup.foxbackup");
        write_archive(&archive, &manifest, &sources).unwrap();
        let archive_bytes = fs::read(&archive).unwrap();
        let archive_text = String::from_utf8_lossy(&archive_bytes);
        assert!(!archive_text.contains("api-key"));
        assert!(!archive_text.contains("access-token"));

        read_archive(&archive, Some(&extracted)).unwrap();
        rebase_internal_paths(&extracted.join("fox.db"), &fresh, None).unwrap();
        let restored = crate::database::Database::open(extracted.join("fox.db")).unwrap();
        let detail = restored.load_conversation(&conversation.id).unwrap();
        assert_eq!(detail.conversation.title, "可恢复会话");
        assert!(detail
            .messages
            .iter()
            .any(|message| message.content == "恢复后的消息"));
        assert_eq!(
            PathBuf::from(restored.known_attachment_paths().unwrap()[0].clone()),
            fresh.join("attachments/notes.txt")
        );
        assert_eq!(
            PathBuf::from(
                restored
                    .get_runtime_session(&conversation.id)
                    .unwrap()
                    .unwrap()
                    .session_path
                    .unwrap()
            ),
            fresh.join("runtime-sessions/session-restore.json")
        );
        assert!(extracted.join("skills/example/SKILL.md").is_file());
        drop(restored);
        let _ = fs::remove_dir_all(root);
    }
}
