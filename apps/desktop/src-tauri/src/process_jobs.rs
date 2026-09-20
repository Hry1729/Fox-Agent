//! JOB-v1 — independent long-process lifecycle (batch `C-MODULE-01`).
//!
//! **Status: NOT WIRED.** This module is intentionally *not* declared in
//! `lib.rs`, is not registered as a Host tool, and is not reachable from
//! `run_command`. It exists to make start / status / output / cancel / recovery
//! verifiable *before* anything production consumes it.
//!
//! ## Why a separate module
//!
//! `runtime_host/background_jobs.rs` already owns the *attachment-compute* job
//! tools (`compute_job_*`). That is a lifecycle over pure computation. A general
//! long-running command has three properties compute jobs do not have: it can
//! have side effects, it can spawn a process tree that survives its parent, and
//! its output arrives concurrently on two pipes. Reusing the compute lifecycle
//! unchanged would either lie about resumability or replay side effects, so this
//! module implements the same *shape* (durable-ish bookkeeping, idempotent start,
//! pure-read status, cancel-as-request) with different terminal semantics.
//!
//! ## Reused, not re-invented
//!
//! The four invariants below are taken from the production job lifecycle
//! (`database/repositories/kernel_jobs.rs`) and kept deliberately:
//!
//! * **Start is idempotent.** `(runId, idempotencyKey)` identifies a job. A
//!   repeat with the same canonical request returns the existing job; the same
//!   key with a different request is a conflict, never a second launch.
//! * **Status is a pure read.** `status`/`output` never advance the state
//!   machine and never touch a process, so polling cannot cause side effects.
//! * **Cancel is a request until acknowledged.** `cancel` records the request
//!   first and only reports `acknowledged` once the process tree is stopped and
//!   the output readers have drained.
//! * **A restart can tell "gone" from "running".** Ownership is `(pid, start
//!   marker)`, never a bare pid: the OS reuses pids, so a `running` row whose
//!   pid now belongs to somebody else is *replaced*, not *ours*.
//!
//! ## Deliberate deviations from the compute lifecycle (for review)
//!
//! 1. **An orphaned long command is not `paused`.** Compute jobs are pure, so an
//!    orphan is resumable. A long command may already have written files, so
//!    this module never claims resumability and never auto-replays:
//!    [`JobRunState::Interrupted`] maps to production `failed` with an
//!    `interrupted*` error code, and [`RecoveryDecision::allow_replay`] is
//!    always `false`. See `patches/C/C-REQ-002` for the mapping table.
//! 2. **Unknown ownership is not treated as gone.** If the identity probe cannot
//!    answer (access denied, unexpected API failure) the job is labelled
//!    `interrupted` with [`OwnerStatus::Unknown`] and `needs_review`, and the
//!    state is left for a human/verifier — matching `process_identity.rs`, which
//!    refuses to treat unknown ownership as an orphan.
//! 3. **Output offsets are per stream.** stdout and stderr are separate ordered
//!    streams with independent cursors, so a stalled stderr cannot make stdout
//!    unreadable. Retention is bounded per stream and the dropped count is part
//!    of the result rather than a silent gap.
//!
//! ## Boundaries this module does NOT cross
//!
//! * No `crate::` imports: std-only plus a pluggable adapter boundary, so it can
//!   be compiled and tested on its own (`tests/harness_process_jobs.rs`).
//! * No second production executor: everything is behind [`ProcessSpawner`].
//!   Production wiring must supply the *existing* `tool_host::execute_command`
//!   result semantics and `runtime_host::terminate_process_tree`; the fallback
//!   implementations here exist only so the module is self-contained, and are
//!   expected to be deleted at wiring time.
//! * Process-tree cleanup is **not** OS isolation. It stops descendants after
//!   the fact; it does not confine them. `S01` keeps its own milestone.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::io::{self, Read};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Stable machine-readable failures. `code()` values are part of the contract
/// and must be reused verbatim by the wiring layer instead of being re-invented.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JobError {
    UnknownJob(String),
    /// Same `(runId, idempotencyKey)`, different canonical request.
    IdempotencyConflict {
        job_id: String,
    },
    InvalidRequest(String),
    Capacity {
        max_jobs: usize,
    },
    /// The process could not be started at all. Distinct from a non-zero exit.
    StartFailed(String),
}

impl JobError {
    pub fn code(&self) -> &'static str {
        match self {
            JobError::UnknownJob(_) => "job.unknown",
            JobError::IdempotencyConflict { .. } => "job.idempotency_conflict",
            JobError::InvalidRequest(_) => "job.invalid_request",
            JobError::Capacity { .. } => "job.capacity",
            JobError::StartFailed(_) => "job.start_failed",
        }
    }

    pub fn message(&self) -> String {
        match self {
            JobError::UnknownJob(id) => format!("unknown job '{id}'"),
            JobError::IdempotencyConflict { job_id } => format!(
                "idempotency key already belongs to job '{job_id}' with a different request"
            ),
            JobError::InvalidRequest(detail) => detail.clone(),
            JobError::Capacity { max_jobs } => {
                format!("job capacity reached (max {max_jobs} tracked jobs)")
            }
            JobError::StartFailed(detail) => detail.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// Core vocabulary
// ---------------------------------------------------------------------------

/// Which pipe a byte came from. Kept separate so one stream cannot starve the
/// other's cursor.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Stream {
    Stdout,
    Stderr,
}

impl Stream {
    pub fn as_str(self) -> &'static str {
        match self {
            Stream::Stdout => "stdout",
            Stream::Stderr => "stderr",
        }
    }
}

/// Internal lifecycle state. Distinct from the production `JobState` on
/// purpose — see `production_state()` for the explicit mapping.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum JobRunState {
    /// Claimed but not yet successfully spawned.
    Queued,
    Running,
    Succeeded,
    Failed,
    Cancelled,
    TimedOut,
    /// Restart found the owning process gone/replaced/unknown. Never auto-replayed.
    Interrupted,
}

impl JobRunState {
    pub fn as_str(self) -> &'static str {
        match self {
            JobRunState::Queued => "queued",
            JobRunState::Running => "running",
            JobRunState::Succeeded => "succeeded",
            JobRunState::Failed => "failed",
            JobRunState::Cancelled => "cancelled",
            JobRunState::TimedOut => "timed_out",
            JobRunState::Interrupted => "interrupted",
        }
    }

    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            JobRunState::Succeeded
                | JobRunState::Failed
                | JobRunState::Cancelled
                | JobRunState::TimedOut
                | JobRunState::Interrupted
        )
    }

    /// The production `JobState` string this state persists as.
    ///
    /// `TimedOut` and `Interrupted` both land on `failed` because production has
    /// no such states; the distinction survives in `error_code`, and
    /// `Interrupted` deliberately does **not** become `paused` (that would
    /// advertise a resumability a side-effecting command does not have).
    pub fn production_state(self) -> &'static str {
        match self {
            JobRunState::Queued => "queued",
            JobRunState::Running => "running",
            JobRunState::Succeeded => "completed",
            JobRunState::Failed => "failed",
            JobRunState::Cancelled => "cancelled",
            JobRunState::TimedOut => "failed",
            JobRunState::Interrupted => "failed",
        }
    }

    /// `error_code` to persist alongside `production_state()`, when one is owed.
    pub fn production_error_code(self) -> Option<&'static str> {
        match self {
            JobRunState::TimedOut => Some("timed_out"),
            JobRunState::Interrupted => Some("interrupted"),
            _ => None,
        }
    }
}

/// Why a stop was requested. Both paths terminate the tree; they differ in the
/// terminal state they produce.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StopReason {
    Cancelled,
    Timeout,
}

/// What a tree-termination attempt actually achieved.
///
/// `Terminated` and `Incomplete` are deliberately separate. An earlier shape
/// returned a bare `Ok(())` from the walker and mapped it to `Terminated`,
/// which meant a `taskkill` that never started (or that failed) was reported as
/// a completed teardown. "We tried to kill the tree" and "the tree is gone" are
/// different facts and a caller must be able to tell them apart.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TerminateOutcome {
    /// The child had already exited; nothing was signalled.
    AlreadyExited,
    /// The tree walker ran and reported success. `walker_visited` is how many
    /// processes it reported acting on, so "0 visited" is visible rather than
    /// silently implied.
    Terminated { walker_exit_code: Option<i32>, walker_visited: u32 },
    /// The walker could not be run or did not report success. The direct child
    /// was still signalled as a fallback, so the outcome for *that* process is
    /// known, but the tree as a whole is not. Never reported as `Terminated`.
    Incomplete { walker_exit_code: Option<i32>, walker_visited: u32, detail: String },
    /// The pid no longer resolves to our process instance, so it was **not**
    /// touched. This is the guard that keeps a reused pid from being killed.
    SkippedIdentityMismatch,
}

impl TerminateOutcome {
    /// Whether the attempt is a positive statement that the tree stopped.
    pub fn tree_stopped(&self) -> bool {
        matches!(self, TerminateOutcome::AlreadyExited | TerminateOutcome::Terminated { .. })
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            TerminateOutcome::AlreadyExited => "already_exited",
            TerminateOutcome::Terminated { .. } => "terminated",
            TerminateOutcome::Incomplete { .. } => "incomplete",
            TerminateOutcome::SkippedIdentityMismatch => "skipped_identity_mismatch",
        }
    }
}

/// Outcome of `start`. Mirrors the production `JobStartOutcome`.
#[derive(Clone, Debug)]
pub enum StartOutcome {
    /// A new job was claimed and launched.
    Launched(JobStatusView),
    /// The same canonical request already existed; nothing new was launched.
    Existing(JobStatusView),
}

impl StartOutcome {
    pub fn view(&self) -> &JobStatusView {
        match self {
            StartOutcome::Launched(view) | StartOutcome::Existing(view) => view,
        }
    }
    pub fn launched(&self) -> bool {
        matches!(self, StartOutcome::Launched(_))
    }
}

// ---------------------------------------------------------------------------
// Canonical request + digest
// ---------------------------------------------------------------------------

/// How the child's environment is built. Inheriting is the default so this
/// module does not silently narrow what the caller's policy already allowed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EnvMode {
    Inherit,
    /// Exactly these entries. Used by tests to prove isolation is achievable.
    Replace(Vec<(String, String)>),
}

/// A launch request. Every field participates in the idempotency comparison.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpawnSpec {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: String,
    pub env: EnvMode,
}

impl SpawnSpec {
    /// The production default shell, reproduced so tests exercise the same
    /// shape. **Wiring must use `tool_host`'s construction, not this helper**,
    /// so the two can never drift apart.
    pub fn shell(command: &str, cwd: &str) -> SpawnSpec {
        #[cfg(windows)]
        let (program, args) = (
            "cmd.exe".to_owned(),
            vec!["/D".to_owned(), "/S".to_owned(), "/C".to_owned(), command.to_owned()],
        );
        #[cfg(not(windows))]
        let (program, args) = ("sh".to_owned(), vec!["-lc".to_owned(), command.to_owned()]);
        SpawnSpec {
            program,
            args,
            cwd: cwd.to_owned(),
            env: EnvMode::Inherit,
        }
    }
}

/// The caller's request. `run_id` is carried for scope binding only; this module
/// performs no authorization of its own and must be handed an already
/// authorized Run (production: the frozen `run_control_binding`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StartRequest {
    pub run_id: String,
    pub idempotency_key: String,
    pub spec: SpawnSpec,
    /// Execution budget. Counted from the moment the child is spawned, so any
    /// waiting/approval time *before* launch is outside it.
    pub execution_budget: Duration,
}

impl StartRequest {
    /// Deterministic, human-diffable canonical form of everything that decides
    /// whether two starts are "the same request".
    ///
    /// A trailing separator in `cwd` is folded away so `/a/b` and `/a/b/` are one
    /// request. Nothing else is normalized: case folding and symlink resolution
    /// are platform- and policy-dependent and belong to the caller, which must
    /// pass an already-canonical cwd.
    pub fn canonical(&self) -> String {
        let cwd = trim_trailing_separators(&self.spec.cwd);
        let mut out = String::with_capacity(256);
        out.push_str("fox-process-job-v1\n");
        push_field(&mut out, "run", &self.run_id);
        push_field(&mut out, "key", &self.idempotency_key);
        push_field(&mut out, "program", &self.spec.program);
        out.push_str(&format!("args={}\n", self.spec.args.len()));
        for arg in &self.spec.args {
            push_field(&mut out, "arg", arg);
        }
        push_field(&mut out, "cwd", &cwd);
        match &self.spec.env {
            EnvMode::Inherit => out.push_str("env=inherit\n"),
            EnvMode::Replace(entries) => {
                let mut sorted = entries.clone();
                sorted.sort();
                out.push_str(&format!("env=replace:{}\n", sorted.len()));
                for (k, v) in sorted {
                    push_field(&mut out, "env-k", &k);
                    push_field(&mut out, "env-v", &v);
                }
            }
        }
        out.push_str(&format!("budget_ms={}\n", self.execution_budget.as_millis()));
        out
    }

    /// 64-bit FNV-1a over the canonical bytes, for durable comparison.
    ///
    /// Wiring note: production already hashes job parameters with SHA-256, so the
    /// wiring patch should persist `sha256(canonical())` and treat this digest as
    /// a display/debug value. It is not used for any security decision here.
    pub fn digest(&self) -> String {
        format!("{:016x}", fnv1a64(self.canonical().as_bytes()))
    }
}

fn push_field(out: &mut String, name: &str, value: &str) {
    // Length-prefixed so no value can forge a field boundary.
    out.push_str(&format!("{name}={}:{value}\n", value.len()));
}

fn trim_trailing_separators(value: &str) -> String {
    let trimmed = value.trim_end_matches(['/', '\\']);
    // Do not reduce a drive root such as `C:\` or `/` to an empty string.
    if trimmed.is_empty() || trimmed.ends_with(':') {
        value.to_owned()
    } else {
        trimmed.to_owned()
    }
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

// ---------------------------------------------------------------------------
// Process identity — pid reuse safety
// ---------------------------------------------------------------------------

/// Identity of a *process instance*, not of a pid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProcessIdentity {
    pub pid: u32,
    /// Creation time in milliseconds. `None` means "not determinable", which must
    /// never be read as "matches".
    pub start_marker: Option<i64>,
}

/// What the OS says about a pid right now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Liveness {
    Alive { start_marker: Option<i64> },
    Gone,
    /// The probe could not answer. Never collapse this into `Gone`.
    Unknown(String),
}

/// How a recorded owner compares with the live process.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OwnerStatus {
    Alive,
    /// The pid exists but belongs to a different process instance.
    Replaced,
    Gone,
    Unknown,
}

impl OwnerStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            OwnerStatus::Alive => "alive",
            OwnerStatus::Replaced => "replaced",
            OwnerStatus::Gone => "gone",
            OwnerStatus::Unknown => "unknown",
        }
    }
}

/// Classifies a recorded owner against a live probe result.
///
/// A marker comparison is only made when **both** sides have one: an
/// undeterminable marker on either side yields `Unknown`, never a false
/// `Alive` (which would hide a dead job) and never a false `Replaced` (which
/// would hide a live one).
pub fn classify_owner(expected: &ProcessIdentity, actual: &Liveness) -> OwnerStatus {
    match actual {
        Liveness::Gone => OwnerStatus::Gone,
        Liveness::Unknown(_) => OwnerStatus::Unknown,
        Liveness::Alive { start_marker } => match (expected.start_marker, start_marker) {
            (Some(want), Some(got)) if want == *got => OwnerStatus::Alive,
            (Some(_), Some(_)) => OwnerStatus::Replaced,
            _ => OwnerStatus::Unknown,
        },
    }
}

pub trait IdentityProbe: Send + Sync {
    /// Identity of this process.
    fn current(&self) -> ProcessIdentity;
    fn probe(&self, pid: u32) -> Liveness;
}

pub trait Clock: Send + Sync {
    fn now_ms(&self) -> i64;
}

#[derive(Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_millis().min(i64::MAX as u128) as i64)
            .unwrap_or(0)
    }
}

/// Real probe using the OS.
///
/// **Wiring must replace this with `runtime_host::process_identity`
/// (`owner_is_gone` / `process_start_marker`)** so the product keeps exactly one
/// implementation of pid-reuse detection. This copy exists only because the
/// module is not yet declared in `lib.rs` and therefore cannot reach `pub(crate)`
/// items.
#[derive(Default)]
pub struct SystemIdentityProbe;

impl IdentityProbe for SystemIdentityProbe {
    fn current(&self) -> ProcessIdentity {
        ProcessIdentity {
            pid: std::process::id(),
            start_marker: current_process_start_marker(),
        }
    }

    fn probe(&self, pid: u32) -> Liveness {
        probe_pid(pid)
    }
}

#[cfg(windows)]
fn current_process_start_marker() -> Option<i64> {
    #[repr(C)]
    struct FileTime {
        low: u32,
        high: u32,
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentProcess() -> isize;
        fn GetProcessTimes(
            process: isize,
            creation: *mut FileTime,
            exit: *mut FileTime,
            kernel: *mut FileTime,
            user: *mut FileTime,
        ) -> i32;
    }
    unsafe {
        let mut creation: FileTime = std::mem::zeroed();
        let mut exit: FileTime = std::mem::zeroed();
        let mut kernel: FileTime = std::mem::zeroed();
        let mut user: FileTime = std::mem::zeroed();
        if GetProcessTimes(GetCurrentProcess(), &mut creation, &mut exit, &mut kernel, &mut user) == 0
        {
            return None;
        }
        let ticks = ((creation.high as u64) << 32) | creation.low as u64;
        // FILETIME is 100 ns units since 1601; ms is enough to separate instances.
        (ticks > 0).then(|| (ticks / 10_000) as i64)
    }
}

#[cfg(not(windows))]
fn current_process_start_marker() -> Option<i64> {
    None
}

#[cfg(windows)]
fn probe_pid(pid: u32) -> Liveness {
    #[repr(C)]
    struct FileTime {
        low: u32,
        high: u32,
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> isize;
        fn CloseHandle(handle: isize) -> i32;
        fn GetLastError() -> u32;
        fn GetExitCodeProcess(handle: isize, code: *mut u32) -> i32;
        fn GetProcessTimes(
            process: isize,
            creation: *mut FileTime,
            exit: *mut FileTime,
            kernel: *mut FileTime,
            user: *mut FileTime,
        ) -> i32;
    }
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const ERROR_INVALID_PARAMETER: u32 = 87;
    const ERROR_NOT_FOUND: u32 = 1168;
    const STILL_ACTIVE: u32 = 259;
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle == 0 {
            return match GetLastError() {
                ERROR_INVALID_PARAMETER | ERROR_NOT_FOUND => Liveness::Gone,
                other => Liveness::Unknown(format!("OpenProcess failed with {other}")),
            };
        }
        let mut exit_code = STILL_ACTIVE;
        let queried = GetExitCodeProcess(handle, &mut exit_code) != 0;
        let mut creation: FileTime = std::mem::zeroed();
        let mut exit: FileTime = std::mem::zeroed();
        let mut kernel: FileTime = std::mem::zeroed();
        let mut user: FileTime = std::mem::zeroed();
        let times = GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user) != 0;
        CloseHandle(handle);
        if !queried {
            return Liveness::Unknown("GetExitCodeProcess failed".to_owned());
        }
        if exit_code != STILL_ACTIVE {
            return Liveness::Gone;
        }
        let start_marker = if times {
            let ticks = ((creation.high as u64) << 32) | creation.low as u64;
            (ticks > 0).then(|| (ticks / 10_000) as i64)
        } else {
            None
        };
        Liveness::Alive { start_marker }
    }
}

#[cfg(not(windows))]
fn probe_pid(pid: u32) -> Liveness {
    match std::fs::metadata(format!("/proc/{pid}")) {
        Ok(_) => Liveness::Alive { start_marker: None },
        Err(error) if error.kind() == io::ErrorKind::NotFound => Liveness::Gone,
        Err(error) => Liveness::Unknown(error.to_string()),
    }
}

// ---------------------------------------------------------------------------
// Bounded output with absolute per-stream offsets
// ---------------------------------------------------------------------------

/// One pipe's retained window.
///
/// Offsets are **absolute logical byte offsets** from the start of the stream,
/// not indices into the retained buffer: a cursor stays meaningful after
/// eviction, and eviction is reported instead of silently shifting data.
#[derive(Debug)]
pub struct StreamWindow {
    bytes: Vec<u8>,
    /// Absolute offset of `bytes[0]`.
    retained_start: u64,
    /// Total bytes ever produced by this stream.
    total: u64,
    retention: usize,
}

impl StreamWindow {
    pub fn new(retention: usize) -> Self {
        Self {
            bytes: Vec::new(),
            retained_start: 0,
            total: 0,
            retention: retention.max(1),
        }
    }

    pub fn push(&mut self, chunk: &[u8]) {
        self.total = self.total.saturating_add(chunk.len() as u64);
        self.bytes.extend_from_slice(chunk);
        self.evict();
    }

    fn evict(&mut self) {
        if self.bytes.len() <= self.retention {
            return;
        }
        let mut cut = self.bytes.len() - self.retention;
        // Never start the retained window in the middle of a UTF-8 sequence.
        while cut < self.bytes.len() && (self.bytes[cut] & 0xC0) == 0x80 {
            cut += 1;
        }
        self.bytes.drain(..cut);
        self.retained_start = self.retained_start.saturating_add(cut as u64);
    }

    pub fn total_bytes(&self) -> u64 {
        self.total
    }

    pub fn retained_start(&self) -> u64 {
        self.retained_start
    }

    pub fn retained_bytes(&self) -> u64 {
        self.bytes.len() as u64
    }

    pub fn dropped_bytes(&self) -> u64 {
        self.retained_start
    }

    pub fn end_offset(&self) -> u64 {
        self.total
    }

    pub fn retention_limit(&self) -> usize {
        self.retention
    }

    /// Reads `[offset, offset + limit)` of the retained window.
    ///
    /// `lost_before_offset` is non-zero when the requested start has already been
    /// evicted, so a caller can tell "no output yet" from "your cursor is too old"
    /// instead of guessing.
    pub fn page(&self, offset: u64, limit: usize) -> OutputPage {
        let lost = self.retained_start.saturating_sub(offset);
        let start = offset.max(self.retained_start);
        let rel = (start - self.retained_start) as usize;
        let rel = rel.min(self.bytes.len());
        let mut end = rel.saturating_add(limit.max(1)).min(self.bytes.len());
        while end > rel && end < self.bytes.len() && (self.bytes[end] & 0xC0) == 0x80 {
            end += 1;
        }
        let slice = &self.bytes[rel..end];
        let next = self.retained_start + end as u64;
        OutputPage {
            content: String::from_utf8_lossy(slice).into_owned(),
            offset: start,
            next_offset: next,
            at_end_of_available: next >= self.total,
            // A buffer cannot know whether its producer is finished.
            stream_closed: false,
            lost_before_offset: lost,
            dropped_bytes: self.dropped_bytes(),
            total_bytes: self.total,
        }
    }
}

/// One page of one stream.
///
/// `at_end_of_available` and `stream_closed` are deliberately **two** flags.
/// "I have read everything produced so far" and "nothing more will ever be
/// produced" are different facts, and a poller that conflates them either stops
/// early or spins forever. Only the manager can answer the second one, because
/// only it knows whether the producing pipe ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutputPage {
    pub content: String,
    pub offset: u64,
    pub next_offset: u64,
    /// This cursor has consumed every byte produced so far.
    pub at_end_of_available: bool,
    /// The pipe ended: no further bytes can arrive on this stream.
    pub stream_closed: bool,
    /// Bytes the caller asked for that were already evicted.
    pub lost_before_offset: u64,
    pub dropped_bytes: u64,
    pub total_bytes: u64,
}

/// A read-only summary of one stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StreamView {
    pub stream: Stream,
    pub total_bytes: u64,
    pub retained_bytes: u64,
    pub dropped_bytes: u64,
    pub retained_start: u64,
    pub end_offset: u64,
    pub retention_limit: usize,
}

/// Both streams. `stdout` and `stderr` are independent so neither can starve the
/// other's cursor.
#[derive(Debug)]
pub struct OutputWindow {
    stdout: StreamWindow,
    stderr: StreamWindow,
}

impl OutputWindow {
    pub fn new(retention: usize) -> Self {
        Self {
            stdout: StreamWindow::new(retention),
            stderr: StreamWindow::new(retention),
        }
    }

    fn window(&self, stream: Stream) -> &StreamWindow {
        match stream {
            Stream::Stdout => &self.stdout,
            Stream::Stderr => &self.stderr,
        }
    }

    fn window_mut(&mut self, stream: Stream) -> &mut StreamWindow {
        match stream {
            Stream::Stdout => &mut self.stdout,
            Stream::Stderr => &mut self.stderr,
        }
    }

    pub fn push(&mut self, stream: Stream, chunk: &[u8]) {
        self.window_mut(stream).push(chunk);
    }

    pub fn page(&self, stream: Stream, offset: u64, limit: usize) -> OutputPage {
        self.window(stream).page(offset, limit)
    }

    pub fn view(&self, stream: Stream) -> StreamView {
        let window = self.window(stream);
        StreamView {
            stream,
            total_bytes: window.total_bytes(),
            retained_bytes: window.retained_bytes(),
            dropped_bytes: window.dropped_bytes(),
            retained_start: window.retained_start(),
            end_offset: window.end_offset(),
            retention_limit: window.retention_limit(),
        }
    }
}

// ---------------------------------------------------------------------------
// Spawn adapter
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExitReport {
    /// `None` when the process was signalled rather than exiting normally.
    pub code: Option<i32>,
}

/// Evidence from a tree-termination attempt.
///
/// Carried out of `ChildHandle` instead of a bare `Result<()>` because the
/// caller must be able to distinguish three different situations that all used
/// to collapse into `Ok(())`: the walker succeeded, the walker failed, and the
/// walker could not be launched at all. Fields are encoding-independent on
/// purpose — the walker's own text is localised (CP936 here), so only counts and
/// exit codes are reported.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeKillReport {
    /// Exit code of the tree walker, or `None` when it never ran.
    pub walker_exit_code: Option<i32>,
    /// Processes the walker reported acting on (counted from its output, which
    /// is one line per process). `0` means it reported nothing.
    pub walker_visited: u32,
    /// Size of the walker's diagnostic stream, kept as evidence that there was
    /// output to look at without guessing its encoding.
    pub walker_stderr_bytes: u32,
    /// Set when the walk was not performed or did not report success.
    pub walker_error: Option<String>,
    /// Whether the direct child was also signalled as a fallback.
    pub direct_kill: bool,
}

impl TreeKillReport {
    /// Did the walker run and report success?
    pub fn walker_succeeded(&self) -> bool {
        self.walker_error.is_none() && self.walker_exit_code == Some(0)
    }
}

/// A spawned child, abstracted so tests can script one without a real process
/// and so production can supply its own launcher.
pub trait ChildHandle: Send {
    fn pid(&self) -> u32;
    /// `Ok(None)` while still running.
    fn try_wait(&mut self) -> io::Result<Option<ExitReport>>;
    fn take_stdout(&mut self) -> Option<Box<dyn Read + Send>>;
    fn take_stderr(&mut self) -> Option<Box<dyn Read + Send>>;
    /// Stops this process **and its descendants**, reporting what it actually
    /// managed to do rather than only that it tried.
    fn terminate_tree(&mut self) -> TreeKillReport;
}

pub trait ProcessSpawner: Send + Sync {
    fn spawn(&self, spec: &SpawnSpec) -> io::Result<Box<dyn ChildHandle>>;
}

/// Real spawner. Wiring should keep this only if A's executor does not already
/// own process creation.
#[derive(Default)]
pub struct SystemSpawner;

impl ProcessSpawner for SystemSpawner {
    fn spawn(&self, spec: &SpawnSpec) -> io::Result<Box<dyn ChildHandle>> {
        let mut command = std::process::Command::new(&spec.program);
        command
            .args(&spec.args)
            .current_dir(&spec.cwd)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        if let EnvMode::Replace(entries) = &spec.env {
            command.env_clear();
            for (key, value) in entries {
                command.env(key, value);
            }
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // CREATE_NO_WINDOW: a background job must not flash a console.
            command.creation_flags(0x0800_0000);
        }
        let child = command.spawn()?;
        Ok(Box::new(SystemChild { child: Some(child) }))
    }
}

struct SystemChild {
    child: Option<std::process::Child>,
}

impl ChildHandle for SystemChild {
    fn pid(&self) -> u32 {
        self.child.as_ref().map(|child| child.id()).unwrap_or(0)
    }

    fn try_wait(&mut self) -> io::Result<Option<ExitReport>> {
        let Some(child) = self.child.as_mut() else {
            return Ok(None);
        };
        Ok(child.try_wait()?.map(|status| ExitReport { code: status.code() }))
    }

    fn take_stdout(&mut self) -> Option<Box<dyn Read + Send>> {
        self.child
            .as_mut()
            .and_then(|child| child.stdout.take())
            .map(|stream| Box::new(stream) as Box<dyn Read + Send>)
    }

    fn take_stderr(&mut self) -> Option<Box<dyn Read + Send>> {
        self.child
            .as_mut()
            .and_then(|child| child.stderr.take())
            .map(|stream| Box::new(stream) as Box<dyn Read + Send>)
    }

    fn terminate_tree(&mut self) -> TreeKillReport {
        let Some(child) = self.child.as_mut() else {
            return TreeKillReport {
                walker_exit_code: None,
                walker_visited: 0,
                walker_stderr_bytes: 0,
                walker_error: Some("the child handle was already consumed".to_owned()),
                direct_kill: false,
            };
        };
        let pid = child.id();
        let mut report = TreeKillReport {
            walker_exit_code: None,
            walker_visited: 0,
            walker_stderr_bytes: 0,
            walker_error: None,
            direct_kill: false,
        };
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // `Child::kill` only stops cmd.exe; descendants (a compiler, a
            // package manager) would survive invisibly. Same approach as
            // tool_host. The walker's status and output are captured rather than
            // discarded: a tree walk that silently failed must not be reported
            // as a completed teardown.
            // `output()` captures both streams for us; the walker's text is
            // localised, so only its shape is used below.
            match std::process::Command::new("taskkill")
                .args(["/PID", &pid.to_string(), "/T", "/F"])
                .creation_flags(0x0800_0000)
                .output()
            {
                Ok(output) => {
                    report.walker_exit_code = output.status.code();
                    // taskkill prints one line per process it acted on. Counting
                    // terminators works whatever the console code page is, so
                    // the count is evidence even though the text is not.
                    report.walker_visited = output
                        .stdout
                        .iter()
                        .chain(output.stderr.iter())
                        .filter(|byte| **byte == b'\n')
                        .count()
                        .min(u32::MAX as usize) as u32;
                    report.walker_stderr_bytes =
                        output.stderr.len().min(u32::MAX as usize) as u32;
                    if !output.status.success() {
                        report.walker_error = Some(match output.status.code() {
                            Some(code) => format!("taskkill exited with {code}"),
                            None => "taskkill was signalled".to_owned(),
                        });
                    }
                }
                Err(error) => {
                    report.walker_error = Some(format!("could not run taskkill: {error}"));
                }
            }
        }
        #[cfg(not(windows))]
        {
            // Best effort at the group level; the report says so instead of
            // claiming a tree walk that never happened.
            report.walker_error = Some("no tree walker on this platform".to_owned());
        }
        // Best-effort direct kill as well, so a failed tree walk still stops at
        // least the process we own.
        report.direct_kill = child.kill().is_ok();
        report
    }
}

// ---------------------------------------------------------------------------
// Events — makes the ordering of stop/finalize verifiable
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JobEventKind {
    Started,
    StopRequested(StopReason),
    TreeTerminated(TerminateOutcome),
    ReadersJoined,
    /// Readers did not finish within the grace window; output may be incomplete.
    ReadersOrphaned,
    Finished(JobRunState),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JobEvent {
    pub at_ms: i64,
    pub kind: JobEventKind,
}

// ---------------------------------------------------------------------------
// Views
// ---------------------------------------------------------------------------

/// Immutable snapshot for callers. Production maps this onto `JobSnapshot`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JobStatusView {
    pub job_id: String,
    pub run_id: String,
    pub idempotency_key: String,
    pub digest: String,
    pub state: JobRunState,
    pub exit_code: Option<i32>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    /// The Run-visible resolved command, for the receipt.
    pub program: String,
    pub args: Vec<String>,
    pub cwd: String,
    pub created_at: i64,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
    pub stop_requested: Option<StopReason>,
    pub cancel_requested_at: Option<i64>,
    pub cancel_acknowledged_at: Option<i64>,
    pub owner: Option<ProcessIdentity>,
    pub owner_status: Option<OwnerStatus>,
    /// What the teardown attempt reported, including whether the tree walker
    /// itself succeeded.
    pub terminate: Option<TerminateOutcome>,
    /// Ownership of the spawned pid, re-probed immediately **after** the
    /// teardown attempt. This is the difference between "we asked for a
    /// termination" and "the process is actually gone": `Some(Gone)` is
    /// reclamation evidence, `Some(Alive)` means the attempt did not take.
    pub owner_after_stop: Option<OwnerStatus>,
    pub readers_orphaned: bool,
    pub execution_budget_ms: u64,
    pub stdout: StreamView,
    pub stderr: StreamView,
    pub events: Vec<JobEvent>,
}

impl JobStatusView {
    pub fn is_terminal(&self) -> bool {
        self.state.is_terminal()
    }

    pub fn succeeded(&self) -> bool {
        self.state == JobRunState::Succeeded
    }

    /// Convenience for the wiring layer: what to persist as `JobState` plus any
    /// `error_code` owed by the mapping.
    pub fn production_state(&self) -> &'static str {
        self.state.production_state()
    }

    pub fn production_error_code(&self) -> Option<&'static str> {
        self.state.production_error_code()
    }
}

/// Result of a cancel request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CancelOutcome {
    pub view: JobStatusView,
    /// False when the request was a no-op (already stopping, or already terminal).
    pub changed: bool,
    /// True only once the terminal `cancelled` state has actually been written.
    pub acknowledged: bool,
}

/// What one supervision step did, for tests and logs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TickAction {
    StopRequested { job_id: String, reason: StopReason },
    TerminateAttempt { job_id: String, outcome: TerminateOutcome },
    Finalized { job_id: String, state: JobRunState },
    ReadersOrphaned { job_id: String },
}

// ---------------------------------------------------------------------------
// Manager
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct ManagerConfig {
    /// Per-stream retention cap. The dropped count is always reported.
    pub stream_retention_bytes: usize,
    pub default_execution_budget: Duration,
    /// Upper bound accepted from a caller, so one request cannot pin a job forever.
    pub max_execution_budget: Duration,
    /// How long to wait for output readers after the process exits, before
    /// declaring them orphaned. Producers may keep a pipe open via a descendant.
    pub reader_grace: Duration,
    pub max_jobs: usize,
}

impl Default for ManagerConfig {
    fn default() -> Self {
        Self {
            stream_retention_bytes: 256 * 1024,
            default_execution_budget: Duration::from_secs(60),
            max_execution_budget: Duration::from_secs(3600),
            reader_grace: Duration::from_millis(500),
            max_jobs: 512,
        }
    }
}

#[derive(Clone, Debug)]
struct KeyClaim {
    job_id: String,
    canonical: String,
}

struct ChildControl {
    child: Option<Box<dyn ChildHandle>>,
    stdout_reader: Option<JoinHandle<()>>,
    stderr_reader: Option<JoinHandle<()>>,
}

#[derive(Debug)]
struct JobStateCell {
    state: JobRunState,
    exit_code: Option<i32>,
    error_code: Option<String>,
    error_message: Option<String>,
    created_at: i64,
    started_at: Option<i64>,
    finished_at: Option<i64>,
    stop_requested: Option<StopReason>,
    stop_requested_at: Option<i64>,
    cancel_requested_at: Option<i64>,
    cancel_acknowledged_at: Option<i64>,
    owner: Option<ProcessIdentity>,
    owner_status: Option<OwnerStatus>,
    terminate: Option<TerminateOutcome>,
    owner_after_stop: Option<OwnerStatus>,
    readers_orphaned: bool,
    /// Set once the child has exited and we are waiting for reader threads.
    drain_deadline: Option<Instant>,
    /// True while teardown steps (terminate + drain) are still owed.
    teardown_started: bool,
    /// True from the moment the record becomes visible until `launch` has
    /// finished its single spawn attempt.
    ///
    /// Needed because "there is no child handle" has two very different
    /// meanings: *the launch has not produced one yet* and *there never will be
    /// one*. Treating the first as the second let a cancel be acknowledged
    /// before the process existed, after which the launch published `Running`
    /// and left a live, unowned tree behind. While this flag is set, teardown
    /// and finalization are withheld.
    launch_in_flight: bool,
}

struct JobEntry {
    id: String,
    run_id: String,
    idempotency_key: String,
    canonical: String,
    digest: String,
    spec: SpawnSpec,
    execution_budget: Duration,
    deadline: Mutex<Option<Instant>>,
    state: Mutex<JobStateCell>,
    /// Shared so the two reader threads can append without owning the entry.
    output: Arc<Mutex<OutputWindow>>,
    control: Mutex<ChildControl>,
    events: Mutex<Vec<JobEvent>>,
}

impl std::fmt::Debug for JobEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JobEntry").field("id", &self.id).finish_non_exhaustive()
    }
}

/// The lifecycle manager.
///
/// Advancing the state machine is explicit: [`ProcessJobManager::tick`].
/// `status`/`output`/`list` are pure reads and never call it, which is what makes
/// "polling cannot cause side effects" a structural property rather than a
/// promise. Production runs `tick` from a background thread via
/// [`ProcessJobManager::start_supervisor`]; tests call it directly so ordering
/// assertions are deterministic.
pub struct ProcessJobManager {
    clock: Arc<dyn Clock>,
    spawner: Arc<dyn ProcessSpawner>,
    probe: Arc<dyn IdentityProbe>,
    config: ManagerConfig,
    jobs: Mutex<BTreeMap<String, Arc<JobEntry>>>,
    keys: Mutex<BTreeMap<(String, String), KeyClaim>>,
    seq: AtomicU64,
}

impl ProcessJobManager {
    pub fn new(
        clock: Arc<dyn Clock>,
        spawner: Arc<dyn ProcessSpawner>,
        probe: Arc<dyn IdentityProbe>,
        config: ManagerConfig,
    ) -> Self {
        Self {
            clock,
            spawner,
            probe,
            config,
            jobs: Mutex::new(BTreeMap::new()),
            keys: Mutex::new(BTreeMap::new()),
            seq: AtomicU64::new(0),
        }
    }

    // -- start ---------------------------------------------------------------

    /// Claims and launches a job.
    ///
    /// The claim is inserted **before** spawning, so two concurrent starts with
    /// the same key cannot both launch: whichever reaches the map first owns the
    /// job and the other observes [`StartOutcome::Existing`].
    ///
    /// Two invariants make that observable rather than aspirational:
    ///
    /// 1. **The record is published whole.** An earlier version inserted the key
    ///    claim, released the `keys` lock, and only then inserted the job entry,
    ///    so a concurrent same-key start landing in that window found a claim it
    ///    could not resolve and returned `UnknownJob`. The key claim and the job
    ///    entry are now written together inside one `keys` critical section,
    ///    with the entry first: whoever sees the claim is guaranteed to see the
    ///    job.
    /// 2. **The clock is never consulted under a lock.** `now_ms()` is an
    ///    injected dependency and may be a test's pause point; calling it while
    ///    holding `keys` would turn such a pause into a deadlock. Exactly one
    ///    call is made before publication (for `created_at`/id), and the launch
    ///    below makes the next one, which is therefore always *after* the record
    ///    is visible.
    pub fn start(&self, request: StartRequest) -> Result<StartOutcome, JobError> {
        if request.run_id.trim().is_empty() {
            return Err(JobError::InvalidRequest("runId must not be empty".into()));
        }
        if request.idempotency_key.trim().is_empty() {
            return Err(JobError::InvalidRequest("idempotencyKey must not be empty".into()));
        }
        if request.spec.program.trim().is_empty() {
            return Err(JobError::InvalidRequest("program must not be empty".into()));
        }
        if request.spec.cwd.trim().is_empty() {
            return Err(JobError::InvalidRequest("cwd must not be empty".into()));
        }
        if request.execution_budget.is_zero() {
            return Err(JobError::InvalidRequest("execution budget must be greater than zero".into()));
        }
        if request.execution_budget > self.config.max_execution_budget {
            return Err(JobError::InvalidRequest(format!(
                "execution budget exceeds the {} s maximum",
                self.config.max_execution_budget.as_secs()
            )));
        }

        let canonical = request.canonical();
        let key = (request.run_id.clone(), request.idempotency_key.clone());

        // The only clock read before publication, and it happens off-lock.
        let created_at = self.clock.now_ms();
        let job_id = self.allocate_job_id(created_at);

        let entry = Arc::new(JobEntry {
            id: job_id.clone(),
            run_id: request.run_id.clone(),
            idempotency_key: request.idempotency_key.clone(),
            canonical: canonical.clone(),
            digest: request.digest(),
            spec: request.spec.clone(),
            execution_budget: request.execution_budget,
            deadline: Mutex::new(None),
            state: Mutex::new(JobStateCell {
                state: JobRunState::Queued,
                exit_code: None,
                error_code: None,
                error_message: None,
                created_at,
                started_at: None,
                finished_at: None,
                stop_requested: None,
                stop_requested_at: None,
                cancel_requested_at: None,
                cancel_acknowledged_at: None,
                owner: None,
                owner_status: None,
                terminate: None,
                owner_after_stop: None,
                readers_orphaned: false,
                drain_deadline: None,
                teardown_started: false,
                // Set from the moment the record is visible: between here and the
                // end of `launch` there is no child to terminate yet, and teardown
                // must not treat that absence as "already gone".
                launch_in_flight: true,
            }),
            output: Arc::new(Mutex::new(OutputWindow::new(self.config.stream_retention_bytes))),
            control: Mutex::new(ChildControl { child: None, stdout_reader: None, stderr_reader: None }),
            events: Mutex::new(Vec::new()),
        });

        // --- atomic claim + publication ---
        {
            let mut keys = lock(&self.keys);
            if let Some(claim) = keys.get(&key) {
                if claim.canonical != canonical {
                    return Err(JobError::IdempotencyConflict { job_id: claim.job_id.clone() });
                }
                let existing_id = claim.job_id.clone();
                drop(keys);
                // Guaranteed present: a claim is only ever written together with
                // its entry, below.
                let existing = self.entry(&existing_id)?;
                return Ok(StartOutcome::Existing(self.view(&existing)));
            }
            {
                let mut jobs = lock(&self.jobs);
                if jobs.len() >= self.config.max_jobs {
                    return Err(JobError::Capacity { max_jobs: self.config.max_jobs });
                }
                // Entry first, claim second: an observer holding `keys` can only
                // ever see a claim whose entry is already published.
                jobs.insert(job_id.clone(), Arc::clone(&entry));
            }
            keys.insert(
                key.clone(),
                KeyClaim { job_id: job_id.clone(), canonical },
            );
        }

        self.launch(&entry);
        Ok(StartOutcome::Launched(self.view(&entry)))
    }

    /// Builds a job id from an already-read timestamp.
    ///
    /// Takes the timestamp rather than reading the clock so that `start` makes
    /// exactly one clock call before anything becomes visible (see the
    /// invariants on [`ProcessJobManager::start`]).
    fn allocate_job_id(&self, now_ms: i64) -> String {
        let seq = self.seq.fetch_add(1, Ordering::SeqCst) + 1;
        format!("job-{:08x}-{:04x}", now_ms as u64 & 0xffff_ffff, seq & 0xffff)
    }

    /// Performs the single spawn attempt for a claimed job.
    ///
    /// A failure here is recorded as a terminal `failed` with
    /// `error_code = start_failed` **and the claim is kept**: a retry with the
    /// same key reports the failed job rather than silently launching a second
    /// process, so "one key = at most one launch" holds even across a failed
    /// start. A genuinely new attempt needs a new key.
    ///
    /// The spawn is the one step that can block for an unbounded time, and a
    /// stop can arrive while it does. Such a stop must not be treated as an
    /// already-finished job, and the process it eventually produces must not be
    /// published as `Running` — see the two branches below.
    fn launch(&self, entry: &Arc<JobEntry>) {
        let spawned = self.spawner.spawn(&entry.spec);
        match spawned {
            Err(error) => {
                let finished_at = self.clock.now_ms();
                let mut state = lock(&entry.state);
                state.launch_in_flight = false;
                state.error_code = Some("start_failed".to_owned());
                state.error_message = Some(format!("failed to start the process: {error}"));
                // Nothing was started, so a stop request has nothing to own: the
                // truthful terminal state is the failed start itself. Leaving it
                // non-terminal would strand a job that can never progress.
                state.state = JobRunState::Failed;
                state.finished_at = Some(finished_at);
                drop(state);
                self.record(entry, JobEventKind::Finished(JobRunState::Failed));
            }
            Ok(child) => {
                let pid = child.pid();
                let identity = ProcessIdentity {
                    pid,
                    start_marker: match self.probe.probe(pid) {
                        Liveness::Alive { start_marker } => start_marker,
                        // Unknown/gone right after spawn is not a reason to guess:
                        // keep None, which classify_owner treats as Unknown.
                        _ => None,
                    },
                };
                let mut control = lock(&entry.control);
                control.child = Some(child);
                let (stdout, stderr) = {
                    let child = control.child.as_mut().expect("child just stored");
                    (child.take_stdout(), child.take_stderr())
                };
                if let Some(source) = stdout {
                    let output = Arc::clone(&entry.output);
                    control.stdout_reader = Some(spawn_reader(source, Stream::Stdout, output));
                }
                if let Some(source) = stderr {
                    let output = Arc::clone(&entry.output);
                    control.stderr_reader = Some(spawn_reader(source, Stream::Stderr, output));
                }
                drop(control);

                // Off-lock, and deliberately the next clock read after
                // publication: a caller that pauses here must find a complete
                // record, not a half-written one.
                let started_at = self.clock.now_ms();
                *lock(&entry.deadline) = Some(Instant::now() + entry.execution_budget);
                let mut state = lock(&entry.state);
                state.owner = Some(identity);
                state.started_at = Some(started_at);
                state.launch_in_flight = false;
                let stopping = state.stop_requested.is_some();
                if !stopping {
                    state.state = JobRunState::Running;
                }
                drop(state);
                if stopping {
                    // A stop was requested while we were starting. The child is
                    // real and must be reclaimed, but the job never gets to be
                    // `Running`: publishing that state after an acknowledged
                    // cancel is exactly the revival this guards against. Hand
                    // over to the single finalizer, which now has a child to
                    // terminate and will write the terminal state.
                    self.supervise(entry);
                    return;
                }
                self.record(entry, JobEventKind::Started);
            }
        }
    }

    // -- pure reads ----------------------------------------------------------

    pub fn status(&self, job_id: &str) -> Result<JobStatusView, JobError> {
        let entry = self.entry(job_id)?;
        Ok(self.view(&entry))
    }

    pub fn output(
        &self,
        job_id: &str,
        stream: Stream,
        offset: u64,
        limit: usize,
    ) -> Result<OutputPage, JobError> {
        let entry = self.entry(job_id)?;
        let mut page = lock(&entry.output).page(stream, offset, limit);
        page.stream_closed = self.stream_closed(&entry, stream);
        Ok(page)
    }

    /// Whether the producing pipe for `stream` has ended.
    ///
    /// A pure read: it inspects reader-thread completion and never joins or
    /// advances anything. A missing reader means either the child had no such
    /// pipe or it was detached after the grace window — closed either way.
    fn stream_closed(&self, entry: &Arc<JobEntry>, stream: Stream) -> bool {
        let control = lock(&entry.control);
        let handle = match stream {
            Stream::Stdout => control.stdout_reader.as_ref(),
            Stream::Stderr => control.stderr_reader.as_ref(),
        };
        handle.is_none_or(|handle| handle.is_finished())
    }

    /// All jobs, optionally filtered to one Run. Ordered by jobId so evidence is
    /// reproducible.
    pub fn list(&self, run_id: Option<&str>) -> Vec<JobStatusView> {
        let jobs = lock(&self.jobs);
        jobs.values()
            .filter(|entry| run_id.is_none_or(|want| entry.run_id == want))
            .map(|entry| self.view(entry))
            .collect()
    }

    // -- cancel --------------------------------------------------------------

    /// Requests cancellation and immediately performs one supervision step so a
    /// cancellation is not delayed to the next tick.
    ///
    /// Idempotent: a second call while stopping, or any call after a terminal
    /// state, changes nothing and reports `changed = false`.
    pub fn cancel(&self, job_id: &str) -> Result<CancelOutcome, JobError> {
        let entry = self.entry(job_id)?;
        let changed = {
            let mut state = lock(&entry.state);
            if state.state.is_terminal() || state.stop_requested.is_some() {
                false
            } else {
                let now = self.clock.now_ms();
                state.stop_requested = Some(StopReason::Cancelled);
                state.stop_requested_at = Some(now);
                state.cancel_requested_at = Some(now);
                true
            }
        };
        if changed {
            self.record(&entry, JobEventKind::StopRequested(StopReason::Cancelled));
        }
        self.supervise(&entry);
        let view = self.view(&entry);
        let acknowledged = view.state == JobRunState::Cancelled;
        Ok(CancelOutcome { view, changed, acknowledged })
    }

    // -- supervision ---------------------------------------------------------

    /// Advances every non-terminal job by one step.
    pub fn tick(&self) -> Vec<TickAction> {
        let entries: Vec<Arc<JobEntry>> = lock(&self.jobs).values().cloned().collect();
        let mut actions = Vec::new();
        for entry in entries {
            actions.extend(self.supervise(&entry));
        }
        actions
    }

    /// Spawns the background supervisor. Returns a handle that stops it.
    pub fn start_supervisor(self: &Arc<Self>, interval: Duration) -> SupervisorHandle {
        let manager = Arc::clone(self);
        let stop = Arc::new(AtomicU64::new(0));
        let stop_flag = Arc::clone(&stop);
        let handle = std::thread::spawn(move || {
            while stop_flag.load(Ordering::SeqCst) == 0 {
                manager.tick();
                std::thread::sleep(interval);
            }
        });
        SupervisorHandle { stop, handle: Some(handle) }
    }

    fn supervise(&self, entry: &Arc<JobEntry>) -> Vec<TickAction> {
        let mut actions = Vec::new();

        // 1. decide whether a stop is owed
        let budget_expired = {
            let mut state = lock(&entry.state);
            if state.state.is_terminal() {
                return actions;
            }
            let mut expired = false;
            if state.stop_requested.is_none() {
                expired = lock(&entry.deadline)
                    .map(|deadline| Instant::now() >= deadline)
                    .unwrap_or(false);
                if expired {
                    // Off-lock clock read: the deadline is only ever armed once
                    // the launch has produced a child, so a job that is still
                    // starting cannot expire here.
                    let now = self.clock.now_ms();
                    state.stop_requested = Some(StopReason::Timeout);
                    state.stop_requested_at = Some(now);
                }
            }
            expired
        };
        if budget_expired {
            self.record(entry, JobEventKind::StopRequested(StopReason::Timeout));
            actions.push(TickAction::StopRequested {
                job_id: entry.id.clone(),
                reason: StopReason::Timeout,
            });
        }

        // 2. stop the tree, guarded by process identity
        let owed_terminate = {
            let state = lock(&entry.state);
            // While the launch is in flight there is no child *yet*. Reading that
            // absence as "already exited" is what allowed a cancel to be
            // acknowledged before the process existed, and it would also burn the
            // one teardown attempt on a job whose real child is about to appear.
            // Withhold teardown — and finalization, below — until `launch` has
            // recorded its result.
            !state.launch_in_flight && state.stop_requested.is_some() && !state.teardown_started
        };
        if owed_terminate {
            let outcome = self.terminate_guarded(entry);
            // Re-probe right after the attempt: "we asked for a termination" and
            // "the process is gone" are different claims, and only the second one
            // is reclamation evidence.
            let reclaimed = {
                let state = lock(&entry.state);
                state
                    .owner
                    .map(|owner| classify_owner(&owner, &self.probe.probe(owner.pid)))
            };
            let mut state = lock(&entry.state);
            state.teardown_started = true;
            state.terminate = Some(outcome.clone());
            state.owner_after_stop = reclaimed;
            drop(state);
            self.record(entry, JobEventKind::TreeTerminated(outcome.clone()));
            actions.push(TickAction::TerminateAttempt {
                job_id: entry.id.clone(),
                outcome,
            });
        }

        // 3. observe exit
        let mut exited = false;
        {
            let mut control = lock(&entry.control);
            if let Some(child) = control.child.as_mut() {
                match child.try_wait() {
                    Ok(Some(report)) => {
                        let mut state = lock(&entry.state);
                        if state.exit_code.is_none() {
                            state.exit_code = report.code;
                        }
                        exited = true;
                    }
                    Ok(None) => {}
                    Err(error) => {
                        let mut state = lock(&entry.state);
                        state.error_message = Some(format!("failed to wait for the process: {error}"));
                        exited = true;
                    }
                }
            } else {
                // No child at all: either the start failed (terminal already) or
                // the launch has not finished. Only the latter is withheld.
                let state = lock(&entry.state);
                exited = !state.launch_in_flight;
            }
        }

        if !exited {
            return actions;
        }

        // A child exists but `launch` has not yet recorded its outcome. It is the
        // only writer allowed to move a job out of `queued`, so finalizing here
        // would let it publish `running` on top of a terminal state afterwards.
        if lock(&entry.state).launch_in_flight {
            return actions;
        }

        // 4. wait for the output readers, bounded
        {
            let mut control = lock(&entry.control);
            let stdout_done = control
                .stdout_reader
                .as_ref()
                .is_none_or(|handle| handle.is_finished());
            let stderr_done = control
                .stderr_reader
                .as_ref()
                .is_none_or(|handle| handle.is_finished());
            if stdout_done && stderr_done {
                // Finished: joining now cannot block.
                if let Some(handle) = control.stdout_reader.take() {
                    let _ = handle.join();
                }
                if let Some(handle) = control.stderr_reader.take() {
                    let _ = handle.join();
                }
                self.record(entry, JobEventKind::ReadersJoined);
            } else {
                let mut state = lock(&entry.state);
                let deadline = *state.drain_deadline.get_or_insert_with(|| Instant::now() + self.config.reader_grace);
                if Instant::now() < deadline {
                    return actions;
                }
                // A descendant may still hold the pipe open. Do not block on a
                // join that may never return: detach and report incomplete output.
                state.readers_orphaned = true;
                drop(state);
                control.stdout_reader = None;
                control.stderr_reader = None;
                self.record(entry, JobEventKind::ReadersOrphaned);
                actions.push(TickAction::ReadersOrphaned { job_id: entry.id.clone() });
            }
        }

        // 5. write the terminal state last
        let now = self.clock.now_ms();
        let final_state = {
            let mut state = lock(&entry.state);
            if state.state.is_terminal() {
                return actions;
            }
            let terminal = match state.stop_requested {
                // A stop that we requested owns the outcome: the exit code of a
                // process we killed is not a task result.
                Some(StopReason::Cancelled) => {
                    state.cancel_acknowledged_at = Some(now);
                    JobRunState::Cancelled
                }
                Some(StopReason::Timeout) => JobRunState::TimedOut,
                None => match state.exit_code {
                    Some(0) => JobRunState::Succeeded,
                    Some(_) => {
                        state.error_code = Some("nonzero_exit".to_owned());
                        JobRunState::Failed
                    }
                    None => {
                        state.error_code = Some("signalled".to_owned());
                        JobRunState::Failed
                    }
                },
            };
            if terminal == JobRunState::TimedOut {
                state.error_code = Some("timed_out".to_owned());
            }
            state.state = terminal;
            state.finished_at = Some(now);
            terminal
        };
        self.record(entry, JobEventKind::Finished(final_state));
        actions.push(TickAction::Finalized { job_id: entry.id.clone(), state: final_state });
        actions
    }

    /// Terminates the process tree only if the pid still resolves to the process
    /// instance we spawned.
    ///
    /// Without this guard a `taskkill /T /F` could hit an unrelated program that
    /// inherited a reused pid. A residual race remains between the probe and the
    /// kill; it is bounded by the fact that we only ever target a pid we spawned
    /// and that we re-verify creation time first.
    fn terminate_guarded(&self, entry: &Arc<JobEntry>) -> TerminateOutcome {
        let mut control = lock(&entry.control);
        let Some(child) = control.child.as_mut() else {
            return TerminateOutcome::AlreadyExited;
        };
        match child.try_wait() {
            Ok(Some(_)) => return TerminateOutcome::AlreadyExited,
            Ok(None) => {}
            Err(error) => {
                return TerminateOutcome::Incomplete {
                    walker_exit_code: None,
                    walker_visited: 0,
                    detail: format!("could not observe the child before terminating it: {error}"),
                };
            }
        }
        let pid = child.pid();
        let expected = {
            let state = lock(&entry.state);
            state.owner
        };
        if let Some(expected) = expected {
            match classify_owner(&expected, &self.probe.probe(pid)) {
                OwnerStatus::Alive => {}
                // Not our instance any more: leave it alone.
                OwnerStatus::Replaced | OwnerStatus::Gone => {
                    return TerminateOutcome::SkippedIdentityMismatch;
                }
                // Cannot prove it is ours. Killing on a guess is worse than not
                // killing; the reader grace and the terminal state still land.
                OwnerStatus::Unknown => return TerminateOutcome::SkippedIdentityMismatch,
            }
        }
        let report = child.terminate_tree();
        if report.walker_succeeded() {
            TerminateOutcome::Terminated {
                walker_exit_code: report.walker_exit_code,
                walker_visited: report.walker_visited,
            }
        } else {
            // The fallback kill may still have stopped the direct child, but the
            // tree as a whole is unproven. Reporting `Terminated` here is what
            // let a failed walk look like a completed teardown.
            let detail = report.walker_error.unwrap_or_else(|| {
                format!(
                    "the tree walker reported no success (exit {:?}, {} process line(s), {} byte(s) of diagnostics)",
                    report.walker_exit_code, report.walker_visited, report.walker_stderr_bytes
                )
            });
            TerminateOutcome::Incomplete {
                walker_exit_code: report.walker_exit_code,
                walker_visited: report.walker_visited,
                detail,
            }
        }
    }

    // -- recovery ------------------------------------------------------------

    /// Classifies persisted jobs after a restart.
    ///
    /// Pure: it mutates nothing and launches nothing. The caller decides what to
    /// persist. A `running` row is **never** taken at face value — ownership is
    /// re-probed, and a pid that now belongs to someone else is `replaced`, not
    /// ours.
    pub fn recover(&self, records: &[PersistedJobRecord]) -> Vec<RecoveryDecision> {
        records
            .iter()
            .map(|record| {
                if record.state.is_terminal() {
                    return RecoveryDecision {
                        job_id: record.job_id.clone(),
                        from: record.state,
                        to: record.state,
                        reason: RecoveryReason::Terminal,
                        owner_status: OwnerStatus::Unknown,
                        needs_review: false,
                        allow_replay: false,
                    };
                }
                let (owner_status, reason) = match record.owner {
                    None => (OwnerStatus::Unknown, RecoveryReason::NoOwnerRecorded),
                    Some(owner) => {
                        let status = classify_owner(&owner, &self.probe.probe(owner.pid));
                        let reason = match status {
                            OwnerStatus::Alive => RecoveryReason::OwnerAlive,
                            OwnerStatus::Replaced => RecoveryReason::OwnerReplaced,
                            OwnerStatus::Gone => RecoveryReason::OwnerGone,
                            OwnerStatus::Unknown => RecoveryReason::OwnerUnknown,
                        };
                        (status, reason)
                    }
                };
                if reason == RecoveryReason::OwnerAlive {
                    return RecoveryDecision {
                        job_id: record.job_id.clone(),
                        from: record.state,
                        to: JobRunState::Running,
                        reason,
                        owner_status,
                        needs_review: false,
                        allow_replay: false,
                    };
                }
                RecoveryDecision {
                    job_id: record.job_id.clone(),
                    from: record.state,
                    to: JobRunState::Interrupted,
                    reason,
                    owner_status,
                    // Only an unprovable owner needs a human look; a confirmed
                    // dead/replaced one is a definite interruption.
                    needs_review: matches!(
                        reason,
                        RecoveryReason::OwnerUnknown | RecoveryReason::NoOwnerRecorded
                    ),
                    // Never true: a long command may already have had effects.
                    allow_replay: false,
                }
            })
            .collect()
    }

    // -- internals -----------------------------------------------------------

    fn entry(&self, job_id: &str) -> Result<Arc<JobEntry>, JobError> {
        lock(&self.jobs)
            .get(job_id)
            .cloned()
            .ok_or_else(|| JobError::UnknownJob(job_id.to_owned()))
    }

    fn view(&self, entry: &Arc<JobEntry>) -> JobStatusView {
        let state = lock(&entry.state);
        let output = lock(&entry.output);
        let events = lock(&entry.events).clone();
        let owner_status = state.owner_status;
        JobStatusView {
            job_id: entry.id.clone(),
            run_id: entry.run_id.clone(),
            idempotency_key: entry.idempotency_key.clone(),
            digest: entry.digest.clone(),
            state: state.state,
            exit_code: state.exit_code,
            error_code: state.error_code.clone(),
            error_message: state.error_message.clone(),
            program: entry.spec.program.clone(),
            args: entry.spec.args.clone(),
            cwd: entry.spec.cwd.clone(),
            created_at: state.created_at,
            started_at: state.started_at,
            finished_at: state.finished_at,
            stop_requested: state.stop_requested,
            cancel_requested_at: state.cancel_requested_at,
            cancel_acknowledged_at: state.cancel_acknowledged_at,
            owner: state.owner,
            owner_status: owner_status.or(state.owner.map(|_| OwnerStatus::Alive)),
            terminate: state.terminate.clone(),
            owner_after_stop: state.owner_after_stop,
            readers_orphaned: state.readers_orphaned,
            execution_budget_ms: entry.execution_budget.as_millis().min(u64::MAX as u128) as u64,
            stdout: output.view(Stream::Stdout),
            stderr: output.view(Stream::Stderr),
            events,
        }
    }

    fn record(&self, entry: &Arc<JobEntry>, kind: JobEventKind) {
        let at_ms = self.clock.now_ms();
        lock(&entry.events).push(JobEvent { at_ms, kind });
    }
}

/// Stops the background supervisor.
pub struct SupervisorHandle {
    stop: Arc<AtomicU64>,
    handle: Option<JoinHandle<()>>,
}

impl SupervisorHandle {
    pub fn stop(mut self) {
        self.stop.store(1, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for SupervisorHandle {
    fn drop(&mut self) {
        self.stop.store(1, Ordering::SeqCst);
    }
}

fn spawn_reader(
    mut source: Box<dyn Read + Send>,
    stream: Stream,
    output: Arc<Mutex<OutputWindow>>,
) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let mut buffer = [0u8; 8 * 1024];
        loop {
            match source.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => {
                    let mut window = lock(&output);
                    window.push(stream, &buffer[..read]);
                }
                Err(ref error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }
    })
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    // A panic in a supervisor step must not poison the whole lifecycle: the
    // remaining data is still structurally valid.
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

// ---------------------------------------------------------------------------
// Recovery vocabulary
// ---------------------------------------------------------------------------

/// A job as it exists in durable storage. This module does not read the database;
/// the wiring layer maps `JobSnapshot` onto this.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PersistedJobRecord {
    pub job_id: String,
    pub run_id: String,
    pub state: JobRunState,
    pub owner: Option<ProcessIdentity>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecoveryReason {
    Terminal,
    OwnerAlive,
    OwnerGone,
    OwnerReplaced,
    OwnerUnknown,
    NoOwnerRecorded,
}

impl RecoveryReason {
    pub fn as_str(self) -> &'static str {
        match self {
            RecoveryReason::Terminal => "terminal",
            RecoveryReason::OwnerAlive => "owner_alive",
            RecoveryReason::OwnerGone => "owner_gone",
            RecoveryReason::OwnerReplaced => "owner_replaced",
            RecoveryReason::OwnerUnknown => "owner_unknown",
            RecoveryReason::NoOwnerRecorded => "no_owner_recorded",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecoveryDecision {
    pub job_id: String,
    pub from: JobRunState,
    pub to: JobRunState,
    pub reason: RecoveryReason,
    pub owner_status: OwnerStatus,
    /// True when the evidence is insufficient and a human/verifier should look.
    pub needs_review: bool,
    /// Always false. Kept as an explicit field so the wiring layer cannot
    /// "helpfully" replay a side-effecting command during recovery.
    pub allow_replay: bool,
}

impl RecoveryDecision {
    pub fn unchanged(&self) -> bool {
        self.from == self.to
    }

    /// `error_code` to persist when the decision interrupts a job.
    pub fn error_code(&self) -> Option<&'static str> {
        if self.to != JobRunState::Interrupted {
            return None;
        }
        Some(if self.needs_review { "interrupted_unknown" } else { "interrupted" })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::sync::mpsc;

    // -- fakes ---------------------------------------------------------------

    /// Clock that advances only when a test moves it.
    #[derive(Default)]
    struct ManualClock {
        now: AtomicU64,
    }

    impl ManualClock {
        fn advance(&self, ms: u64) {
            self.now.fetch_add(ms, Ordering::SeqCst);
        }
    }

    impl Clock for ManualClock {
        fn now_ms(&self) -> i64 {
            self.now.load(Ordering::SeqCst) as i64
        }
    }

    /// Probe with a table of answers, so every owner classification is reachable.
    struct ScriptedProbe {
        answers: Mutex<BTreeMap<u32, Liveness>>,
        current: ProcessIdentity,
    }

    impl ScriptedProbe {
        fn new(answers: Vec<(u32, Liveness)>) -> Self {
            Self {
                answers: Mutex::new(answers.into_iter().collect()),
                current: ProcessIdentity { pid: 4242, start_marker: Some(7) },
            }
        }
        fn set(&self, pid: u32, liveness: Liveness) {
            lock(&self.answers).insert(pid, liveness);
        }
    }

    impl IdentityProbe for ScriptedProbe {
        fn current(&self) -> ProcessIdentity {
            self.current
        }
        fn probe(&self, pid: u32) -> Liveness {
            lock(&self.answers)
                .get(&pid)
                .cloned()
                .unwrap_or(Liveness::Unknown("no answer scripted".into()))
        }
    }

    /// A child whose lifecycle is decided by the test.
    struct ScriptedChild {
        pid: u32,
        remain_ticks: usize,
        exit: ExitReport,
        stdout: Option<Vec<u8>>,
        stderr: Option<Vec<u8>>,
        terminated: Arc<AtomicUsize>,
        /// Makes the tree walker report failure, so `Incomplete` is reachable
        /// without a real OS.
        walker_fails: bool,
    }

    impl ChildHandle for ScriptedChild {
        fn pid(&self) -> u32 {
            self.pid
        }
        fn try_wait(&mut self) -> io::Result<Option<ExitReport>> {
            if self.terminated.load(Ordering::SeqCst) > 0 {
                return Ok(Some(ExitReport { code: None }));
            }
            if self.remain_ticks == 0 {
                return Ok(Some(self.exit));
            }
            self.remain_ticks -= 1;
            Ok(None)
        }
        fn take_stdout(&mut self) -> Option<Box<dyn Read + Send>> {
            self.stdout.take().map(|bytes| Box::new(io::Cursor::new(bytes)) as Box<dyn Read + Send>)
        }
        fn take_stderr(&mut self) -> Option<Box<dyn Read + Send>> {
            self.stderr.take().map(|bytes| Box::new(io::Cursor::new(bytes)) as Box<dyn Read + Send>)
        }
        fn terminate_tree(&mut self) -> TreeKillReport {
            self.terminated.fetch_add(1, Ordering::SeqCst);
            if self.walker_fails {
                TreeKillReport {
                    walker_exit_code: Some(128),
                    walker_visited: 0,
                    walker_stderr_bytes: 12,
                    walker_error: Some("taskkill exited with 128".to_owned()),
                    direct_kill: true,
                }
            } else {
                TreeKillReport {
                    walker_exit_code: Some(0),
                    walker_visited: 3,
                    walker_stderr_bytes: 0,
                    walker_error: None,
                    direct_kill: true,
                }
            }
        }
    }

    struct ScriptedSpawner {
        plan: Mutex<Vec<ScriptedPlan>>,
        spawns: AtomicUsize,
        terminates: Arc<AtomicUsize>,
        /// Makes every child's tree walker report failure.
        walker_fails: std::sync::atomic::AtomicBool,
    }

    struct ScriptedPlan {
        pid: u32,
        remain_ticks: usize,
        exit_code: Option<i32>,
        stdout: Vec<u8>,
        stderr: Vec<u8>,
        fail: bool,
    }

    impl ScriptedSpawner {
        fn new(plans: Vec<ScriptedPlan>) -> Arc<Self> {
            Arc::new(Self {
                plan: Mutex::new(plans),
                spawns: AtomicUsize::new(0),
                terminates: Arc::new(AtomicUsize::new(0)),
                walker_fails: std::sync::atomic::AtomicBool::new(false),
            })
        }
        /// Makes the next (and every) tree walker report failure.
        fn fail_tree_walker(&self) {
            self.walker_fails.store(true, Ordering::SeqCst);
        }
        fn spawn_count(&self) -> usize {
            self.spawns.load(Ordering::SeqCst)
        }
        fn terminate_count(&self) -> usize {
            self.terminates.load(Ordering::SeqCst)
        }
    }

    impl ProcessSpawner for ScriptedSpawner {
        fn spawn(&self, _spec: &SpawnSpec) -> io::Result<Box<dyn ChildHandle>> {
            self.spawns.fetch_add(1, Ordering::SeqCst);
            let mut plan = lock(&self.plan);
            if plan.is_empty() {
                return Err(io::Error::new(io::ErrorKind::Other, "no plan left"));
            }
            let plan = plan.remove(0);
            if plan.fail {
                return Err(io::Error::new(io::ErrorKind::NotFound, "scripted spawn failure"));
            }
            Ok(Box::new(ScriptedChild {
                pid: plan.pid,
                remain_ticks: plan.remain_ticks,
                exit: ExitReport { code: plan.exit_code },
                stdout: Some(plan.stdout),
                stderr: Some(plan.stderr),
                terminated: Arc::clone(&self.terminates),
                walker_fails: self.walker_fails.load(Ordering::SeqCst),
            }))
        }
    }

    fn scripted_plan(pid: u32, stdout: &str) -> ScriptedPlan {
        ScriptedPlan {
            pid,
            remain_ticks: 1,
            exit_code: Some(0),
            stdout: stdout.as_bytes().to_vec(),
            stderr: Vec::new(),
            fail: false,
        }
    }

    // -- fakes for the concurrency regressions -------------------------------
    //
    // These two races were reported against this module and are pinned here as
    // permanent regression tests. They need a clock and a spawner that can be
    // *parked* at an exact point, which is why they do not reuse the scripted
    // fakes above.

    /// Clock that parks on its `pause_at`-th read.
    ///
    /// Parking a clock is how these tests hold a manager mid-operation without
    /// sleeping and hoping. It only works because the manager never reads the
    /// clock while holding one of its own locks — if it did, this fake would
    /// deadlock instead of reporting anything.
    struct ParkingClock {
        calls: AtomicUsize,
        pause_at: usize,
        entered: mpsc::Sender<()>,
        release: Mutex<mpsc::Receiver<()>>,
    }

    impl ParkingClock {
        fn new(pause_at: usize) -> (Arc<Self>, mpsc::Receiver<()>, mpsc::Sender<()>) {
            let (entered_tx, entered_rx) = mpsc::channel();
            let (release_tx, release_rx) = mpsc::channel();
            let clock = Arc::new(Self {
                calls: AtomicUsize::new(0),
                pause_at,
                entered: entered_tx,
                release: Mutex::new(release_rx),
            });
            (clock, entered_rx, release_tx)
        }
    }

    impl Clock for ParkingClock {
        fn now_ms(&self) -> i64 {
            if self.calls.fetch_add(1, Ordering::SeqCst) == self.pause_at {
                self.entered.send(()).expect("the test is still listening");
                self.release
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(5))
                    .expect("the test must release the clock");
            }
            1_000
        }
    }

    /// A child that stays running until it is terminated, then reports gone.
    struct ParkingChild {
        pid: u32,
        terminated: Arc<AtomicUsize>,
        probe: Arc<ScriptedProbe>,
    }

    impl ChildHandle for ParkingChild {
        fn pid(&self) -> u32 {
            self.pid
        }
        fn try_wait(&mut self) -> io::Result<Option<ExitReport>> {
            if self.terminated.load(Ordering::SeqCst) > 0 {
                return Ok(Some(ExitReport { code: None }));
            }
            Ok(None)
        }
        fn take_stdout(&mut self) -> Option<Box<dyn Read + Send>> {
            None
        }
        fn take_stderr(&mut self) -> Option<Box<dyn Read + Send>> {
            None
        }
        fn terminate_tree(&mut self) -> TreeKillReport {
            self.terminated.fetch_add(1, Ordering::SeqCst);
            // The process really is gone afterwards, so the re-probe that
            // follows the teardown has something true to report.
            self.probe.set(self.pid, Liveness::Gone);
            TreeKillReport {
                walker_exit_code: Some(0),
                walker_visited: 2,
                walker_stderr_bytes: 0,
                walker_error: None,
                direct_kill: true,
            }
        }
    }

    /// Spawner that parks inside `spawn` until the test releases it.
    struct ParkingSpawner {
        pid: u32,
        entered: mpsc::Sender<()>,
        release: Mutex<mpsc::Receiver<()>>,
        spawns: AtomicUsize,
        terminates: Arc<AtomicUsize>,
        probe: Arc<ScriptedProbe>,
    }

    impl ProcessSpawner for ParkingSpawner {
        fn spawn(&self, _spec: &SpawnSpec) -> io::Result<Box<dyn ChildHandle>> {
            self.spawns.fetch_add(1, Ordering::SeqCst);
            self.entered.send(()).expect("the test is still listening");
            self.release
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(5))
                .expect("the test must release the spawn");
            Ok(Box::new(ParkingChild {
                pid: self.pid,
                terminated: Arc::clone(&self.terminates),
                probe: Arc::clone(&self.probe),
            }))
        }
    }

    /// Ticks until terminal (or panics), for tests whose finalization is gated on
    /// reader threads rather than on the clock.
    fn wait_terminal(manager: &ProcessJobManager, job_id: &str) -> JobStatusView {
        for _ in 0..100 {
            let view = manager.status(job_id).expect("job exists");
            if view.state.is_terminal() {
                return view;
            }
            manager.tick();
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("job {job_id} never reached a terminal state");
    }

    fn manager(
        spawner: Arc<ScriptedSpawner>,
        probe: Arc<ScriptedProbe>,
        clock: Arc<ManualClock>,
        config: ManagerConfig,
    ) -> ProcessJobManager {
        ProcessJobManager::new(clock, spawner, probe, config)
    }

    fn request(key: &str, command: &str) -> StartRequest {
        StartRequest {
            run_id: "run-1".into(),
            idempotency_key: key.into(),
            spec: SpawnSpec {
                program: "cmd.exe".into(),
                args: vec!["/C".into(), command.into()],
                cwd: "C:/project".into(),
                env: EnvMode::Inherit,
            },
            execution_budget: Duration::from_secs(30),
        }
    }

    /// Drives supervision until the job reaches a terminal state.
    ///
    /// A job deliberately does **not** become terminal in the same step that
    /// reaps the process: the output readers are drained first, so the terminal
    /// state always reflects complete output. Tests therefore tick until
    /// terminal rather than assuming a fixed number of steps.
    fn run_to_terminal(manager: &ProcessJobManager, job_id: &str) -> JobStatusView {
        for _ in 0..300 {
            let view = manager.status(job_id).unwrap();
            if view.is_terminal() {
                return view;
            }
            manager.tick();
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("job {job_id} never reached a terminal state");
    }

    // -- canonical / digest --------------------------------------------------

    #[test]
    fn canonical_folds_trailing_separators_and_is_stable() {
        let mut a = request("k", "echo hi");
        a.spec.cwd = "C:/project/".into();
        let mut b = request("k", "echo hi");
        b.spec.cwd = "C:/project".into();
        assert_eq!(a.canonical(), b.canonical());
        assert_eq!(a.digest(), b.digest());
    }

    #[test]
    fn canonical_distinguishes_every_decision_field() {
        let base = request("k", "echo hi");
        let mut other_key = base.clone();
        other_key.idempotency_key = "k2".into();
        let mut other_run = base.clone();
        other_run.run_id = "run-2".into();
        let mut other_cmd = base.clone();
        other_cmd.spec.args = vec!["/C".into(), "echo bye".into()];
        let mut other_budget = base.clone();
        other_budget.execution_budget = Duration::from_secs(31);
        let mut other_env = base.clone();
        other_env.spec.env = EnvMode::Replace(vec![("A".into(), "1".into())]);

        let baseline = base.canonical();
        for variant in [other_key, other_run, other_cmd, other_budget, other_env] {
            assert_ne!(baseline, variant.canonical());
        }
    }

    #[test]
    fn canonical_cannot_be_forged_by_field_boundaries() {
        // "a" + arg "bc" must not collide with "ab" + arg "c".
        let mut one = request("k", "x");
        one.spec.args = vec!["a".into(), "bc".into()];
        let mut two = request("k", "x");
        two.spec.args = vec!["ab".into(), "c".into()];
        assert_ne!(one.canonical(), two.canonical());
    }

    // -- identity ------------------------------------------------------------

    #[test]
    fn owner_classification_never_guesses() {
        let expected = ProcessIdentity { pid: 10, start_marker: Some(5) };
        assert_eq!(
            classify_owner(&expected, &Liveness::Alive { start_marker: Some(5) }),
            OwnerStatus::Alive
        );
        assert_eq!(
            classify_owner(&expected, &Liveness::Alive { start_marker: Some(6) }),
            OwnerStatus::Replaced
        );
        assert_eq!(classify_owner(&expected, &Liveness::Gone), OwnerStatus::Gone);
        assert_eq!(
            classify_owner(&expected, &Liveness::Unknown("denied".into())),
            OwnerStatus::Unknown
        );
        // A missing marker on either side must not become Alive.
        assert_eq!(
            classify_owner(&expected, &Liveness::Alive { start_marker: None }),
            OwnerStatus::Unknown
        );
        assert_eq!(
            classify_owner(
                &ProcessIdentity { pid: 10, start_marker: None },
                &Liveness::Alive { start_marker: Some(5) }
            ),
            OwnerStatus::Unknown
        );
    }

    // -- idempotency ---------------------------------------------------------

    #[test]
    fn same_key_same_request_returns_existing_and_does_not_relaunch() {
        let spawner = ScriptedSpawner::new(vec![scripted_plan(100, "one")]);
        let manager = manager(
            Arc::clone(&spawner),
            Arc::new(ScriptedProbe::new(vec![])),
            Arc::new(ManualClock::default()),
            ManagerConfig::default(),
        );

        let first = manager.start(request("k1", "echo hi")).unwrap();
        assert!(first.launched());
        let jid = first.view().job_id.clone();

        let second = manager.start(request("k1", "echo hi")).unwrap();
        assert!(!second.launched(), "a repeat must not be a new launch");
        assert_eq!(second.view().job_id, jid);
        assert_eq!(spawner.spawn_count(), 1, "exactly one spawn for one key");
    }

    #[test]
    fn same_key_different_request_is_a_conflict_not_a_second_launch() {
        let spawner = ScriptedSpawner::new(vec![scripted_plan(100, "one"), scripted_plan(101, "two")]);
        let manager = manager(
            Arc::clone(&spawner),
            Arc::new(ScriptedProbe::new(vec![])),
            Arc::new(ManualClock::default()),
            ManagerConfig::default(),
        );
        manager.start(request("k1", "echo one")).unwrap();
        let error = manager.start(request("k1", "echo two")).unwrap_err();
        assert_eq!(error.code(), "job.idempotency_conflict");
        assert_eq!(spawner.spawn_count(), 1);
    }

    #[test]
    fn concurrent_duplicate_starts_launch_once() {
        let spawner = ScriptedSpawner::new(vec![scripted_plan(100, "one")]);
        let manager = Arc::new(manager(
            Arc::clone(&spawner),
            Arc::new(ScriptedProbe::new(vec![])),
            Arc::new(ManualClock::default()),
            ManagerConfig::default(),
        ));

        let mut handles = Vec::new();
        for _ in 0..8 {
            let manager = Arc::clone(&manager);
            handles.push(std::thread::spawn(move || {
                manager.start(request("race", "echo hi")).map(|outcome| outcome.launched())
            }));
        }
        let launched = handles
            .into_iter()
            .filter_map(|handle| handle.join().ok())
            .filter_map(Result::ok)
            .filter(|launched| *launched)
            .count();
        assert_eq!(launched, 1, "one key must produce exactly one launch");
        assert_eq!(spawner.spawn_count(), 1);
    }

    #[test]
    fn a_failed_start_is_recorded_and_retained_under_the_same_key() {
        let spawner = ScriptedSpawner::new(vec![ScriptedPlan {
            pid: 0,
            remain_ticks: 0,
            exit_code: None,
            stdout: Vec::new(),
            stderr: Vec::new(),
            fail: true,
        }]);
        let manager = manager(
            Arc::clone(&spawner),
            Arc::new(ScriptedProbe::new(vec![])),
            Arc::new(ManualClock::default()),
            ManagerConfig::default(),
        );
        let started = manager.start(request("k1", "echo hi")).unwrap();
        let view = started.view();
        assert_eq!(view.state, JobRunState::Failed);
        assert_eq!(view.error_code.as_deref(), Some("start_failed"));
        // The claim survives, so the same key cannot launch a second process.
        let again = manager.start(request("k1", "echo hi")).unwrap();
        assert!(!again.launched());
        assert_eq!(spawner.spawn_count(), 1);
    }

    #[test]
    fn concurrent_distinct_keys_are_capacity_bounded() {
        let plans = (0..4).map(|i| scripted_plan(200 + i, "x")).collect();
        let spawner = ScriptedSpawner::new(plans);
        let manager = manager(
            Arc::clone(&spawner),
            Arc::new(ScriptedProbe::new(vec![])),
            Arc::new(ManualClock::default()),
            ManagerConfig { max_jobs: 2, ..ManagerConfig::default() },
        );
        manager.start(request("a", "echo a")).unwrap();
        manager.start(request("b", "echo b")).unwrap();
        let error = manager.start(request("c", "echo c")).unwrap_err();
        assert_eq!(error.code(), "job.capacity");
        assert_eq!(spawner.spawn_count(), 2);
    }

    // -- concurrency regressions (reported against an earlier revision) ------

    /// Regression: the claim was published before the job entry.
    ///
    /// `start` used to insert the key claim, release the `keys` lock, and only
    /// then publish the job entry. A concurrent start with the same key landing
    /// in that window found a claim it could not resolve and returned
    /// `UnknownJob` — a visible half-record, and a breach of "same key, same
    /// request, same job" that only showed up under real concurrency.
    ///
    /// The clock parks on its second read, which is the first read *after* the
    /// record must already be complete. The test then asserts, from the parked
    /// state, that a duplicate resolves to the same job and that the job is
    /// listed.
    #[test]
    fn a_duplicate_start_never_observes_a_half_published_job() {
        let (clock, entered, release) = ParkingClock::new(1);
        let pid = 900;
        let probe = Arc::new(ScriptedProbe::new(vec![(
            pid,
            Liveness::Alive { start_marker: Some(5) },
        )]));
        let spawner = ScriptedSpawner::new(vec![scripted_plan(pid, "hello")]);
        // Concrete fakes are kept for their counters; the manager gets trait
        // objects, which needs an explicit coercion.
        let clock: Arc<dyn Clock> = clock;
        let spawner_for_manager: Arc<dyn ProcessSpawner> = spawner.clone();
        let probe_for_manager: Arc<dyn IdentityProbe> = probe.clone();
        let manager = Arc::new(ProcessJobManager::new(
            clock,
            spawner_for_manager,
            probe_for_manager,
            ManagerConfig::default(),
        ));

        let worker = Arc::clone(&manager);
        let first = std::thread::spawn(move || worker.start(request("k", "echo hello")));
        entered
            .recv_timeout(Duration::from_secs(5))
            .expect("the clock must reach its parked read");

        // The first start is suspended here. Nothing may be half-visible.
        let duplicate = manager.start(request("k", "echo hello"));
        let listed = manager.list(None);

        release.send(()).unwrap();
        let first = first.join().unwrap().expect("the first start succeeds");

        assert_eq!(spawner.spawn_count(), 1, "one key must launch at most once");
        assert_eq!(
            listed.len(),
            1,
            "a job that is still starting must already be listed: {listed:?}"
        );
        match duplicate {
            Ok(StartOutcome::Existing(view)) => {
                assert_eq!(view.job_id, first.view().job_id, "the duplicate must name the same job")
            }
            other => panic!("a same-key duplicate must resolve, not fail: {other:?}"),
        }
    }

    /// Regression: a cancel arriving during the spawn was treated as a completed
    /// job, and the launch then revived the job as `Running`.
    ///
    /// While the spawner was parked there was no child handle yet. Supervision
    /// read that absence as "the child already exited", burned the one teardown
    /// attempt on `AlreadyExited`, and wrote `Finished(Cancelled)`. When the
    /// spawn finally returned, `launch` published `Running` on top — and because
    /// `teardown_started` was already set, the live tree it had just created was
    /// never reclaimed. The recorded event order was
    /// `StopRequested → TreeTerminated(AlreadyExited) → ReadersJoined →
    /// Finished(Cancelled) → Started`.
    ///
    /// The fix withholds teardown and finalization while the launch is in
    /// flight, and lets the launch hand a real child to the single finalizer.
    #[test]
    fn a_stop_during_launch_is_not_acknowledged_early_and_never_revives_the_job() {
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let pid = 901;
        let probe = Arc::new(ScriptedProbe::new(vec![(
            pid,
            Liveness::Alive { start_marker: Some(5) },
        )]));
        let terminates = Arc::new(AtomicUsize::new(0));
        let spawner = Arc::new(ParkingSpawner {
            pid,
            entered: entered_tx,
            release: Mutex::new(release_rx),
            spawns: AtomicUsize::new(0),
            terminates: Arc::clone(&terminates),
            probe: Arc::clone(&probe),
        });
        let spawner_for_manager: Arc<dyn ProcessSpawner> = spawner.clone();
        let probe_for_manager: Arc<dyn IdentityProbe> = probe.clone();
        let manager = Arc::new(ProcessJobManager::new(
            Arc::new(ManualClock::default()),
            spawner_for_manager,
            probe_for_manager,
            ManagerConfig::default(),
        ));

        let worker = Arc::clone(&manager);
        let first = std::thread::spawn(move || worker.start(request("k", "echo hi")));
        entered_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("the spawner must park");

        let listed = manager.list(None);
        assert_eq!(listed.len(), 1, "a starting job must be visible: {listed:?}");
        let job_id = listed[0].job_id.clone();
        let cancelled = manager.cancel(&job_id).expect("cancel");

        // No process exists yet, so the request cannot have taken effect.
        assert!(
            !cancelled.acknowledged,
            "cancel was acknowledged before a child existed: {cancelled:?}"
        );
        assert_eq!(cancelled.view.state, JobRunState::Queued);

        release_tx.send(()).unwrap();
        let launched = first.join().unwrap().expect("the first start succeeds");
        assert!(launched.launched(), "the job was genuinely launched once");

        let view = wait_terminal(&manager, &job_id);
        assert_eq!(
            view.state,
            JobRunState::Cancelled,
            "the stop owns the outcome: {view:?}"
        );
        assert_ne!(view.state, JobRunState::Running);
        assert_eq!(
            terminates.load(Ordering::SeqCst),
            1,
            "the tree the launch created must actually be reclaimed"
        );
        assert_eq!(view.terminate.as_ref().map(TerminateOutcome::as_str), Some("terminated"));
        assert_eq!(
            view.owner_after_stop,
            Some(OwnerStatus::Gone),
            "the re-probe must confirm the process is gone: {view:?}"
        );
        assert!(
            !view
                .events
                .iter()
                .any(|event| matches!(event.kind, JobEventKind::Started)),
            "a job stopped during launch must never be published as started: {:?}",
            view.events
        );
    }

    /// Capacity must hold under concurrency, not only when calls are serialized.
    #[test]
    fn concurrent_starts_with_distinct_keys_cannot_exceed_the_capacity_limit() {
        let plans = (0..4).map(|i| scripted_plan(910 + i, "x")).collect();
        let spawner = ScriptedSpawner::new(plans);
        let spawner_for_manager: Arc<dyn ProcessSpawner> = spawner.clone();
        let manager = Arc::new(ProcessJobManager::new(
            Arc::new(ManualClock::default()),
            spawner_for_manager,
            Arc::new(ScriptedProbe::new(vec![])),
            ManagerConfig { max_jobs: 2, ..ManagerConfig::default() },
        ));

        let barrier = Arc::new(std::sync::Barrier::new(4));
        let workers: Vec<_> = (0..4)
            .map(|index| {
                let manager = Arc::clone(&manager);
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    manager.start(request(&format!("k{index}"), "echo x"))
                })
            })
            .collect();
        let results: Vec<_> = workers.into_iter().map(|handle| handle.join().unwrap()).collect();

        let launched = results.iter().filter(|result| result.is_ok()).count();
        let rejected = results
            .iter()
            .filter(|result| matches!(result, Err(JobError::Capacity { .. })))
            .count();
        assert_eq!(launched, 2, "exactly the capacity must be admitted: {results:?}");
        assert_eq!(rejected, 2, "the rest must be rejected, not queued: {results:?}");
        assert_eq!(
            spawner.spawn_count(),
            2,
            "a rejected start must not spawn anything"
        );
        assert_eq!(manager.list(None).len(), 2);
    }

    /// A tree walk that failed must not be reported as a completed teardown.
    ///
    /// The walker used to return a bare `Ok(())` whose value was ignored, so a
    /// `taskkill` that never ran — or ran and failed — was still recorded as
    /// `Terminated`. Both halves of the truth are asserted here: the outcome says
    /// the walk was incomplete, and the re-probe says the process is still alive.
    #[test]
    fn a_failed_tree_walk_is_reported_as_incomplete_and_the_reprobe_agrees() {
        let pid = 920;
        let spawner = ScriptedSpawner::new(vec![scripted_plan(pid, "")]);
        spawner.fail_tree_walker();
        let probe = Arc::new(ScriptedProbe::new(vec![(
            pid,
            Liveness::Alive { start_marker: Some(5) },
        )]));
        let manager = manager(
            Arc::clone(&spawner),
            Arc::clone(&probe),
            Arc::new(ManualClock::default()),
            ManagerConfig::default(),
        );

        let mut request = request("k", "echo hi");
        request.execution_budget = Duration::from_millis(1);
        let job_id = manager.start(request).unwrap().view().job_id.clone();
        std::thread::sleep(Duration::from_millis(20));

        let view = wait_terminal(&manager, &job_id);
        match &view.terminate {
            Some(outcome @ TerminateOutcome::Incomplete { walker_exit_code, .. }) => {
                assert_eq!(*walker_exit_code, Some(128));
                assert!(
                    !outcome.tree_stopped(),
                    "an incomplete walk must not claim the tree stopped"
                );
            }
            other => panic!("a failed tree walk must be reported as incomplete, got {other:?}"),
        }
        assert_eq!(
            view.owner_after_stop,
            Some(OwnerStatus::Alive),
            "the re-probe must show the process was not reclaimed: {view:?}"
        );
    }

    // -- terminal states -----------------------------------------------------

    #[test]
    fn zero_exit_is_success_and_a_nonzero_exit_keeps_its_code_and_output() {
        let spawner = ScriptedSpawner::new(vec![
            scripted_plan(100, "fine"),
            ScriptedPlan {
                pid: 101,
                remain_ticks: 1,
                exit_code: Some(3),
                stdout: "useful diagnostics".as_bytes().to_vec(),
                stderr: "bad thing".as_bytes().to_vec(),
                fail: false,
            },
        ]);
        let manager = manager(
            Arc::clone(&spawner),
            Arc::new(ScriptedProbe::new(vec![])),
            Arc::new(ManualClock::default()),
            ManagerConfig::default(),
        );

        let ok = manager.start(request("ok", "echo fine")).unwrap().view().job_id.clone();
        let bad = manager.start(request("bad", "echo bad")).unwrap().view().job_id.clone();

        // State must not be judged before the process is reaped.
        assert_eq!(manager.status(&bad).unwrap().state, JobRunState::Running);
        let ok_view = run_to_terminal(&manager, &ok);
        let bad_view = run_to_terminal(&manager, &bad);

        assert_eq!(ok_view.state, JobRunState::Succeeded);
        assert_eq!(ok_view.production_state(), "completed");

        assert_eq!(bad_view.state, JobRunState::Failed);
        assert_eq!(bad_view.exit_code, Some(3));
        assert_eq!(bad_view.error_code.as_deref(), Some("nonzero_exit"));
        assert_eq!(bad_view.production_state(), "failed");
        // A01's promise: the diagnostics produced before the failure survive.
        let page = manager.output(&bad, Stream::Stdout, 0, 4096).unwrap();
        assert_eq!(page.content, "useful diagnostics");
        assert!(page.stream_closed, "the pipe ended with the process");
        let err = manager.output(&bad, Stream::Stderr, 0, 4096).unwrap();
        assert_eq!(err.content, "bad thing");
    }

    #[test]
    fn status_and_output_are_pure_reads() {
        let spawner = ScriptedSpawner::new(vec![ScriptedPlan {
            pid: 100,
            // Never exits on its own: only polling can be blamed for a change.
            remain_ticks: usize::MAX,
            exit_code: Some(0),
            stdout: "partial".as_bytes().to_vec(),
            stderr: Vec::new(),
            fail: false,
        }]);
        let manager = manager(
            Arc::clone(&spawner),
            Arc::new(ScriptedProbe::new(vec![])),
            Arc::new(ManualClock::default()),
            ManagerConfig::default(),
        );
        let job = manager.start(request("k", "echo hi")).unwrap().view().job_id.clone();
        for _ in 0..25 {
            let view = manager.status(&job).unwrap();
            assert_eq!(view.state, JobRunState::Running, "status must not advance");
            let _ = manager.output(&job, Stream::Stdout, 0, 1024).unwrap();
        }
        assert_eq!(spawner.spawn_count(), 1, "reads must never re-execute");
        assert_eq!(spawner.terminate_count(), 0, "reads must never signal");
    }

    // -- timeout -------------------------------------------------------------

    #[test]
    fn polling_and_backgrounding_do_not_reset_the_execution_budget() {
        let spawner = ScriptedSpawner::new(vec![ScriptedPlan {
            pid: 100,
            remain_ticks: usize::MAX,
            exit_code: Some(0),
            stdout: Vec::new(),
            stderr: Vec::new(),
            fail: false,
        }]);
        let probe = Arc::new(ScriptedProbe::new(vec![(
            100,
            Liveness::Alive { start_marker: Some(5) },
        )]));
        let manager = manager(
            Arc::clone(&spawner),
            probe,
            Arc::new(ManualClock::default()),
            ManagerConfig::default(),
        );
        let mut start = request("k", "sleep forever");
        start.execution_budget = Duration::from_millis(80);
        let job = manager.start(start).unwrap().view().job_id.clone();
        assert_eq!(
            manager.status(&job).unwrap().owner.unwrap().start_marker,
            Some(5),
            "the owner is recorded from the identity probe at launch"
        );

        // Poll steadily; each poll must leave the deadline alone.
        for _ in 0..4 {
            std::thread::sleep(Duration::from_millis(10));
            assert!(manager.tick().is_empty(), "not expired yet");
        }
        assert_eq!(spawner.terminate_count(), 0);

        // Past the original 80 ms deadline: polling must not have pushed it out.
        std::thread::sleep(Duration::from_millis(60));
        assert!(
            manager
                .tick()
                .iter()
                .any(|action| matches!(action, TickAction::StopRequested { .. })),
            "the deadline is absolute, not idle-time based"
        );
    }

    #[test]
    fn timeout_with_a_short_budget_terminates_and_keeps_output() {
        let spawner = ScriptedSpawner::new(vec![ScriptedPlan {
            pid: 100,
            remain_ticks: usize::MAX,
            exit_code: Some(0),
            stdout: "produced before timeout".as_bytes().to_vec(),
            stderr: Vec::new(),
            fail: false,
        }]);
        let probe = Arc::new(ScriptedProbe::new(vec![(
            100,
            Liveness::Alive { start_marker: Some(5) },
        )]));
        let manager = manager(
            Arc::clone(&spawner),
            probe,
            Arc::new(ManualClock::default()),
            ManagerConfig::default(),
        );
        let mut start = request("k", "sleep forever");
        start.execution_budget = Duration::from_millis(30);
        let job = manager.start(start).unwrap().view().job_id.clone();

        std::thread::sleep(Duration::from_millis(60));
        let actions = manager.tick();
        assert!(
            actions.iter().any(|action| matches!(
                action,
                TickAction::StopRequested { reason: StopReason::Timeout, .. }
            )),
            "a timeout must be recorded as a stop request: {actions:?}"
        );
        assert_eq!(spawner.terminate_count(), 1, "the tree must be signalled exactly once");

        // The terminal state is only written once readers have drained.
        for _ in 0..20 {
            if manager.status(&job).unwrap().state.is_terminal() {
                break;
            }
            manager.tick();
            std::thread::sleep(Duration::from_millis(10));
        }
        let view = manager.status(&job).unwrap();
        assert_eq!(view.state, JobRunState::TimedOut);
        assert_eq!(view.error_code.as_deref(), Some("timed_out"));
        assert_eq!(view.production_state(), "failed");
        assert_eq!(
            view.terminate,
            Some(TerminateOutcome::Terminated {
                walker_exit_code: Some(0),
                walker_visited: 3
            }),
            "a walked tree must report the walker's own evidence"
        );
        assert!(view.terminate.as_ref().unwrap().tree_stopped());

        // Ordering is observable: stop was requested, the tree was terminated,
        // readers were joined, and only then did the job finish.
        let kinds: Vec<&JobEventKind> = view.events.iter().map(|event| &event.kind).collect();
        let stop = kinds
            .iter()
            .position(|kind| matches!(kind, JobEventKind::StopRequested(_)))
            .expect("stop requested");
        let terminated = kinds
            .iter()
            .position(|kind| matches!(kind, JobEventKind::TreeTerminated(_)))
            .expect("tree terminated");
        let joined = kinds
            .iter()
            .position(|kind| matches!(kind, JobEventKind::ReadersJoined))
            .expect("readers joined");
        let finished = kinds
            .iter()
            .position(|kind| matches!(kind, JobEventKind::Finished(_)))
            .expect("finished");
        assert!(stop < terminated && terminated < finished);
        assert!(joined < finished, "the terminal state must be written last");

        // Output produced before the deadline stays readable.
        let page = manager.output(&job, Stream::Stdout, 0, 4096).unwrap();
        assert_eq!(page.content, "produced before timeout");
    }

    // -- cancel --------------------------------------------------------------

    #[test]
    fn cancel_is_idempotent_and_only_acknowledged_when_terminal() {
        let spawner = ScriptedSpawner::new(vec![ScriptedPlan {
            pid: 100,
            remain_ticks: usize::MAX,
            exit_code: Some(0),
            stdout: "kept output".as_bytes().to_vec(),
            stderr: Vec::new(),
            fail: false,
        }]);
        let probe = Arc::new(ScriptedProbe::new(vec![(
            100,
            Liveness::Alive { start_marker: Some(5) },
        )]));
        let manager = manager(
            Arc::clone(&spawner),
            probe,
            Arc::new(ManualClock::default()),
            ManagerConfig::default(),
        );
        let job = manager.start(request("k", "sleep forever")).unwrap().view().job_id.clone();

        let first = manager.cancel(&job).unwrap();
        assert!(first.changed);
        for _ in 0..20 {
            if manager.status(&job).unwrap().state.is_terminal() {
                break;
            }
            manager.tick();
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(manager.status(&job).unwrap().state, JobRunState::Cancelled);
        assert!(manager.status(&job).unwrap().cancel_acknowledged_at.is_some());
        assert!(manager.status(&job).unwrap().production_state() == "cancelled");

        let second = manager.cancel(&job).unwrap();
        assert!(!second.changed, "a repeat cancel must be a no-op");
        assert!(second.acknowledged);
        assert_eq!(
            spawner.terminate_count(),
            1,
            "a repeat cancel must not signal the tree again"
        );
    }

    #[test]
    fn cancel_after_completion_does_not_rewrite_history() {
        let spawner = ScriptedSpawner::new(vec![scripted_plan(100, "done")]);
        let manager = manager(
            Arc::clone(&spawner),
            Arc::new(ScriptedProbe::new(vec![])),
            Arc::new(ManualClock::default()),
            ManagerConfig::default(),
        );
        let job = manager.start(request("k", "echo done")).unwrap().view().job_id.clone();
        assert_eq!(run_to_terminal(&manager, &job).state, JobRunState::Succeeded);

        let outcome = manager.cancel(&job).unwrap();
        assert!(!outcome.changed);
        assert!(!outcome.acknowledged);
        assert_eq!(outcome.view.state, JobRunState::Succeeded);
        assert_eq!(spawner.terminate_count(), 0);
    }

    // -- output window -------------------------------------------------------

    #[test]
    fn output_reports_dropped_bytes_instead_of_hiding_them() {
        let mut window = OutputWindow::new(16);
        window.push(Stream::Stdout, b"0123456789ABCDEF");
        assert_eq!(window.view(Stream::Stdout).dropped_bytes, 0);
        window.push(Stream::Stdout, b"GHIJ");

        let view = window.view(Stream::Stdout);
        assert_eq!(view.total_bytes, 20, "every byte the child wrote is counted");
        assert_eq!(view.retained_bytes, 16);
        assert_eq!(view.dropped_bytes, 4);
        assert_eq!(view.retained_start, 4);
        assert_eq!(view.end_offset, 20);
        assert_eq!(view.retention_limit, 16);

        let page = window.page(Stream::Stdout, 0, 4096);
        assert_eq!(page.content, "456789ABCDEFGHIJ");
        assert_eq!(page.next_offset, 20);
        assert!(page.at_end_of_available);
        assert_eq!(
            page.lost_before_offset, 4,
            "a cursor behind the retained window is told what it lost"
        );
    }

    #[test]
    fn an_old_cursor_reports_what_was_lost() {
        let mut window = OutputWindow::new(8);
        window.push(Stream::Stdout, b"0123456789");

        let page = window.page(Stream::Stdout, 1, 2);
        assert_eq!(page.content, "23");
        assert_eq!(page.offset, 2, "reading starts at the retained window");
        assert_eq!(page.next_offset, 4);
        assert_eq!(page.lost_before_offset, 1);
        assert!(!page.at_end_of_available, "more output is still available");

        let rest = window.page(Stream::Stdout, 2, 4096);
        assert_eq!(rest.content, "23456789");
        assert!(rest.at_end_of_available);
        assert_eq!(rest.lost_before_offset, 0);
        assert_eq!(rest.next_offset, 10);
    }

    #[test]
    fn stream_cursors_are_independent() {
        let mut window = OutputWindow::new(64);
        window.push(Stream::Stdout, b"out");

        let stdout = window.page(Stream::Stdout, 0, 4096);
        let stderr = window.page(Stream::Stderr, 0, 4096);
        assert_eq!(stdout.content, "out");
        assert_eq!(stdout.total_bytes, 3);
        assert_eq!(stdout.next_offset, 3);
        // A silent stream reports zero bytes, which is not the same as unknown.
        assert_eq!(stderr.content, "");
        assert_eq!(stderr.total_bytes, 0);
        assert_eq!(stderr.next_offset, 0);
        assert_eq!(stderr.dropped_bytes, 0);

        // Activity on one stream must not move the other's cursor or totals.
        window.push(Stream::Stderr, b"err");
        assert_eq!(window.page(Stream::Stdout, 0, 4096).total_bytes, 3);
        let stderr_after = window.page(Stream::Stderr, 0, 4096);
        assert_eq!(stderr_after.content, "err");
        assert_eq!(stderr_after.next_offset, 3);
    }

    #[test]
    fn end_of_bytes_and_closed_pipe_are_separate_facts() {
        // A buffer can only answer the first question; a caller must not read
        // "I caught up" as "nothing more is coming".
        let mut window = OutputWindow::new(64);
        window.push(Stream::Stdout, b"partial");
        let page = window.page(Stream::Stdout, 0, 4096);
        assert!(page.at_end_of_available, "the cursor caught up with the producer");
        assert!(
            !page.stream_closed,
            "a buffer cannot claim the pipe ended; only the manager knows"
        );
    }

    #[test]
    fn multibyte_content_is_never_split_mid_character() {
        let mut window = OutputWindow::new(6);
        window.push(Stream::Stdout, "中文测试".as_bytes());
        let view = window.view(Stream::Stdout);
        assert_eq!(view.total_bytes, 12);
        assert_eq!(view.dropped_bytes, 6);

        let page = window.page(Stream::Stdout, 0, 4096);
        assert_eq!(page.content, "测试");
        assert!(
            !page.content.contains('\u{FFFD}'),
            "eviction must land on a character boundary, not inside one"
        );
    }

    // -- terminate guard -----------------------------------------------------

    #[test]
    fn a_reused_pid_is_never_signalled() {
        let spawner = ScriptedSpawner::new(vec![ScriptedPlan {
            pid: 100,
            remain_ticks: usize::MAX,
            exit_code: Some(0),
            stdout: Vec::new(),
            stderr: Vec::new(),
            fail: false,
        }]);
        let probe = Arc::new(ScriptedProbe::new(vec![(
            100,
            Liveness::Alive { start_marker: Some(5) },
        )]));
        let manager = manager(
            Arc::clone(&spawner),
            Arc::clone(&probe),
            Arc::new(ManualClock::default()),
            ManagerConfig::default(),
        );
        let job = manager.start(request("k", "sleep forever")).unwrap().view().job_id.clone();
        assert_eq!(manager.status(&job).unwrap().owner.unwrap().start_marker, Some(5));

        // The pid now belongs to a different process instance.
        probe.set(100, Liveness::Alive { start_marker: Some(999) });
        let outcome = manager.cancel(&job).unwrap();

        assert!(outcome.changed, "the request itself is recorded");
        assert_eq!(
            outcome.view.terminate,
            Some(TerminateOutcome::SkippedIdentityMismatch)
        );
        assert_eq!(
            spawner.terminate_count(),
            0,
            "an unrelated program must never be killed through a reused pid"
        );
        // Cancel stays a request: nothing was stopped, so nothing is acknowledged.
        assert!(outcome.view.cancel_requested_at.is_some());
        assert!(outcome.view.cancel_acknowledged_at.is_none());
        assert!(!outcome.acknowledged);
        assert_eq!(outcome.view.state, JobRunState::Running);
    }

    #[test]
    fn an_unprovable_owner_is_not_killed_either() {
        let spawner = ScriptedSpawner::new(vec![ScriptedPlan {
            pid: 100,
            remain_ticks: usize::MAX,
            exit_code: Some(0),
            stdout: Vec::new(),
            stderr: Vec::new(),
            fail: false,
        }]);
        let probe = Arc::new(ScriptedProbe::new(vec![(
            100,
            Liveness::Alive { start_marker: Some(5) },
        )]));
        let manager = manager(
            Arc::clone(&spawner),
            Arc::clone(&probe),
            Arc::new(ManualClock::default()),
            ManagerConfig::default(),
        );
        let job = manager.start(request("k", "sleep forever")).unwrap().view().job_id.clone();
        probe.set(100, Liveness::Unknown("access denied".into()));
        manager.cancel(&job).unwrap();
        assert_eq!(
            spawner.terminate_count(),
            0,
            "killing on an unprovable identity is a guess, so it is refused"
        );
    }

    // -- production mapping --------------------------------------------------

    #[test]
    fn production_state_mapping_is_explicit() {
        let table = [
            (JobRunState::Queued, "queued"),
            (JobRunState::Running, "running"),
            (JobRunState::Succeeded, "completed"),
            (JobRunState::Failed, "failed"),
            (JobRunState::Cancelled, "cancelled"),
            (JobRunState::TimedOut, "failed"),
            (JobRunState::Interrupted, "failed"),
        ];
        for (state, expected) in table {
            assert_eq!(state.production_state(), expected, "{state:?}");
        }
        assert_eq!(JobRunState::TimedOut.production_error_code(), Some("timed_out"));
        assert_eq!(JobRunState::Interrupted.production_error_code(), Some("interrupted"));
        assert_eq!(JobRunState::Succeeded.production_error_code(), None);
        assert_eq!(JobRunState::Failed.production_error_code(), None);

        for state in [
            JobRunState::Succeeded,
            JobRunState::Failed,
            JobRunState::Cancelled,
            JobRunState::TimedOut,
            JobRunState::Interrupted,
        ] {
            assert!(state.is_terminal(), "{state:?} must be terminal");
        }
        assert!(!JobRunState::Queued.is_terminal());
        assert!(!JobRunState::Running.is_terminal());

        // The compute lifecycle treats an orphan as `paused` (resumable). A long
        // command must not, because it may already have had side effects.
        assert_ne!(JobRunState::Interrupted.production_state(), "paused");
    }

    // -- recovery ------------------------------------------------------------

    #[test]
    fn recovery_uses_ownership_evidence_and_never_allows_replay() {
        let probe = Arc::new(ScriptedProbe::new(vec![
            (11, Liveness::Alive { start_marker: Some(1) }),
            (12, Liveness::Gone),
            (13, Liveness::Alive { start_marker: Some(999) }),
            (14, Liveness::Unknown("access denied".into())),
        ]));
        let manager = manager(
            ScriptedSpawner::new(vec![]),
            probe,
            Arc::new(ManualClock::default()),
            ManagerConfig::default(),
        );
        let record = |job_id: &str, state: JobRunState, owner: Option<ProcessIdentity>| {
            PersistedJobRecord { job_id: job_id.to_owned(), run_id: "r".into(), state, owner }
        };
        let records = vec![
            record("alive", JobRunState::Running, Some(ProcessIdentity { pid: 11, start_marker: Some(1) })),
            record("gone", JobRunState::Running, Some(ProcessIdentity { pid: 12, start_marker: Some(2) })),
            record("reused", JobRunState::Running, Some(ProcessIdentity { pid: 13, start_marker: Some(3) })),
            record("unknown", JobRunState::Running, Some(ProcessIdentity { pid: 14, start_marker: Some(4) })),
            record("norecord", JobRunState::Running, None),
            record("done", JobRunState::Succeeded, Some(ProcessIdentity { pid: 11, start_marker: Some(1) })),
        ];
        let decisions = manager.recover(&records);
        let find = |id: &str| {
            decisions
                .iter()
                .find(|decision| decision.job_id == id)
                .unwrap_or_else(|| panic!("no decision for {id}"))
        };

        // Ours and still alive: keep running, and still no replay.
        assert!(find("alive").unchanged());
        assert_eq!(find("alive").reason, RecoveryReason::OwnerAlive);
        assert!(!find("alive").needs_review);
        assert_eq!(find("alive").owner_status, OwnerStatus::Alive);

        // Confirmed dead.
        assert_eq!(find("gone").to, JobRunState::Interrupted);
        assert_eq!(find("gone").reason, RecoveryReason::OwnerGone);
        assert_eq!(find("gone").error_code(), Some("interrupted"));
        assert!(!find("gone").needs_review);

        // The pid exists but is somebody else's process.
        assert_eq!(find("reused").to, JobRunState::Interrupted);
        assert_eq!(find("reused").reason, RecoveryReason::OwnerReplaced);
        assert_eq!(find("reused").owner_status, OwnerStatus::Replaced);

        // Cannot prove either way: interrupted, flagged, and left for review.
        assert_eq!(find("unknown").to, JobRunState::Interrupted);
        assert_eq!(find("unknown").reason, RecoveryReason::OwnerUnknown);
        assert!(find("unknown").needs_review);
        assert_eq!(find("unknown").error_code(), Some("interrupted_unknown"));

        // No ownership was ever recorded.
        assert_eq!(find("norecord").reason, RecoveryReason::NoOwnerRecorded);
        assert!(find("norecord").needs_review);

        // A terminal job is left exactly as it is.
        assert!(find("done").unchanged());
        assert_eq!(find("done").reason, RecoveryReason::Terminal);
        assert_eq!(find("done").error_code(), None);

        // The property that matters most: recovery never replays side effects.
        assert!(
            decisions.iter().all(|decision| !decision.allow_replay),
            "recovery must never authorize a replay"
        );
    }

    #[test]
    fn recovery_does_not_claim_running_without_a_positive_probe() {
        // A row saying `running` with no owner is not evidence that anything runs.
        let manager = manager(
            ScriptedSpawner::new(vec![]),
            Arc::new(ScriptedProbe::new(vec![])),
            Arc::new(ManualClock::default()),
            ManagerConfig::default(),
        );
        let decisions = manager.recover(&[PersistedJobRecord {
            job_id: "orphan".into(),
            run_id: "r".into(),
            state: JobRunState::Running,
            owner: Some(ProcessIdentity { pid: 9, start_marker: Some(1) }),
        }]);
        assert_eq!(decisions[0].to, JobRunState::Interrupted);
        assert_eq!(decisions[0].owner_status, OwnerStatus::Unknown);
    }

    // -- request validation / unknown ids -----------------------------------

    #[test]
    fn invalid_requests_are_rejected_before_any_spawn() {
        let spawner = ScriptedSpawner::new(vec![scripted_plan(100, "x")]);
        let manager = manager(
            Arc::clone(&spawner),
            Arc::new(ScriptedProbe::new(vec![])),
            Arc::new(ManualClock::default()),
            ManagerConfig::default(),
        );
        let mutations: Vec<(&str, Box<dyn Fn(&mut StartRequest)>)> = vec![
            ("empty run", Box::new(|request: &mut StartRequest| request.run_id.clear())),
            ("empty key", Box::new(|request: &mut StartRequest| request.idempotency_key.clear())),
            ("empty program", Box::new(|request: &mut StartRequest| request.spec.program.clear())),
            ("empty cwd", Box::new(|request: &mut StartRequest| request.spec.cwd.clear())),
            ("zero budget", Box::new(|request: &mut StartRequest| {
                request.execution_budget = Duration::ZERO;
            })),
            ("budget too large", Box::new(|request: &mut StartRequest| {
                request.execution_budget = Duration::from_secs(99_999);
            })),
        ];
        for (label, mutate) in mutations {
            let mut candidate = request("k", "echo hi");
            mutate(&mut candidate);
            let error = manager.start(candidate).unwrap_err();
            assert_eq!(error.code(), "job.invalid_request", "{label}");
        }
        assert_eq!(spawner.spawn_count(), 0, "validation happens before any spawn");
    }

    #[test]
    fn reads_for_an_unknown_job_fail_with_a_stable_code() {
        let manager = manager(
            ScriptedSpawner::new(vec![]),
            Arc::new(ScriptedProbe::new(vec![])),
            Arc::new(ManualClock::default()),
            ManagerConfig::default(),
        );
        assert_eq!(manager.status("nope").unwrap_err().code(), "job.unknown");
        assert_eq!(
            manager.output("nope", Stream::Stdout, 0, 16).unwrap_err().code(),
            "job.unknown"
        );
        assert_eq!(manager.cancel("nope").unwrap_err().code(), "job.unknown");
    }

    #[test]
    fn jobs_are_listed_per_run_in_a_stable_order() {
        let spawner = ScriptedSpawner::new(vec![
            scripted_plan(100, "a"),
            scripted_plan(101, "b"),
            scripted_plan(102, "c"),
        ]);
        let manager = manager(
            Arc::clone(&spawner),
            Arc::new(ScriptedProbe::new(vec![])),
            Arc::new(ManualClock::default()),
            ManagerConfig::default(),
        );
        let mut other = request("other", "echo x");
        other.run_id = "run-2".into();
        manager.start(request("a", "echo a")).unwrap();
        manager.start(request("b", "echo b")).unwrap();
        manager.start(other).unwrap();

        assert_eq!(manager.list(Some("run-1")).len(), 2);
        assert_eq!(manager.list(Some("run-2")).len(), 1);
        assert_eq!(manager.list(None).len(), 3);

        let ids: Vec<String> = manager
            .list(Some("run-1"))
            .into_iter()
            .map(|view| view.job_id)
            .collect();
        let mut sorted = ids.clone();
        sorted.sort();
        assert_eq!(ids, sorted, "listing is ordered so evidence is reproducible");
    }
}
