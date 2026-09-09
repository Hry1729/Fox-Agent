//! Bounded Rust executors for the four canonical project readers. Every opened
//! resource is checked against the frozen root; no runtime fallback is allowed.
use crate::kernel::CancellationToken;
use fox_engine_protocol::{ResourceExecutor, RunControlBinding};
use serde_json::{json, Value};
use std::{collections::VecDeque, fs::{self, File, OpenOptions}, io::Read, path::{Path, PathBuf}, time::{Duration, Instant}};

const MAX_ENTRIES: usize = 4_000;
const MAX_MATCHES: usize = 200;
const MAX_CHARS: usize = 120_000;
const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;
const MAX_SCAN_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Clone, Copy)]
struct ReadLimits { entries: usize, matches: usize, read_chars: usize, output_chars: usize, line_chars: usize }

impl ReadLimits {
    fn for_profile(profile: &str) -> Self {
        if profile == "graph_readonly_preview" {
            Self { entries: 1_000, matches: 50, read_chars: 24_000, output_chars: 24_000, line_chars: 1_000 }
        } else {
            Self { entries: MAX_ENTRIES, matches: MAX_MATCHES, read_chars: MAX_CHARS, output_chars: MAX_CHARS, line_chars: 8_000 }
        }
    }
}

pub fn is_reader(tool: &str) -> bool { matches!(tool, "read" | "ls" | "find" | "grep") }

/// Independent byte-level observation. No model text or prior result is used
/// as evidence, and a matching file is not proof of which process wrote it.
pub(crate) fn reconciliation_fingerprint(binding: &RunControlBinding, path: &str, token: &CancellationToken) -> Result<Value,String> {
    use sha2::{Digest,Sha256};
    binding.validate()?;
    let approved=crate::tool_guard::approve_read_only_tool("read",&json!({"path":path}),binding.permission.project_root.as_deref())?;
    let root=Path::new(binding.permission.project_root.as_deref().ok_or("missing frozen project root")?).canonicalize().map_err(|_|"原项目目录不可访问")?;
    let gateway=Gateway {root,cancellation:token,deadline:Instant::now()+Duration::from_secs(10),remaining_bytes:MAX_FILE_BYTES,limits:ReadLimits::for_profile(&binding.execution_profile_id)};
    let mut file=gateway.open(&approved.resolved_path,false)?;
    let mut hash=Sha256::new();let mut bytes=0u64;let mut chunk=[0u8;16*1024];
    loop {gateway.check()?;let n=file.read(&mut chunk).map_err(|_|"文件读取失败")?;if n==0 {break;}bytes+=n as u64;if bytes>MAX_FILE_BYTES{return Err("文件超过 8 MiB 独立核验上限".into());}hash.update(&chunk[..n]);}
    Ok(json!({"bytes":bytes,"sha256":format!("sha256:{}",hex::encode(hash.finalize()))}))
}

struct Gateway<'a> {
    root: PathBuf,
    cancellation: &'a CancellationToken,
    deadline: Instant,
    remaining_bytes: u64,
    limits: ReadLimits,
}

impl Gateway<'_> {
    fn check(&self) -> Result<(), String> {
        self.cancellation.check()?;
        if Instant::now() >= self.deadline { return Err("resource gateway execution budget exceeded".into()); }
        Ok(())
    }
    fn open(&self, path: &Path, directory: bool) -> Result<File, String> {
        self.check()?;
        let canonical = path.canonicalize().map_err(|error| format!("resource cannot be resolved: {error}"))?;
        if !canonical.starts_with(&self.root) { return Err("resource is outside the frozen project root".into()); }
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            // Pin against replacement/rename while reading; allow other readers.
            options.share_mode(1);
            if directory { options.custom_flags(0x0200_0000); }
        }
        let file = options.open(&canonical).map_err(|error| format!("resource cannot be opened: {error}"))?;
        let metadata = file.metadata().map_err(|error| error.to_string())?;
        if directory != metadata.is_dir() || (!directory && !metadata.is_file()) {
            return Err("resource type does not match the requested operation".into());
        }
        verify_opened_path(&file, &canonical, &self.root)?;
        self.check()?;
        Ok(file)
    }
    fn bytes(&mut self, path: &Path) -> Result<Vec<u8>, String> {
        let mut file = self.open(path, false)?;
        let mut contents = Vec::new();
        let mut chunk = [0u8; 16 * 1024];
        loop {
            self.check()?;
            let count = file.read(&mut chunk).map_err(|error| error.to_string())?;
            if count == 0 { break; }
            if contents.len() as u64 + count as u64 > MAX_FILE_BYTES || count as u64 > self.remaining_bytes {
                return Err("resource gateway text byte limit exceeded".into());
            }
            self.remaining_bytes -= count as u64;
            contents.extend_from_slice(&chunk[..count]);
        }
        Ok(contents)
    }
    fn text(&mut self, path: &Path, extract_office: bool) -> Result<String, String> {
        let contents = self.bytes(path)?;
        if extract_office {
            if let Some(text) = crate::local_knowledge_import::extract_office_text(&contents, &path.to_string_lossy())? {
                self.check()?;
                return Ok(text);
            }
        }
        Ok(String::from_utf8_lossy(&contents).into_owned())
    }
    fn entries(&self, path: &Path, limit: usize) -> Result<(Vec<Entry>, bool), String> {
        let _directory_guard = self.open(path, true)?;
        let mut entries = Vec::new();
        for item in fs::read_dir(path).map_err(|error| error.to_string())? {
            self.check()?;
            let item = item.map_err(|error| error.to_string())?;
            if entries.len() == limit { return Ok((entries, true)); }
            let target = item.path();
            let canonical = target.canonicalize().map_err(|error| format!("nested resource cannot be resolved: {error}"))?;
            if !canonical.starts_with(&self.root) { return Err("nested resource is outside the frozen project root".into()); }
            let kind = item.file_type().map_err(|error| error.to_string())?;
            // Never traverse links. File reads still validate the actual handle.
            entries.push(Entry { path: target, name: item.file_name().to_string_lossy().into_owned(), directory: kind.is_dir() });
        }
        entries.sort_by(|a,b| a.name.cmp(&b.name));
        Ok((entries, false))
    }
    fn walk(&self, path: &Path) -> Result<(Vec<Entry>, bool), String> {
        let mut queue = VecDeque::from([path.to_path_buf()]);
        let mut found = Vec::new();
        while let Some(directory) = queue.pop_front() {
            let (entries, truncated) = self.entries(&directory, self.limits.entries - found.len())?;
            for entry in entries {
                if entry.directory { queue.push_back(entry.path.clone()); }
                found.push(entry);
            }
            if truncated || found.len() == self.limits.entries { return Ok((found, truncated || !queue.is_empty())); }
        }
        Ok((found, false))
    }
}

struct Entry { path: PathBuf, name: String, directory: bool }

#[cfg(windows)]
fn verify_opened_path(file: &File, _requested: &Path, root: &Path) -> Result<(), String> {
    use std::os::windows::{io::AsRawHandle, ffi::OsStringExt};
    use windows::Win32::{Foundation::HANDLE, Storage::FileSystem::{GetFinalPathNameByHandleW, FILE_NAME_NORMALIZED}};
    let mut buffer = vec![0u16; 32_768];
    let count = unsafe { GetFinalPathNameByHandleW(HANDLE(file.as_raw_handle()), &mut buffer, FILE_NAME_NORMALIZED) } as usize;
    if count == 0 || count >= buffer.len() { return Err("cannot validate opened resource handle".into()); }
    let opened = PathBuf::from(std::ffi::OsString::from_wide(&buffer[..count]));
    if !opened.starts_with(root) { return Err("opened resource escaped the frozen project root".into()); }
    Ok(())
}

#[cfg(not(windows))]
fn verify_opened_path(file: &File, requested: &Path, root: &Path) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    let opened = file.metadata().map_err(|error| error.to_string())?;
    let canonical = requested.canonicalize().map_err(|error| error.to_string())?;
    let current = fs::metadata(&canonical).map_err(|error| error.to_string())?;
    if !canonical.starts_with(root) || current.dev() != opened.dev() || current.ino() != opened.ino() {
        return Err("opened resource identity changed".into());
    }
    Ok(())
}

fn slice_utf16(text: &str, offset: usize, limit: usize) -> String {
    String::from_utf16_lossy(&text.encode_utf16().skip(offset).take(limit).collect::<Vec<_>>())
}
fn result(text: String, mut details: Value, max_chars: usize) -> Value {
    let length = text.encode_utf16().count();
    details["outputTruncated"] = json!(length > max_chars);
    json!({ "content": [{"type":"text", "text": slice_utf16(&text, 0, max_chars)}], "details": details })
}

pub fn execute(binding: &RunControlBinding, tool: &str, input: &Value, cancellation: &CancellationToken) -> Result<Value, String> {
    execute_with_budget(binding, tool, input, cancellation, Duration::from_millis(binding.budgets.tool_execution_ms as u64))
}

pub(crate) fn execute_with_budget(binding: &RunControlBinding, tool: &str, input: &Value, cancellation: &CancellationToken, budget: Duration) -> Result<Value,String> {
    binding.validate()?;
    if binding.read_only_executor != ResourceExecutor::Rust || !is_reader(tool) {
        return Err("resource gateway is not the frozen executor for this tool".into());
    }
    let approved = crate::tool_guard::approve_read_only_tool(tool, input, binding.permission.project_root.as_deref())?;
    let root = Path::new(binding.permission.project_root.as_deref().ok_or("missing frozen project root")?).canonicalize().map_err(|error| error.to_string())?;
    let limits = ReadLimits::for_profile(&binding.execution_profile_id);
    let mut gateway = Gateway { root, cancellation, deadline: Instant::now() + budget.min(Duration::from_millis(binding.budgets.tool_execution_ms as u64)), remaining_bytes: MAX_SCAN_BYTES, limits };
    gateway.check()?;
    let path = approved.resolved_path;
    if tool == "read" {
        let text = gateway.text(&path, true)?;
        let offset = input.get("offset").and_then(Value::as_f64).unwrap_or(0.0).max(0.0) as usize;
        let limit = input.get("limit").and_then(Value::as_f64).filter(|v| *v != 0.0).unwrap_or(limits.read_chars as f64).clamp(1.0, limits.read_chars as f64) as usize;
        let truncated = offset.saturating_add(limit) < text.encode_utf16().count();
        return Ok(result(slice_utf16(&text, offset, limit), json!({"path":path,"truncated":truncated}), limits.output_chars));
    }
    if tool == "ls" {
        let (entries, truncated) = gateway.entries(&path, limits.entries)?;
        return Ok(result(entries.iter().map(|e| format!("{} {}", if e.directory {"[dir]"} else {"[file]"}, e.name)).collect::<Vec<_>>().join("\n"), json!({"path":path,"count":entries.len(),"truncated":truncated}), limits.output_chars));
    }
    let needle = input.get("pattern").and_then(Value::as_str).unwrap_or_default().to_lowercase();
    let (entries, scan_truncated) = gateway.walk(&path)?;
    let mut matches = Vec::new();
    if tool == "find" {
        for entry in entries {
            gateway.check()?;
            if entry.name.to_lowercase().contains(&needle) { matches.push(entry.path.to_string_lossy().into_owned()); }
            if matches.len() == limits.matches { break; }
        }
    } else {
        for entry in entries.into_iter().filter(|e| !e.directory) {
            let text = gateway.text(&entry.path, false)?;
            for (index, line) in text.split('\n').enumerate() {
                gateway.check()?;
                let line = line.strip_suffix('\r').unwrap_or(line);
                if line.to_lowercase().contains(&needle) {
                    matches.push(format!("{}:{}:{}",entry.path.display(),index+1,slice_utf16(line,0,limits.line_chars)));
                }
                if matches.len() == limits.matches { break; }
            }
            if matches.len() == limits.matches { break; }
        }
    }
    Ok(result(matches.join("\n"), json!({"count":matches.len(),"scanTruncated":scan_truncated,"matchLimitReached":matches.len()==limits.matches}), limits.output_chars))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::{CancellationRegistry, CancellationPort};
    use fox_engine_protocol::{ExecutionAuthority, FrozenPermission, PermissionMode, TimeBudgets};

    fn fixture() -> (RunControlBinding, CancellationRegistry, CancellationToken) {
        let root = std::env::temp_dir().join(format!("fox-gateway-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/note.txt"), "A😀Fox\r\nsecond Fox line\n").unwrap();
        let permission = FrozenPermission { mode: PermissionMode::ReadOnly, project_root: Some(root.to_string_lossy().into_owned()), grants: vec![] };
        let binding = RunControlBinding {
            schema_version: 1, run_id: "gateway-run".into(), conversation_id: "gateway-conversation".into(), engine_id: "pi".into(),
            execution_profile_id: "test-profile".into(), authority: ExecutionAuthority::Legacy, read_only_executor: ResourceExecutor::Rust,
            permission_snapshot_id: crate::database::Database::run_control_permission_hash(&permission).unwrap(), permission, budgets: TimeBudgets::default(),
        };
        let registry = CancellationRegistry::default();
        registry.register_run(&binding.run_id).unwrap();
        let token = registry.tool_token(&binding.run_id, "tool").unwrap();
        (binding, registry, token)
    }

    #[test]
    fn resource_gateway_uses_frozen_root_when_directory_path_is_omitted() {
        let (binding, _, token) = fixture();
        let root = Path::new(binding.permission.project_root.as_ref().unwrap());
        fs::write(root.join("AGV长时间任务汇总统计表.xlsx"), "fixture").unwrap();
        let list = execute(&binding, "ls", &json!({}), &token).unwrap();
        assert!(list["content"][0]["text"].as_str().unwrap().contains("AGV长时间任务汇总统计表.xlsx"));
        let found = execute(&binding, "find", &json!({"pattern":"AGV"}), &token).unwrap();
        assert_eq!(found["details"]["count"], 1);
        let grep = execute(&binding, "grep", &json!({"pattern":"second Fox"}), &token).unwrap();
        assert_eq!(grep["details"]["count"], 1);
        assert!(execute(&binding, "read", &json!({}), &token).is_err());
        assert!(execute(&binding, "ls", &json!({"path":".."}), &token).is_err());
        let mut unbound = binding.clone();
        unbound.permission.project_root = None;
        unbound.permission_snapshot_id = crate::database::Database::run_control_permission_hash(&unbound.permission).unwrap();
        assert!(execute(&unbound, "ls", &json!({}), &token).is_err());
    }

    #[test]
    fn resource_gateway_executes_read_ls_find_grep_with_utf16_offsets() {
        let (binding, _, token) = fixture();
        let read = execute(&binding, "read", &json!({"path":"src/note.txt","offset":1,"limit":2}), &token).unwrap();
        assert_eq!(read["content"][0]["text"], "😀");
        assert_eq!(read["details"]["truncated"], true);
        let list = execute(&binding, "ls", &json!({"path":"src"}), &token).unwrap();
        assert_eq!(list["content"][0]["text"], "[file] note.txt");
        let found = execute(&binding, "find", &json!({"path":".","pattern":"NOTE"}), &token).unwrap();
        assert_eq!(found["details"]["count"], 1);
        let searched = execute(&binding, "grep", &json!({"path":".","pattern":"fox"}), &token).unwrap();
        assert_eq!(searched["details"]["count"], 2);
        assert!(searched["content"][0]["text"].as_str().unwrap().contains(":2:second Fox line"));
    }

    #[test]
    fn reconciliation_fingerprint_reads_exact_bytes_and_rejects_scope_escape() {
        use sha2::{Digest,Sha256};
        let (binding,_,token)=fixture();
        let observed=reconciliation_fingerprint(&binding,"src/note.txt",&token).unwrap();
        let content="A😀Fox\r\nsecond Fox line\n";
        assert_eq!(observed["bytes"],content.len());
        assert_eq!(observed["sha256"],format!("sha256:{}",hex::encode(Sha256::digest(content.as_bytes()))));
        assert!(reconciliation_fingerprint(&binding,"../outside.txt",&token).is_err());
        let large=Path::new(binding.permission.project_root.as_ref().unwrap()).join("large.bin");
        File::create(&large).unwrap().set_len(MAX_FILE_BYTES+1).unwrap();
        assert!(reconciliation_fingerprint(&binding,"large.bin",&token).is_err());
    }

    #[test]
    fn graph_profile_keeps_stricter_read_match_and_output_limits() {
        let (mut binding, _, token) = fixture();
        binding.execution_profile_id = "graph_readonly_preview".into();
        let file = Path::new(binding.permission.project_root.as_ref().unwrap()).join("large.txt");
        fs::write(&file, "x".repeat(30_000)).unwrap();
        let read = execute(&binding, "read", &json!({"path":file,"limit":120_000}), &token).unwrap();
        assert_eq!(read["content"][0]["text"].as_str().unwrap().len(), 24_000);
        assert_eq!(read["details"]["truncated"], true);
        fs::write(&file, format!("{}\n", "x".repeat(2_000)).repeat(100)).unwrap();
        let grep = execute(&binding, "grep", &json!({"path":".","pattern":"x"}), &token).unwrap();
        assert_eq!(grep["details"]["count"], 50);
        assert_eq!(grep["details"]["matchLimitReached"], true);
        assert_eq!(grep["details"]["outputTruncated"], true);
        assert!(grep["content"][0]["text"].as_str().unwrap().encode_utf16().count() <= 24_000);
    }

    #[test]
    fn resource_gateway_rejects_wrong_executor_cancelled_calls_and_oversized_files() {
        let (mut binding, registry, token) = fixture();
        binding.read_only_executor = ResourceExecutor::Runtime;
        assert!(execute(&binding,"read",&json!({"path":"src/note.txt"}),&token).is_err());
        binding.read_only_executor = ResourceExecutor::Rust;
        let huge = Path::new(binding.permission.project_root.as_ref().unwrap()).join("huge.txt");
        File::create(&huge).unwrap().set_len(MAX_FILE_BYTES + 1).unwrap();
        assert!(execute(&binding,"read",&json!({"path":huge}),&token).unwrap_err().contains("byte limit"));
        registry.request_run_cancel(&binding.run_id);
        assert!(execute(&binding,"ls",&json!({"path":"."}),&token).unwrap_err().contains("tool.cancelled"));
    }

    #[test]
    fn resource_gateway_blocks_nested_links_outside_frozen_root() {
        let (binding, _, token) = fixture();
        let outside = std::env::temp_dir().join(format!("fox-gateway-outside-{}",uuid::Uuid::new_v4()));
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("secret.txt"), "must never be returned").unwrap();
        let link = Path::new(binding.permission.project_root.as_ref().unwrap()).join("outside-link");
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            let status = std::process::Command::new("cmd.exe").args(["/D","/C","mklink","/J"])
                .arg(&link).arg(&outside).creation_flags(0x0800_0000).stdout(std::process::Stdio::null()).status().unwrap();
            assert!(status.success(), "junction fixture must be created");
        }
        #[cfg(not(windows))]
        std::os::unix::fs::symlink(&outside,&link).unwrap();
        assert!(execute(&binding,"grep",&json!({"path":".","pattern":"must"}),&token).unwrap_err().contains("outside"));
        assert!(execute(&binding,"read",&json!({"path":outside.join("secret.txt")}),&token).is_err());
    }
}
