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
            let name = item.file_name().to_string_lossy().into_owned();
            if is_ignored_name(&name) { continue; }
            let target = item.path();
            let canonical = target.canonicalize().map_err(|error| format!("nested resource cannot be resolved: {error}"))?;
            if !canonical.starts_with(&self.root) { return Err("nested resource is outside the frozen project root".into()); }
            let kind = item.file_type().map_err(|error| error.to_string())?;
            // Never traverse links. File reads still validate the actual handle.
            entries.push(Entry { index: 0, path: target, name, directory: kind.is_dir() });
        }
        entries.sort_by(|a,b| a.name.cmp(&b.name));
        let truncated = entries.len() > limit;
        entries.truncate(limit);
        Ok((entries, truncated))
    }
    fn walk(&self, path: &Path, start_after: usize) -> Result<(Vec<Entry>, bool), String> {
        let mut queue = VecDeque::from([path.to_path_buf()]);
        let mut found = Vec::new();
        let mut index = 0usize;
        while let Some(directory) = queue.pop_front() {
            let (entries, truncated) = self.entries(&directory, self.limits.entries)?;
            for entry in entries {
                if entry.directory { queue.push_back(entry.path.clone()); }
                index += 1;
                if index <= start_after { continue; }
                found.push(Entry { index, ..entry });
                if found.len() == self.limits.entries { return Ok((found, true)); }
            }
            if truncated { return Ok((found, true)); }
        }
        Ok((found, false))
    }
}

struct Entry { index: usize, path: PathBuf, name: String, directory: bool }

fn is_ignored_name(name: &str) -> bool {
    matches!(name, "node_modules" | ".git" | ".hg" | ".svn" | "target" | "dist" | "build" | "out" | ".output" | "__pycache__" | ".pytest_cache" | ".mypy_cache" | ".next" | ".nuxt" | ".svelte-kit" | "coverage" | ".nyc_output" | ".turbo" | ".cache" | "vendor" | "third_party" | "third-party")
}

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

// Simple glob over file names: `*` matches any sequence, `?` matches one
// character. Matching is case-insensitive, like the Node runtime helper.
fn glob_match(glob: &str, name: &str) -> bool {
    let name_lower = name.to_lowercase();
    let glob_lower: String = glob.to_lowercase();
    fn recurse(pattern: &[char], text: &[char]) -> bool {
        match pattern.split_first() {
            None => text.is_empty(),
            Some((&'*', rest)) => {
                (0..=text.len()).any(|i| recurse(rest, &text[i..]))
            }
            Some((&'?', rest)) => !text.is_empty() && recurse(rest, &text[1..]),
            Some((&p, rest)) => !text.is_empty() && text[0] == p && recurse(rest, &text[1..]),
        }
    }
    let pattern: Vec<char> = glob_lower.chars().collect();
    let text: Vec<char> = name_lower.chars().collect();
    recurse(&pattern, &text)
}

// Slice UTF-16 code units without splitting a surrogate pair. Units stay
// UTF-16 (contract). Returns (text, safe_end): `safe_end` is the exclusive
// UTF-16 end index actually returned, so sequential pages advance by real
// length (no overlap, no gap). A trailing high surrogate whose low half is
// outside the window is dropped; the next page starts at `safe_end` and
// re-reads the pair whole. Never returns a lone surrogate.
fn slice_utf16_range(text: &str, offset: usize, limit: usize) -> (String, usize) {
    let units: Vec<u16> = text.encode_utf16().collect();
    let total = units.len();
    let mut start = offset.min(total);
    // Never start inside a pair: a low half at `start` steps back one unit.
    if start > 0 && start < total && (0xd800..=0xdbff).contains(&units[start - 1]) && (0xdc00..=0xdfff).contains(&units[start]) {
        start -= 1;
    }
    let mut safe_end = start.saturating_add(limit).min(total);
    // Never end inside a pair: drop a dangling high half.
    if safe_end > start && safe_end < total && (0xd800..=0xdbff).contains(&units[safe_end - 1]) && (0xdc00..=0xdfff).contains(&units[safe_end]) {
        safe_end -= 1;
    }
    // Defensive: never return a lone trailing high surrogate.
    if safe_end > start && safe_end == total && (0xd800..=0xdbff).contains(&units[safe_end - 1]) {
        safe_end -= 1;
    }
    // O-REVIEW-03 item 1: a window too small for even one whole character
    // must PROGRESS, not return a dead empty page (no infinite loop). Extend
    // just enough to return one complete character (at most one extra unit).
    if safe_end == start && start < total {
        safe_end = (start + 2).min(total);
        if safe_end < total && (0xd800..=0xdbff).contains(&units[safe_end - 1]) {
            safe_end += 1;
        }
    }
    (String::from_utf16_lossy(&units[start..safe_end]), safe_end)
}

// Surrogate-pair-safe prefix cut to a UTF-16 unit budget (O-REVIEW-03 item 2).
// A trailing high surrogate whose low half would be cut is dropped, so the
// result is always well-formed; callers derive navigation from the RETURNED
// length.
fn slice_utf16_budget(text: &str, max_units: usize) -> String {
    let units: Vec<u16> = text.encode_utf16().collect();
    if units.len() <= max_units {
        return text.to_owned();
    }
    let mut end = max_units;
    if end > 0 && end < units.len() && (0xd800..=0xdbff).contains(&units[end - 1]) && (0xdc00..=0xdfff).contains(&units[end]) {
        end -= 1;
    }
    if end > 0 && end == units.len() && (0xd800..=0xdbff).contains(&units[end - 1]) {
        end -= 1;
    }
    String::from_utf16_lossy(&units[..end])
}

fn bounded_nonempty_utf16(text: &str, max_units: usize) -> Result<String, String> {
    let bounded = slice_utf16_budget(text, max_units);
    if !text.is_empty() && bounded.is_empty() {
        return Err("output budget is too small for the next complete UTF-16 character; raise the output budget".into());
    }
    Ok(bounded)
}
// Opaque search cursor (RD-v1 B02), mirroring the Node runtime encoding so the
// two executors agree on token shape: base64url(JSON { v, scope, consumed }).
// `consumed` is tool-shaped: find uses an entry position; grep uses
// { f, l } = (file position, next line). Scope is a fingerprint of the query
// and search root — used only for mismatch rejection, never as an
// authorization basis, and never a snapshot fact (see the reported semantics).
#[derive(Clone, Copy)]
enum CursorPosition { Entry(usize), Pair(usize, usize) }

fn encode_search_cursor(scope: &str, consumed: usize) -> String {
    let payload = serde_json::json!({ "v": 1, "scope": scope, "consumed": consumed }).to_string();
    base64_encode_url(payload.as_bytes())
}

fn encode_search_cursor_pair(scope: &str, file: usize, line: usize) -> String {
    let payload = serde_json::json!({ "v": 1, "scope": scope, "consumed": { "f": file, "l": line } }).to_string();
    base64_encode_url(payload.as_bytes())
}

fn decode_search_cursor(cursor: &str) -> Result<(String, CursorPosition), String> {
    if cursor.is_empty() || cursor.len() > 4096 {
        return Err("invalid cursor: cursor must be a non-empty bounded string".into());
    }
    let bytes = base64_decode_url(cursor).map_err(|_| "invalid cursor: not decodable".to_owned())?;
    let text = String::from_utf8(bytes).map_err(|_| "invalid cursor: not valid UTF-8".to_owned())?;
    let payload: Value = serde_json::from_str(&text).map_err(|_| "invalid cursor: not JSON".to_owned())?;
    if payload.get("v").and_then(Value::as_u64) != Some(1) {
        return Err("invalid cursor: unknown version".into());
    }
    let scope = payload.get("scope").and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or("invalid cursor: missing scope")?;
    let consumed = payload.get("consumed").ok_or("invalid cursor: missing position")?;
    if let Some(entry) = consumed.as_u64() {
        return Ok((scope.to_owned(), CursorPosition::Entry(entry as usize)));
    }
    let file = consumed.get("f").and_then(Value::as_u64).ok_or("invalid cursor: missing file position")?;
    let line = consumed.get("l").and_then(Value::as_u64).ok_or("invalid cursor: missing line position")?;
    Ok((scope.to_owned(), CursorPosition::Pair(file as usize, line as usize)))
}

// Minimal base64url helpers (no new dependencies): encode/decode with padding
// tolerated on decode. Only used for the opaque cursor token.
fn base64_encode_url(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[((triple >> 18) & 0x3f) as usize] as char);
        out.push(TABLE[((triple >> 12) & 0x3f) as usize] as char);
        if chunk.len() > 1 { out.push(TABLE[((triple >> 6) & 0x3f) as usize] as char); }
        if chunk.len() > 2 { out.push(TABLE[(triple & 0x3f) as usize] as char); }
    }
    out
}

fn base64_decode_url(text: &str) -> Result<Vec<u8>, ()> {
    let mut values = Vec::with_capacity(text.len());
    for byte in text.bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'-' => 62,
            b'_' => 63,
            b'=' => continue,
            _ => return Err(()),
        };
        values.push(value);
    }
    let mut out = Vec::with_capacity(values.len() / 4 * 3);
    for chunk in values.chunks(4) {
        if chunk.len() == 1 { return Err(()); }
        let mut triple = 0u32;
        for (position, value) in chunk.iter().enumerate() {
            triple |= (*value as u32) << (18 - 6 * position);
        }
        out.push(((triple >> 16) & 0xff) as u8);
        if chunk.len() > 2 { out.push(((triple >> 8) & 0xff) as u8); }
        if chunk.len() > 3 { out.push((triple & 0xff) as u8); }
    }
    Ok(out)
}

fn result(text: String, mut details: Value, max_chars: usize) -> Value {
    // O-REVIEW-03 item 2: the final output budget is surrogate-pair safe and
    // consumers must derive navigation from the RETURNED length, so expose the
    // returned unit count here.
    let returned = slice_utf16_budget(&text, max_chars);
    let returned_units = returned.encode_utf16().count();
    details["outputTruncated"] = json!(returned_units < text.encode_utf16().count());
    details["returnedUnits"] = json!(returned_units);
    json!({ "content": [{"type":"text", "text": returned}], "details": details })
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
        // B03: line-mode parameters are mutually exclusive with offset/limit.
        let has_line_mode = input.get("startLine").is_some() || input.get("lineCount").is_some();
        let has_char_mode = input.get("offset").is_some() || input.get("limit").is_some();
        if has_line_mode && has_char_mode {
            return Err("startLine/lineCount and offset/limit are mutually exclusive; use one mode only".into());
        }
        if has_line_mode {
            // RD-v1: startLine/lineCount must be paired positive integers.
            let start_line = input.get("startLine").and_then(Value::as_u64)
                .filter(|v| *v >= 1)
                .ok_or("startLine and lineCount must both be positive integers (startLine is 1-based)")? as usize;
            let line_count = input.get("lineCount").and_then(Value::as_u64)
                .filter(|v| *v >= 1)
                .ok_or("startLine and lineCount must both be positive integers (startLine is 1-based)")? as usize;
            // Split into lines preserving line endings for accurate range extraction.
            let lines: Vec<&str> = text.split_inclusive('\n').collect();
            let total_lines = lines.len();
            if start_line > total_lines {
                return Ok(result(String::new(), json!({
                    "path": path, "truncated": false,
                    "startLine": start_line, "lineCount": line_count,
                    "totalLines": total_lines, "pageComplete": true,
                    "scanComplete": true,
                    "readMode": "lines",
                }), limits.output_chars));
            }
            let end_line = start_line.saturating_add(line_count).saturating_sub(1).min(total_lines);
            let selected: String = lines[start_line - 1..end_line].concat();
            let selected_utf16 = selected.encode_utf16().count();
            // RD-v1: pageComplete must reflect the actually returned range.
            // Any internal (read_chars) or output (result) truncation means the
            // requested page was not fully returned. Both cuts are
            // surrogate-pair safe (O-REVIEW-03 item 2).
            let budget = limits.read_chars.min(limits.output_chars);
            let bounded = bounded_nonempty_utf16(&selected, budget)?;
            let delivered_utf16 = bounded.encode_utf16().count();
            let truncated = delivered_utf16 < selected_utf16;
            let selected_start: usize = lines[..start_line - 1]
                .iter()
                .map(|line| line.encode_utf16().count())
                .sum();
            let delivered_lines = bounded.matches('\n').count();
            let next_start_line = start_line + delivered_lines;
            return Ok(result(bounded, json!({
                "path": path, "truncated": truncated,
                "startLine": start_line, "lineCount": line_count,
                "totalLines": total_lines, "pageComplete": end_line == total_lines && !truncated,
                "scanComplete": true,
                "readMode": "lines",
                "nextOffset": if truncated { json!(selected_start + delivered_utf16) } else { Value::Null },
                "nextStartLine": if truncated && bounded.ends_with('\n') { json!(next_start_line) } else { Value::Null },
                "returnedUnits": delivered_utf16, "totalUnits": text.encode_utf16().count(),
            }), limits.output_chars));
        }
        // Legacy UTF-16 code-unit mode: surrogate-pair safe, with nextOffset
        // derived from the ACTUALLY RETURNED text after the output budget
        // (O-REVIEW-03 item 2).
        let offset = input.get("offset").and_then(Value::as_f64).unwrap_or(0.0).max(0.0) as usize;
        let limit = input.get("limit").and_then(Value::as_f64).filter(|v| *v != 0.0).unwrap_or(limits.read_chars as f64).clamp(1.0, limits.read_chars as f64) as usize;
        let total_units = text.encode_utf16().count();
        let (page_text, safe_end) = slice_utf16_range(&text, offset, limit);
        let final_text = bounded_nonempty_utf16(&page_text, limits.output_chars)?;
        let final_units = final_text.encode_utf16().count();
        let page_units = page_text.encode_utf16().count();
        let output_cut = final_units < page_units;
        let source_exhausted = safe_end >= total_units;
        let start = offset.min(total_units);
        let fully_delivered = source_exhausted && !output_cut;
        let next_offset = if fully_delivered { Value::Null } else { json!(start + final_units) };
        return Ok(result(final_text, json!({
            "path": path, "truncated": !source_exhausted || output_cut,
            "readMode": "utf16",
            "offset": start,
            "nextOffset": next_offset,
            "totalUnits": total_units,
        }), limits.output_chars));
    }
    if tool == "ls" {
        let (entries, truncated) = gateway.entries(&path, limits.entries)?;
        return Ok(result(entries.iter().map(|e| format!("{} {}", if e.directory {"[dir]"} else {"[file]"}, e.name)).collect::<Vec<_>>().join("\n"), json!({"path":path,"count":entries.len(),"truncated":truncated}), limits.output_chars));
    }
    let raw_pattern = input.get("pattern").and_then(Value::as_str).unwrap_or_default();
    if input.get("regex").and_then(Value::as_bool).unwrap_or(false) {
        return Err("regex search is unavailable on the Rust resource route; use literal search or the runtime route".into());
    }
    let needle = raw_pattern.to_lowercase();
    let case_sensitive = input.get("caseSensitive").and_then(Value::as_bool).unwrap_or(false);
    let glob = input.get("glob").and_then(Value::as_str).unwrap_or_default();
    // RD-v1 cursor (B02): deterministic continuation bound to query + scope.
    // Scope fingerprint = resolved search path + tool + pattern + options, so
    // a cursor from another scope or another query is rejected rather than
    // silently answering a different question.
    let scope = format!(
        "{}|{}|{}|{}|{}|{}|{}|{}",
        tool, path.display(), raw_pattern,
        if case_sensitive { "cs" } else { "ci" },
        if glob.is_empty() { "-" } else { glob },
        limits.matches,
        binding.run_id,
        binding.permission_snapshot_id
    );
    let mut consumed: usize = 0;
    let mut resume: Option<(usize, usize)> = None;
    if let Some(raw_cursor) = input.get("cursor").and_then(Value::as_str) {
        let (cursor_scope, position) = decode_search_cursor(raw_cursor)?;
        if cursor_scope != scope {
            return Err("cursor scope mismatch: this cursor was issued for a different search scope or query".into());
        }
        match position {
            CursorPosition::Entry(value) => consumed = value,
            CursorPosition::Pair(file, line) => { consumed = file; resume = Some((file, line)); }
        }
    }
    let walk_start = match resume {
        Some((file, _)) => file.saturating_sub(1),
        None => consumed,
    };
    let (entries, scan_truncated) = gateway.walk(&path, walk_start)?;
    let mut total_hits: usize = 0;
    let mut unreadable: usize = 0;
    let mut last_consumed: usize = consumed;
    // Name/content matcher: literal (default) with per-call case handling.
    // Full regex evaluation stays a Node-runtime feature; the Rust path is
    // documented literal to avoid diverging semantics across engines.
    let name_hit = |value: &str| -> bool {
        if case_sensitive { value.contains(raw_pattern) } else { value.to_lowercase().contains(&needle) }
    };
    // Glob filter (* and ?), applied to file names in both find and grep.
    let glob_hit = |name: &str| -> bool {
        if glob.is_empty() { return true; }
        glob_match(&glob, name)
    };
    if tool == "find" {
        // O-REVIEW-03 item 3: the cursor follows the last DELIVERED match, so
        // matches scanned but not delivered (match cap or output budget) stay
        // reachable on the next page instead of being skipped.
        let mut delivered: Vec<usize> = Vec::new();
        let mut text = String::new();
        let mut output_cut = false;
        let mut last_scanned: usize = consumed;
        for entry in entries {
            gateway.check()?;
            if entry.index <= consumed { continue; }
            last_scanned = entry.index;
            if !glob_hit(&entry.name) { continue; }
            if !name_hit(&entry.name) { continue; }
            total_hits += 1;
            if delivered.len() >= limits.matches { continue; }
            let candidate = if text.is_empty() { entry.path.to_string_lossy().into_owned() }
                else { format!("{}\n{}", text, entry.path.display()) };
            if candidate.encode_utf16().count() > limits.output_chars {
                if delivered.is_empty() {
                    return Err("match line exceeds the output budget; narrow the query or raise limits".into());
                }
                output_cut = true;
                break;
            }
            text = candidate;
            delivered.push(entry.index);
        }
        let cap_reached = total_hits > delivered.len();
        let scan_complete = !scan_truncated;
        let next_position = if delivered.is_empty() {
            if scan_complete { None } else { Some(last_scanned) }
        } else {
            Some(*delivered.last().unwrap())
        };
        let next_cursor = if scan_complete && !cap_reached && !output_cut {
            Value::Null
        } else {
            match next_position.filter(|value| *value > 0) {
                Some(value) => json!(encode_search_cursor(&scope, value)),
                None => Value::Null,
            }
        };
        let scanned_from_start = consumed == 0;
        return Ok(result(text, json!({
            "count": delivered.len(), "returnedCount": delivered.len(),
            "pageComplete": !output_cut && !cap_reached,
            "scanComplete": scan_complete, "scanTruncated": scan_truncated,
            "matchLimitReached": cap_reached,
            "skippedUnreadable": unreadable,
            "totalMatches": if scanned_from_start && scan_complete && !cap_reached && !output_cut { json!(total_hits) } else { Value::Null },
            "nextCursor": next_cursor,
            "cursorConsistency": "live", "cursorVersion": 1, "cursorStalePossible": true,
        }), limits.output_chars));
    }
    // grep
    // O-REVIEW-03 item 3: grep's cursor is file+line shaped, so one file with
    // more hits than the cap continues mid-file instead of losing the rest.
    let (resume_file, resume_line) = match &resume {
        Some((file, line)) => (*file, *line),
        None => (0usize, 0usize),
    };
    let walk_skip = resume_file.saturating_sub(1);
    let page_last_index = entries.last().map(|entry| entry.index).unwrap_or(walk_skip);
    let mut text = String::new();
    let mut delivered: usize = 0;
    let mut output_cut = false;
    let mut cap_reached = false;
    let mut resume_out = (0usize, 0usize);
    let mut broke_mid_file = false;
    for entry in entries.into_iter().filter(|e| !e.directory) {
        if entry.index <= walk_skip { continue; }
        if !glob_hit(&entry.name) { continue; }
        let contents = match gateway.text(&entry.path, false) {
            Ok(text) => text,
            Err(_) => { unreadable += 1; continue; }
        };
        let start_line = if entry.index == resume_file { resume_line } else { 0 };
        let mut stop = false;
        for (line_index, line) in contents.split('\n').enumerate() {
            gateway.check()?;
            if line_index < start_line { continue; }
            let line = line.strip_suffix('\r').unwrap_or(line);
            if !name_hit(line) { continue; }
            total_hits += 1;
            if delivered >= limits.matches {
                cap_reached = true;
                broke_mid_file = true;
                resume_out = (entry.index, line_index);
                stop = true;
                break;
            }
            let (bounded_line, _) = slice_utf16_range(line, 0, limits.line_chars);
            let formatted = format!("{}:{}:{}", entry.path.display(), line_index + 1, bounded_line);
            let candidate = if text.is_empty() { formatted } else { format!("{}\n{}", text, formatted) };
            if candidate.encode_utf16().count() > limits.output_chars {
                if delivered == 0 {
                    return Err("match line exceeds the output budget; narrow the query or raise limits".into());
                }
                output_cut = true;
                broke_mid_file = true;
                resume_out = (entry.index, line_index);
                stop = true;
                break;
            }
            text = candidate;
            delivered += 1;
            resume_out = (entry.index, line_index + 1);
        }
        if stop { break; }
    }
    let cap_or_cut = cap_reached || output_cut;
    let scan_complete = !scan_truncated && !broke_mid_file && unreadable == 0;
    let next_cursor = if scan_complete && !cap_or_cut {
        Value::Null
    } else {
        let position = if delivered > 0 || cap_or_cut { resume_out } else { (page_last_index + 1, 0) };
        if position.0 == 0 && position.1 == 0 { Value::Null }
        else { json!(encode_search_cursor_pair(&scope, position.0, position.1)) }
    };
    let scanned_from_start = consumed == 0 && resume.is_none();
    Ok(result(text, json!({
        "count": delivered, "returnedCount": delivered,
        "pageComplete": !output_cut && !cap_reached && unreadable == 0,
        "scanComplete": scan_complete, "scanTruncated": !scan_complete,
        "matchLimitReached": cap_reached,
        "skippedUnreadable": unreadable,
        "totalMatches": if scanned_from_start && scan_complete && !cap_or_cut { json!(total_hits) } else { Value::Null },
        "nextCursor": next_cursor,
        "cursorConsistency": "live", "cursorVersion": 1, "cursorStalePossible": true,
    }), limits.output_chars))
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
        assert!(execute(&binding, "grep", &json!({"path":".","pattern":"f.*x","regex":true}), &token)
            .unwrap_err().contains("unavailable on the Rust resource route"));
        let tiny = execute(&binding, "read", &json!({"path":"src/note.txt","offset":1,"limit":1}), &token).unwrap();
        assert_eq!(tiny["content"][0]["text"], "😀");
        assert_eq!(tiny["details"]["nextOffset"], 3);
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
        assert!(grep["details"]["count"].as_u64().unwrap() < 50);
        assert_eq!(grep["details"]["pageComplete"], false);
        assert!(grep["details"]["nextCursor"].is_string());
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

    #[test]
    fn search_cursor_keeps_capped_results_reachable_and_globbed() {
        let (binding, _, token) = fixture();
        let root = Path::new(binding.permission.project_root.as_ref().unwrap());
        for index in 0..205 {
            fs::write(root.join(format!("hit-{index}.txt")), "hit\nhit\n").unwrap();
        }
        fs::write(root.join("hit-no.md"), "hit\n").unwrap();
        let mut cursor = Value::Null;
        let mut seen = std::collections::HashSet::new();
        for _ in 0..10 {
            let mut input = json!({"path":".","pattern":"hit","glob":"*.txt"});
            if !cursor.is_null() { input["cursor"] = cursor.clone(); }
            let page = execute(&binding, "find", &input, &token).unwrap();
            for line in page["content"][0]["text"].as_str().unwrap().lines() { seen.insert(line.to_owned()); }
            cursor = page["details"]["nextCursor"].clone();
            if cursor.is_null() { break; }
        }
        assert_eq!(seen.len(), 205);

        let grep = execute(&binding, "grep", &json!({"path":".","pattern":"hit","glob":"*.txt"}), &token).unwrap();
        assert!(!grep["content"][0]["text"].as_str().unwrap().contains("hit-no.md"));
    }

    #[test]
    fn grep_cursor_advances_directory_only_pages_and_tiny_budget_rejects() {
        let (binding, _, token) = fixture();
        let root = Path::new(binding.permission.project_root.as_ref().unwrap());
        fs::create_dir_all(root.join("a-dir")).unwrap();
        fs::write(root.join("a-dir/match.txt"), "needle").unwrap();
        let gateway = Gateway {
            root: root.canonicalize().unwrap(), cancellation: &token,
            deadline: Instant::now() + Duration::from_secs(5), remaining_bytes: MAX_SCAN_BYTES,
            limits: ReadLimits { entries: 1, matches: 2, read_chars: 100, output_chars: 100, line_chars: 100 },
        };
        let (first, first_more) = gateway.walk(root, 0).unwrap();
        assert!(first_more);
        assert!(first[0].directory);
        let first_index = first[0].index;
        let (second, _) = gateway.walk(root, first_index).unwrap();
        assert!(second[0].index > first_index);

        fs::write(root.join("emoji.txt"), "😀X").unwrap();
        let err = bounded_nonempty_utf16("😀X", 1).unwrap_err();
        assert!(err.contains("output budget is too small"));

        File::create(root.join("unreadable-large.txt")).unwrap().set_len(MAX_FILE_BYTES + 1).unwrap();
        let incomplete = execute(&binding, "grep", &json!({"path":".","pattern":"needle"}), &token).unwrap();
        assert!(incomplete["details"]["skippedUnreadable"].as_u64().unwrap() >= 1);
        assert_eq!(incomplete["details"]["scanComplete"], false);
        assert_eq!(incomplete["details"]["pageComplete"], false);
        assert!(incomplete["details"]["totalMatches"].is_null());
    }
}
