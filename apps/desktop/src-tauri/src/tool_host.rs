use serde::Serialize;
use serde_json::{json, Value};
use std::{
    fs,
    io::Read,
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;
const MAX_COMMAND_OUTPUT_BYTES: usize = 512 * 1024;
const MAX_COMMAND_SECONDS: u64 = 120;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolPreview {
    pub tool: String,
    pub title: String,
    pub target: String,
    pub summary: String,
    pub diff: Option<String>,
    pub command: Option<String>,
    pub cwd: Option<String>,
}

#[derive(Debug, Clone)]
pub enum PreparedToolAction {
    WriteFile {
        root: PathBuf,
        path: PathBuf,
        content: String,
        create_directories: bool,
        preview: ToolPreview,
    },
    EditFile {
        root: PathBuf,
        path: PathBuf,
        content: String,
        preview: ToolPreview,
    },
    RunCommand {
        root: PathBuf,
        command: String,
        cwd: PathBuf,
        timeout: Duration,
        preview: ToolPreview,
    },
}

impl PreparedToolAction {
    pub fn preview(&self) -> &ToolPreview {
        match self {
            Self::WriteFile { preview, .. }
            | Self::EditFile { preview, .. }
            | Self::RunCommand { preview, .. } => preview,
        }
    }
}

pub fn prepare(
    tool: &str,
    input: &Value,
    project_root: &str,
) -> Result<PreparedToolAction, String> {
    let root = canonical_directory(Path::new(project_root), "project folder")?;
    match tool {
        "write_file" => prepare_write(input, &root),
        "edit_file" => prepare_edit(input, &root),
        "run_command" => prepare_command(input, &root),
        _ => Err(format!("unsupported host tool: {tool}")),
    }
}

pub fn execute(action: PreparedToolAction) -> Result<Value, String> {
    execute_with_cancellation(action, None)
}

pub fn execute_with_cancellation(
    action: PreparedToolAction,
    cancellation: Option<&crate::kernel::CancellationToken>,
) -> Result<Value, String> {
    check_cancellation(cancellation)?;
    match action {
        PreparedToolAction::WriteFile {
            root,
            path,
            content,
            create_directories,
            ..
        } => {
            revalidate_target(&root, &path, None)?;
            let operation = if path.exists() { "modified" } else { "created" };
            if create_directories {
                if let Some(parent) = path.parent() {
                    check_cancellation(cancellation)?;
                    fs::create_dir_all(parent)
                        .map_err(|error| format!("failed to create parent directory: {error}"))?;
                }
            }
            check_cancellation(cancellation)?;
            fs::write(&path, content.as_bytes())
                .map_err(|error| format!("failed to write file: {error}"))?;
            Ok(text_result(
                format!("Wrote {} bytes to {}", content.len(), path.display()),
                json!({ "path": path, "bytes": content.len(), "operation": operation }),
            ))
        }
        PreparedToolAction::EditFile {
            root,
            path,
            content,
            ..
        } => {
            revalidate_target(&root, &path, Some(false))?;
            check_cancellation(cancellation)?;
            fs::write(&path, content.as_bytes())
                .map_err(|error| format!("failed to edit file: {error}"))?;
            Ok(text_result(
                format!("Updated {}", path.display()),
                json!({ "path": path, "bytes": content.len(), "operation": "modified" }),
            ))
        }
        PreparedToolAction::RunCommand {
            root,
            command,
            cwd,
            timeout,
            ..
        } => {
            revalidate_target(&root, &cwd, Some(true))?;
            execute_command(&command, &cwd, timeout, cancellation)
        }
    }
}

fn prepare_write(input: &Value, root: &Path) -> Result<PreparedToolAction, String> {
    let path = required_string(input, "path")?;
    let content = required_string(input, "content")?;
    let create_directories = input
        .get("createDirectories")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    enforce_content_size(&content)?;
    let target = resolve_write_target(root, &path)?;
    let old = read_optional_text(&target)?;
    let diff = line_diff(old.as_deref().unwrap_or_default(), &content, &target);
    let summary = if old.is_some() {
        format!("Replace the contents of {}", target.display())
    } else {
        format!("Create {}", target.display())
    };
    let preview = ToolPreview {
        tool: "write_file".to_owned(),
        title: if old.is_some() {
            "修改文件".to_owned()
        } else {
            "创建文件".to_owned()
        },
        target: target.to_string_lossy().into_owned(),
        summary,
        diff: Some(diff),
        command: None,
        cwd: None,
    };
    Ok(PreparedToolAction::WriteFile {
        root: root.to_path_buf(),
        path: target,
        content,
        create_directories,
        preview,
    })
}

fn prepare_edit(input: &Value, root: &Path) -> Result<PreparedToolAction, String> {
    let path = required_string(input, "path")?;
    let old_text = required_string(input, "oldText")?;
    let new_text = required_string(input, "newText")?;
    let replace_all = input
        .get("replaceAll")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let target = resolve_existing_target(root, &path, false)?;
    let current = read_text(&target)?;
    if old_text.is_empty() {
        return Err("oldText cannot be empty".to_owned());
    }
    let occurrences = current.matches(&old_text).count();
    if occurrences == 0 {
        return Err("oldText was not found in the target file".to_owned());
    }
    if occurrences > 1 && !replace_all {
        return Err(format!(
            "oldText occurs {occurrences} times; set replaceAll to true or provide a more specific match"
        ));
    }
    let content = if replace_all {
        current.replace(&old_text, &new_text)
    } else {
        current.replacen(&old_text, &new_text, 1)
    };
    enforce_content_size(&content)?;
    let preview = ToolPreview {
        tool: "edit_file".to_owned(),
        title: "编辑文件".to_owned(),
        target: target.to_string_lossy().into_owned(),
        summary: format!(
            "Replace {occurrences} matching section(s) in {}",
            target.display()
        ),
        diff: Some(line_diff(&current, &content, &target)),
        command: None,
        cwd: None,
    };
    Ok(PreparedToolAction::EditFile {
        root: root.to_path_buf(),
        path: target,
        content,
        preview,
    })
}

fn prepare_command(input: &Value, root: &Path) -> Result<PreparedToolAction, String> {
    let command = required_string(input, "command")?;
    if command.len() > 8_000 {
        return Err("command is too long".to_owned());
    }
    let cwd = input
        .get("cwd")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| resolve_existing_target(root, value, true))
        .transpose()?
        .unwrap_or_else(|| root.to_path_buf());
    let seconds = input
        .get("timeoutSeconds")
        .and_then(Value::as_u64)
        .unwrap_or(60)
        .clamp(1, MAX_COMMAND_SECONDS);
    let preview = ToolPreview {
        tool: "run_command".to_owned(),
        title: "运行命令".to_owned(),
        target: cwd.to_string_lossy().into_owned(),
        summary: format!("Run a command in {}", cwd.display()),
        diff: None,
        command: Some(command.clone()),
        cwd: Some(cwd.to_string_lossy().into_owned()),
    };
    Ok(PreparedToolAction::RunCommand {
        root: root.to_path_buf(),
        command,
        cwd,
        timeout: Duration::from_secs(seconds),
        preview,
    })
}

fn check_cancellation(cancellation: Option<&crate::kernel::CancellationToken>) -> Result<(), String> {
    cancellation.map_or(Ok(()), |token| token.check())
}

fn execute_command(command: &str, cwd: &Path, timeout: Duration, cancellation: Option<&crate::kernel::CancellationToken>) -> Result<Value, String> {
    check_cancellation(cancellation)?;
    #[cfg(windows)]
    let mut process = {
        let mut process = Command::new("cmd.exe");
        process.args(["/D", "/S", "/C", command]);
        process.creation_flags(0x0800_0000);
        process
    };
    #[cfg(not(windows))]
    let mut process = {
        let mut process = Command::new("sh");
        process.args(["-lc", command]);
        process
    };
    let mut child = process
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("failed to start command: {error}"))?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let stdout_reader = thread::spawn(move || read_limited(stdout, MAX_COMMAND_OUTPUT_BYTES));
    let stderr_reader = thread::spawn(move || read_limited(stderr, MAX_COMMAND_OUTPUT_BYTES));
    let started = Instant::now();
    let status = loop {
        if let Err(error) = check_cancellation(cancellation) {
            terminate_process_tree(&mut child);
            return Err(error);
        }
        if let Some(status) = child
            .try_wait()
            .map_err(|error| format!("failed to wait for command: {error}"))?
        {
            break status;
        }
        if started.elapsed() >= timeout {
            terminate_process_tree(&mut child);
            return Err(format!(
                "command timed out after {} seconds",
                timeout.as_secs()
            ));
        }
        thread::sleep(Duration::from_millis(40));
    };
    let stdout = stdout_reader.join().unwrap_or_default();
    let stderr = stderr_reader.join().unwrap_or_default();
    let code = status.code().unwrap_or(-1);
    let text = if stderr.trim().is_empty() {
        stdout.clone()
    } else if stdout.trim().is_empty() {
        stderr.clone()
    } else {
        format!("{stdout}\n\n[stderr]\n{stderr}")
    };
    let result = text_result(
        text,
        json!({ "cwd": cwd, "exitCode": code, "stdout": stdout, "stderr": stderr }),
    );
    if status.success() {
        Ok(result)
    } else {
        Err(format!("command exited with code {code}: {stderr}"))
    }
}

#[cfg(windows)]
fn terminate_process_tree(child: &mut std::process::Child) {
    // `Child::kill` only terminates cmd.exe. Kill its descendants as well so
    // a timed-out compiler, shell script, or package manager cannot survive
    // invisibly after Fox reports the tool failure.
    let pid = child.id().to_string();
    let _ = Command::new("taskkill")
        .args(["/PID", &pid, "/T", "/F"])
        .creation_flags(0x0800_0000)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(not(windows))]
fn terminate_process_tree(child: &mut std::process::Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn read_limited<R: Read>(reader: Option<R>, limit: usize) -> String {
    let Some(reader) = reader else {
        return String::new();
    };
    let mut buffer = Vec::new();
    let _ = reader.take(limit as u64).read_to_end(&mut buffer);
    String::from_utf8_lossy(&buffer).into_owned()
}

fn required_string(input: &Value, key: &str) -> Result<String, String> {
    input
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| format!("tool input must contain a non-empty {key}"))
}

fn enforce_content_size(content: &str) -> Result<(), String> {
    if content.len() as u64 > MAX_FILE_BYTES {
        Err(format!(
            "file content exceeds the {} byte limit",
            MAX_FILE_BYTES
        ))
    } else {
        Ok(())
    }
}

fn canonical_directory(path: &Path, label: &str) -> Result<PathBuf, String> {
    let canonical = path
        .canonicalize()
        .map_err(|error| format!("{label} cannot be resolved: {error}"))?;
    if !canonical.is_dir() {
        return Err(format!("{label} is not a directory"));
    }
    Ok(canonical)
}

fn resolve_existing_target(
    root: &Path,
    value: &str,
    require_directory: bool,
) -> Result<PathBuf, String> {
    validate_path_text(value)?;
    let requested = Path::new(value);
    let joined = if requested.is_absolute() {
        requested.to_path_buf()
    } else {
        root.join(requested)
    };
    let target = joined
        .canonicalize()
        .map_err(|error| format!("tool path cannot be resolved: {error}"))?;
    ensure_inside(root, &target)?;
    if require_directory && !target.is_dir() {
        return Err("tool path must be a directory".to_owned());
    }
    if !require_directory && !target.is_file() {
        return Err("tool path must be a file".to_owned());
    }
    Ok(target)
}

fn resolve_write_target(root: &Path, value: &str) -> Result<PathBuf, String> {
    validate_path_text(value)?;
    let requested = Path::new(value);
    let joined = if requested.is_absolute() {
        requested.to_path_buf()
    } else {
        root.join(requested)
    };
    if joined.exists() {
        let target = joined
            .canonicalize()
            .map_err(|error| format!("tool path cannot be resolved: {error}"))?;
        ensure_inside(root, &target)?;
        if target.is_dir() {
            return Err("write target cannot be a directory".to_owned());
        }
        return Ok(target);
    }

    let mut ancestor = joined.as_path();
    let mut missing = Vec::new();
    while !ancestor.exists() {
        let name = ancestor
            .file_name()
            .ok_or_else(|| "write target has no resolvable parent".to_owned())?;
        missing.push(name.to_os_string());
        ancestor = ancestor
            .parent()
            .ok_or_else(|| "write target has no resolvable parent".to_owned())?;
    }
    let canonical_ancestor = ancestor
        .canonicalize()
        .map_err(|error| format!("write parent cannot be resolved: {error}"))?;
    ensure_inside(root, &canonical_ancestor)?;
    let mut target = canonical_ancestor;
    for component in missing.into_iter().rev() {
        target.push(component);
    }
    ensure_inside(root, &target)?;
    Ok(target)
}

fn revalidate_target(
    root: &Path,
    target: &Path,
    require_directory: Option<bool>,
) -> Result<(), String> {
    let canonical_root = root
        .canonicalize()
        .map_err(|error| format!("project folder changed or is unavailable: {error}"))?;
    let canonical_target = if target.exists() {
        target
            .canonicalize()
            .map_err(|error| format!("tool target changed or is unavailable: {error}"))?
    } else {
        let mut ancestor = target;
        let mut missing = Vec::new();
        while !ancestor.exists() {
            let name = ancestor
                .file_name()
                .ok_or_else(|| "tool target has no resolvable parent".to_owned())?;
            missing.push(name.to_os_string());
            ancestor = ancestor
                .parent()
                .ok_or_else(|| "tool target has no resolvable parent".to_owned())?;
        }
        let mut canonical = ancestor
            .canonicalize()
            .map_err(|error| format!("tool target parent changed or is unavailable: {error}"))?;
        for component in missing.into_iter().rev() {
            canonical.push(component);
        }
        canonical
    };
    ensure_inside(&canonical_root, &canonical_target)?;
    if require_directory == Some(false) && !canonical_target.is_file() {
        return Err("tool target is no longer a file".to_owned());
    }
    if require_directory == Some(true) && !canonical_target.is_dir() {
        return Err("tool target is no longer a directory".to_owned());
    }
    if require_directory.is_none() && canonical_target.exists() && canonical_target.is_dir() {
        return Err("tool target cannot be a directory".to_owned());
    }
    Ok(())
}

fn validate_path_text(value: &str) -> Result<(), String> {
    if value.contains('\0') {
        return Err("tool path contains an invalid null byte".to_owned());
    }
    if Path::new(value)
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err("tool path cannot contain parent traversal".to_owned());
    }
    Ok(())
}

fn ensure_inside(root: &Path, target: &Path) -> Result<(), String> {
    if target.starts_with(root) {
        Ok(())
    } else {
        Err("tool path is outside the authorized project folder".to_owned())
    }
}

fn read_text(path: &Path) -> Result<String, String> {
    let metadata =
        fs::metadata(path).map_err(|error| format!("failed to inspect file: {error}"))?;
    if metadata.len() > MAX_FILE_BYTES {
        return Err(format!("file exceeds the {} byte limit", MAX_FILE_BYTES));
    }
    fs::read_to_string(path).map_err(|error| format!("file is not valid UTF-8 text: {error}"))
}

fn read_optional_text(path: &Path) -> Result<Option<String>, String> {
    if path.exists() {
        read_text(path).map(Some)
    } else {
        Ok(None)
    }
}

fn line_diff(old: &str, new: &str, path: &Path) -> String {
    if old == new {
        return "No textual changes.".to_owned();
    }
    let old_lines = old.lines().collect::<Vec<_>>();
    let new_lines = new.lines().collect::<Vec<_>>();
    let mut prefix = 0;
    while prefix < old_lines.len()
        && prefix < new_lines.len()
        && old_lines[prefix] == new_lines[prefix]
    {
        prefix += 1;
    }
    let mut old_suffix = old_lines.len();
    let mut new_suffix = new_lines.len();
    while old_suffix > prefix
        && new_suffix > prefix
        && old_lines[old_suffix - 1] == new_lines[new_suffix - 1]
    {
        old_suffix -= 1;
        new_suffix -= 1;
    }
    let mut output = format!("--- {}\n+++ {}\n", path.display(), path.display());
    for line in old_lines[prefix..old_suffix].iter().take(240) {
        output.push_str(&format!("-{line}\n"));
    }
    for line in new_lines[prefix..new_suffix].iter().take(240) {
        output.push_str(&format!("+{line}\n"));
    }
    if old_suffix - prefix > 240 || new_suffix - prefix > 240 {
        output.push_str("... diff truncated ...\n");
    }
    output
}

fn text_result(text: String, details: Value) -> Value {
    json!({
        "content": [{ "type": "text", "text": text }],
        "details": details,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn project() -> PathBuf {
        let root = std::env::temp_dir().join(format!("fox-tool-host-{}", Uuid::new_v4()));
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src").join("note.txt"), "hello\nworld\n").unwrap();
        root
    }

    #[test]
    fn cancelled_tool_does_not_write_or_create_parent_directories() {
        use crate::kernel::CancellationPort;
        let root = project();
        let registry = crate::kernel::CancellationRegistry::default();
        registry.register_run("r").unwrap();
        let token = registry.tool_token("r", "write").unwrap();
        let action = prepare("write_file", &json!({"path":"cancelled/note.txt","content":"must not be written"}), root.to_str().unwrap()).unwrap();
        registry.request_tool_cancel("r", "write");
        assert!(execute_with_cancellation(action, Some(&token)).unwrap_err().contains("tool.cancelled"));
        assert!(!root.join("cancelled").exists());
    }

    #[test]
    fn cancellation_stops_an_in_flight_command() {
        use crate::kernel::CancellationPort;
        let root = project();
        let registry = crate::kernel::CancellationRegistry::default();
        registry.register_run("r").unwrap();
        let token = registry.tool_token("r", "command").unwrap();
        let command_root = root.clone();
        #[cfg(windows)]
        let command = "echo ready>started.txt & ping -n 30 127.0.0.1 > NUL";
        #[cfg(not(windows))]
        let command = "printf ready > started.txt; exec sleep 30";
        let worker = thread::spawn(move || execute_command(command, &command_root, Duration::from_secs(60), Some(&token)));
        let deadline = Instant::now() + Duration::from_secs(5);
        while !root.join("started.txt").exists() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        let started = root.join("started.txt").exists();
        let cancelled_at = Instant::now();
        registry.request_tool_cancel("r", "command");
        let result = worker.join().unwrap();
        assert!(started, "test command must actually start before cancellation");
        assert!(result.unwrap_err().contains("tool.cancelled"));
        assert!(cancelled_at.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn prepares_and_executes_file_edits_inside_the_project() {
        let root = project();
        let action = prepare(
            "edit_file",
            &json!({ "path": "src/note.txt", "oldText": "world", "newText": "Fox" }),
            root.to_str().unwrap(),
        )
        .unwrap();
        assert!(action.preview().diff.as_deref().unwrap().contains("+Fox"));
        let result = execute(action).unwrap();
        assert_eq!(result["details"]["operation"], "modified");
        assert_eq!(
            fs::read_to_string(root.join("src/note.txt")).unwrap(),
            "hello\nFox\n"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn prepares_new_files_but_blocks_parent_traversal() {
        let root = project();
        let action = prepare(
            "write_file",
            &json!({ "path": "generated/report.md", "content": "# Report" }),
            root.to_str().unwrap(),
        )
        .unwrap();
        let result = execute(action).unwrap();
        assert_eq!(result["details"]["operation"], "created");
        assert_eq!(
            fs::read_to_string(root.join("generated/report.md")).unwrap(),
            "# Report"
        );
        assert!(prepare(
            "write_file",
            &json!({ "path": "../outside.txt", "content": "blocked" }),
            root.to_str().unwrap(),
        )
        .is_err());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn commands_must_use_a_working_directory_inside_the_project() {
        let root = project();
        let outside = root.parent().unwrap().to_path_buf();
        assert!(prepare(
            "run_command",
            &json!({ "command": "echo hello", "cwd": outside }),
            root.to_str().unwrap(),
        )
        .is_err());
        let action = prepare(
            "run_command",
            &json!({ "command": "echo hello", "cwd": ".", "timeoutSeconds": 5 }),
            root.to_str().unwrap(),
        )
        .unwrap();
        let result = execute(action).unwrap();
        assert!(result["content"][0]["text"]
            .as_str()
            .unwrap()
            .to_lowercase()
            .contains("hello"));
        let _ = fs::remove_dir_all(root);
    }
}
