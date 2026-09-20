use serde::Serialize;
use serde_json::{json, Value};
use std::{
    fs,
    io::Read,
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Mutex,
    },
    thread,
    time::{Duration, Instant},
};

#[path = "command_environment.rs"]
mod command_environment;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;
/// Per-stream hard retention for command output. Everything up to this cap is
/// kept and handed to the Host result pipeline (which spills large results to
/// the blob store and pages them back); beyond it the stream keeps draining
/// so the child cannot deadlock, and the result reports the dropped bytes
/// explicitly with a remedy instead of silently losing them.
const MAX_COMMAND_OUTPUT_BYTES: usize = 16 * 1024 * 1024;
const MAX_COMMAND_SECONDS: u64 = 120;
/// How long an already-terminated child's output threads are waited for. The
/// pipes reach end-of-file once the process tree is gone, so this only has to
/// cover the OS teardown; without a bound a wedged grandchild could hold the
/// tool call open past its own budget and drop the diagnostics it produced.
const STREAM_DRAIN_GRACE: Duration = Duration::from_secs(2);

/// The execution outcomes CONTRACTS v1.2 §1 (EX-v1) distinguishes. The tag is part
/// of the Host-to-Runtime failure text contract (`[tool.<code>] …`), so a
/// failure never has to be re-guessed from prose by the Host mapping, the
/// persisted receipt or the model view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolErrorCode {
    /// The child ran and reported a non-zero exit code.
    NonZeroExit,
    /// The child could not be created at all (missing binary, bad cwd).
    StartFailed,
    /// The call is malformed: missing field, wrong type, empty required text,
    /// size overflow, unreadable or non-UTF-8 target.
    InvalidInput,
    /// The Host refused the call: frozen scope escape, read-only mode.
    PermissionDenied,
    /// The command ran past its own timeout with its output kept.
    TimedOut,
    /// Run or tool cancellation was requested.
    Cancelled,
    /// The operation failed for a reason the executor cannot classify, or the
    /// effect of a side-effecting operation is genuinely unknown.
    Unknown,
}

impl ToolErrorCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NonZeroExit => "tool.nonzero_exit",
            Self::StartFailed => "tool.start_failed",
            Self::InvalidInput => "tool.invalid_input",
            Self::PermissionDenied => "tool.permission_denied",
            Self::TimedOut => "tool.timed_out",
            Self::Cancelled => "tool.cancelled",
            Self::Unknown => "tool.unknown",
        }
    }

    /// Tag one failure message. Already-tagged messages are kept verbatim so a
    /// specific site can override the default classification of its helper.
    pub fn error(self, message: impl Into<String>) -> String {
        let message = message.into();
        if ToolErrorCode::classify(&message).is_some() {
            message
        } else {
            format!("[{}] {message}", self.as_str())
        }
    }

    /// The tag at the start of a failure message, if there is one.
    pub fn classify(message: &str) -> Option<Self> {
        let rest = message.strip_prefix('[')?;
        let (tag, _) = rest.split_once(']')?;
        Self::from_tag(tag.trim())
    }

    fn from_tag(tag: &str) -> Option<Self> {
        [
            (Self::NonZeroExit.as_str(), Self::NonZeroExit),
            (Self::StartFailed.as_str(), Self::StartFailed),
            (Self::InvalidInput.as_str(), Self::InvalidInput),
            (Self::PermissionDenied.as_str(), Self::PermissionDenied),
            (Self::TimedOut.as_str(), Self::TimedOut),
            (Self::Cancelled.as_str(), Self::Cancelled),
            (Self::Unknown.as_str(), Self::Unknown),
        ]
        .into_iter()
        .find(|(candidate, _)| *candidate == tag)
        .map(|(_, code)| code)
    }

    /// `true` when the text describes a completed business failure rather than
    /// a refusal to run. A non-zero exit produced a real process and real
    /// output; the other categories did not start or were not allowed to.
    pub const fn is_completed_failure(self) -> bool {
        matches!(self, Self::NonZeroExit | Self::TimedOut)
    }
}

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
    CommandJob {root:PathBuf,input:Value,preview:ToolPreview},
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
            Self::CommandJob { preview, .. }
            | Self::WriteFile { preview, .. }
            | Self::EditFile { preview, .. }
            | Self::RunCommand { preview, .. } => preview,
        }
    }

    /// The file a write/edit will replace, used for Host-side version
    /// capture. Commands have no single managed target.
    pub fn target_path(&self) -> Option<&Path> {
        match self {
            Self::WriteFile { path, .. } | Self::EditFile { path, .. } => Some(path),
            Self::RunCommand { .. } | Self::CommandJob { .. } => None,
        }
    }
}

pub fn prepare(
    tool: &str,
    input: &Value,
    project_root: &str,
) -> Result<PreparedToolAction, String> {
    let root = canonical_directory(Path::new(project_root), "project folder")
        .map_err(|error| ToolErrorCode::InvalidInput.error(error))?;
    match tool {
        "write_file" => prepare_write(input, &root),
        "edit_file" => prepare_edit(input, &root),
        "run_command" if input.get("action").is_some_and(|a|a!="sync") => crate::runtime_host::command_jobs::prepare(input,&root),
        "run_command" => prepare_command(input, &root),
        _ => Err(ToolErrorCode::InvalidInput.error(format!("unsupported host tool: {tool}"))),
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
        PreparedToolAction::CommandJob { .. } => Err(ToolErrorCode::PermissionDenied.error("Command jobs require the durable Host adapter")),
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
                    fs::create_dir_all(parent).map_err(|error| {
                        ToolErrorCode::Unknown.error(format!(
                            "failed to create parent directory: {error}"
                        ))
                    })?;
                }
            }
            check_cancellation(cancellation)?;
            fs::write(&path, content.as_bytes()).map_err(|error| {
                ToolErrorCode::Unknown.error(format!("failed to write file: {error}"))
            })?;
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
            fs::write(&path, content.as_bytes()).map_err(|error| {
                ToolErrorCode::Unknown.error(format!("failed to edit file: {error}"))
            })?;
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
    let content = content_field(input, "content")?;
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
    // Deleting a matched block is a legitimate edit, so the replacement may be
    // empty or whitespace only. It is stored exactly as given.
    let new_text = content_field(input, "newText")?;
    let replace_all = input
        .get("replaceAll")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let target = resolve_existing_target(root, &path, false)?;
    let current = read_text(&target)?;
    if old_text.is_empty() {
        return Err(ToolErrorCode::InvalidInput.error("oldText cannot be empty"));
    }
    let occurrences = current.matches(&old_text).count();
    if occurrences == 0 {
        return Err(ToolErrorCode::InvalidInput
            .error("oldText was not found in the target file"));
    }
    if occurrences > 1 && !replace_all {
        return Err(ToolErrorCode::InvalidInput.error(format!(
            "oldText occurs {occurrences} times; set replaceAll to true or provide a more specific match"
        )));
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
        return Err(ToolErrorCode::InvalidInput.error("command is too long"));
    }
    // An absent, null or blank `cwd` keeps its documented meaning: run in the
    // frozen project root. A present value of the wrong type is a malformed
    // call, not something to ignore.
    if let Some(cwd) = input.get("cwd") {
        if !cwd.is_null() && cwd.as_str().is_none() {
            return Err(ToolErrorCode::InvalidInput
                .error("tool input must contain a string cwd"));
        }
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
    cancellation.map_or(Ok(()), |token| {
        token
            .check()
            .map_err(|message| ToolErrorCode::Cancelled.error(message))
    })
}

/// How one command attempt ended. `Exited(None)` means the child produced no
/// exit code at all (signalled or otherwise unreported); that is deliberately
/// not the same as exit code 0. The payload type is `std::process::ExitStatus`'s
/// own `Option<i32>`; it is not widened to 64 bits anywhere in this path.
enum CommandOutcome {
    Exited(Option<i32>),
    TimedOut,
    Cancelled(String),
    /// `Child::try_wait` itself failed, so neither the exit code nor whether the
    /// child is still alive is knowable. The attempt ends as an error; output
    /// already produced is still collected, because skipping that drain would
    /// strand two reader slots and their threads.
    WaitFailed,
}

struct CommandContainment {
    #[cfg(windows)]
    job: Option<crate::process_jobs::WindowsJob>,
}

impl CommandContainment {
    fn attach(child: &std::process::Child) -> std::io::Result<Self> {
        #[cfg(windows)]
        {
            return crate::process_jobs::WindowsJob::assign_and_resume(child)
                .map(|job| Self { job: Some(job) });
        }
        #[cfg(not(windows))]
        {
            let _ = child;
            Ok(Self {})
        }
    }

    fn terminate(&mut self, child: &mut std::process::Child) {
        #[cfg(windows)]
        if let Some(job) = self.job.take() {
            let _ = job.terminate();
            drop(job);
        }
        terminate_process_tree(child);
    }

    fn close_after_parent_exit(&mut self) -> std::io::Result<()> {
        #[cfg(windows)]
        if let Some(job) = self.job.take() {
            let terminated = job.terminate();
            drop(job);
            terminated?;
        }
        Ok(())
    }
}

pub(crate) fn command_spawn_spec(
    command: &str,
    cwd: &Path,
) -> crate::process_jobs::SpawnSpec {
    #[cfg(windows)]
    let (program, args) = (
        "cmd.exe".to_owned(),
        vec![
            "/D".to_owned(),
            "/S".to_owned(),
            "/C".to_owned(),
            command.to_owned(),
        ],
    );
    #[cfg(not(windows))]
    let (program, args) = ("sh".to_owned(), vec!["-lc".to_owned(), command.to_owned()]);

    crate::process_jobs::SpawnSpec {
        program,
        args,
        cwd: cwd.to_string_lossy().into_owned(),
        env: crate::process_jobs::EnvMode::Inherit,
    }
}

fn execute_command(command: &str, cwd: &Path, timeout: Duration, cancellation: Option<&crate::kernel::CancellationToken>) -> Result<Value, String> {
    // The reader budget is process-wide, so the tests that measure it cannot run
    // against each other's reservations. Compiles out of the product build; in
    // tests it serializes command execution on a re-entrant guard (a test that
    // already holds it - the ceiling tests - simply nests).
    #[cfg(test)]
    let _budget = tests::lock_reader_budget();
    check_cancellation(cancellation)?;
    // Reap output readers left behind by earlier commands whose pipes only now
    // reached EOF, before this call adds any of its own.
    reap_finished_readers();
    let spawn_spec = command_spawn_spec(command, cwd);
    let mut process = Command::new(&spawn_spec.program);
    crate::process_jobs::apply_spawn_arguments(&mut process, &spawn_spec);
    #[cfg(windows)]
    {
        process.creation_flags(
            crate::process_jobs::CREATE_NO_WINDOW_FLAG
                | crate::process_jobs::CREATE_SUSPENDED_FLAG,
        );
    }
    match &spawn_spec.env {
        crate::process_jobs::EnvMode::Inherit => {
            command_environment::apply_inherited(&mut process)
        }
        crate::process_jobs::EnvMode::Replace(entries) => {
            command_environment::apply_explicit(&mut process, entries)
                .map_err(|error| ToolErrorCode::InvalidInput.error(error.to_string()))?;
        }
    }
    // A command costs two drain slots, and both of its pipes exist from here on.
    // Reserve them while there is still no child: after spawning, the only ways
    // out of a full ceiling are leaking a thread or hanging on a read that no
    // user-space code can cancel, so refusal has to happen at this seam.
    if !reserve_reader_slots(READERS_PER_COMMAND) {
        return Err(ToolErrorCode::StartFailed.error(format!(
            "Fox 的输出读取线程已达上限（{used}/{MAX_LIVE_OUTPUT_READERS}）：有前序命令的后代仍占着管道，其读取线程无法从用户态取消。**本次命令未启动，未产生任何副作用**。等待片刻后重试即可（新命令会先回收已结束的读取线程），或让命令把输出重定向到文件（command > out.txt 2>&1）后用读取工具分页查看",
            used = live_output_readers()
        )));
    }
    let mut child = match process
        .current_dir(&spawn_spec.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            // No reader was started, so the whole reservation goes back.
            for _ in 0..READERS_PER_COMMAND {
                release_reader_slot();
            }
            return Err(ToolErrorCode::StartFailed.error(format!(
                "failed to start command: {error}"
            )));
        }
    };
    let mut containment = match CommandContainment::attach(&child) {
        Ok(containment) => containment,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            for _ in 0..READERS_PER_COMMAND {
                release_reader_slot();
            }
            return Err(ToolErrorCode::StartFailed.error(format!(
                "failed to confine command before execution: {error}"
            )));
        }
    };
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    // Each call consumes exactly one of the two reserved slots, releasing it
    // again if that pipe turned out not to exist.
    let mut stdout = attach_output(stdout, MAX_COMMAND_OUTPUT_BYTES);
    let mut stderr = attach_output(stderr, MAX_COMMAND_OUTPUT_BYTES);
    let started = Instant::now();
    let mut wait_failure = None;
    let outcome = loop {
        if let Err(error) = check_cancellation(cancellation) {
            containment.terminate(&mut child);
            break CommandOutcome::Cancelled(error);
        }
        let status = match child.try_wait() {
            Ok(status) => status,
            Err(error) => {
                // An OS-level wait failure used to return straight out of the
                // loop, which skipped the drain and abandoned both reader slots
                // and threads. Record it, tear the tree down and fall through to
                // the common collection below instead.
                wait_failure = Some(ToolErrorCode::Unknown.error(format!(
                    "failed to wait for command: {error}"
                )));
                containment.terminate(&mut child);
                break CommandOutcome::WaitFailed;
            }
        };
        if let Some(status) = status {
            break CommandOutcome::Exited(status.code());
        }
        if started.elapsed() >= timeout {
            containment.terminate(&mut child);
            break CommandOutcome::TimedOut;
        }
        thread::sleep(Duration::from_millis(40));
    };
    if matches!(outcome, CommandOutcome::Exited(_)) {
        if let Err(error) = containment.close_after_parent_exit() {
            wait_failure = Some(ToolErrorCode::Unknown.error(format!(
                "failed to terminate command descendants after shell exit: {error}"
            )));
        }
    }
    // The tree is gone (or was never going to be killed), so both pipes normally
    // reach end of file; whatever the child printed before it stopped is what
    // the model needs in order to act. The snapshot is taken even when a
    // surviving descendant keeps a pipe open past the grace window, and that
    // case is reported instead of being presented as an empty result.
    let (stdout, stderr) = collect_streams(stdout.take(), stderr.take(), STREAM_DRAIN_GRACE);
    let elapsed_ms = started.elapsed().as_millis() as u64;
    let undrained = stdout.undrained || stderr.undrained;
    let mut text = if stderr.text.trim().is_empty() {
        stdout.text.clone()
    } else if stdout.text.trim().is_empty() {
        stderr.text.clone()
    } else {
        format!("{}\n\n[stderr]\n{}", stdout.text, stderr.text)
    };
    // Every byte the child produced is accounted for. Output within the
    // retention cap reaches the Host result pipeline intact (and from there
    // the blob store); only bytes beyond the per-stream cap were dropped,
    // and that fact is part of the result, with the way to get the rest.
    let dropped = stdout.dropped_bytes + stderr.dropped_bytes;
    if dropped > 0 {
        text.push_str(&format!(
            "\n\n[fox: 输出超过每路 {} 字节保留上限，丢弃 {dropped} 字节；可将输出重定向到文件后用读取工具分页查看]",
            MAX_COMMAND_OUTPUT_BYTES,
        ));
    }
    let (code, error_code, outcome_text) = match &outcome {
        CommandOutcome::Exited(Some(0)) => (Some(0), None, String::new()),
        CommandOutcome::Exited(code) => (
            *code,
            Some(ToolErrorCode::NonZeroExit),
            match code {
                Some(code) => format!("[fox: 命令以退出码 {code} 结束（非零退出，工具失败）]\n"),
                None => "[fox: 命令未报告退出码（进程被终止或异常结束，工具失败）]\n".to_owned(),
            },
        ),
        CommandOutcome::TimedOut => (
            None,
            Some(ToolErrorCode::TimedOut),
            format!(
                "[fox: 命令超过 {timeout:?} 超时上限被终止，以下是终止前已产生的输出]\n"
            ),
        ),
        CommandOutcome::Cancelled(_) => (
            None,
            Some(ToolErrorCode::Cancelled),
            "[fox: 命令被取消，以下是取消前已产生的输出]\n".to_owned(),
        ),
        CommandOutcome::WaitFailed => (
            None,
            Some(ToolErrorCode::Unknown),
            "[fox: 等待子进程结束的调用本身失败，退出码与该进程是否仍在运行不可确定，以下是已产生的输出]\n".to_owned(),
        ),
    };
    let mut text = if outcome_text.is_empty() {
        text
    } else {
        format!("{outcome_text}{text}").trim_end().to_owned()
    };
    if undrained {
        text.push_str("\n\n[fox: 子进程仍有输出流未被读完，上面的输出不完整；退出码与该流的剩余部分不可确定]");
    }
    let encoding = if stdout.encoding == stderr.encoding {
        stdout.encoding.to_owned()
    } else {
        format!("{}+{}", stdout.encoding, stderr.encoding)
    };
    let mut details = json!({
        "cwd": cwd,
        "exitCode": code,
        "timedOut": matches!(outcome, CommandOutcome::TimedOut),
        "cancelled": matches!(outcome, CommandOutcome::Cancelled(_)),
        "undrainedOutput": undrained,
        "stdout": stdout.text,
        "stderr": stderr.text,
        "stdoutBytes": stdout.total_bytes,
        "stderrBytes": stderr.total_bytes,
        "outputDroppedBytes": dropped,
        "outputRetentionBytes": MAX_COMMAND_OUTPUT_BYTES,
        "elapsedMs": elapsed_ms,
        "timeoutMs": timeout.as_millis() as u64,
        "shell": if cfg!(windows) { "cmd.exe /D /S /C" } else { "sh -lc" },
        "platform": if cfg!(windows) { "windows" } else { "posix" },
        "outputEncoding": encoding,
    });
    if let Some(error_code) = error_code {
        if let Some(object) = details.as_object_mut() {
            object.insert("errorCode".to_owned(), json!(error_code.as_str()));
        }
        if encoding.contains("lossy") || encoding.contains("undecodable") || encoding.contains("truncated") {
            if let Some(object) = details.as_object_mut() {
                object.insert(
                    "outputEncodingNote".to_owned(),
                    json!("子进程输出不是 UTF-8，已按控制台代码页尽力解码（见 outputEncoding）；需要逐字精确的文本时，让命令把输出重定向到文件（command > out.txt 2>&1）后用读取工具查看。"),
                );
            }
        }
    }
    if let Some(message) = wait_failure {
        // The attempt cannot be described as a business result: whether the
        // child ran to completion is unknowable. It fails the same way an
        // uncertain effect must, and the output Fox did manage to read travels
        // with the message rather than being thrown away with the reader.
        let retained = text.chars().take(4_000).collect::<String>();
        return Err(format!("{message}; 失败前已产生的输出：\n{retained}"));
    }
    if let CommandOutcome::Cancelled(error) = outcome {
        // Cancellation is not a business result: it must keep failing the Run
        // terminal path. The retained output still travels with it.
        let retained = text.chars().take(4_000).collect::<String>();
        return Err(ToolErrorCode::Cancelled.error(format!(
            "{error}; 取消前已产生的输出：\n{retained}"
        )));
    }
    let result = failure_text_result(text, details, error_code.is_some());
    Ok(result)
}

#[cfg(windows)]
pub(crate) fn terminate_process_tree(child: &mut std::process::Child) {
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
pub(crate) fn terminate_process_tree(child: &mut std::process::Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// What one output reader has absorbed so far, published under the lock after
/// every chunk. The waiting side can take a consistent snapshot even while the
/// reader is still blocked on a pipe that a surviving descendant holds open -
/// which is exactly the moment when the already-produced output must not be
/// thrown away together with the thread.
#[derive(Debug, Default)]
struct StreamProgress {
    kept: Vec<u8>,
    total_bytes: u64,
    finished: bool,
    /// Set once the owning command has taken its snapshot. A reader that is
    /// still blocked then keeps draining (the child must never deadlock on a
    /// full pipe) but stops retaining bytes: parking a wedged thread would
    /// otherwise also park up to `MAX_COMMAND_OUTPUT_BYTES` per stream for as
    /// long as the descendant lives.
    detached: bool,
}

/// One attached output reader: shared progress plus the thread feeding it. The
/// handle is always either joined or parked for a later join by
/// [`reap_finished_readers`]; it is never silently dropped.
struct OutputStream {
    progress: std::sync::Arc<Mutex<StreamProgress>>,
    reader: Option<thread::JoinHandle<()>>,
}

fn stream_finished(stream: &OutputStream) -> bool {
    stream
        .progress
        .lock()
        .map(|progress| progress.finished)
        .unwrap_or(true)
}

/// Readers that outlived their command's drain grace. A blocked `read` cannot
/// be interrupted from outside, so the thread is parked here and joined by a
/// later command once its pipe reached EOF. Parking (rather than forgetting) is
/// what makes a wedged reader visible and reclaimable; keeping the shared
/// progress next to the handle is what makes its retained bytes observable, so
/// the ceiling can be proven in bytes as well as in threads.
struct ParkedReader {
    handle: thread::JoinHandle<()>,
    progress: std::sync::Arc<Mutex<StreamProgress>>,
}

static ABANDONED_READERS: Mutex<Vec<ParkedReader>> = Mutex::new(Vec::new());

/// Ceiling on output reader threads alive at once, i.e. draining plus parked but
/// still blocked. Parking alone bounds nothing: a descendant that never closes
/// the write end means its reader never returns from `read`, so every such
/// command would permanently add threads (and their retained buffers) for the
/// rest of the process. A blocked read cannot be cancelled in user space, so the
/// bound is enforced where it still has a choice - before the child exists - and
/// exceeding it is a reported refusal rather than a silent leak or a hang.
pub(crate) const MAX_LIVE_OUTPUT_READERS: usize = 64;

/// Both of a command's streams are piped, so every command costs exactly this
/// many drain slots, whether or not it later produces output on them.
const READERS_PER_COMMAND: usize = 2;

/// Slots currently held by live readers. Reserved by
/// [`reserve_reader_slots`] and released when a reader is joined, so it counts
/// what is actually still running rather than what has been parked.
static LIVE_OUTPUT_READERS: AtomicUsize = AtomicUsize::new(0);

fn live_output_readers() -> usize {
    LIVE_OUTPUT_READERS.load(Ordering::Acquire)
}

/// Take `wanted` slots atomically, or none. Called before the child is spawned
/// so a refusal cannot leave a side effect behind.
fn reserve_reader_slots(wanted: usize) -> bool {
    let mut current = LIVE_OUTPUT_READERS.load(Ordering::Acquire);
    loop {
        if current.saturating_add(wanted) > MAX_LIVE_OUTPUT_READERS {
            return false;
        }
        match LIVE_OUTPUT_READERS.compare_exchange_weak(
            current,
            current + wanted,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => return true,
            Err(observed) => current = observed,
        }
    }
}

/// One reader is done being a thread: joined, or never started because the pipe
/// was absent. `attach_output` consumes exactly one reserved slot per call, so
/// the two callers below together account for both streams of a command.
fn release_reader_slot() {
    let previous = LIVE_OUTPUT_READERS.fetch_sub(1, Ordering::AcqRel);
    debug_assert!(
        previous > 0,
        "an output reader slot was released more often than it was reserved"
    );
}

/// How many readers are still owed a join. A test-only observation point: the
/// count is what proves a wedged reader was parked rather than forgotten, that a
/// later command reaped it, and that the ceiling actually holds.
#[cfg(test)]
pub(crate) fn abandoned_reader_count() -> usize {
    ABANDONED_READERS
        .lock()
        .map(|handles| handles.len())
        .unwrap_or_default()
}

#[cfg(test)]
pub(crate) fn live_reader_slots() -> usize {
    live_output_readers()
}

/// Bytes still retained by parked (still-blocked) readers. A test-only probe for
/// the other half of the ceiling: wedged threads must keep draining so their
/// child never deadlocks, but they must not keep holding output nobody will
/// read again.
#[cfg(test)]
pub(crate) fn parked_retained_bytes() -> usize {
    let Ok(readers) = ABANDONED_READERS.lock() else { return usize::MAX };
    readers
        .iter()
        .map(|parked| {
            parked
                .progress
                .lock()
                .map(|state| state.kept.len())
                .unwrap_or(usize::MAX)
        })
        .sum()
}

/// Join parked readers that have since finished, without ever blocking. Runs
/// before each command so reaping happens on a path that is actually used, and
/// every join gives its slot back.
pub(crate) fn reap_finished_readers() {
    let Ok(mut handles) = ABANDONED_READERS.lock() else { return };
    if handles.is_empty() {
        return;
    }
    // `JoinHandle::join` consumes the handle, so the parked set is rebuilt from
    // the still-blocked readers instead of retaining it in place.
    let pending = std::mem::take(&mut *handles);
    let mut still_running = Vec::with_capacity(pending.len());
    for parked in pending {
        if parked.handle.is_finished() {
            let _ = parked.handle.join();
            release_reader_slot();
        } else {
            still_running.push(parked);
        }
    }
    *handles = still_running;
}

fn retire_reader(
    handle: thread::JoinHandle<()>,
    progress: std::sync::Arc<Mutex<StreamProgress>>,
) {
    if handle.is_finished() {
        let _ = handle.join();
        release_reader_slot();
        return;
    }
    if let Ok(mut handles) = ABANDONED_READERS.lock() {
        handles.push(ParkedReader { handle, progress });
        return;
    }
    // A poisoned registry leaves the thread detached: it still belongs to the
    // process and exits as soon as the pipe closes, but nothing can join it any
    // more, so its slot stays taken. Under-reserving would let the real thread
    // count drift past [`MAX_LIVE_OUTPUT_READERS`] unnoticed.
}

/// Attach a draining reader to one child pipe, consuming one reserved slot. It
/// keeps draining past the retention cap so a verbose child can never deadlock
/// on a full pipe, while the accounting makes any dropped bytes explicit in the
/// result. `None` means the child had no such pipe; the reservation is handed
/// back in that case, so the caller never has to know which stream was absent.
fn attach_output<R>(reader: Option<R>, retain: usize) -> Option<OutputStream>
where
    R: Read + Send + 'static,
{
    let mut reader = match reader {
        Some(reader) => reader,
        None => {
            release_reader_slot();
            return None;
        }
    };
    let progress = std::sync::Arc::new(Mutex::new(StreamProgress::default()));
    let shared = std::sync::Arc::clone(&progress);
    let handle = thread::spawn(move || {
        let mut buffer = [0u8; 8192];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(count) => {
                    let Ok(mut state) = shared.lock() else { break };
                    state.total_bytes = state.total_bytes.saturating_add(count as u64);
                    if state.detached {
                        // Already reported: keep the pipe moving, hold nothing.
                        if !state.kept.is_empty() {
                            state.kept = Vec::new();
                        }
                        continue;
                    }
                    let keep = count.min(retain.saturating_sub(state.kept.len()));
                    state.kept.extend_from_slice(&buffer[..keep]);
                }
            }
        }
        if let Ok(mut state) = shared.lock() {
            state.finished = true;
        }
    });
    Some(OutputStream {
        progress,
        reader: Some(handle),
    })
}

/// Wait for both pipes against one shared deadline, then take both snapshots and
/// retire both readers. Collecting the streams together is what keeps the grace
/// period from being paid twice, and taking the snapshot before the join is what
/// preserves output from a stream that never reached EOF.
fn collect_streams(
    stdout: Option<OutputStream>,
    stderr: Option<OutputStream>,
    grace: Duration,
) -> (CapturedStream, CapturedStream) {
    let deadline = Instant::now() + grace;
    loop {
        let done = stdout.as_ref().map_or(true, stream_finished)
            && stderr.as_ref().map_or(true, stream_finished);
        if done || Instant::now() >= deadline {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    let past_deadline = Instant::now() >= deadline;
    (
        collect_stream(stdout, past_deadline),
        collect_stream(stderr, past_deadline),
    )
}

/// Take the snapshot first and only then retire the reader: a stream that never
/// reached EOF still contributes everything it had already produced.
fn collect_stream(stream: Option<OutputStream>, undrained_expected: bool) -> CapturedStream {
    let Some(OutputStream { progress, mut reader }) = stream else {
        return CapturedStream {
            encoding: "none".to_owned(),
            ..Default::default()
        };
    };
    let (kept, total_bytes, finished) = match progress.lock() {
        Ok(mut state) => {
            let (total_bytes, finished) = (state.total_bytes, state.finished);
            // Hand the bytes over instead of cloning them, and tell the reader
            // to stop retaining: a stream that never reached EOF is about to be
            // parked, and a parked thread must not also park its whole retention
            // cap for as long as the descendant holds the pipe open.
            let kept = std::mem::take(&mut state.kept);
            if !finished {
                state.detached = true;
            }
            (kept, total_bytes, finished)
        }
        Err(_) => (Vec::new(), 0, false),
    };
    if let Some(handle) = reader.take() {
        retire_reader(handle, std::sync::Arc::clone(&progress));
    }
    let undrained = !finished;
    debug_assert!(!undrained || undrained_expected);
    let (text, mut encoding) = decode_child_bytes(&kept);
    if undrained {
        encoding.push_str("+truncated");
    }
    CapturedStream {
        text,
        total_bytes,
        dropped_bytes: total_bytes.saturating_sub(kept.len() as u64),
        encoding,
        undrained,
    }
}

/// A decoded view of one output stream: everything the reader had absorbed when
/// the command stopped or the drain window closed.
#[derive(Debug, Default)]
struct CapturedStream {
    text: String,
    total_bytes: u64,
    dropped_bytes: u64,
    /// How `text` was obtained from the child's bytes, e.g. `utf-8`, `cp936` or
    /// `cp936+truncated`. `none` means there was no stream at all.
    encoding: String,
    /// `true` when the reader had not reached EOF, so the text is what was
    /// produced *so far* rather than the whole stream.
    undrained: bool,
}

/// Turn a child's raw bytes into text plus the name of what was actually
/// applied. UTF-8 output is taken verbatim; a stream cut mid-sequence keeps its
/// decodable prefix and says so; anything else is decoded with the codepage the
/// console is actually using instead of being silently replaced.
fn decode_child_bytes(bytes: &[u8]) -> (String, String) {
    match std::str::from_utf8(bytes) {
        Ok(text) => (text.to_owned(), "utf-8".to_owned()),
        Err(error) if error.error_len().is_none() => {
            let mut text = String::from_utf8_lossy(&bytes[..error.valid_up_to()]).into_owned();
            text.push('\u{FFFD}');
            (text, "utf-8+truncated".to_owned())
        }
        Err(_) => decode_console_codepage(bytes),
    }
}

/// The code page a child writes its redirected output with. A GUI host owns no
/// console, so the per-process console code page can be unset; the machine OEM
/// code page is what `cmd.exe` falls back to (CP936 on a Chinese Windows).
#[cfg(windows)]
fn console_output_codepage() -> u32 {
    use windows::Win32::Globalization::GetOEMCP;
    use windows::Win32::System::Console::GetConsoleOutputCP;
    let (console_codepage, oem_codepage) = unsafe { (GetConsoleOutputCP(), GetOEMCP()) };
    if console_codepage == 0 {
        oem_codepage
    } else {
        console_codepage
    }
}

#[cfg(windows)]
fn decode_console_codepage(bytes: &[u8]) -> (String, String) {
    use windows::Win32::Globalization::{MultiByteToWideChar, MULTI_BYTE_TO_WIDE_CHAR_FLAGS};
    let codepage = console_output_codepage();
    let lossy = || String::from_utf8_lossy(bytes).into_owned();
    if codepage == 0 || codepage == 65001 || bytes.is_empty() {
        return (lossy(), format!("cp{codepage}-lossy"));
    }
    // Two calls, as WinAPI requires: ask for the buffer size, then convert.
    // Flags are left at 0 so an undecodable byte produces a replacement rather
    // than failing the whole stream; the label below still reports which case hit.
    let flags = MULTI_BYTE_TO_WIDE_CHAR_FLAGS(0);
    let wide = unsafe {
        let count = MultiByteToWideChar(codepage, flags, bytes, None);
        if count <= 0 {
            return (lossy(), format!("cp{codepage}-undecodable"));
        }
        let mut buffer = vec![0u16; count as usize];
        let written = MultiByteToWideChar(codepage, flags, bytes, Some(&mut buffer));
        if written <= 0 {
            return (lossy(), format!("cp{codepage}-undecodable"));
        }
        buffer.truncate(written as usize);
        buffer
    };
    match String::from_utf16(&wide) {
        Ok(text) => (text, format!("cp{codepage}")),
        Err(_) => (lossy(), format!("cp{codepage}-undecodable")),
    }
}

#[cfg(not(windows))]
fn decode_console_codepage(bytes: &[u8]) -> (String, String) {
    (
        String::from_utf8_lossy(bytes).into_owned(),
        "non-utf8-lossy".to_owned(),
    )
}

fn required_string(input: &Value, key: &str) -> Result<String, String> {
    input
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            ToolErrorCode::InvalidInput
                .error(format!("tool input must contain a non-empty {key}"))
        })
}

/// One of the two file payloads (`content`, `newText`). Unlike a path or a
/// command, an empty or whitespace-only file body is a valid request: clearing
/// a file and deleting a matched block are both ordinary edits. The field must
/// still be present and be a string, and the size limit is enforced separately.
fn content_field(input: &Value, key: &str) -> Result<String, String> {
    match input.get(key) {
        None => Err(ToolErrorCode::InvalidInput.error(format!(
            "tool input must contain a string {key}"
        ))),
        Some(Value::String(value)) => Ok(value.clone()),
        Some(_) => Err(ToolErrorCode::InvalidInput.error(format!(
            "tool input must contain a string {key}"
        ))),
    }
}

fn enforce_content_size(content: &str) -> Result<(), String> {
    if content.len() as u64 > MAX_FILE_BYTES {
        Err(ToolErrorCode::InvalidInput.error(format!(
            "file content exceeds the {} byte limit",
            MAX_FILE_BYTES
        )))
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
        return Err(ToolErrorCode::InvalidInput.error("tool path contains an invalid null byte"));
    }
    if Path::new(value)
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(ToolErrorCode::PermissionDenied
            .error("tool path cannot contain parent traversal"));
    }
    Ok(())
}

fn ensure_inside(root: &Path, target: &Path) -> Result<(), String> {
    if target.starts_with(root) {
        Ok(())
    } else {
        Err(ToolErrorCode::PermissionDenied
            .error("tool path is outside the authorized project folder"))
    }
}

fn read_text(path: &Path) -> Result<String, String> {
    let metadata = fs::metadata(path)
        .map_err(|error| ToolErrorCode::InvalidInput.error(format!("failed to inspect file: {error}")))?;
    if metadata.len() > MAX_FILE_BYTES {
        return Err(ToolErrorCode::InvalidInput.error(format!(
            "file exceeds the {} byte limit",
            MAX_FILE_BYTES
        )));
    }
    fs::read_to_string(path).map_err(|error| {
        ToolErrorCode::InvalidInput.error(format!("file is not valid UTF-8 text: {error}"))
    })
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

/// One completed-but-unsuccessful execution. The diagnostics stay with the
/// result (that is what makes the failure actionable for the model), while the
/// `isError` flag is the single fact every consumer uses to keep the outcome
/// failed: the Host persistence mapping, the receipt, the managed-file version
/// registration and the Kernel settled state all read it. This is not a
/// success value carrying an error note — `finalize_host_tool_execution` turns
/// it into a `failed` ToolCall with an error message.
fn failure_text_result(text: String, details: Value, failed: bool) -> Value {
    if !failed {
        return text_result(text, details);
    }
    json!({
        "isError": true,
        "content": [{ "type": "text", "text": text }],
        "details": details,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};
    use uuid::Uuid;

    fn project() -> PathBuf {
        let root = std::env::temp_dir().join(format!("fox-tool-host-{}", Uuid::new_v4()));
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src").join("note.txt"), "hello\nworld\n").unwrap();
        root
    }

    /// The reader ceiling is one process-wide budget, so tests that consume it
    /// must not observe each other's bookkeeping or be starved by the test that
    /// deliberately fills it. Every budget-touching test takes this guard, at
    /// test level and inside the helpers; it is re-entrant per thread, so a
    /// guarded test can call a guarded helper without deadlocking, and the
    /// underlying lock is released when the outermost guard goes away.
    static READER_BUDGET_TEST_LOCK: Mutex<()> = Mutex::new(());

    thread_local! {
        static BUDGET_GUARD: RefCell<Option<std::sync::MutexGuard<'static, ()>>> =
            const { RefCell::new(None) };
        static BUDGET_DEPTH: Cell<usize> = const { Cell::new(0) };
    }

    pub(super) struct BudgetGuard;

    impl Drop for BudgetGuard {
        fn drop(&mut self) {
            let remaining = BUDGET_DEPTH.with(|depth| {
                let value = depth.get().saturating_sub(1);
                depth.set(value);
                value
            });
            if remaining == 0 {
                BUDGET_GUARD.with(|slot| *slot.borrow_mut() = None);
            }
        }
    }

    pub(super) fn lock_reader_budget() -> BudgetGuard {
        BUDGET_DEPTH.with(|depth| depth.set(depth.get() + 1));
        if !BUDGET_GUARD.with(|slot| slot.borrow().is_some()) {
            // Do not hold the RefCell borrow across a blocking lock.
            let guard = READER_BUDGET_TEST_LOCK
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            BUDGET_GUARD.with(|slot| *slot.borrow_mut() = Some(guard));
        }
        BudgetGuard
    }

    /// `attach_output` consumes one reserved reader slot per call - that is the
    /// invariant `execute_command` upholds. Direct callers have to reserve the
    /// same way, or the ceiling would be measured against a broken baseline.
    fn attach_reserved<R>(reader: Option<R>, retain: usize) -> Option<OutputStream>
    where
        R: Read + Send + 'static,
    {
        let _budget = lock_reader_budget();
        assert!(
            reserve_reader_slots(1),
            "no reader slot was free while the test held the budget lock"
        );
        attach_output(reader, retain)
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
    fn command_output_beyond_the_old_cap_is_fully_retained_and_accounted() {
        let root = project();
        // ~1 MiB of output used to be cut at 512 KiB (and could wedge the
        // pipe); it must now reach the result intact with exact accounting.
        let script = root.join("emit.js");
        fs::write(
            &script,
            "const s='abcdefgh'.repeat(2000); for(let i=0;i<70;i++) process.stdout.write(s+'\\n');",
        )
        .unwrap();
        let command = "node emit.js";
        let result = execute_command(command, &root, Duration::from_secs(60), None).unwrap();
        let stdout = result["details"]["stdout"].as_str().unwrap();
        let expected = ("abcdefgh".repeat(2000) + "\n").repeat(70).len();
        assert_eq!(result["details"]["stdoutBytes"].as_u64().unwrap() as usize, expected);
        assert_eq!(stdout.len(), expected, "every byte is retained");
        assert_eq!(result["details"]["outputDroppedBytes"], 0);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn attached_readers_drain_past_retention_and_report_dropped_bytes() {
        let data = vec![b'x'; MAX_COMMAND_OUTPUT_BYTES + 10_000];
        let attached = attach_reserved(Some(std::io::Cursor::new(data)), MAX_COMMAND_OUTPUT_BYTES);
        let (captured, _) = collect_streams(attached, None, Duration::from_secs(5));
        assert_eq!(captured.text.len(), MAX_COMMAND_OUTPUT_BYTES);
        assert_eq!(captured.total_bytes, (MAX_COMMAND_OUTPUT_BYTES + 10_000) as u64);
        assert_eq!(captured.dropped_bytes, 10_000);
        assert!(!captured.undrained, "a finished stream must not claim truncation");
        let (empty, _) = collect_streams(None, None, Duration::from_millis(1));
        assert_eq!(empty.total_bytes, 0);
        assert_eq!(empty.encoding, "none");
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

    // ---------------------------------------------------------------- A01/A02

    fn command_text(result: &Value) -> &str {
        result["content"][0]["text"].as_str().unwrap_or_default()
    }

    #[test]
    fn a_non_zero_exit_that_only_wrote_to_stdout_still_reports_that_output() {
        // The regression this guards: the failure branch used to build its
        // message from stderr only, so a compiler that prints its real reason
        // on stdout left the model with "exited with code 3: ".
        let root = project();
        #[cfg(windows)]
        let command = "echo failure-explained-on-stdout & exit 3";
        #[cfg(not(windows))]
        let command = "echo failure-explained-on-stdout; exit 3";
        let result = execute_command(command, &root, Duration::from_secs(30), None).unwrap();
        assert_eq!(result["isError"], Value::Bool(true));
        assert_eq!(result["details"]["exitCode"], 3);
        assert_eq!(result["details"]["errorCode"], "tool.nonzero_exit");
        assert!(
            command_text(&result).contains("failure-explained-on-stdout"),
            "retained stdout must reach the model: {result}"
        );
        assert_eq!(result["details"]["timedOut"], Value::Bool(false));
        assert_eq!(result["details"]["cancelled"], Value::Bool(false));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_non_zero_exit_keeps_both_streams_and_labels_them() {
        let root = project();
        #[cfg(windows)]
        let command = "echo built-stdout-marker & echo broken-stderr-marker 1>&2 & exit 9";
        #[cfg(not(windows))]
        let command = "echo built-stdout-marker; echo broken-stderr-marker 1>&2; exit 9";
        let result = execute_command(command, &root, Duration::from_secs(30), None).unwrap();
        let text = command_text(&result);
        assert!(text.contains("built-stdout-marker"), "{text}");
        assert!(text.contains("broken-stderr-marker"), "{text}");
        assert!(text.contains("[stderr]"), "streams must stay distinguishable: {text}");
        assert!(text.contains("退出码 9"), "the outcome line explains the classification: {text}");
        assert!(result["details"]["stdoutBytes"].as_u64().unwrap() >= 19);
        assert!(result["details"]["stderr"].as_str().unwrap().contains("broken-stderr-marker"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_timeout_reports_the_output_produced_before_the_tree_was_killed() {
        let root = project();
        #[cfg(windows)]
        let command = "echo partial-before-timeout & ping -n 30 127.0.0.1 > NUL";
        #[cfg(not(windows))]
        let command = "echo partial-before-timeout; sleep 30";
        let started = Instant::now();
        let result = execute_command(command, &root, Duration::from_secs(2), None).unwrap();
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "the timeout must actually bound the call"
        );
        assert_eq!(result["isError"], Value::Bool(true));
        assert_eq!(result["details"]["timedOut"], Value::Bool(true));
        assert_eq!(result["details"]["errorCode"], "tool.timed_out");
        assert!(
            result["details"].get("exitCode").unwrap().is_null(),
            "a killed child has no exit code; it must not be reported as 0"
        );
        assert!(command_text(&result).contains("partial-before-timeout"), "{result}");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn cancellation_keeps_failed_semantics_and_carries_the_partial_output() {
        use crate::kernel::CancellationPort;
        let root = project();
        let registry = crate::kernel::CancellationRegistry::default();
        registry.register_run("r").unwrap();
        let token = registry.tool_token("r", "cancel-me").unwrap();
        #[cfg(windows)]
        let command = "echo marker-before-cancel & echo started>started.txt & ping -n 30 127.0.0.1 > NUL";
        #[cfg(not(windows))]
        let command = "echo marker-before-cancel; echo started > started.txt; sleep 30";
        let command_root = root.clone();
        let worker = thread::spawn(move || {
            execute_command(command, &command_root, Duration::from_secs(60), Some(&token))
        });
        // Wait until the child really ran instead of sleeping a fixed time: the
        // reader budget is process-wide, so this call may legitimately queue
        // before the child exists, and cancelling something that never started
        // would throw away the very output under test.
        let deadline = Instant::now() + Duration::from_secs(10);
        while !root.join("started.txt").exists() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        let started = root.join("started.txt").exists();
        registry.request_tool_cancel("r", "cancel-me");
        let error = worker.join().unwrap().unwrap_err();
        assert!(started, "the test command must start before cancellation");
        // Cancellation is a Run-level outcome, not a business result: it still
        // travels through the error channel so the Run terminal state governs.
        assert!(error.contains("tool.cancelled"), "{error}");
        assert!(error.contains("marker-before-cancel"), "retained output must not be lost: {error}");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn oversized_output_is_counted_and_a_remedy_is_offered() {
        let parked_before = abandoned_reader_count();
        let data = vec![b'x'; 100_000];
        let stream = attach_reserved(Some(std::io::Cursor::new(data)), 1_024);
        let (captured, _) = collect_streams(stream, None, Duration::from_secs(5));
        assert_eq!(captured.total_bytes, 100_000);
        assert_eq!(captured.dropped_bytes, 100_000 - 1_024);
        assert_eq!(captured.text.len(), 1_024);
        assert!(!captured.undrained);
        assert_eq!(captured.encoding, "utf-8");
        // A drained reader is joined, not parked. Other tests may have parked
        // their own, so this compares against the count before this call.
        assert_eq!(abandoned_reader_count(), parked_before);
    }

    /// O-A-01 item 3: a descendant that keeps the write end open must not cost
    /// the already-produced output, and the reader thread must end up joined
    /// rather than abandoned. A real OS pipe makes this deterministic.
    #[test]
    fn a_pipe_that_never_closes_keeps_its_partial_output_and_is_reaped_later() {
        let _budget = lock_reader_budget();
        let baseline = abandoned_reader_count();
        let (receiver, sender) = std::io::pipe().expect("OS pipe");
        {
            use std::io::Write;
            let mut sender = sender.try_clone().expect("clone pipe writer");
            sender.write_all(b"first-part-of-the-log\n").unwrap();
        }
        let stream = attach_reserved(Some(receiver), MAX_COMMAND_OUTPUT_BYTES);
        let (captured, _) = collect_streams(stream, None, Duration::from_millis(150));
        assert!(
            captured.undrained,
            "the writer is still open, so the stream cannot be called complete"
        );
        assert_eq!(captured.encoding, "utf-8+truncated");
        assert!(
            captured.text.contains("first-part-of-the-log"),
            "output produced before the wedge must survive: {:?}",
            captured.text
        );
        assert!(
            abandoned_reader_count() >= baseline + 1,
            "the unfinished reader is parked for a later join: {} vs baseline {}",
            abandoned_reader_count(),
            baseline
        );
        let parked = abandoned_reader_count();
        drop(sender);
        // The next command's reaping pass joins it once the pipe reaches EOF.
        // Global counts are shared with other tests, so the assertion is that the
        // parked set shrank - this reader was joined - not an absolute number.
        for _ in 0..50 {
            reap_finished_readers();
            if abandoned_reader_count() < parked {
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
        assert!(
            abandoned_reader_count() < parked,
            "reader thread was not reaped after its pipe closed"
        );
    }

    /// O-A-01 follow-up: parking a wedged reader is not the same as bounding it.
    /// A descendant that never closes the write end never returns from `read`, so
    /// repeating such a command would add a thread every time. The bound has to
    /// be enforced where a choice still exists - before the child is created -
    /// and the refusal has to be visible instead of turning into a hang or a
    /// degraded read. Real OS pipes, no timing luck.
    #[test]
    fn wedged_readers_hit_a_ceiling_and_the_refusal_is_reported() {
        let _budget = lock_reader_budget();
        let parked_before = abandoned_reader_count();
        let mut senders = Vec::new();
        let mut held = 0usize;
        let deadline = Instant::now() + Duration::from_secs(20);
        while held < MAX_LIVE_OUTPUT_READERS {
            if Instant::now() >= deadline {
                break;
            }
            if !reserve_reader_slots(1) {
                // Something else is still holding a slot; it is released when its
                // reader is joined, so waiting is the only correct action.
                reap_finished_readers();
                thread::sleep(Duration::from_millis(5));
                continue;
            }
            let (receiver, sender) = std::io::pipe().expect("OS pipe");
            let stream = attach_output(Some(receiver), 4_096);
            // The snapshot detaches the reader and the still-open writer parks it.
            let (captured, _) = collect_streams(stream, None, Duration::from_millis(2));
            assert!(captured.undrained, "an open writer cannot read as drained");
            senders.push(sender);
            held += 1;
        }
        assert_eq!(
            held, MAX_LIVE_OUTPUT_READERS,
            "the ceiling was not reached within the deadline; live slots were {}",
            live_reader_slots()
        );
        // The invariant that matters: repetition stops adding readers.
        assert!(
            live_reader_slots() <= MAX_LIVE_OUTPUT_READERS,
            "the ceiling was exceeded: {}",
            live_reader_slots()
        );
        assert!(
            abandoned_reader_count() >= parked_before + held,
            "the wedged readers were parked, not forgotten"
        );
        assert_eq!(
            parked_retained_bytes(),
            0,
            "64 parked readers must not hold 64 retention caps"
        );

        // At the ceiling a new command is refused before a child exists: nothing
        // runs, no further thread is created, and the message says what happened
        // and what to do instead.
        let root = project();
        let marker = root.join("must-not-exist.txt");
        let error = execute_command(
            &format!("echo refused > {}", marker.display()),
            &root,
            Duration::from_secs(5),
            None,
        )
        .expect_err("a full reader ceiling has to refuse, not run degraded");
        assert!(error.starts_with("[tool.start_failed]"), "{error}");
        assert!(error.contains("未启动"), "{error}");
        assert!(error.contains("重定向到文件"), "{error}");
        assert!(!marker.exists(), "a refused command must not have run: {error}");

        // The pressure is transient: once the wedged pipes close, the parked
        // readers are joined, their slots come back and the same command runs.
        drop(senders);
        let mut recovered = false;
        for _ in 0..200 {
            reap_finished_readers();
            if execute_command("echo after-pressure", &root, Duration::from_secs(20), None).is_ok()
            {
                recovered = true;
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert!(
            recovered,
            "reader capacity never came back; live slots were {}",
            live_reader_slots()
        );
        let _ = fs::remove_dir_all(root);
    }

    /// The other half of the same bound: a parked reader must not also park its
    /// retained output. The producer keeps writing after the command gave up on
    /// the stream; a reader that kept retaining would climb back to its cap and
    /// turn a thread ceiling into a memory leak instead.
    #[test]
    fn a_parked_reader_keeps_draining_but_holds_no_bytes() {
        use std::io::Write;
        let _budget = lock_reader_budget();
        let (receiver, mut sender) = std::io::pipe().expect("OS pipe");
        sender.write_all(&[b'a'; 512]).unwrap();
        let stream = attach_reserved(Some(receiver), 1_024);
        let (captured, _) = collect_streams(stream, None, Duration::from_millis(200));
        assert!(captured.undrained);
        assert_eq!(
            captured.text.len(),
            512,
            "the snapshot must hand over what was produced before the wedge"
        );
        assert_eq!(
            parked_retained_bytes(),
            0,
            "the reported bytes must not stay parked"
        );
        for _ in 0..3 {
            // 2 KiB at a time, under the OS pipe capacity: the writes only
            // succeed while the parked reader keeps draining.
            sender.write_all(&[b'b'; 2_048]).unwrap();
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            parked_retained_bytes(),
            0,
            "a detached reader counts bytes, it does not accumulate them"
        );
        drop(sender);
        let parked = abandoned_reader_count();
        for _ in 0..100 {
            reap_finished_readers();
            if abandoned_reader_count() < parked {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert!(
            abandoned_reader_count() < parked,
            "the reader was not joined after its pipe closed"
        );
        assert!(
            live_reader_slots() <= MAX_LIVE_OUTPUT_READERS,
            "the ceiling held while this reader lived: {}",
            live_reader_slots()
        );
    }

    #[test]
    fn cancelling_a_command_that_outruns_its_grace_window_still_reaps_readers() {
        let baseline = abandoned_reader_count();
        let root = project();
        // The child itself is short-lived; the pipe holder is its descendant, so
        // the drain window can close while a reader is still blocked.
        #[cfg(windows)]
        let command = "echo before-detach & start /b cmd /c \"ping -n 6 127.0.0.1 > NUL\" & ping -n 6 127.0.0.1 > NUL";
        #[cfg(not(windows))]
        let command = "echo before-detach; (sleep 6 &) ; sleep 6";
        let result = execute_command(command, &root, Duration::from_millis(400), None).unwrap();
        assert_eq!(result["details"]["errorCode"], "tool.timed_out");
        assert!(
            result["content"][0]["text"].as_str().unwrap().contains("before-detach"),
            "{result}"
        );
        let _ = fs::remove_dir_all(root);
        // Whatever this call parked, a following call reaps it.
        execute_command("echo next", &std::env::temp_dir(), Duration::from_secs(20), None).ok();
        for _ in 0..50 {
            reap_finished_readers();
            if abandoned_reader_count() <= baseline {
                break;
            }
            thread::sleep(Duration::from_millis(40));
        }
        assert!(
            abandoned_reader_count() <= baseline,
            "readers from the timed-out call were not reaped: {}",
            abandoned_reader_count()
        );
    }

    #[test]
    fn error_codes_round_trip_and_never_double_tag() {
        for code in [
            ToolErrorCode::NonZeroExit,
            ToolErrorCode::StartFailed,
            ToolErrorCode::InvalidInput,
            ToolErrorCode::PermissionDenied,
            ToolErrorCode::TimedOut,
            ToolErrorCode::Cancelled,
            ToolErrorCode::Unknown,
        ] {
            let message = code.error("a reason");
            assert_eq!(message, format!("[{}] a reason", code.as_str()));
            assert_eq!(ToolErrorCode::classify(&message), Some(code));
            assert_eq!(
                ToolErrorCode::PermissionDenied.error(message.as_str()),
                message,
                "an existing classification must survive re-wrapping"
            );
        }
        assert_eq!(ToolErrorCode::classify("plain prose"), None);
        assert_eq!(ToolErrorCode::classify("[tool.made_up] prose"), None);
        assert!(ToolErrorCode::NonZeroExit.is_completed_failure());
        assert!(!ToolErrorCode::PermissionDenied.is_completed_failure());
    }

    // ------------------------------------------------------------------- A03

    #[test]
    fn an_empty_and_a_whitespace_only_file_body_are_written_exactly_as_given() {
        let root = project();
        let empty = prepare(
            "write_file",
            &json!({ "path": "cleared.txt", "content": "" }),
            root.to_str().unwrap(),
        )
        .unwrap();
        assert_eq!(execute(empty).unwrap()["details"]["bytes"], 0);
        assert_eq!(fs::read(root.join("cleared.txt")).unwrap().len(), 0);

        let blank = prepare(
            "write_file",
            &json!({ "path": "indent.txt", "content": "   \n\t  \n" }),
            root.to_str().unwrap(),
        )
        .unwrap();
        execute(blank).unwrap();
        assert_eq!(fs::read_to_string(root.join("indent.txt")).unwrap(), "   \n\t  \n");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn an_edit_with_an_empty_replacement_deletes_the_matched_text() {
        let root = project();
        let action = prepare(
            "edit_file",
            &json!({ "path": "src/note.txt", "oldText": "world\n", "newText": "" }),
            root.to_str().unwrap(),
        )
        .unwrap();
        execute(action).unwrap();
        assert_eq!(fs::read_to_string(root.join("src/note.txt")).unwrap(), "hello\n");

        let blanked = prepare(
            "edit_file",
            &json!({ "path": "src/note.txt", "oldText": "hello\n", "newText": "  " }),
            root.to_str().unwrap(),
        )
        .unwrap();
        execute(blanked).unwrap();
        assert_eq!(fs::read_to_string(root.join("src/note.txt")).unwrap(), "  ");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn unicode_and_crlf_bodies_survive_a_write_and_an_edit() {
        let root = project();
        let action = prepare(
            "write_file",
            &json!({ "path": "中文 名称.txt", "content": "第一行\r\n第二行 😀\r\n" }),
            root.to_str().unwrap(),
        )
        .unwrap();
        execute(action).unwrap();
        assert_eq!(
            fs::read_to_string(root.join("中文 名称.txt")).unwrap(),
            "第一行\r\n第二行 😀\r\n"
        );
        let edit = prepare(
            "edit_file",
            &json!({ "path": "中文 名称.txt", "oldText": "第二行 😀", "newText": "第二行" }),
            root.to_str().unwrap(),
        )
        .unwrap();
        execute(edit).unwrap();
        assert_eq!(
            fs::read_to_string(root.join("中文 名称.txt")).unwrap(),
            "第一行\r\n第二行\r\n"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn malformed_write_and_edit_calls_stay_rejected_with_a_classification() {
        let root = project();
        for (tool, input) in [
            ("write_file", json!({ "path": "a.txt" })),
            ("write_file", json!({ "path": "a.txt", "content": Value::Null })),
            ("write_file", json!({ "path": "a.txt", "content": 7 })),
            ("write_file", json!({ "content": "" })),
            ("write_file", json!({ "path": "  ", "content": "" })),
            ("write_file", json!({ "path": "../escape.txt", "content": "" })),
            ("edit_file", json!({ "path": "src/note.txt", "oldText": "hello" })),
            ("edit_file", json!({ "path": "src/note.txt", "oldText": "hello", "newText": 1 })),
            ("edit_file", json!({ "path": "src/note.txt", "oldText": "", "newText": "x" })),
            ("edit_file", json!({ "path": "src/note.txt", "oldText": "absent", "newText": "" })),
            ("run_command", json!({ "command": "" })),
            ("run_command", json!({ "command": "echo x", "cwd": 5 })),
        ] {
            let error = prepare(tool, &input, root.to_str().unwrap()).unwrap_err();
            let code = if input
                .get("path")
                .and_then(Value::as_str)
                .is_some_and(|value| value.contains(".."))
            {
                ToolErrorCode::PermissionDenied
            } else {
                ToolErrorCode::InvalidInput
            };
            assert_eq!(
                ToolErrorCode::classify(&error),
                Some(code),
                "{tool} {input} produced {error}"
            );
        }
        let huge = "x".repeat(MAX_FILE_BYTES as usize + 1);
        assert_eq!(
            ToolErrorCode::classify(
                &prepare("write_file", &json!({ "path": "big.txt", "content": huge }), root.to_str().unwrap())
                    .unwrap_err()
            ),
            Some(ToolErrorCode::InvalidInput)
        );
        let _ = fs::remove_dir_all(root);
    }

    // ------------------------------------------------------------------- A05

    #[test]
    fn the_result_names_the_shell_and_platform_that_actually_ran() {
        let root = project();
        let action = prepare(
            "run_command",
            &json!({ "command": "echo reported-shell", "timeoutSeconds": 10_000 }),
            root.to_str().unwrap(),
        )
        .unwrap();
        let result = execute(action).unwrap();
        let expected_shell = if cfg!(windows) { "cmd.exe /D /S /C" } else { "sh -lc" };
        let expected_platform = if cfg!(windows) { "windows" } else { "posix" };
        assert_eq!(result["details"]["shell"], expected_shell);
        assert_eq!(result["details"]["platform"], expected_platform);
        // The advertised clamp is the clamp that was applied.
        assert_eq!(result["details"]["timeoutMs"], 120_000);
        assert_eq!(result["details"]["outputEncoding"], "utf-8");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_command_runs_in_a_directory_with_chinese_and_spaces_and_propagates_its_code() {
        let root = std::env::temp_dir().join(format!("fox-项目 {}", Uuid::new_v4()));
        let nested = root.join("新建 文件夹");
        fs::create_dir_all(&nested).unwrap();
        let root_text = root.to_str().unwrap().to_owned();

        let action = prepare(
            "run_command",
            // The command writes into its own working directory, which is the
            // only way to prove the non-ASCII cwd actually reached the child.
            &json!({ "command": "echo ran-here > env-marker.txt" , "cwd": "新建 文件夹" }),
            &root_text,
        )
        .unwrap();
        assert_eq!(execute(action).unwrap()["details"]["exitCode"], 0);
        let marker = nested.join("env-marker.txt");
        assert!(marker.exists(), "the cwd must resolve through a non-ASCII path");
        assert!(fs::read_to_string(&marker).unwrap().contains("ran-here"));

        #[cfg(windows)]
        let failing = "if defined COMSPEC (echo env-seen) else (exit 5)";
        #[cfg(not(windows))]
        let failing = "test -n \"$HOME\" && echo env-seen || exit 5";
        let action = prepare("run_command", &json!({ "command": failing }), &root_text).unwrap();
        let result = execute(action).unwrap();
        // A success keeps the baseline result shape unchanged: no `isError` key
        // is invented for it, so existing consumers of the payload cannot
        // regress. Only a failed call gains `isError: true`.
        assert!(
            result.get("isError").is_none() || result["isError"] == Value::Bool(false),
            "success must not be reported as a failure: {result}"
        );
        assert!(command_text(&result).contains("env-seen"));
        assert_eq!(result["details"]["exitCode"], 0);
        assert!(result["details"].get("errorCode").is_none());

        let nonzero = "exit 7";
        let action = prepare("run_command", &json!({ "command": nonzero }), &root_text).unwrap();
        let result = execute(action).unwrap();
        assert_eq!(result["details"]["exitCode"], 7);
        assert_eq!(result["details"]["errorCode"], "tool.nonzero_exit");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn utf8_child_bytes_are_kept_and_other_bytes_use_the_console_codepage() {
        let chinese = "编译失败：3 errors".as_bytes().to_vec();
        let (text, encoding) = decode_child_bytes(&chinese);
        assert_eq!(encoding, "utf-8");
        assert_eq!(text, "编译失败：3 errors");

        // A stream cut in the middle of a multi-byte character keeps the part
        // that is real and marks the boundary, instead of re-decoding the tail
        // as some other codepage.
        let truncated = "失败".as_bytes()[..5].to_vec();
        let (_, encoding) = decode_child_bytes(&truncated);
        assert_eq!(encoding, "utf-8+truncated");

        // "编译失败" in GBK/CP936: valid console bytes, invalid UTF-8. On a
        // Chinese Windows the executor must give the original characters back;
        // elsewhere it says which code page it applied rather than hiding it.
        let gbk: Vec<u8> = vec![0xB1, 0xE0, 0xD2, 0xEB, 0xCA, 0xA7, 0xB0, 0xDC];
        let (text, encoding) = decode_child_bytes(&gbk);
        if cfg!(windows) {
            let codepage = console_output_codepage();
            if codepage == 65001 {
                // These deliberately invalid UTF-8 bytes cannot round-trip in
                // a UTF-8 console; the result must disclose the lossy fallback.
                assert_eq!(encoding, "cp65001-lossy");
                assert!(text.contains('\u{fffd}'));
                return;
            }
            assert!(
                encoding == format!("cp{codepage}")
                    || encoding == format!("cp{codepage}-undecodable"),
                "the label must name the code page that was applied, got {encoding}"
            );
            if codepage == 936 && encoding == "cp936" {
                assert_eq!(text, "编译失败", "CP936 bytes must round-trip");
            }
        } else {
            assert_eq!(encoding, "non-utf8-lossy");
        }
    }
}
