//! Conversation-scoped authorization and immutable input snapshots for JS computation.
use crate::database::Database;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    time::Instant,
};

pub(crate) mod host_lifecycle;
pub(crate) mod jobs;

/// Bounded copy buffer for the snapshot phase. Peak memory of snapshotting is
/// this buffer plus one hash state, regardless of how large the input set is:
/// the previous implementation held every input's bytes simultaneously.
const SNAPSHOT_COPY_BUFFER_BYTES: usize = 256 * 1024;

/// Owns one execution workspace. Unless the run is explicitly kept, dropping
/// the guard removes it, so a failed, cancelled or over-limit attempt can
/// never publish a partial snapshot set that a later step might treat as a
/// complete input. Only this execution's own directory is touched.
struct WorkspaceGuard {
    path: PathBuf,
    keep: bool,
}

impl WorkspaceGuard {
    fn new(path: PathBuf) -> Self {
        Self { path, keep: false }
    }

    /// Keep the workspace (and return its path) once the computation succeeded
    /// and its outputs are referenced by the result.
    fn keep(mut self) -> PathBuf {
        self.keep = true;
        self.path.clone()
    }
}

impl Drop for WorkspaceGuard {
    fn drop(&mut self) {
        if !self.keep {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

fn check_snapshot_progress(
    cancelled: &dyn Fn() -> bool,
    deadline: Instant,
) -> Result<(), String> {
    if cancelled() {
        return Err("Attachment computation cancelled".into());
    }
    if Instant::now() >= deadline {
        return Err("Attachment computation timed out while snapshotting inputs".into());
    }
    Ok(())
}

/// Stream one authorized input into the private snapshot directory while
/// hashing it incrementally. The source file is never handed to the compute
/// engine and never fully buffered: `total_bytes` is the running accounted
/// size and `total_limit` the tier's cumulative ceiling.
#[allow(clippy::too_many_arguments)]
fn stream_snapshot(
    source: &Path,
    destination: &Path,
    per_file_limit: u64,
    cancelled: &dyn Fn() -> bool,
    deadline: Instant,
    total_bytes: &mut u64,
    total_limit: usize,
) -> Result<(String, u64), String> {
    let before = fs::metadata(source)
        .map_err(|error| format!("Unable to inspect input: {error}"))?;
    if !before.is_file() {
        return Err("Input is not a regular file".into());
    }
    if before.len() > per_file_limit {
        return Err(format!(
            "输入为 {} 字节，超过档位的 {} 字节上限；请选择更小文件",
            before.len(),
            per_file_limit,
        ));
    }
    let mut reader =
        fs::File::open(source).map_err(|error| format!("Unable to read input: {error}"))?;
    let mut writer = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(destination)
        .map_err(|error| format!("Unable to create the private snapshot: {error}"))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; SNAPSHOT_COPY_BUFFER_BYTES];
    let mut copied: u64 = 0;
    loop {
        check_snapshot_progress(cancelled, deadline)?;
        let read = reader
            .read(&mut buffer)
            .map_err(|error| format!("Unable to read input: {error}"))?;
        if read == 0 {
            break;
        }
        copied = copied.saturating_add(read as u64);
        if copied > per_file_limit {
            return Err(format!(
                "输入超过档位的 {} 字节上限；请选择更小文件",
                per_file_limit,
            ));
        }
        *total_bytes = total_bytes.saturating_add(read as u64);
        if *total_bytes > total_limit as u64 {
            return Err(format!(
                "计算输入合计超过档位的 {total_limit} 字节；请减少附件数量或分批计算",
            ));
        }
        hasher.update(&buffer[..read]);
        writer
            .write_all(&buffer[..read])
            .map_err(|error| format!("Unable to write the private snapshot: {error}"))?;
    }
    writer
        .flush()
        .map_err(|error| format!("Unable to flush the private snapshot: {error}"))?;
    drop(writer);
    // A source that changed while it was copied would produce a snapshot that
    // matches no recorded state: reject instead of computing on mixed bytes.
    #[cfg(test)]
    run_post_copy_hook();
    let after = fs::metadata(source)
        .map_err(|error| format!("Unable to re-inspect input: {error}"))?;
    if after.len() != before.len() || after.modified().ok() != before.modified().ok() {
        return Err("输入在快照期间发生变化；请在文件稳定后重试".into());
    }
    Ok((hex::encode(hasher.finalize()), copied))
}

/// Deterministic seam for the "source changed while snapshotting" test: the
/// hook runs on the same thread, after the copy loop and before the re-stat,
/// so the mutation can be scheduled without any timing race.
#[cfg(test)]
thread_local! {
    static POST_COPY_HOOK: std::cell::RefCell<Option<Box<dyn FnMut()>>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
fn run_post_copy_hook() {
    POST_COPY_HOOK.with(|slot| {
        if let Some(hook) = slot.borrow_mut().as_mut() {
            hook();
        }
    });
}

#[cfg(test)]
pub(crate) fn set_post_copy_hook(hook: Option<Box<dyn FnMut()>>) {
    POST_COPY_HOOK.with(|slot| {
        *slot.borrow_mut() = hook;
    });
}

/// Test-only allocation accounting. Production builds never install it. Peak
/// is process-wide, so the tests that assert on it run with a single test
/// thread and read only the peak accumulated since their own reset.
#[cfg(test)]
pub(crate) mod alloc_probe {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::sync::atomic::{AtomicUsize, Ordering};

    static CURRENT: AtomicUsize = AtomicUsize::new(0);
    static PEAK: AtomicUsize = AtomicUsize::new(0);

    pub(crate) struct TrackingAllocator;

    unsafe impl GlobalAlloc for TrackingAllocator {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            let pointer = unsafe { System.alloc(layout) };
            if !pointer.is_null() {
                let current =
                    CURRENT.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
                PEAK.fetch_max(current, Ordering::Relaxed);
            }
            pointer
        }

        unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
            CURRENT.fetch_sub(layout.size(), Ordering::Relaxed);
            unsafe { System.dealloc(pointer, layout) }
        }

        unsafe fn realloc(
            &self,
            pointer: *mut u8,
            layout: Layout,
            new_size: usize,
        ) -> *mut u8 {
            let new_pointer = unsafe { System.realloc(pointer, layout, new_size) };
            if !new_pointer.is_null() {
                if new_size >= layout.size() {
                    let growth = new_size - layout.size();
                    let current = CURRENT.fetch_add(growth, Ordering::Relaxed) + growth;
                    PEAK.fetch_max(current, Ordering::Relaxed);
                } else {
                    CURRENT.fetch_sub(layout.size() - new_size, Ordering::Relaxed);
                }
            }
            new_pointer
        }
    }

    #[global_allocator]
    static ALLOCATOR: TrackingAllocator = TrackingAllocator;

    /// Start a fresh measurement window at the current live bytes.
    pub(crate) fn reset_peak() {
        PEAK.store(CURRENT.load(Ordering::Relaxed), Ordering::Relaxed);
    }

    pub(crate) fn peak_bytes() -> usize {
        PEAK.load(Ordering::Relaxed)
    }

    pub(crate) fn current_bytes() -> usize {
        CURRENT.load(Ordering::Relaxed)
    }
}

/// Snapshot-stage limits per controlled tier (#16). The compute sandbox
/// itself (QuickJS memory/stack, output caps) never changes with the tier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SnapshotTier {
    pub name: &'static str,
    pub max_inputs: usize,
    pub max_attachment_bytes: u64,
    pub max_total_input_bytes: usize,
    pub max_artifact_bytes: i64,
}

fn snapshot_tier(profile: Option<&str>) -> Result<SnapshotTier, String> {
    match profile.unwrap_or("standard") {
        "standard" => Ok(SnapshotTier {
            name: "standard",
            max_inputs: 8,
            max_attachment_bytes: 5 * 1024 * 1024,
            max_total_input_bytes: 20 * 1024 * 1024,
            max_artifact_bytes: 16 * 1024 * 1024,
        }),
        "large" => Ok(SnapshotTier {
            name: "large",
            max_inputs: 8,
            max_attachment_bytes: 32 * 1024 * 1024,
            max_total_input_bytes: 64 * 1024 * 1024,
            max_artifact_bytes: 16 * 1024 * 1024,
        }),
        other => Err(format!(
            "data-compute profile '{other}' 不存在（可用：standard、large）"
        )),
    }
}

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
    execute_with_options(
        database,
        attachments_dir,
        sessions_dir,
        conversation_id,
        run_id,
        input,
        cancelled,
        ComputeOptions::default(),
    )
}

/// Explicit execution options. The synchronous tool path derives everything
/// from the model input; the background-job path passes its own deadline and
/// progress sink (CONTRACTS §4: waiting and execution budgets stay separate).
#[derive(Default)]
pub(crate) struct ComputeOptions<'a> {
    /// Hard execution deadline. Sync callers leave this None and the input's
    /// bounded timeoutMs applies (15s default / 30s ceiling, unchanged).
    pub deadline: Option<Instant>,
    /// Per-chunk progress sink (rows processed). Sync callers pass None.
    pub progress: Option<&'a dyn Fn(crate::data_compute::chunked::ChunkProgress)>,
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn execute_with_options(
    database: &Database,
    attachments_dir: &Path,
    sessions_dir: &Path,
    conversation_id: &str,
    run_id: &str,
    input: &Value,
    cancelled: impl Fn() -> bool + 'static,
    options: ComputeOptions,
) -> Result<Value, String> {
    let processing = input
        .get("processing")
        .and_then(Value::as_str)
        .unwrap_or("whole");
    if !matches!(processing, "whole" | "chunked") {
        return Err("processing must be 'whole' or 'chunked'".into());
    }
    let profile_name = input.get("profile").and_then(Value::as_str);
    let tier = snapshot_tier(profile_name)?;
    if processing == "whole" && tier.name != "standard" {
        return Err(
            "profile=large 需要 processing=chunked；whole 模式保持 standard 档位的完整加载限制"
                .into(),
        );
    }
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
    if ids.len() + artifacts.len() > tier.max_inputs {
        return Err(format!(
            "选择了 {} 个输入，超过 {} 档位的 {} 个上限；请减少附件或分批计算",
            ids.len() + artifacts.len(),
            tier.name,
            tier.max_inputs,
        ));
    }
    if input["code"]
        .as_str()
        .is_none_or(|code| code.trim().is_empty() || code.len() > 128 * 1024)
    {
        return Err("code must be nonempty JavaScript within 128 KiB".into());
    }
    let storage = fs::canonicalize(attachments_dir).map_err(|e| e.to_string())?;
    // The snapshot deadline bounds verification+copy. On the job path it is the
    // execution deadline (snapshotting is execution work); on the synchronous
    // path it mirrors the model's bounded timeout so a hostile input cannot
    // stall the tool call before the interpreter's own clock starts.
    let snapshot_deadline = options.deadline.unwrap_or_else(|| {
        Instant::now()
            + std::time::Duration::from_millis(
                input["timeoutMs"].as_u64().unwrap_or(15_000).min(30_000),
            )
    });
    // Create the private workspace before copying: nothing is buffered, so the
    // snapshot is written straight to its immutable location.
    let workspace = safe_workspace(sessions_dir, conversation_id, run_id)?;
    let guard = WorkspaceGuard::new(workspace.clone());
    let input_root = workspace.join("inputs");
    fs::create_dir(&input_root).map_err(|e| e.to_string())?;
    let output_root = workspace.join("outputs");
    fs::create_dir(&output_root).map_err(|e| e.to_string())?;
    let mut seen = std::collections::HashSet::new();
    let mut total: u64 = 0;
    let mut paths: Vec<(String, PathBuf)> = Vec::new();
    let mut snapshot = |id: &str,
                        extension: &str,
                        source: &Path,
                        per_file_limit: u64,
                        paths: &mut Vec<(String, PathBuf)>|
     -> Result<(String, u64), String> {
        let destination = input_root.join(format!(
            "{}.{}",
            hex::encode(Sha256::digest(id.as_bytes())),
            extension
        ));
        let (digest, copied) = stream_snapshot(
            source,
            &destination,
            per_file_limit,
            &cancelled,
            snapshot_deadline,
            &mut total,
            tier.max_total_input_bytes,
        )?;
        paths.push((id.to_owned(), destination));
        Ok((digest, copied))
    };
    for value in ids {
        check_snapshot_progress(&cancelled, snapshot_deadline)?;
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
            || metadata.len() > tier.max_attachment_bytes
            || a.byte_size < 0
        {
            return Err(format!(
                "附件不在存储内或超过 {} 档位的 {} 字节上限；请选择更小文件，或用 {} 档位支持的范围",
                tier.name,
                tier.max_attachment_bytes,
                tier.name,
            ));
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
        let (digest, copied) =
            snapshot(id, &extension, &path, tier.max_attachment_bytes, &mut paths)?;
        if let Some(expected) = a.sha256.as_deref().filter(|s| !s.is_empty()) {
            let expected = expected.strip_prefix("sha256:").unwrap_or(expected);
            if !expected.eq_ignore_ascii_case(&digest) {
                return Err("Attachment integrity check failed".into());
            }
        }
        if a.byte_size >= 0 && a.byte_size as u64 != copied {
            return Err("Attachment size does not match the recorded size".into());
        }
    }
    if !artifacts.is_empty() {
        let root = authorized_artifact_root(sessions_dir, conversation_id)?
            .ok_or("No computed files exist in this conversation")?;
        for value in artifacts {
            check_snapshot_progress(&cancelled, snapshot_deadline)?;
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
            if artifact.byte_size > tier.max_artifact_bytes {
                return Err(format!(
                    "计算产物超过 {} 档位的 {} 字节上限",
                    tier.name, tier.max_artifact_bytes,
                ));
            }
            let path = crate::artifact_gateway::validate_record(&artifact, &[root.clone()])
                .map_err(|e| e.message)?;
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
            snapshot(id, &extension, &path, tier.max_artifact_bytes as u64, &mut paths)?;
        }
    }
    let mut normalized = input.clone();
    normalized["attachmentIds"] = json!(paths.iter().map(|(id, _)| id).collect::<Vec<_>>());
    let mut result = if processing == "chunked" {
        let deadline = options.deadline.unwrap_or_else(|| {
            Instant::now()
                + std::time::Duration::from_millis(
                    input["timeoutMs"].as_u64().unwrap_or(15_000).min(30_000),
                )
        });
        fn noop_progress(_: crate::data_compute::chunked::ChunkProgress) {}
        let progress = options.progress.unwrap_or(&noop_progress);
        crate::data_compute::chunked::execute(
            &paths,
            &output_root,
            &normalized,
            cancelled,
            deadline,
            progress,
        )?
    } else {
        crate::data_compute::execute(&paths, &output_root, &normalized, cancelled)?
    };
    if let Some(files) = result["files"].as_array_mut() {
        for file in files {
            if let Some(path) = file["path"].as_str() {
                file["id"] = json!(Database::computed_artifact_id(path));
            }
        }
    }
    result["computedBy"] = json!("fox-quickjs");
    result["attachmentIds"] = json!(ids);
    // Only a completed computation keeps its workspace: outputs referenced by
    // the result live there. Any earlier failure, cancellation or limit breach
    // dropped the guard and removed the partial snapshot set.
    guard.keep();
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::AttachmentRecord;
    use std::cell::Cell;
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    use std::time::Duration;

    struct Fixture {
        root: PathBuf,
        storage: PathBuf,
        sessions: PathBuf,
        database: Database,
        conversation_id: String,
    }

    impl Fixture {
        fn new(tag: &str) -> Self {
            let root = std::env::temp_dir()
                .join(format!("fox-snapshot-{tag}-{}", uuid::Uuid::new_v4()));
            let storage = root.join("storage");
            let sessions = root.join("sessions");
            fs::create_dir_all(&storage).unwrap();
            fs::create_dir_all(&sessions).unwrap();
            let database = Database::open(root.join("test.db")).unwrap();
            let conversation = database
                .create_conversation(database.default_agent_id(), None, None, None)
                .unwrap();
            Self {
                root,
                storage,
                sessions,
                database,
                conversation_id: conversation.id,
            }
        }

        /// Write a CSV attachment and register it exactly as the upload path
        /// would (recorded size + sha256). `size_delta` fakes a stale record.
        fn attach_csv_with_size(
            &self,
            id: &str,
            rows: usize,
            corrupt_hash: bool,
            size_delta: i64,
        ) -> AttachmentRecord {
            let path = self.storage.join(format!("{id}.csv"));
            let mut text = String::from("id,name,value\n");
            for index in 0..rows {
                text.push_str(&format!("{index},item-{index},{}\n", index % 100));
            }
            let bytes = text.into_bytes();
            fs::write(&path, &bytes).unwrap();
            let digest = if corrupt_hash {
                "0".repeat(64)
            } else {
                hex::encode(Sha256::digest(&bytes))
            };
            let record = AttachmentRecord {
                id: id.to_owned(),
                conversation_id: self.conversation_id.clone(),
                message_id: None,
                display_name: format!("{id}.csv"),
                storage_path: path.to_string_lossy().into_owned(),
                media_type: Some("text/csv".to_owned()),
                byte_size: bytes.len() as i64 + size_delta,
                sha256: Some(digest),
                status: "ready".to_owned(),
                created_at: 1,
            };
            self.database
                .add_attachments(std::slice::from_ref(&record))
                .unwrap();
            record
        }

        fn attach_csv(&self, id: &str, rows: usize, corrupt_hash: bool) -> AttachmentRecord {
            self.attach_csv_with_size(id, rows, corrupt_hash, 0)
        }

        fn run_dir(&self, run_id: &str) -> PathBuf {
            conversation_compute_root(&self.sessions, &self.conversation_id)
                .join(hex::encode(Sha256::digest(run_id.as_bytes())))
        }

        fn run_dir_is_empty(&self, run_id: &str) -> bool {
            match fs::read_dir(self.run_dir(run_id)) {
                Ok(mut entries) => entries.next().is_none(),
                Err(_) => true,
            }
        }
    }

    fn chunked_input(ids: &[&str]) -> Value {
        json!({
            "processing": "chunked",
            "profile": "large",
            "attachmentIds": ids,
            "code": "let rows=0;\nfunction onChunk(c){ rows+=c.rows.length; }\nfunction onFinish(){ return {rows:rows}; }",
        })
    }

    fn options(progress: Option<&dyn Fn(crate::data_compute::chunked::ChunkProgress)>) -> ComputeOptions<'_> {
        ComputeOptions {
            deadline: Some(Instant::now() + Duration::from_secs(300)),
            progress,
        }
    }

    #[test]
    fn snapshot_streams_inputs_and_publishes_only_complete_runs() {
        let fixture = Fixture::new("ok");
        fixture.attach_csv("a1", 5_000, false);
        fixture.attach_csv("a2", 5_000, false);
        let result = execute_with_options(
            &fixture.database,
            &fixture.storage,
            &fixture.sessions,
            &fixture.conversation_id,
            "run-ok",
            &chunked_input(&["a1", "a2"]),
            || false,
            options(None),
        )
        .expect("a valid snapshot set computes");
        assert_eq!(result["result"]["rows"], 10_002);
        assert_eq!(result["rowsProcessed"], 10_002);
        // The engine only ever saw the private snapshot copies, inside this
        // execution's own (uuid) workspace directory.
        let workspace = fs::read_dir(fixture.run_dir("run-ok"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let snapshots: Vec<_> = fs::read_dir(workspace.join("inputs"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(snapshots.len(), 2);
        assert!(snapshots.iter().all(|name| name.ends_with(".csv")));
        let _ = fs::remove_dir_all(&fixture.root);
    }

    #[test]
    fn snapshot_rejects_integrity_and_size_mismatch_without_leaving_partials() {
        let fixture = Fixture::new("integrity");
        fixture.attach_csv("bad", 100, true);
        let error = execute_with_options(
            &fixture.database,
            &fixture.storage,
            &fixture.sessions,
            &fixture.conversation_id,
            "run-bad",
            &chunked_input(&["bad"]),
            || false,
            options(None),
        )
        .unwrap_err();
        assert!(error.contains("integrity"), "{error}");
        assert!(
            fixture.run_dir_is_empty("run-bad"),
            "a rejected snapshot must not be published"
        );

        // Recorded size disagrees with the bytes on disk.
        let fixture2 = Fixture::new("size");
        fixture2.attach_csv_with_size("short", 100, false, 1);
        let error = execute_with_options(
            &fixture2.database,
            &fixture2.storage,
            &fixture2.sessions,
            &fixture2.conversation_id,
            "run-short",
            &chunked_input(&["short"]),
            || false,
            options(None),
        )
        .unwrap_err();
        assert!(error.contains("size"), "{error}");
        assert!(fixture2.run_dir_is_empty("run-short"));
        let _ = fs::remove_dir_all(&fixture.root);
        let _ = fs::remove_dir_all(&fixture2.root);
    }

    #[test]
    fn changed_source_and_over_limit_total_are_rejected_with_cleanup() {
        let fixture = Fixture::new("changed");
        let record = fixture.attach_csv("moving", 2_000, false);
        let source = PathBuf::from(&record.storage_path);
        // Deterministic seam: append to the source after its copy completes and
        // before the snapshot re-checks the source identity.
        set_post_copy_hook(Some(Box::new(move || {
            let mut file = fs::OpenOptions::new().append(true).open(&source).unwrap();
            file.write_all(b"x\n").unwrap();
        })));
        let error = execute_with_options(
            &fixture.database,
            &fixture.storage,
            &fixture.sessions,
            &fixture.conversation_id,
            "run-moving",
            &chunked_input(&["moving"]),
            || false,
            options(None),
        )
        .unwrap_err();
        set_post_copy_hook(None);
        assert!(error.contains("发生变化"), "{error}");
        assert!(fixture.run_dir_is_empty("run-moving"));

        // Cumulative limit: two 20 MiB inputs are individually allowed but
        // breach the large tier's 64 MiB total only after the second copy.
        let fixture2 = Fixture::new("total");
        let big = "x".repeat(20 * 1024 * 1024);
        for id in ["t1", "t2", "t3", "t4"] {
            let path = fixture2.storage.join(format!("{id}.csv"));
            fs::write(&path, &big).unwrap();
            let bytes = fs::read(&path).unwrap();
            fixture2
                .database
                .add_attachments(&[AttachmentRecord {
                    id: id.to_owned(),
                    conversation_id: fixture2.conversation_id.clone(),
                    message_id: None,
                    display_name: format!("{id}.csv"),
                    storage_path: path.to_string_lossy().into_owned(),
                    media_type: Some("text/csv".to_owned()),
                    byte_size: bytes.len() as i64,
                    sha256: Some(hex::encode(Sha256::digest(&bytes))),
                    status: "ready".to_owned(),
                    created_at: 1,
                }])
                .unwrap();
        }
        let error = execute_with_options(
            &fixture2.database,
            &fixture2.storage,
            &fixture2.sessions,
            &fixture2.conversation_id,
            "run-total",
            &chunked_input(&["t1", "t2", "t3", "t4"]),
            || false,
            options(None),
        )
        .unwrap_err();
        assert!(error.contains("合计"), "{error}");
        assert!(fixture2.run_dir_is_empty("run-total"));
        let _ = fs::remove_dir_all(&fixture.root);
        let _ = fs::remove_dir_all(&fixture2.root);
    }

    #[test]
    fn cancellation_during_snapshot_stops_before_the_next_input_and_cleans_up() {
        let fixture = Fixture::new("cancel");
        fixture.attach_csv("c1", 1_000, false);
        fixture.attach_csv("c2", 1_000, false);
        fixture.attach_csv("c3", 1_000, false);
        let cancelled = Arc::new(AtomicBool::new(false));
        // Flip cancellation after the first snapshot copy, i.e. the third
        // input's progress check is the deterministic stop point.
        let flag = Arc::clone(&cancelled);
        let fired = Arc::new(AtomicBool::new(false));
        let fired_hook = Arc::clone(&fired);
        set_post_copy_hook(Some(Box::new(move || {
            if !fired_hook.swap(true, Ordering::SeqCst) {
                flag.store(true, Ordering::SeqCst);
            }
        })));
        let flag = Arc::clone(&cancelled);
        let error = execute_with_options(
            &fixture.database,
            &fixture.storage,
            &fixture.sessions,
            &fixture.conversation_id,
            "run-cancel",
            &chunked_input(&["c1", "c2", "c3"]),
            move || flag.load(Ordering::SeqCst),
            options(None),
        )
        .unwrap_err();
        set_post_copy_hook(None);
        assert!(error.contains("cancelled"), "{error}");
        assert!(
            fixture.run_dir_is_empty("run-cancel"),
            "a cancelled snapshot set must not stay behind"
        );
        let _ = fs::remove_dir_all(&fixture.root);
    }

    /// R7's acceptance measurement: peak allocation of the snapshot phase and
    /// of the whole run, driven through the production entry point. Run with
    /// `--test-threads=1` so the process-wide allocator probe is clean.
    #[test]
    #[ignore = "memory measurement: run explicitly with --test-threads=1"]
    fn snapshot_peak_memory_stays_bounded_with_production_entry() {
        let fixture = Fixture::new("peak");
        // Close to the `large` tier's ceilings: 4 inputs of ~14 MiB each is
        // ~55 MiB of source, inside the tier's 64 MiB cumulative limit and the
        // 32 MiB per-file limit. The old implementation held all of it.
        const PER_INPUT_ROWS: usize = 700_000;
        const INPUTS: usize = 4;
        let ids = ["p1", "p2", "p3", "p4"];
        let mut total_bytes = 0u64;
        for id in ids.iter().take(INPUTS) {
            let record = fixture.attach_csv(id, PER_INPUT_ROWS, false);
            total_bytes += record.byte_size as u64;
        }
        let snapshot_peak = Cell::new(0usize);
        let snapshot_live = Cell::new(0usize);
        let measured = Cell::new(false);
        // Borrows (not moves) the cells: `Cell` gives interior mutability, so
        // the measurement stays readable after the run.
        let progress = |chunk: crate::data_compute::chunked::ChunkProgress| {
            if !measured.get() && chunk.phase == "measured" {
                snapshot_peak.set(alloc_probe::peak_bytes());
                snapshot_live.set(alloc_probe::current_bytes());
                measured.set(true);
            }
        };
        alloc_probe::reset_peak();
        let result = execute_with_options(
            &fixture.database,
            &fixture.storage,
            &fixture.sessions,
            &fixture.conversation_id,
            "run-peak",
            &chunked_input(&ids),
            || false,
            options(Some(&progress)),
        )
        .expect("the peak measurement run succeeds");
        let total_peak = alloc_probe::peak_bytes();
        // Control: the previous implementation's snapshot phase (read every
        // input into memory at once) measured with the same probe, so the
        // report can state the actual delta rather than "it was worse".
        alloc_probe::reset_peak();
        let mut buffers: Vec<Vec<u8>> = Vec::new();
        for id in ids.iter().take(INPUTS) {
            let path = fixture.storage.join(format!("{id}.csv"));
            buffers.push(fs::read(&path).unwrap());
        }
        let old_style_peak = alloc_probe::peak_bytes();
        let held = buffers.iter().map(|buffer| buffer.len()).sum::<usize>();
        drop(buffers);
        let rows = result["result"]["rows"].as_u64().unwrap();
        assert_eq!(rows, (PER_INPUT_ROWS * INPUTS + INPUTS) as u64);
        println!(
            "SNAPSHOT_MEMORY {}",
            json!({
                "inputs": INPUTS,
                "sourceBytes": total_bytes,
                "snapshotPeakBytes": snapshot_peak.get(),
                "snapshotLiveBytesAtMeasured": snapshot_live.get(),
                "runTotalPeakBytes": total_peak,
                "copyBufferBytes": SNAPSHOT_COPY_BUFFER_BYTES,
                "rowsProcessed": rows,
                "oldStyleHeldBytes": held,
                "oldStylePeakBytes": old_style_peak,
            })
        );
        // Streaming proof: snapshotting ~55 MiB must not hold anything close to
        // the input size. The old implementation held every byte at once.
        assert!(
            (snapshot_peak.get() as u64) < total_bytes / 8,
            "snapshot peak {} must stay far below the {} source bytes",
            snapshot_peak.get(),
            total_bytes
        );
        let _ = fs::remove_dir_all(&fixture.root);
    }

    /// End-to-end consumption loop, all in-process and default-set (no CLI):
    /// a large result spills to a JSON artifact -> a later compute reads that
    /// artifact by id and converts it to CSV -> the CSV artifact is what an
    /// Office import resolves byte-for-byte. The model only ever sees a bounded
    /// summary and references; the full table never returns to the request.
    #[test]
    fn spilled_json_artifact_is_consumed_converted_and_imported_by_reference() {
        let fixture = Fixture::new("consume");

        // 1) First compute returns a large table: it spills to large-result.json.
        let first = execute(
            &fixture.database,
            &fixture.storage,
            &fixture.sessions,
            &fixture.conversation_id,
            "run-consume-1",
            &json!({"code":
                "return Array.from({length:6000},(_,i)=>({id:i,name:'task-'+i,qty:i%100}));"}),
            || false,
        )
        .unwrap();
        let first_result_bytes =
            serde_json::to_vec(&first["result"]).unwrap().len();
        assert!(first_result_bytes <= 64 * 1024, "the inline result stays bounded");
        let summary = &first["result"]["summary"];
        assert_eq!(summary["complete"], false);
        let spill_file = first["files"]
            .as_array()
            .unwrap()
            .iter()
            .find(|file| file["displayName"] == "large-result.json")
            .expect("the spilled artifact is published as a file");
        let spill_id = spill_file["id"].as_str().unwrap();
        assert!(spill_id.starts_with("compute-artifact:"));

        // Finalization persists files by conversation. Seed the artifact
        // rows the same way computed_artifacts::persist would (run_id is not
        // used for scoped reads).
        let persist = |file: &Value| {
            fixture
                .database
                .with_connection(|connection| {
                    connection.execute(
                        "INSERT INTO artifacts(id,conversation_id,run_id,display_name,artifact_type,storage_path,byte_size,sha256,media_type,status,created_at,updated_at)
                         VALUES(?1,?2,NULL,?3,'created_file',?4,?5,?6,?7,'ready',?8,?8)",
                        rusqlite::params![
                            file["id"].as_str().unwrap(),
                            fixture.conversation_id,
                            file["displayName"].as_str().unwrap(),
                            file["path"].as_str().unwrap(),
                            file["bytes"].as_i64().unwrap(),
                            file["sha256"].as_str().unwrap(),
                            file["mediaType"].as_str().unwrap(),
                            crate::database::now_ms(),
                        ],
                    )
                })
                .unwrap();
        };
        persist(spill_file);

        // 2) Second compute reads the stored JSON by reference and converts it
        //    to CSV via saveFile. It never re-runs the original statistics.
        let conversion_code = r#"
            const rows = JSON.parse(attachments[0].text);
            let csv = 'id,name,qty\n';
            for (const row of rows) csv += row.id + ',' + row.name + ',' + row.qty + '\n';
            saveFile('converted.csv', csv);
            return { rows: rows.length, csvBytes: csv.length };
        "#;
        let second = execute(
            &fixture.database,
            &fixture.storage,
            &fixture.sessions,
            &fixture.conversation_id,
            "run-consume-2",
            &json!({"code": conversion_code, "artifactIds": [spill_id]}),
            || false,
        )
        .unwrap();
        assert_eq!(second["result"]["rows"], 6000);
        let csv_file = second["files"]
            .as_array()
            .unwrap()
            .iter()
            .find(|file| file["displayName"] == "converted.csv")
            .expect("a CSV artifact is produced");
        let csv_path = csv_file["path"].as_str().unwrap();
        let csv_bytes = fs::read(csv_path).unwrap();
        let csv_text = String::from_utf8(csv_bytes.clone()).unwrap();
        assert_eq!(csv_text.lines().count(), 6001, "header plus every source row");
        assert!(csv_text.starts_with("id,name,qty\n"));
        assert!(csv_text.contains("\n5999,task-5999,99\n"));
        assert_eq!(
            hex::encode(Sha256::digest(&csv_bytes)),
            csv_file["sha256"].as_str().unwrap()
        );
        persist(csv_file);

        // 3) The Office import resolves the CSV artifact to its exact bytes.
        let office_context = crate::office::OfficeCallContext {
            database: &fixture.database,
            sessions_dir: &fixture.sessions,
            conversation_id: &fixture.conversation_id,
            artifacts_dir: None,
        };
        let prepared = crate::office::prepare_with_context(
            "office_import_data",
            &json!({
                "output": "out.xlsx",
                "sheet": "Sheet1",
                "artifactId": csv_file["id"].as_str().unwrap(),
            }),
            Some(fixture.root.to_str().unwrap()),
            "allow",
            Some(&office_context),
        )
        .expect("the converted CSV artifact resolves for Office");
        // The CLI receives the stored CSV verbatim; the request never carried it.
        assert_eq!(prepared.stdin_payload().unwrap(), csv_text);
        assert_eq!(prepared.import_dimensions(), Some((6001, 3)));

        // The spill JSON is NOT a valid direct import target (wrong format).
        let json_import = crate::office::prepare_with_context(
            "office_import_data",
            &json!({
                "output": "bad.xlsx",
                "sheet": "Sheet1",
                "artifactId": spill_id,
            }),
            Some(fixture.root.to_str().unwrap()),
            "allow",
            Some(&office_context),
        );
        assert!(json_import.is_err_and(|error| error.contains("不一致")));

        let _ = fs::remove_dir_all(&fixture.root);
    }
}
