//! Host-side ACP v1 client (plan 6.7) on the official `agent-client-protocol`
//! crate. One process per bound session (6.1): spawned with an allowlisted
//! environment, in its own process group on Unix, and killed as a tree on
//! close. On Windows the child is created suspended, placed in a Job Object of
//! its own (`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`, no breakaway) and only then
//! resumed, so it and every descendant that stays in the job die with the job
//! (`win_job`). In sandboxed mode the backend itself is NOT a descendant in
//! that sense: srt-win starts it through the Secondary Logon service as the
//! `srt-sandbox` account (two-hop launch), outside this job; its cleanup rests
//! on srt-win's own job chain (docs/sandbox.md section 7.4).
//!
//! What this module guarantees, matching the port rules:
//! * `session/cancel` answers every pending permission request of that session
//!   with `cancelled` (ACP spec requirement; plan 6.4 rule 1).
//! * Permission answers are reported as written, never as acknowledged: ACP v1
//!   has no acknowledgement for a permission response (plan 6.3 rule 3).
//! * Capabilities are read from `initialize` and default to "unsupported".
//! * Once a stop has begun ([`AcpConnection::begin_stopping`]) the connection
//!   refuses new work and answers every permission request that still arrives
//!   with `cancelled` itself; such a request never reaches the user as
//!   answerable ([`BackendEvent::PermissionAutoCancelled`] is observation only).
//! * A stop reports whether the whole process tree is confirmed gone
//!   ([`crate::port::StopConfirmation`], see [`crate::proctree`]).

use crate::launch::{child_environment, LaunchSpec};
use crate::port::{CapabilityDeclaration, SessionCapabilities};
use crate::reverse::WriteOutcome;
use agent_client_protocol::schema::v1::{
    CancelNotification, ContentBlock, InitializeRequest, NewSessionRequest, PromptRequest,
    RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
    SelectedPermissionOutcome, SessionNotification, TextContent,
};
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::{Agent, ByteStreams, Client, ConnectionTo, Responder};
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, oneshot};
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

/// Something the backend told the Host.
#[derive(Debug, Clone)]
pub enum BackendEvent {
    Initialized {
        capabilities: CapabilityDeclaration,
        initialize_response: Value,
    },
    /// A `session/update` notification, kept as JSON for the Host mapper.
    Update { session_id: String, update: Value },
    /// The agent asks for permission. Answer with [`AcpConnection::answer_permission`].
    PermissionRequested {
        native_request_id: String,
        session_id: String,
        option_ids: Vec<String>,
        request: Value,
    },
    /// A permission request that arrived after the connection began stopping
    /// (revocation). The Host already answered it `cancelled`; it is NOT
    /// answerable and must not be shown as a pending approval. Observation
    /// only.
    PermissionAutoCancelled {
        session_id: String,
        reason: String,
        request: Value,
    },
    TurnEnded { session_id: String, stop_reason: Value },
    Failed(String),
    Closed,
}

enum Command {
    NewSession {
        cwd: PathBuf,
        reply: oneshot::Sender<Result<String, String>>,
    },
    Prompt {
        session_id: String,
        text: String,
    },
    Cancel {
        session_id: String,
    },
}

/// Pending permission requests plus the stopping flag, under ONE lock: the
/// request handler checks the flag and registers the request atomically, and
/// [`AcpConnection::begin_stopping`] sets the flag and drains the map
/// atomically, so a request can never slip in between.
#[derive(Default)]
struct PermissionRegistry {
    stopping: bool,
    map: HashMap<String, (String, Responder<RequestPermissionResponse>)>,
}

type PendingPermissions = Arc<Mutex<PermissionRegistry>>;

/// Error text for work refused while the connection is stopping.
pub const STOPPING_REFUSAL: &str = "backend connection is stopping (full access revoked); no new tasks are accepted";

pub struct AcpConnection {
    commands: mpsc::UnboundedSender<Command>,
    pub events: mpsc::UnboundedReceiver<BackendEvent>,
    pending: PendingPermissions,
    child_id: Option<u32>,
    child: Arc<tokio::sync::Mutex<tokio::process::Child>>,
    /// Windows: the Job Object holding the backend tree (`None` elsewhere).
    job: ProcessJob,
    stderr_tail: Arc<Mutex<std::collections::VecDeque<String>>>,
    /// Per-session srt state (settings file), removed when the connection
    /// goes away. `None` in full-access mode.
    sandbox: Option<crate::sandbox::SandboxSession>,
    permission_mode: crate::sandbox::PermissionMode,
}

/// Map an ACP `initialize` response onto the port's capability declaration.
/// Anything the agent does not declare stays unsupported.
pub fn capabilities_from_initialize(response: &Value) -> CapabilityDeclaration {
    let caps = response.get("agentCapabilities").cloned().unwrap_or(Value::Null);
    let session_caps = caps.get("sessionCapabilities").cloned().unwrap_or(Value::Null);
    let present = |value: &Value, key: &str| match value.get(key) {
        Some(Value::Bool(flag)) => *flag,
        Some(Value::Null) | None => false,
        Some(_) => true,
    };
    let mut declared = CapabilityDeclaration::unverified();
    declared.session = SessionCapabilities {
        load: present(&caps, "loadSession"),
        resume: present(&session_caps, "resume"),
        close: present(&session_caps, "close"),
        list: present(&session_caps, "list"),
        steer: false,
    };
    declared.approval_answer_acknowledged = false;
    declared
}

/// How long `initialize` may take. Generous: a cold `npx` install or a slow
/// filesystem (dsh measured ~76 s from a network-backed workspace) is normal.
pub const INITIALIZE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(180);
/// Grace between SIGTERM and SIGKILL on close. pi cleans up its own bash
/// children (which run in their own process groups) on SIGTERM; a bare
/// SIGKILL of Fox's group orphaned them in the pi branch measurement.
pub const TERMINATE_GRACE: std::time::Duration = std::time::Duration::from_secs(3);
const STDERR_TAIL_BYTES: usize = 8 * 1024;


/// Keep the last bytes of the backend's stderr for diagnostics, with
/// secret-shaped tokens masked. Failure reasons (auth, missing binary) often
/// appear only there.
fn redact_line(line: &str) -> String {
    line.split_whitespace()
        .map(|word| {
            let upper = word.to_ascii_uppercase();
            let secretish = word.starts_with("sk-")
                || ["KEY=", "TOKEN=", "SECRET=", "PASSWORD="].iter().any(|marker| upper.contains(marker))
                || (word.len() >= 32 && word.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
            if secretish { "[redacted]".to_owned() } else { word.to_owned() }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Windows Job Object owning the backend process tree.
///
/// Race: the child is spawned with `CREATE_SUSPENDED`, assigned, and only then
/// resumed (every thread of the new process, found with a Toolhelp snapshot;
/// a fresh suspended process has exactly one). It cannot run, and so cannot
/// spawn anything, before it is in the job. The job handle is not inheritable
/// and the limit has no `BREAKAWAY_OK`, so a descendant can leave only through
/// a different creation path: a process started by a service on its behalf
/// (Secondary Logon / `CreateProcessWithLogonW`, Task Scheduler, COM servers,
/// WMI) is created outside the job. srt-win's runner is exactly that case.
#[cfg(windows)]
pub mod win_job {
    use std::os::windows::io::RawHandle;
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, IsProcessInJob, QueryInformationJobObject, SetInformationJobObject,
        TerminateJobObject, JobObjectBasicAccountingInformation, JobObjectExtendedLimitInformation,
        JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    use windows_sys::Win32::System::Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME};

    /// `CREATE_SUSPENDED` for `Command::creation_flags`.
    pub const CREATE_SUSPENDED: u32 = windows_sys::Win32::System::Threading::CREATE_SUSPENDED;

    pub struct Job(HANDLE);

    // The handle is a kernel object handle owned only by this value.
    unsafe impl Send for Job {}
    unsafe impl Sync for Job {}

    fn last_error() -> u32 {
        unsafe { GetLastError() }
    }

    impl Job {
        /// Unnamed job, kill-on-close, no breakaway allowed.
        pub fn new() -> Result<Self, String> {
            let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
            if handle.is_null() {
                return Err(format!("CreateJobObjectW failed (Win32 error {})", last_error()));
            }
            let job = Self(handle);
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let ok = unsafe {
                SetInformationJobObject(
                    job.0,
                    JobObjectExtendedLimitInformation,
                    &limits as *const _ as *const core::ffi::c_void,
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                )
            };
            if ok == 0 {
                return Err(format!("SetInformationJobObject(KILL_ON_JOB_CLOSE) failed (Win32 error {})", last_error()));
            }
            Ok(job)
        }

        pub fn assign(&self, process: RawHandle) -> Result<(), String> {
            if unsafe { AssignProcessToJobObject(self.0, process as HANDLE) } == 0 {
                return Err(format!("AssignProcessToJobObject failed (Win32 error {})", last_error()));
            }
            Ok(())
        }

        /// Whether `process` is in this job (diagnostics and tests).
        pub fn contains(&self, process: RawHandle) -> Option<bool> {
            let mut result = 0;
            (unsafe { IsProcessInJob(process as HANDLE, self.0, &mut result) } != 0).then_some(result != 0)
        }

        /// Number of processes currently in the job
        /// (`JobObjectBasicAccountingInformation.ActiveProcesses`). `Err`
        /// carries the Win32 error.
        pub fn active_processes(&self) -> Result<u32, u32> {
            let mut info: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = unsafe { std::mem::zeroed() };
            let ok = unsafe {
                QueryInformationJobObject(
                    self.0,
                    JobObjectBasicAccountingInformation,
                    &mut info as *mut _ as *mut core::ffi::c_void,
                    std::mem::size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
                    std::ptr::null_mut(),
                )
            };
            if ok == 0 {
                return Err(last_error());
            }
            Ok(info.ActiveProcesses)
        }

        /// Poll [`Self::active_processes`] until it is zero or `timeout`
        /// passes. Only an observed zero is `Confirmed`.
        pub fn confirm_empty(&self, timeout: std::time::Duration) -> crate::port::StopConfirmation {
            let deadline = std::time::Instant::now() + timeout;
            loop {
                let reason = match self.active_processes() {
                    Ok(0) => return crate::port::StopConfirmation::Confirmed,
                    Ok(count) => format!("Job Object still holds {count} active process(es)"),
                    Err(code) => format!("QueryInformationJobObject failed (Win32 error {code})"),
                };
                if std::time::Instant::now() >= deadline {
                    return crate::port::StopConfirmation::FailedOrUnconfirmed { reasons: vec![reason] };
                }
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
        }

        /// Kill every process in the job now. `Err` carries the Win32 error.
        pub fn terminate(&self, exit_code: u32) -> Result<(), u32> {
            if unsafe { TerminateJobObject(self.0, exit_code) } == 0 {
                return Err(last_error());
            }
            Ok(())
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            // KILL_ON_JOB_CLOSE: closing the last handle kills what is left.
            unsafe { CloseHandle(self.0) };
        }
    }

    /// Resume every thread of a process created with `CREATE_SUSPENDED`.
    /// Returns how many threads were resumed (must be at least one).
    pub fn resume_process(pid: u32) -> Result<usize, String> {
        let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
        if snapshot == INVALID_HANDLE_VALUE || snapshot.is_null() {
            return Err(format!("CreateToolhelp32Snapshot failed (Win32 error {})", last_error()));
        }
        let mut entry: THREADENTRY32 = unsafe { std::mem::zeroed() };
        entry.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
        let mut resumed = 0usize;
        let mut failure = None;
        let mut more = unsafe { Thread32First(snapshot, &mut entry) } != 0;
        while more {
            if entry.th32OwnerProcessID == pid {
                let thread = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID) };
                if thread.is_null() {
                    failure = Some(format!("OpenThread({}) failed (Win32 error {})", entry.th32ThreadID, last_error()));
                } else {
                    if unsafe { ResumeThread(thread) } == u32::MAX {
                        failure = Some(format!("ResumeThread({}) failed (Win32 error {})", entry.th32ThreadID, last_error()));
                    } else {
                        resumed += 1;
                    }
                    unsafe { CloseHandle(thread) };
                }
            }
            more = unsafe { Thread32Next(snapshot, &mut entry) } != 0;
        }
        unsafe { CloseHandle(snapshot) };
        match (resumed, failure) {
            (0, Some(error)) => Err(error),
            (0, None) => Err(format!("no thread of process {pid} found to resume")),
            (count, _) => Ok(count),
        }
    }
}

/// How a close ended, for the Host's process-cleanup record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminateReport {
    /// Whether the whole process tree is confirmed gone: Unix, process group
    /// gone (or zombies only) and every tracked descendant gone; Windows,
    /// Fox's Job Object empty (`taskkill` fallback: never confirmed).
    pub confirmation: crate::port::StopConfirmation,
    /// Set when "confirmed" covers less than the whole tree (Windows
    /// sandboxed: the srt-win runner subtree is outside Fox's job).
    pub confirmation_scope_note: Option<String>,
    /// Unix: the backend's process group.
    pub process_group: Option<u32>,
    /// Unix: every descendant seen before the kill (pid + start time).
    pub tracked: Vec<crate::proctree::ProcIdentity>,
    /// The backend did not exit within [`TERMINATE_GRACE`] and was killed.
    pub force_killed: bool,
    /// Windows: `Some(true)` when `TerminateJobObject` succeeded, `Some(false)`
    /// when it failed (see `notes`), `None` when there was no job (not
    /// Windows, or the job was never created).
    pub job_terminated: Option<bool>,
    /// What happened beyond the normal path (job failure, taskkill fallback
    /// and its result). Also written to stderr with a `fox-agent-backend:`
    /// prefix.
    pub notes: Vec<String>,
}

impl Default for TerminateReport {
    fn default() -> Self {
        Self {
            confirmation: crate::port::StopConfirmation::FailedOrUnconfirmed { reasons: vec!["not checked".into()] },
            confirmation_scope_note: None,
            process_group: None,
            tracked: Vec::new(),
            force_killed: false,
            job_terminated: None,
            notes: Vec::new(),
        }
    }
}

impl TerminateReport {
    /// The port-level record of this stop.
    pub fn process_tree_stop(&self) -> crate::port::ProcessTreeStop {
        crate::port::ProcessTreeStop {
            force_killed: self.force_killed,
            confirmation: self.confirmation.clone(),
            confirmation_scope_note: self.confirmation_scope_note.clone(),
            process_group: self.process_group,
            tracked: self.tracked.clone(),
        }
    }
}

/// What "confirmed" means for a Windows sandboxed process (two-hop launch).
pub const WINDOWS_SANDBOXED_SCOPE_NOTE: &str = "Windows sandboxed: confirmed = Fox's Job Object is empty (node + srt CLI). The srt-win runner and the backend it starts as srt-sandbox are outside that job; they end through srt-win's own job chain when srt exits, which is not yet measured (docs/sandbox.md section 7.4).";

#[cfg(windows)]
type ProcessJob = Option<win_job::Job>;
#[cfg(not(windows))]
type ProcessJob = Option<NoJob>;
/// Placeholder: no Job Object off Windows (process groups instead).
#[cfg(not(windows))]
pub struct NoJob;

fn note(report: &mut TerminateReport, text: String) {
    eprintln!("fox-agent-backend: {text}");
    report.notes.push(text);
}

/// Kill the backend tree. Unix: SIGKILL to the process group. Windows:
/// `TerminateJobObject`; `taskkill /T /F` only as a fallback when there is no
/// job or terminating it failed, with its exit status checked and recorded.
fn kill_tree(pid: u32, job: &ProcessJob, report: &mut TerminateReport) {
    let _ = &job;
    #[cfg(windows)]
    {
        if let Some(job) = job {
            match job.terminate(1) {
                Ok(()) => {
                    report.job_terminated = Some(true);
                    return;
                }
                Err(code) => {
                    report.job_terminated = Some(false);
                    note(report, format!("TerminateJobObject failed (Win32 error {code}); falling back to taskkill /T /F for pid {pid}"));
                }
            }
        } else {
            note(report, format!("no Job Object for pid {pid}; falling back to taskkill /T /F"));
        }
    }
    #[cfg(unix)]
    {
        if let Err(error) = crate::proctree::signal_group(pid, libc::SIGKILL) {
            note(report, error);
        }
    }
    #[cfg(windows)]
    {
        let result = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        match result {
            // 128: no such process (it had already exited).
            Ok(status) if status.success() => note(report, format!("taskkill /T /F pid {pid}: ok")),
            Ok(status) => note(report, format!("taskkill /T /F pid {pid}: exit {:?} (128 = process already gone)", status.code())),
            Err(error) => note(report, format!("taskkill /T /F pid {pid}: could not run ({error})")),
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (pid, report);
    }
}

impl AcpConnection {
    /// Spawn the backend and complete `initialize`. Returns after the
    /// `Initialized` event has been queued.
    pub async fn spawn(spec: &LaunchSpec, cwd: &std::path::Path) -> Result<Self, String> {
        let mut command = tokio::process::Command::new(&spec.program);
        command
            .args(&spec.args)
            .current_dir(cwd)
            .env_clear()
            .envs(child_environment(spec, std::env::vars()))
            .envs(spec.session_env.iter().cloned())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        #[cfg(unix)]
        command.process_group(0);
        #[cfg(windows)]
        command.creation_flags(win_job::CREATE_SUSPENDED);
        let mut child = command
            .spawn()
            .map_err(|error| format!("failed to start {}: {error}", spec.program))?;
        // Windows: into the job while suspended, then resume. Any failure
        // kills the suspended child; nothing runs outside the job.
        #[cfg(windows)]
        let job: ProcessJob = {
            let placed = (|| {
                let job = win_job::Job::new()?;
                let handle = child.raw_handle().ok_or("backend exited before it could be placed in a Job Object")?;
                job.assign(handle)?;
                let pid = child.id().ok_or("backend has no pid")?;
                win_job::resume_process(pid)?;
                Ok::<_, String>(job)
            })();
            match placed {
                Ok(job) => Some(job),
                Err(error) => {
                    let _ = child.start_kill();
                    return Err(format!("could not contain {} in a Job Object: {error}", spec.program));
                }
            }
        };
        #[cfg(not(windows))]
        let job: ProcessJob = None;
        let stdin = child.stdin.take().ok_or("backend stdin unavailable")?;
        let stdout = child.stdout.take().ok_or("backend stdout unavailable")?;
        let child_id = child.id();
        let stderr_tail: Arc<Mutex<std::collections::VecDeque<String>>> = Arc::default();
        if let Some(stderr) = child.stderr.take() {
            let tail = Arc::clone(&stderr_tail);
            tokio::spawn(async move {
                use tokio::io::AsyncBufReadExt;
                let mut lines = tokio::io::BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    if let Ok(mut tail) = tail.lock() {
                        tail.push_back(redact_line(&line));
                        while tail.iter().map(String::len).sum::<usize>() > STDERR_TAIL_BYTES {
                            tail.pop_front();
                        }
                    }
                }
            });
        }
        let transport = ByteStreams::new(stdin.compat_write(), stdout.compat());

        let (event_tx, events) = mpsc::unbounded_channel();
        let (commands, mut command_rx) = mpsc::unbounded_channel::<Command>();
        let pending: PendingPermissions = Arc::default();
        let (ready_tx, ready_rx) = oneshot::channel::<Result<(), String>>();

        let notify_tx = event_tx.clone();
        let permission_tx = event_tx.clone();
        let permission_pending = Arc::clone(&pending);
        let main_tx = event_tx.clone();
        let next_request = Arc::new(std::sync::atomic::AtomicU64::new(0));

        tokio::spawn(async move {
            let result = Client
                .builder()
                .name("fox-host")
                .on_receive_notification(
                    async move |notification: SessionNotification, _cx| {
                        let update = serde_json::to_value(&notification.update).unwrap_or(Value::Null);
                        let _ = notify_tx.send(BackendEvent::Update {
                            session_id: notification.session_id.to_string(),
                            update,
                        });
                        Ok(())
                    },
                    agent_client_protocol::on_receive_notification!(),
                )
                .on_receive_request(
                    async move |request: RequestPermissionRequest, responder: Responder<RequestPermissionResponse>, _cx| {
                        let id = format!(
                            "perm-{}",
                            next_request.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                        );
                        let session_id = request.session_id.to_string();
                        let option_ids = request.options.iter().map(|option| option.option_id.to_string()).collect();
                        let raw = serde_json::to_value(&request).unwrap_or(Value::Null);
                        let mut registry = permission_pending.lock().expect("permission registry poisoned");
                        if registry.stopping {
                            // Late request after the stop began: answer it
                            // `cancelled` here; it never becomes answerable.
                            drop(registry);
                            let _ = responder.respond(RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled));
                            let _ = permission_tx.send(BackendEvent::PermissionAutoCancelled {
                                session_id,
                                reason: "connection stopping (full access revoked)".to_owned(),
                                request: raw,
                            });
                            return Ok(());
                        }
                        registry.map.insert(id.clone(), (session_id.clone(), responder));
                        // Queued while holding the lock, so it is ordered
                        // before any drain by begin_stopping.
                        let _ = permission_tx.send(BackendEvent::PermissionRequested {
                            native_request_id: id,
                            session_id,
                            option_ids,
                            request: raw,
                        });
                        drop(registry);
                        Ok(())
                    },
                    agent_client_protocol::on_receive_request!(),
                )
                .connect_with(transport, async move |connection: ConnectionTo<Agent>| {
                    let init = connection
                        .send_request(InitializeRequest::new(ProtocolVersion::V1))
                        .block_task()
                        .await;
                    let init = match init {
                        Ok(init) => init,
                        Err(error) => {
                            let _ = ready_tx.send(Err(format!("initialize failed: {error}")));
                            return Ok(());
                        }
                    };
                    let initialize_response = serde_json::to_value(&init).unwrap_or(Value::Null);
                    let _ = main_tx.send(BackendEvent::Initialized {
                        capabilities: capabilities_from_initialize(&initialize_response),
                        initialize_response,
                    });
                    let _ = ready_tx.send(Ok(()));
                    while let Some(command) = command_rx.recv().await {
                        match command {
                            Command::NewSession { cwd, reply } => {
                                let created = connection
                                    .send_request(NewSessionRequest::new(cwd))
                                    .block_task()
                                    .await
                                    .map(|response| response.session_id.to_string())
                                    .map_err(|error| error.to_string());
                                let _ = reply.send(created);
                            }
                            Command::Prompt { session_id, text } => {
                                // Prompt completion arrives on its own task so
                                // cancel and permission answers keep flowing.
                                let connection = connection.clone();
                                let tx = main_tx.clone();
                                tokio::spawn(async move {
                                    let outcome = connection
                                        .send_request(PromptRequest::new(
                                            session_id.clone(),
                                            vec![ContentBlock::Text(TextContent::new(text))],
                                        ))
                                        .block_task()
                                        .await;
                                    let _ = tx.send(match outcome {
                                        Ok(response) => BackendEvent::TurnEnded {
                                            session_id,
                                            stop_reason: serde_json::to_value(&response.stop_reason)
                                                .unwrap_or(Value::Null),
                                        },
                                        Err(error) => BackendEvent::Failed(format!("prompt failed: {error}")),
                                    });
                                });
                            }
                            Command::Cancel { session_id } => {
                                let _ = connection.send_notification(CancelNotification::new(session_id));
                            }
                        }
                    }
                    Ok(())
                })
                .await;
            if let Err(error) = result {
                let _ = event_tx.send(BackendEvent::Failed(error.to_string()));
            }
            let _ = event_tx.send(BackendEvent::Closed);
        });

        let tail_text = |tail: &Arc<Mutex<std::collections::VecDeque<String>>>| {
            tail.lock().map(|lines| lines.iter().cloned().collect::<Vec<_>>().join("\n")).unwrap_or_default()
        };
        let failure = match tokio::time::timeout(INITIALIZE_TIMEOUT, ready_rx).await {
            Ok(Ok(Ok(()))) => None,
            Ok(Ok(Err(error))) => Some(error),
            Ok(Err(_)) => Some("backend closed before initialize completed".to_owned()),
            Err(_) => Some(format!("initialize did not complete within {} s", INITIALIZE_TIMEOUT.as_secs())),
        };
        if let Some(error) = failure {
            if let Some(pid) = child_id {
                kill_tree(pid, &job, &mut TerminateReport::default());
            }
            let tail = tail_text(&stderr_tail);
            return Err(if tail.is_empty() { error } else { format!("{error}; backend stderr (redacted):\n{tail}") });
        }
        Ok(Self {
            commands,
            events,
            pending,
            child_id,
            child: Arc::new(tokio::sync::Mutex::new(child)),
            job,
            stderr_tail,
            sandbox: None,
            permission_mode: crate::sandbox::PermissionMode::FullAccess,
        })
    }

    /// Spawn what [`crate::sandbox::prepare_launch`] produced. In sandboxed
    /// mode the backend runs inside srt and the per-session settings live as
    /// long as this connection. Sandbox refusals happen before this call
    /// (fail-closed); a sandboxed launch that cannot start is an error here
    /// too, never a retry outside the sandbox.
    pub async fn spawn_prepared(prepared: crate::sandbox::PreparedLaunch, cwd: &std::path::Path) -> Result<Self, String> {
        let crate::sandbox::PreparedLaunch { mode, spec, sandbox, .. } = prepared;
        let mut connection = Self::spawn(&spec, cwd).await.map_err(|error| match mode {
            crate::sandbox::PermissionMode::Sandboxed => format!("sandboxed launch failed: {error}"),
            crate::sandbox::PermissionMode::FullAccess => error,
        })?;
        connection.sandbox = sandbox;
        connection.permission_mode = mode;
        Ok(connection)
    }

    /// The mode this process runs in. A plain [`Self::spawn`] has no sandbox
    /// and reports `FullAccess`, which is what it is.
    pub fn permission_mode(&self) -> crate::sandbox::PermissionMode {
        self.permission_mode
    }

    pub async fn new_session(&self, cwd: PathBuf) -> Result<String, String> {
        if self.is_stopping() {
            return Err(STOPPING_REFUSAL.to_owned());
        }
        let (reply, response) = oneshot::channel();
        self.commands
            .send(Command::NewSession { cwd, reply })
            .map_err(|_| "backend connection closed".to_string())?;
        response.await.map_err(|_| "backend connection closed".to_string())?
    }

    /// Start a turn. Its end is reported as `TurnEnded` or `Failed`.
    pub fn prompt(&self, session_id: &str, text: &str) -> Result<(), String> {
        if self.is_stopping() {
            return Err(STOPPING_REFUSAL.to_owned());
        }
        self.commands
            .send(Command::Prompt {
                session_id: session_id.to_owned(),
                text: text.to_owned(),
            })
            .map_err(|_| "backend connection closed".to_string())
    }

    /// Send `session/cancel` and answer every pending permission request of
    /// the session with `cancelled`. Returns the native ids that were settled.
    pub fn cancel(&self, session_id: &str) -> Vec<String> {
        let _ = self.commands.send(Command::Cancel {
            session_id: session_id.to_owned(),
        });
        let mut registry = self.pending.lock().expect("permission registry poisoned");
        let ids: Vec<String> = registry
            .map
            .iter()
            .filter(|(_, (session, _))| session == session_id)
            .map(|(id, _)| id.clone())
            .collect();
        for id in &ids {
            if let Some((_, responder)) = registry.map.remove(id) {
                let _ = responder.respond(RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled));
            }
        }
        ids
    }

    /// Answer a permission request with one of the offered option ids, or
    /// `None` to reject as cancelled. ACP has no acknowledgement for this
    /// response, so success is `Written`, never acknowledged; a closed
    /// transport is `Unconfirmed` (outcome unknown).
    pub fn answer_permission(&self, native_request_id: &str, option_id: Option<&str>) -> Result<WriteOutcome, String> {
        let (_, responder) = self
            .pending
            .lock()
            .expect("permission registry poisoned")
            .map
            .remove(native_request_id)
            .ok_or_else(|| "permission request already answered or no longer pending".to_string())?;
        let outcome = match option_id {
            Some(id) => RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(id.to_owned())),
            None => RequestPermissionOutcome::Cancelled,
        };
        Ok(match responder.respond(RequestPermissionResponse::new(outcome)) {
            Ok(()) => WriteOutcome::Written,
            Err(_) => WriteOutcome::Unconfirmed,
        })
    }

    /// Number of permission requests still waiting for the user.
    pub fn pending_permissions(&self) -> usize {
        self.pending.lock().map(|registry| registry.map.len()).unwrap_or(0)
    }

    /// Enter the stopping state (idempotent): new prompts and sessions are
    /// refused with [`STOPPING_REFUSAL`], every pending permission request (all
    /// sessions) is answered `cancelled` now, and any that arrives later is
    /// answered `cancelled` on arrival and reported only as
    /// [`BackendEvent::PermissionAutoCancelled`]. Returns the native ids that
    /// were pending.
    pub fn begin_stopping(&self) -> Vec<String> {
        let mut registry = self.pending.lock().expect("permission registry poisoned");
        registry.stopping = true;
        let drained: Vec<_> = registry.map.drain().collect();
        drop(registry);
        let mut ids = Vec::with_capacity(drained.len());
        for (id, (_, responder)) in drained {
            let _ = responder.respond(RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled));
            ids.push(id);
        }
        ids.sort();
        ids
    }

    /// Whether [`Self::begin_stopping`] has run.
    pub fn is_stopping(&self) -> bool {
        self.pending.lock().map(|registry| registry.stopping).unwrap_or(true)
    }

    /// Last lines of the backend's stderr, secret-shaped tokens masked.
    pub fn stderr_tail(&self) -> String {
        self.stderr_tail
            .lock()
            .map(|lines| lines.iter().cloned().collect::<Vec<_>>().join("\n"))
            .unwrap_or_default()
    }

    /// Terminate the backend process tree: SIGTERM to the group, a grace
    /// period for the backend to stop its own children, then SIGKILL.
    /// Pending permission requests are dropped with the connection; the Host
    /// marks them per `reverse` rules.
    pub async fn close(self) {
        self.close_with_report().await;
    }

    /// [`Self::close`], returning how it ended (force kill, Windows job
    /// termination, taskkill fallback, whether the tree is confirmed gone)
    /// for the Host's cleanup record. Enters the stopping state first.
    pub async fn close_with_report(self) -> TerminateReport {
        self.begin_stopping();
        self.terminate().await
    }

    /// Stop the backend and confirm its process tree is gone.
    ///
    /// Unix: snapshot the descendants, SIGTERM to the group,
    /// [`TERMINATE_GRACE`], snapshot again, SIGKILL to the group, reap the
    /// child, SIGKILL tracked descendants that left the group, then poll
    /// ([`crate::proctree::confirm_tree_gone`]). Windows has no SIGTERM: the
    /// command channel is dropped first (the ACP connection then ends and the
    /// backend's stdin closes, which lets srt run its own reset and remove its
    /// ACEs), then the grace, then `TerminateJobObject` (taskkill only as a
    /// checked fallback), then `ActiveProcesses == 0` is polled.
    async fn terminate(self) -> TerminateReport {
        let Self { commands, child, child_id, job, sandbox, permission_mode, .. } = self;
        let mut report = TerminateReport::default();
        #[cfg(windows)]
        drop(commands);
        #[cfg(not(windows))]
        let _commands = commands;
        let _ = permission_mode;
        let mut child = child.lock().await;
        if let Some(pid) = child_id {
            #[cfg(unix)]
            {
                report.process_group = Some(pid);
                report.tracked = crate::proctree::snapshot_tree(pid);
                if let Err(error) = crate::proctree::signal_group(pid, libc::SIGTERM) {
                    note(&mut report, error);
                }
            }
            report.force_killed = tokio::time::timeout(TERMINATE_GRACE, child.wait()).await.is_err();
            #[cfg(unix)]
            {
                let late = crate::proctree::snapshot_tree(pid);
                crate::proctree::merge(&mut report.tracked, late);
            }
            kill_tree(pid, &job, &mut report);
        }
        let _ = child.kill().await;
        let _ = child.wait().await;
        drop(child);
        report.confirmation = match child_id {
            None => crate::port::StopConfirmation::FailedOrUnconfirmed { reasons: vec!["backend had no process id".into()] },
            Some(pid) => {
                #[cfg(unix)]
                {
                    let tracked = report.tracked.clone();
                    let (notes, confirmation) = tokio::task::spawn_blocking(move || {
                        let notes = crate::proctree::kill_escaped(pid, &tracked);
                        (notes, crate::proctree::confirm_tree_gone(pid, &tracked, crate::proctree::STOP_CONFIRM_TIMEOUT))
                    })
                    .await
                    .unwrap_or_else(|error| {
                        (Vec::new(), crate::port::StopConfirmation::FailedOrUnconfirmed { reasons: vec![format!("confirmation task failed: {error}")] })
                    });
                    for text in notes {
                        note(&mut report, text);
                    }
                    confirmation
                }
                #[cfg(windows)]
                {
                    let _ = pid;
                    match &job {
                        Some(job) => {
                            if permission_mode == crate::sandbox::PermissionMode::Sandboxed {
                                report.confirmation_scope_note = Some(WINDOWS_SANDBOXED_SCOPE_NOTE.to_owned());
                            }
                            job.confirm_empty(crate::proctree::STOP_CONFIRM_TIMEOUT)
                        }
                        None => crate::port::StopConfirmation::FailedOrUnconfirmed {
                            reasons: vec!["no Job Object: taskkill /T /F cannot prove that every descendant ended".into()],
                        },
                    }
                }
                #[cfg(not(any(unix, windows)))]
                {
                    let _ = pid;
                    crate::port::StopConfirmation::FailedOrUnconfirmed { reasons: vec!["no tree confirmation on this platform".into()] }
                }
            }
        };
        if let crate::port::StopConfirmation::FailedOrUnconfirmed { reasons } = &report.confirmation {
            eprintln!("fox-agent-backend: process tree stop NOT confirmed: {}", reasons.join("; "));
        }
        drop(job);
        drop(sandbox);
        report
    }

    /// OS process id of the backend (the srt broker in sandboxed mode).
    pub fn process_id(&self) -> Option<u32> {
        self.child_id
    }

    /// Revocation stop for a live connection: full access was withdrawn while
    /// this process runs outside the sandbox
    /// ([`crate::sandbox::ModeRecheck::RestartSandboxed`]).
    ///
    /// 1. stopping state ([`Self::begin_stopping`]): new tasks refused, every
    ///    pending permission answered `cancelled`, late ones auto-cancelled;
    /// 2. `session/cancel`;
    /// 3. the run's pending requests settled (in flight -> `outcome_unknown`);
    /// 4. the process tree terminated and confirmed gone, the connection
    ///    consumed;
    /// 5. the run's terminal committed. Confirmed -> `NeedsSandboxedRebuild`
    ///    with `replay_last_operation = false`; not confirmed ->
    ///    `StoppedUnconfirmed` and [`crate::port::rebuild_after_revocation`]
    ///    refuses.
    pub async fn stop_for_revocation(
        self,
        session_id: &str,
        pending: &mut crate::settlement::PendingRegistry,
        arbiter: &mut crate::settlement::TerminalArbiter,
        reason: &str,
    ) -> crate::port::RevocationStop {
        let mut cancelled = self.begin_stopping();
        for id in self.cancel(session_id) {
            if !cancelled.contains(&id) {
                cancelled.push(id);
            }
        }
        let settled = crate::port::settle_for_revocation(pending, arbiter, reason);
        let report = self.terminate().await;
        crate::port::finish_revocation(arbiter, reason, cancelled, settled, report.process_tree_stop())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stderr_redaction_masks_secret_shaped_tokens() {
        let line = redact_line("auth failed OPENAI_API_KEY=abc sk-1234567890 token ok");
        assert!(!line.contains("abc") && !line.contains("sk-1234567890"));
        assert!(line.contains("auth failed") && line.contains("ok"));
    }

    #[test]
    fn capabilities_default_to_unsupported_and_read_declared_fields() {
        let empty = capabilities_from_initialize(&serde_json::json!({"protocolVersion": 1}));
        assert_eq!(empty.session, SessionCapabilities::default());
        let declared = capabilities_from_initialize(&serde_json::json!({
            "protocolVersion": 1,
            "agentCapabilities": {
                "loadSession": true,
                "sessionCapabilities": {"resume": {}, "close": {}, "list": null}
            }
        }));
        assert!(declared.session.load && declared.session.resume && declared.session.close);
        assert!(!declared.session.list);
        assert!(!declared.approval_answer_acknowledged);
    }
}
