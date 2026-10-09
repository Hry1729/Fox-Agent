//! JOB-v1 — independent long-process lifecycle (batch `C-MODULE-01`).
//!
//! Used by the Host command-job adapter for start / status / output / cancel /
//! recovery. Production starts still require a verified backend; the default
//! Host proof refuses execution. Real temporary-process tests inject a proof
//! per instance through types that exist only in test builds.
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

#[path = "command_environment.rs"]
mod command_environment;

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
    CredentialMismatch(String),
    Capacity {
        max_jobs: usize,
    },
    /// The process could not be started at all. Distinct from a non-zero exit.
    StartFailed(String),
    /// No execution backend on this Host has been verified to confine the
    /// process, so no process was created. This is a policy refusal, not a
    /// launch failure: it must never be retried as-is, because retrying cannot
    /// make a missing confinement boundary exist.
    SandboxUnavailable(String),
}

impl JobError {
    pub fn code(&self) -> &'static str {
        match self {
            JobError::UnknownJob(_) => "job.unknown",
            JobError::IdempotencyConflict { .. } => "job.idempotency_conflict",
            JobError::InvalidRequest(_) => "job.invalid_request",
            JobError::CredentialMismatch(_) => "credential_mismatch",
            JobError::Capacity { .. } => "job.capacity",
            JobError::StartFailed(_) => "job.start_failed",
            JobError::SandboxUnavailable(_) => "sandbox_unavailable",
        }
    }

    pub fn message(&self) -> String {
        match self {
            JobError::UnknownJob(id) => format!("unknown job '{id}'"),
            JobError::IdempotencyConflict { job_id } => format!(
                "idempotency key already belongs to job '{job_id}' with a different request"
            ),
            JobError::InvalidRequest(detail) | JobError::CredentialMismatch(detail) => detail.clone(),
            JobError::Capacity { max_jobs } => {
                format!("job capacity reached (max {max_jobs} tracked jobs)")
            }
            JobError::StartFailed(detail) => detail.clone(),
            JobError::SandboxUnavailable(detail) => detail.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// Backend capability authority
// ---------------------------------------------------------------------------
//
// Phase 0 result (batch C0, 2026-09-20): this Host has no execution backend
// that has been verified to confine a process tree. The only spawner the
// product wires up today (`SystemSpawner`) creates the child with the calling
// user's token, at the same integrity level, with write access everywhere that
// user can write and with unmediated egress. Measured, not inferred: see
// `evidence/C/C0/logs/os-capability-baseline.json`.
//
// Therefore arbitrary execution is refused at two seams: the manager refuses
// before publishing a job, and the spawner refuses before creating a process.
// The refusal is an artifact-level property, deliberately not a setting:
//
//   * It is not read from the environment, a config file, a project directory
//     or any model-supplied value, so no caller can grant itself execution by
//     naming a different helper, backend, root list or handle.
//   * `BackendAvailability` is only produced by a `BackendCapabilityProof`, and
//     the proof is an **injected dependency** — a constructor argument or an
//     instance field. There is no process-global proof. Production wiring passes
//     `HostVerifiedBackend`, whose only answer on this Host is `Unavailable`.
//   * Injection exists so tests and the `tests/` integration target can exercise
//     the authorized path with a real child. The grant type `AvailableInTests`
//     is `#[cfg(test)]`: that target pulls this module in with
//     `#[path = "../src/process_jobs.rs"]`, and `rustc --test` enables `cfg(test)`
//     for the whole test crate, so tests can inject without the type existing in
//     a shipped build and without any feature flag or global switch.
//   * Failure of a probe keeps execution refused rather than assuming it
//     succeeded, so a broken or unreadable environment fails closed.

/// Execution backends the product can spawn through. Recorded on every
/// availability decision so an acceptance record names the thing that was
/// actually tested, not the thing that was hoped for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExecutionBackend {
    /// The child runs as the caller. Lifecycle containment only (job object,
    /// cancellation, tree teardown). **Not** a confinement boundary.
    HostUserUnconfined,
    /// `@anthropic-ai/sandbox-runtime` on Windows: a dedicated `srt-sandbox`
    /// local account, a machine-wide WFP egress fence keyed on that account's
    /// SID, and per-session explicit ACEs for the configured paths.
    SrtWindows,
}

impl ExecutionBackend {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::HostUserUnconfined => "host_user_unconfined",
            Self::SrtWindows => "srt_windows",
        }
    }
}

/// A property arbitrary execution requires, checked one at a time so a partial
/// backend can never be reported as a passing one. Ordered as reported.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum RequiredCapability {
    /// Write is denied outside the granted roots.
    FilesystemWriteConfinement,
    /// Read is denied for the declared private regions.
    FilesystemReadConfinement,
    /// A distinct principal, so the child cannot open what the caller can.
    DistinctExecutionIdentity,
    /// Direct egress is blocked regardless of proxy environment variables.
    NetworkEgressFence,
    /// System resolver (`getaddrinfo`) is not usable as an exfiltration path.
    SystemDnsFence,
    /// Every descendant is terminated and its exit is confirmed.
    ProcessTreeConfinement,
}

impl RequiredCapability {
    pub const ALL: [Self; 6] = [
        Self::FilesystemWriteConfinement,
        Self::FilesystemReadConfinement,
        Self::DistinctExecutionIdentity,
        Self::NetworkEgressFence,
        Self::SystemDnsFence,
        Self::ProcessTreeConfinement,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::FilesystemWriteConfinement => "filesystem_write_confinement",
            Self::FilesystemReadConfinement => "filesystem_read_confinement",
            Self::DistinctExecutionIdentity => "distinct_execution_identity",
            Self::NetworkEgressFence => "network_egress_fence",
            Self::SystemDnsFence => "system_dns_fence",
            Self::ProcessTreeConfinement => "process_tree_confinement",
        }
    }
}

/// Why execution is refused. Each variant carries the evidence a reader needs
/// to know what to fix; none of them is recoverable by retrying the same call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BackendRefusal {
    /// No backend at all has been verified on this Host.
    NoVerifiedBackend,
    /// The backend decision no longer matches the admitted immutable snapshot.
    CredentialMismatch,
    /// A backend exists but failed specific required capabilities. The list is
    /// echoed verbatim rather than collapsed into "not ready".
    CapabilitiesNotMet {
        backend: ExecutionBackend,
        missing: Vec<RequiredCapability>,
    },
}

impl BackendRefusal {
    pub fn reason(&self) -> String {
        match self {
            Self::NoVerifiedBackend => {
                "no execution backend on this Host has been verified to confine a process tree"
                    .to_owned()
            }
            Self::CredentialMismatch => "credential_mismatch: backend evidence changed after admission".into(),
            Self::CapabilitiesNotMet { backend, missing } => {
                let names: Vec<&str> = missing.iter().map(|c| c.as_str()).collect();
                format!(
                    "backend '{}' has not been verified for: {}",
                    backend.as_str(),
                    names.join(", ")
                )
            }
        }
    }
}

/// What the Host knows about execution right now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BackendAvailability {
    Available {
        backend: ExecutionBackend,
        /// Digest of the exact binary + configuration + policy generation that
        /// was verified. Re-verification is required when this changes.
        digest: String,
    },
    Unavailable(BackendRefusal),
}

impl BackendAvailability {
    pub fn is_available(&self) -> bool {
        matches!(self, Self::Available { .. })
    }

    /// `Ok(backend)` when execution may proceed, otherwise the refusal.
    pub fn authorize(&self) -> Result<ExecutionBackend, BackendRefusal> {
        match self {
            Self::Available { backend, .. } => Ok(*backend),
            Self::Unavailable(refusal) => Err(refusal.clone()),
        }
    }

    /// The refusal this decision carries, for reporting.
    pub fn refusal(&self) -> Option<&BackendRefusal> {
        match self {
            Self::Available { .. } => None,
            Self::Unavailable(refusal) => Some(refusal),
        }
    }
}

/// Evidence that a *Host-controlled* check ran and produced an availability
/// decision.
///
/// The proof is an **explicitly injected dependency**: every component that
/// needs to decide whether execution is allowed receives one as a field or an
/// argument, and there is deliberately no process-global proof to consult. An
/// earlier revision kept a global `OnceLock<Mutex<Option<Box<dyn ..>>>>`; that
/// made two tests in the same binary share one decision, so whichever test
/// installed `Available` silently re-authorized — or de-authorized — every other
/// test. Per-instance injection removes the shared mutable state instead of
/// trying to schedule access to it.
///
/// Production passes [`HostVerifiedBackend`], whose only honest answer on this
/// Host is `Unavailable`. A test passes [`AvailableInTests`]. Because the proof
/// is an argument rather than a global, a test cannot change another test's
/// decision, and the order in which tests run cannot change any of them.
/// `Send + Sync` because a manager holding a proof is moved onto the supervision
/// thread that starts and reaps jobs.
pub trait BackendCapabilityProof: Send + Sync {
    fn availability(&self) -> BackendAvailability;
}

/// The production proof. It exists so that wiring has a single, named place to
/// consult, and it refuses because no verified backend is integrated.
pub struct HostVerifiedBackend;

impl BackendCapabilityProof for HostVerifiedBackend {
    fn availability(&self) -> BackendAvailability {
        // Deliberately unconditional. Phase 0 found no confinement backend in
        // this build; when one is integrated, its probe result replaces this
        // value and must carry the verified binary/config digest with it.
        BackendAvailability::Unavailable(BackendRefusal::NoVerifiedBackend)
    }
}

/// A proof that grants execution, for tests and conformance harnesses only.
///
/// `#[cfg(test)]`, so the type cannot be named by a shipped build at all. The
/// `tests/` integration target reaches it because that target pulls this module
/// in with `#[path = "../src/process_jobs.rs"]` and `rustc --test` enables
/// `cfg(test)` for the whole test crate — verified in this project's actual
/// path-module layout (`O/review1/c-cfg-test-probe`, `c-cfg-test-filter.log`).
///
/// So test authorization needs neither a Cargo feature nor a global switch nor a
/// type visible in production. Production passes [`HostVerifiedBackend`], which
/// refuses, and nothing here is reachable from model output, project files or
/// environment variables.
#[cfg(test)]
#[derive(Clone, Debug)]
pub struct AvailableInTests {
    pub backend: ExecutionBackend,
    pub digest: String,
}

#[cfg(test)]
impl AvailableInTests {
    /// The conventional authorized test backend, named once so harnesses do not
    /// each invent a digest string.
    pub fn host_user_unconfined(digest: impl Into<String>) -> Self {
        Self {
            backend: ExecutionBackend::HostUserUnconfined,
            digest: digest.into(),
        }
    }
}

#[cfg(test)]
impl BackendCapabilityProof for AvailableInTests {
    fn availability(&self) -> BackendAvailability {
        BackendAvailability::Available {
            backend: self.backend,
            digest: self.digest.clone(),
        }
    }
}

/// The production decision, as a value.
///
/// This reads the production proof directly and is *not* consulted by
/// [`ProcessJobManager::start`] or by the spawners — they each consult the proof
/// they were given. Kept because the wiring layer, `command_jobs`, and the
/// capability report need a way to ask "what does the shipped artifact do?".
pub fn execution_availability() -> BackendAvailability {
    HostVerifiedBackend.availability()
}

/// Environment facts, separate from the engine's schema manifest. A catalog
/// declares tool contracts; only the Host proof can declare start availability.
pub fn runtime_capabilities(manifest_hash: &str, profile_id: &str) -> serde_json::Value {
    let availability = execution_availability();
    let (state, reason) = match &availability {
        BackendAvailability::Available { .. } => ("available", None),
        BackendAvailability::Unavailable(refusal) => ("unavailable", Some(refusal.reason())),
    };
    serde_json::json!({
        "schemaVersion":1,"capabilityManifestHash":manifest_hash,"executionProfileId":profile_id,
        "tools":{"run_command":{
            "availability":state,"reasonCode":if availability.is_available(){None}else{Some("sandbox_unavailable")},
            "reason":reason,"source":"HostVerifiedBackend",
            "actions":{"start":state,"status":"available","output":"available","cancel":"available"}
        }}
    })
}

/// Check before approval and again before dispatch. Control-plane operations
/// never start an external process and remain available.
pub fn command_start_availability(action: Option<&str>) -> Result<(), String> {
    if matches!(action, Some("status" | "output" | "cancel")) { return Ok(()); }
    authorize_execution().map(|_| ()).map_err(|refusal|
        format!("[tool.sandbox_unavailable] {}. No process was started. Use available in-process tools; retrying this command cannot provide a backend.", refusal.reason()))
}

/// The production authorization seam: always refuses on this Host.
///
/// Callers that own no injectable proof use this, which keeps them fail-closed.
/// Callers that can be injected should hold a proof instead.
pub fn authorize_execution() -> Result<ExecutionBackend, BackendRefusal> {
    execution_availability().authorize()
}

/// The refusal a `start` must return, in the shared error vocabulary. Kept as a
/// small helper so the message cannot drift between call sites.
fn sandbox_unavailable_error(refusal: &BackendRefusal) -> JobError {
    if matches!(refusal, BackendRefusal::CredentialMismatch) {
        return JobError::CredentialMismatch(refusal.reason());
    }
    JobError::SandboxUnavailable(format!(
        "sandbox_unavailable: {}. No external process was started. Control-plane state is reported separately.",
        refusal.reason()
    ))
}

/// Raw facts about the Host's native confinement primitives.
///
/// This is a *probe report*, not a decision: it says what was found on the
/// machine so a later verification batch has somewhere to record its result and
/// so a support report can distinguish "no backend installed" from "backend
/// installed but a capability missing". None of these booleans, alone or
/// together, authorizes execution.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeConfinementReport {
    /// The `srt-sandbox` local account exists, i.e. a distinct execution
    /// identity has been provisioned for the SRT Windows backend.
    pub srt_sandbox_account_present: bool,
    /// The current process is a member of `sandbox-runtime-users`, i.e. it may
    /// ask the SRT broker to launch as that account.
    pub caller_may_use_srt_sandbox: bool,
    /// `HKLM\SOFTWARE\sandbox-runtime`, the machine-wide marker written by the
    /// elevated `windows-install` step.
    pub srt_machine_marker_present: bool,
}

impl NativeConfinementReport {
    /// True when nothing about a confinement backend is present, which is the
    /// expected phase-0 state and the reason execution is refused.
    pub fn is_unprovisioned(&self) -> bool {
        !self.srt_sandbox_account_present
            && !self.caller_may_use_srt_sandbox
            && !self.srt_machine_marker_present
    }
}

/// Probes the native primitives without elevation and without changing
/// anything. Every check is best-effort: an unreadable value reports as absent,
/// which keeps the result on the refusing side.
#[cfg(windows)]
pub fn probe_native_confinement() -> NativeConfinementReport {
    use std::os::windows::ffi::OsStrExt;

    // Declared locally rather than through a feature-gated binding: the probe
    // needs three read-only calls, and keeping them here means this file's
    // confinement logic cannot be changed by altering a Cargo feature list.
    #[link(name = "advapi32")]
    extern "system" {
        fn LookupAccountNameW(
            system_name: *const u16,
            account_name: *const u16,
            sid: *mut u8,
            sid_len: *mut u32,
            referenced_domain_name: *mut u16,
            referenced_domain_len: *mut u32,
            sid_use: *mut i32,
        ) -> i32;
        fn CheckTokenMembership(token: isize, sid_to_check: *const u8, is_member: *mut i32) -> i32;
    }
    const ERROR_SUCCESS: i32 = 0;

    fn wide(value: &str) -> Vec<u16> {
        std::ffi::OsStr::new(value)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    /// Resolves a local account or group name to its SID bytes. `Ok(None)` means
    /// the principal does not exist on this machine.
    fn resolve_sid(name: &str) -> Result<Option<Vec<u8>>, ()> {
        let name_w = wide(name);
        let mut sid_len: u32 = 0;
        let mut domain_len: u32 = 0;
        let mut sid_use: i32 = 0;
        // Sizing pass. Expected to fail with ERROR_INSUFFICIENT_BUFFER.
        unsafe {
            LookupAccountNameW(
                std::ptr::null(),
                name_w.as_ptr(),
                std::ptr::null_mut(),
                &mut sid_len,
                std::ptr::null_mut(),
                &mut domain_len,
                &mut sid_use,
            );
        }
        if sid_len == 0 {
            return Ok(None);
        }
        let mut sid = vec![0u8; sid_len as usize];
        let mut domain = vec![0u16; domain_len.max(1) as usize];
        let ok = unsafe {
            LookupAccountNameW(
                std::ptr::null(),
                name_w.as_ptr(),
                sid.as_mut_ptr(),
                &mut sid_len,
                domain.as_mut_ptr(),
                &mut domain_len,
                &mut sid_use,
            )
        };
        if ok == 0 {
            // A name that cannot be resolved at all is absent, not an error we
            // should treat as permission to proceed.
            return Ok(None);
        }
        sid.truncate(sid_len as usize);
        Ok(Some(sid))
    }

    fn present(name: &str) -> bool {
        matches!(resolve_sid(name), Ok(Some(_)))
    }

    fn caller_in_group(group_name: &str) -> bool {
        let Ok(Some(sid)) = resolve_sid(group_name) else {
            return false;
        };
        let mut is_member: i32 = 0;
        // A null token handle means "the current process's token".
        let checked = unsafe { CheckTokenMembership(0, sid.as_ptr(), &mut is_member) };
        checked == ERROR_SUCCESS && is_member != 0
    }

    // `%ProgramData%\sandbox-runtime` is the unprivileged proxy for the
    // machine-wide install marker; the HKLM credential key next to it is not
    // readable without elevation, so reading it would report a false negative.
    let marker_present = std::env::var_os("ProgramData")
        .map(std::path::PathBuf::from)
        .map(|root| root.join("sandbox-runtime"))
        .map(|path| path.exists())
        .unwrap_or(false);

    NativeConfinementReport {
        srt_sandbox_account_present: present("srt-sandbox"),
        caller_may_use_srt_sandbox: caller_in_group("sandbox-runtime-users"),
        srt_machine_marker_present: marker_present,
    }
}

#[cfg(not(windows))]
pub fn probe_native_confinement() -> NativeConfinementReport {
    NativeConfinementReport {
        srt_sandbox_account_present: false,
        caller_may_use_srt_sandbox: false,
        srt_machine_marker_present: false,
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

/// How the child's environment is built. `Inherit` means the product's
/// platform allowlist, never the complete Fox parent environment.
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

pub(crate) fn apply_spawn_arguments(command: &mut std::process::Command, spec: &SpawnSpec) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        if spec.program.eq_ignore_ascii_case("cmd.exe")
            && spec.args.len() == 4
            && spec.args[0..3] == ["/D", "/S", "/C"]
        {
            command.args(&spec.args[..3]);
            // `/S /C` requires one quoted command-line tail. `Command::arg`
            // applies CreateProcess/CRT quoting, which turns inner PowerShell
            // quotes into literal text. `raw_arg` preserves cmd.exe's grammar.
            command.raw_arg(format!("\"{}\"", spec.args[3]));
            return;
        }
    }
    command.args(&spec.args);
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
            raw_bytes: slice.to_vec(),
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
    /// Exact pipe bytes for durable adapters that page in raw-byte units.
    /// `content` is a lossy display projection and must never be re-sliced by
    /// offsets from this page when invalid UTF-8 was replaced.
    pub raw_bytes: Vec<u8>,
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
    /// Creates a child, but only if `proof` authorizes execution.
    ///
    /// The proof is a parameter rather than a global so that the manager, the
    /// spawner and any test each consult the decision they were given. A global
    /// proof let one test's authorization change another test's result.
    fn spawn(
        &self,
        proof: &dyn BackendCapabilityProof,
        spec: &SpawnSpec,
    ) -> io::Result<Box<dyn ChildHandle>>;
}

/// Reusable refusal for a spawner that was handed a refusing proof.
fn refuse_spawn(refusal: &BackendRefusal) -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        format!(
            "sandbox_unavailable: {}. No process was created.",
            refusal.reason()
        ),
    )
}

/// Real spawner. Wiring should keep this only if A's executor does not already
/// own process creation.
#[derive(Default)]
pub struct SystemSpawner;

/// CREATE_NO_WINDOW: the product default. A background job must not flash a
/// console.
#[cfg(windows)]
pub const CREATE_NO_WINDOW_FLAG: u32 = 0x0800_0000;

#[cfg(windows)]
pub(crate) const CREATE_SUSPENDED_FLAG: u32 = 0x0000_0004;

/// DETACHED_PROCESS: no console is allocated at all.
///
/// This exists for one open investigation (C-FINDING-01). A console is backed by
/// `conhost.exe`, which is created after the child and inherits the child's
/// standard handles — so it is a candidate holder of the output pipes after the
/// child itself has been killed and re-probed gone. Spawning with this flag
/// removes that candidate, which is what turns "a console host might hold the
/// pipe" from a guess into a measurement.
///
/// It is **not** a product recommendation: console semantics are a decision for
/// the wiring review, not a side effect of a diagnostic.
#[cfg(windows)]
pub const DETACHED_PROCESS_FLAG: u32 = 0x0000_0008;

impl ProcessSpawner for SystemSpawner {
    fn spawn(
        &self,
        proof: &dyn BackendCapabilityProof,
        spec: &SpawnSpec,
    ) -> io::Result<Box<dyn ChildHandle>> {
        // Second, independent check, at the lowest seam. `ProcessJobManager`
        // already refuses, but the raw spawner is reachable on its own and a
        // future wiring change must not be able to create an unconfined child
        // just by reaching it directly. Refusing here means an unconfined
        // process is never created, not that it is created and then reported on.
        //
        // The decision comes from the caller's proof, never from global state:
        // production hands this the refusing proof, and a test that wants a real
        // child hands it its own.
        if let Err(refusal) = proof.availability().authorize() {
            return Err(refuse_spawn(&refusal));
        }
        #[cfg(windows)]
        {
            spawn_system_child(spec, CREATE_NO_WINDOW_FLAG)
        }
        #[cfg(not(windows))]
        {
            spawn_system_child(spec, 0)
        }
    }
}

/// The same real spawner with an explicit console mode.
///
/// Only the creation flags differ, so the environment matrix can compare
/// console modes without duplicating the child handle, the identity-aware
/// teardown or the tree walk.
pub struct SystemSpawnerFlags {
    pub creation_flags: u32,
}

impl ProcessSpawner for SystemSpawnerFlags {
    fn spawn(
        &self,
        proof: &dyn BackendCapabilityProof,
        spec: &SpawnSpec,
    ) -> io::Result<Box<dyn ChildHandle>> {
        // Same refusal as `SystemSpawner`: varying the console mode does not
        // create a confinement boundary, so it cannot be used to reach one.
        if let Err(refusal) = proof.availability().authorize() {
            return Err(refuse_spawn(&refusal));
        }
        spawn_system_child(spec, self.creation_flags)
    }
}

fn spawn_system_child(
    spec: &SpawnSpec,
    creation_flags: u32,
) -> io::Result<Box<dyn ChildHandle>> {
    let mut command = std::process::Command::new(&spec.program);
    command
        .current_dir(&spec.cwd)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    apply_spawn_arguments(&mut command, spec);
    match &spec.env {
        EnvMode::Inherit => command_environment::apply_inherited(&mut command),
        EnvMode::Replace(entries) => command_environment::apply_explicit(&mut command, entries)?,
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(creation_flags | CREATE_SUSPENDED_FLAG);
    }
    #[cfg(not(windows))]
    {
        let _ = creation_flags;
    }
    let mut child = command.spawn()?;
    #[cfg(windows)]
    let job = match WindowsJob::assign_and_resume(&child) {
        Ok(job) => job,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
    };
    Ok(Box::new(SystemChild {
        child: Some(child),
        #[cfg(windows)]
        job: Some(job),
    }))
}

struct SystemChild {
    child: Option<std::process::Child>,
    #[cfg(windows)]
    job: Option<WindowsJob>,
}

#[cfg(windows)]
pub(crate) struct WindowsJob {
    handle: isize,
}

#[cfg(windows)]
unsafe impl Send for WindowsJob {}

#[cfg(windows)]
impl WindowsJob {
    pub(crate) fn assign_and_resume(child: &std::process::Child) -> io::Result<Self> {
        use std::mem::{size_of, zeroed};
        use std::os::windows::io::AsRawHandle;

        const JOB_OBJECT_EXTENDED_LIMIT_INFORMATION_CLASS: i32 = 9;
        const JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: u32 = 0x0000_2000;

        let handle = unsafe { CreateJobObjectW(std::ptr::null_mut(), std::ptr::null()) };
        if handle == 0 {
            return Err(io::Error::last_os_error());
        }

        let mut information: JobObjectExtendedLimitInformation = unsafe { zeroed() };
        information.basic_limit_information.limit_flags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let configured = unsafe {
            SetInformationJobObject(
                handle,
                JOB_OBJECT_EXTENDED_LIMIT_INFORMATION_CLASS,
                (&mut information as *mut JobObjectExtendedLimitInformation).cast(),
                size_of::<JobObjectExtendedLimitInformation>() as u32,
            )
        };
        if configured == 0 {
            let error = io::Error::last_os_error();
            unsafe { CloseHandle(handle) };
            return Err(error);
        }

        let assigned = unsafe { AssignProcessToJobObject(handle, child.as_raw_handle() as isize) };
        if assigned == 0 {
            let error = io::Error::last_os_error();
            unsafe { CloseHandle(handle) };
            return Err(error);
        }
        let job = Self { handle };
        if let Err(error) = resume_process_thread(child.id()) {
            let _ = job.terminate();
            return Err(error);
        }
        Ok(job)
    }

    pub(crate) fn terminate(&self) -> io::Result<()> {
        if unsafe { TerminateJobObject(self.handle, 1) } == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
}

#[cfg(windows)]
fn resume_process_thread(pid: u32) -> io::Result<()> {
    const TH32CS_SNAPTHREAD: u32 = 0x0000_0004;
    const THREAD_SUSPEND_RESUME: u32 = 0x0002;
    const INVALID_HANDLE_VALUE: isize = -1;

    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    let mut entry: ThreadEntry32 = unsafe { std::mem::zeroed() };
    entry.size = std::mem::size_of::<ThreadEntry32>() as u32;
    let mut found = unsafe { Thread32First(snapshot, &mut entry) } != 0;
    let mut thread_id = None;
    while found {
        if entry.owner_process_id == pid {
            thread_id = Some(entry.thread_id);
            break;
        }
        found = unsafe { Thread32Next(snapshot, &mut entry) } != 0;
    }
    unsafe { CloseHandle(snapshot) };
    let thread_id = thread_id.ok_or_else(|| {
        io::Error::new(io::ErrorKind::NotFound, "suspended child thread was not found")
    })?;
    let thread = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, thread_id) };
    if thread == 0 {
        return Err(io::Error::last_os_error());
    }
    let resumed = unsafe { ResumeThread(thread) };
    let resume_error = (resumed == u32::MAX).then(io::Error::last_os_error);
    unsafe { CloseHandle(thread) };
    match resume_error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

#[cfg(windows)]
impl Drop for WindowsJob {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.handle) };
    }
}

#[cfg(windows)]
#[repr(C)]
struct JobObjectBasicLimitInformation {
    per_process_user_time_limit: i64,
    per_job_user_time_limit: i64,
    limit_flags: u32,
    minimum_working_set_size: usize,
    maximum_working_set_size: usize,
    active_process_limit: u32,
    affinity: usize,
    priority_class: u32,
    scheduling_class: u32,
}

#[cfg(windows)]
#[repr(C)]
struct IoCounters {
    read_operation_count: u64,
    write_operation_count: u64,
    other_operation_count: u64,
    read_transfer_count: u64,
    write_transfer_count: u64,
    other_transfer_count: u64,
}

#[cfg(windows)]
#[repr(C)]
struct JobObjectExtendedLimitInformation {
    basic_limit_information: JobObjectBasicLimitInformation,
    io_info: IoCounters,
    process_memory_limit: usize,
    job_memory_limit: usize,
    peak_process_memory_used: usize,
    peak_job_memory_used: usize,
}

#[cfg(windows)]
#[repr(C)]
struct ThreadEntry32 {
    size: u32,
    usage_count: u32,
    thread_id: u32,
    owner_process_id: u32,
    base_priority: i32,
    delta_priority: i32,
    flags: u32,
}

#[cfg(windows)]
#[link(name = "kernel32")]
extern "system" {
    fn CreateJobObjectW(
        job_attributes: *mut std::ffi::c_void,
        name: *const u16,
    ) -> isize;
    fn SetInformationJobObject(
        job: isize,
        information_class: i32,
        information: *mut std::ffi::c_void,
        information_length: u32,
    ) -> i32;
    fn AssignProcessToJobObject(
        job: isize,
        process: isize,
    ) -> i32;
    fn TerminateJobObject(job: isize, exit_code: u32) -> i32;
    fn CreateToolhelp32Snapshot(flags: u32, process_id: u32) -> isize;
    fn Thread32First(snapshot: isize, entry: *mut ThreadEntry32) -> i32;
    fn Thread32Next(snapshot: isize, entry: *mut ThreadEntry32) -> i32;
    fn OpenThread(access: u32, inherit_handle: i32, thread_id: u32) -> isize;
    fn ResumeThread(thread: isize) -> u32;
    fn CloseHandle(object: isize) -> i32;
}

impl ChildHandle for SystemChild {
    fn pid(&self) -> u32 {
        self.child.as_ref().map(|child| child.id()).unwrap_or(0)
    }

    fn try_wait(&mut self) -> io::Result<Option<ExitReport>> {
        let Some(child) = self.child.as_mut() else {
            return Ok(None);
        };
        let Some(status) = child.try_wait()? else {
            return Ok(None);
        };
        #[cfg(windows)]
        if let Some(job) = self.job.take() {
            let terminated = job.terminate();
            drop(job);
            terminated?;
        }
        Ok(Some(ExitReport { code: status.code() }))
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
        let mut report = TreeKillReport {
            walker_exit_code: None,
            walker_visited: 0,
            walker_stderr_bytes: 0,
            walker_error: None,
            direct_kill: false,
        };
        #[cfg(windows)]
        {
            match self.job.as_ref() {
                Some(job) => match job.terminate() {
                    Ok(()) => {
                        report.walker_exit_code = Some(0);
                        report.walker_visited = 1;
                    }
                    Err(error) => {
                        report.walker_error = Some(format!("could not terminate Job Object: {error}"));
                    }
                },
                None => {
                    report.walker_error = Some("child has no Job Object confinement".to_owned());
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

/// What became of the output readers after the grace window expired.
///
/// This exists for one open investigation (C-FINDING-01): `readers_orphaned`
/// only says the pipe was *still* held when the grace ran out. It cannot
/// distinguish "the grace was a little too short" from "something still holds
/// the write end". `stdout_released`/`stderr_released` do: they are sampled
/// later, so a pipe that is released shortly after the orphan is a timing
/// artefact, while one that is still held after seconds is a real handle leak.
///
/// Diagnostic only. It is never consulted by the lifecycle, and the detached
/// threads are never joined.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReaderPostMortem {
    /// How many reader threads were still unfinished when the grace expired
    /// (0..=2). Zero means the readers were joined normally.
    pub pending_at_orphan: u8,
    /// Clock reading when the readers were declared orphaned, if they were.
    pub orphan_observed_at_ms: Option<i64>,
    /// Whether the stdout pipe has since been released.
    ///
    /// `None` means the reader was never abandoned, so there is nothing to
    /// report — distinct from `Some(false)`, which is "detached and still
    /// holding the write end open".
    pub stdout_released: Option<bool>,
    pub stderr_released: Option<bool>,
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
    /// Reader threads that were detached when the grace window expired.
    ///
    /// Detaching is the same thing that dropping a `JoinHandle` does — the
    /// thread keeps running either way — so retaining the handle changes no
    /// behaviour. It only lets a diagnostic ask, afterwards, whether the pipe
    /// was *ever* released, and how long that took. Nothing here is joined, and
    /// nothing in the lifecycle reads it.
    abandoned_stdout: Option<JoinHandle<()>>,
    abandoned_stderr: Option<JoinHandle<()>>,
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
    /// How many reader threads were still unfinished when the grace expired.
    /// Evidence only: 0 when the peers reached EOF in time, up to 2 otherwise.
    readers_pending_at_orphan: u8,
    /// Clock reading at the moment the readers were declared orphaned.
    orphan_observed_at_ms: Option<i64>,
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
    /// The execution decision this manager consults, supplied by its caller.
    ///
    /// An instance field, deliberately: a process-global proof meant two
    /// managers in one process could not hold different decisions, so a test
    /// that authorized execution changed the result of a test that asserted the
    /// refusal. Production constructs its managers with [`HostVerifiedBackend`],
    /// which refuses.
    proof: Arc<dyn BackendCapabilityProof>,
    config: ManagerConfig,
    jobs: Mutex<BTreeMap<String, Arc<JobEntry>>>,
    keys: Mutex<BTreeMap<(String, String), KeyClaim>>,
    seq: AtomicU64,
}

impl ProcessJobManager {
    /// Production constructor. Always refuses on this Host, because
    /// [`HostVerifiedBackend`] always refuses.
    pub fn new(
        clock: Arc<dyn Clock>,
        spawner: Arc<dyn ProcessSpawner>,
        probe: Arc<dyn IdentityProbe>,
        config: ManagerConfig,
    ) -> Self {
        Self::with_backend_proof(clock, spawner, probe, config, Arc::new(HostVerifiedBackend))
    }

    /// Constructor with an explicit execution decision.
    ///
    /// This is the injection point for tests and conformance harnesses: they
    /// pass [`AvailableInTests`] to exercise the authorized path with a real
    /// child. It is also where a future verified backend adapter is wired: the
    /// adapter supplies a proof that reports `Available` with the verified
    /// binary/config digest, and nothing else in this file changes.
    pub fn with_backend_proof(
        clock: Arc<dyn Clock>,
        spawner: Arc<dyn ProcessSpawner>,
        probe: Arc<dyn IdentityProbe>,
        config: ManagerConfig,
        proof: Arc<dyn BackendCapabilityProof>,
    ) -> Self {
        Self {
            clock,
            spawner,
            probe,
            proof,
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
    /// Trusted Host entry: re-check the authoritative admission immediately
    /// before this manager creates any state. The verifier is supplied by the
    /// Host adapter, never from tool input; no credential enters the spawner.
    pub(crate) fn start_authorized(
        &self,
        request: StartRequest,
        verify: impl FnOnce(&StartRequest) -> Result<(), JobError>,
    ) -> Result<StartOutcome, JobError> {
        verify(&request)?;
        self.start(request)
    }

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
        // The confinement decision is checked before the job is published, so a
        // refused request leaves no job record, no checkpoint and nothing a
        // later call could mistake for work already in flight. It is also
        // checked on every call rather than cached, so an availability change
        // cannot be outrun by a caller holding an old answer.
        //
        // The decision comes from this instance's own proof, so a manager
        // constructed with a refusing proof refuses regardless of what any other
        // manager in the process was constructed with.
        if let Err(refusal) = self.proof.availability().authorize() {
            return Err(sandbox_unavailable_error(&refusal));
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
                readers_pending_at_orphan: 0,
                orphan_observed_at_ms: None,
                drain_deadline: None,
                teardown_started: false,
                // Set from the moment the record is visible: between here and the
                // end of `launch` there is no child to terminate yet, and teardown
                // must not treat that absence as "already gone".
                launch_in_flight: true,
            }),
            output: Arc::new(Mutex::new(OutputWindow::new(self.config.stream_retention_bytes))),
            control: Mutex::new(ChildControl {
                child: None,
                stdout_reader: None,
                stderr_reader: None,
                abandoned_stdout: None,
                abandoned_stderr: None,
            }),
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
        let spawned = self.spawner.spawn(self.proof.as_ref(), &entry.spec);
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

    /// What became of the output readers, including whether a pipe that was
    /// still held when the grace expired has since been released.
    ///
    /// A pure read, like every other accessor here: sampling it repeatedly is
    /// how a caller measures "released after N ms" instead of only "not
    /// released in time". `None` when the job is unknown, or when the readers
    /// were joined normally (nothing was abandoned).
    pub fn reader_post_mortem(&self, job_id: &str) -> Option<ReaderPostMortem> {
        let entry = self.entry(job_id).ok()?;
        let control = lock(&entry.control);
        let state = lock(&entry.state);
        Some(ReaderPostMortem {
            pending_at_orphan: state.readers_pending_at_orphan,
            orphan_observed_at_ms: state.orphan_observed_at_ms,
            stdout_released: control
                .abandoned_stdout
                .as_ref()
                .map(|handle| handle.is_finished()),
            stderr_released: control
                .abandoned_stderr
                .as_ref()
                .map(|handle| handle.is_finished()),
        })
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
                // The handles are kept on the side (never joined) so the
                // investigation into *who* holds the pipe can ask later whether it
                // was ever released at all, instead of only knowing that it had
                // not been released yet when the grace expired.
                state.readers_orphaned = true;
                state.readers_pending_at_orphan =
                    u8::from(!stdout_done) + u8::from(!stderr_done);
                drop(state);
                // Taken after the state lock is released: the clock is an injected
                // dependency and must never be called while holding a lock.
                let orphan_at = self.clock.now_ms();
                lock(&entry.state).orphan_observed_at_ms = Some(orphan_at);
                if let Some(handle) = control.stdout_reader.take() {
                    control.abandoned_stdout = Some(handle);
                }
                if let Some(handle) = control.stderr_reader.take() {
                    control.abandoned_stderr = Some(handle);
                }
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
    use std::sync::atomic::{AtomicBool, AtomicUsize};
    use std::sync::mpsc;

    // -- test authorization --------------------------------------------------
    //
    // Authorization is injected per manager instance via
    // `ProcessJobManager::with_backend_proof`. There is no global proof to
    // install, leak or restore, so no test can change another test's decision.

    /// The lifecycle suites in this module are about manager semantics
    /// (idempotency, capacity, cancellation, output cursors, recovery), all of
    /// which sit behind the confinement decision, so they inject an authorizing
    /// proof when they build their manager.
    ///
    /// Nothing global is touched: injection happens per manager instance in
    /// [`ProcessJobManager::with_backend_proof`]. That is what lets a test in
    /// this module authorize execution while another test in the same binary
    /// simultaneously asserts the refusal.
    fn lifecycle_proof() -> Arc<dyn BackendCapabilityProof> {
        Arc::new(AvailableInTests::host_user_unconfined(
            "process-jobs-lifecycle-tests",
        ))
    }

    // -- backend capability authority ----------------------------------------

    /// The production constructor must refuse, whether or not some other test in
    /// this binary built an authorized manager.
    #[test]
    fn production_constructor_refuses_start_before_any_job_is_published() {
        let spawner: Arc<ScriptedSpawner> = ScriptedSpawner::new(vec![]);
        let manager = ProcessJobManager::new(
            Arc::new(ManualClock::default()),
            Arc::clone(&spawner) as Arc<dyn ProcessSpawner>,
            Arc::new(ScriptedProbe::new(vec![])),
            ManagerConfig::default(),
        );
        let error = manager
            .start(request("denied", "echo nope"))
            .expect_err("the production constructor must refuse");
        assert_eq!(error.code(), "sandbox_unavailable");
        assert!(
            error.message().contains("sandbox_unavailable"),
            "{error:?}"
        );
        assert_eq!(
            spawner.spawn_count(),
            0,
            "a refused start must not reach the spawner at all"
        );
    }

    /// Two managers alive at once must be able to hold *different* decisions.
    /// This is the direct regression test for the global-proof defect: with one
    /// process-global proof, whichever manager was built second decided for both.
    #[test]
    fn two_managers_in_one_process_keep_independent_decisions() {
        let authorized_spawner: Arc<ScriptedSpawner> =
            ScriptedSpawner::new(vec![scripted_plan(101, "ok")]);
        let authorized = manager(
            Arc::clone(&authorized_spawner),
            Arc::new(ScriptedProbe::new(vec![(101, Liveness::Alive { start_marker: Some(1) })])),
            Arc::new(ManualClock::default()),
            ManagerConfig::default(),
        );
        let refusing_spawner_concrete: Arc<ScriptedSpawner> = ScriptedSpawner::new(vec![]);
        let refusing_spawner: Arc<dyn ProcessSpawner> = refusing_spawner_concrete;
        let refusing = ProcessJobManager::new(
            Arc::new(ManualClock::default()),
            refusing_spawner,
            Arc::new(ScriptedProbe::new(vec![])),
            ManagerConfig::default(),
        );

        // Order matters to the old defect: assert the refusing manager *after*
        // the authorized one exists, and again in the other order below.
        assert!(
            refusing.start(request("k", "echo nope")).is_err(),
            "the refusing manager must still refuse while an authorized manager exists"
        );
        assert!(
            authorized.start(request("k", "echo ok")).is_ok(),
            "the authorized manager must still be authorized"
        );
        // And the refusing manager is still refusing afterwards.
        assert!(refusing.start(request("k2", "echo nope")).is_err());
        assert_eq!(authorized_spawner.spawn_count(), 1);
    }

    /// Order-independent by construction: refuse first, then authorize, then
    /// re-check the refusing manager.
    #[test]
    fn an_authorized_manager_never_opens_the_gate_for_a_refusing_one() {
        let refusing_spawner_concrete: Arc<ScriptedSpawner> = ScriptedSpawner::new(vec![]);
        let refusing: Arc<dyn ProcessSpawner> = refusing_spawner_concrete;
        let refusing = ProcessJobManager::new(
            Arc::new(ManualClock::default()),
            refusing,
            Arc::new(ScriptedProbe::new(vec![])),
            ManagerConfig::default(),
        );
        assert!(refusing.start(request("a", "echo nope")).is_err());

        let authorized_spawner: Arc<ScriptedSpawner> =
            ScriptedSpawner::new(vec![scripted_plan(102, "ok")]);
        let authorized = manager(
            Arc::clone(&authorized_spawner),
            Arc::new(ScriptedProbe::new(vec![(102, Liveness::Alive { start_marker: Some(1) })])),
            Arc::new(ManualClock::default()),
            ManagerConfig::default(),
        );
        assert!(authorized.start(request("b", "echo ok")).is_ok());

        assert!(
            refusing.start(request("c", "echo nope")).is_err(),
            "authorizing a different manager must not open this one's gate"
        );
    }

    /// A spawner handed a refusing proof must refuse even when called directly,
    /// so reaching the low-level seam is not a way around the manager.
    #[test]
    fn system_spawner_refuses_when_handed_a_refusing_proof() {
        let spec = SpawnSpec {
            program: "cmd.exe".into(),
            args: vec!["/C".into(), "echo unconfined".into()],
            cwd: std::env::temp_dir().to_string_lossy().into_owned(),
            env: EnvMode::Inherit,
        };
        // `Box<dyn ChildHandle>` is not `Debug`, so match rather than expect_err.
        match SystemSpawner.spawn(&HostVerifiedBackend, &spec) {
            Ok(child) => panic!("the refusing proof must not create a child: pid {}", child.pid()),
            Err(error) => {
                assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
                assert!(error.to_string().contains("sandbox_unavailable"), "{error}");
            }
        }
        // The same seam with an authorizing proof is allowed through, proving the
        // refusal above came from the proof and not from something incidental.
        assert!(SystemSpawner
            .spawn(&AvailableInTests::host_user_unconfined("spawner-probe"), &spec)
            .is_ok());
    }

    /// Stress the independence property: many authorized and refusing managers
    /// created and used concurrently, every refusal must survive.
    ///
    /// This is the shape of the defect O reproduced against the previous
    /// revision (a refusing guard was overridden by another test installing an
    /// authorizing one). With per-instance injection there is no shared state to
    /// race on, so this asserts the invariant rather than the mechanism: a
    /// manager built with the refusing proof refuses, no matter how many
    /// authorized managers exist at the same time.
    #[test]
    fn refusing_managers_stay_refusing_under_concurrent_authorized_managers() {
        let stop = Arc::new(AtomicBool::new(false));
        let mut handlers = Vec::new();

        // Continuously build and use authorized managers, so an authorizing
        // decision is always "live" while the refusals below are checked.
        for index in 0..4 {
            let stop = Arc::clone(&stop);
            handlers.push(std::thread::spawn(move || {
                while !stop.load(Ordering::SeqCst) {
                    let spawner: Arc<ScriptedSpawner> =
                        ScriptedSpawner::new(vec![scripted_plan(700 + index, "x")]);
                    let authorized = manager(
                        Arc::clone(&spawner),
                        Arc::new(ScriptedProbe::new(vec![(
                            700 + index,
                            Liveness::Alive { start_marker: Some(1) },
                        )])),
                        Arc::new(ManualClock::default()),
                        ManagerConfig::default(),
                    );
                    assert!(
                        authorized.start(request("live", "echo x")).is_ok(),
                        "an authorized manager must stay authorized"
                    );
                }
            }));
        }

        // Meanwhile, refusals must be unaffected by all of that activity.
        for _ in 0..200 {
            let spawner: Arc<ScriptedSpawner> = ScriptedSpawner::new(vec![]);
            let refusing = ProcessJobManager::new(
                Arc::new(ManualClock::default()),
                Arc::clone(&spawner) as Arc<dyn ProcessSpawner>,
                Arc::new(ScriptedProbe::new(vec![])),
                ManagerConfig::default(),
            );
            let error = refusing
                .start(request("denied", "echo nope"))
                .expect_err("a refusing manager must refuse while authorized ones are live");
            assert_eq!(error.code(), "sandbox_unavailable");
            assert_eq!(spawner.spawn_count(), 0);
        }

        stop.store(true, Ordering::SeqCst);
        for handler in handlers {
            handler.join().expect("authorized-manager thread panicked");
        }
    }

    #[test]
    fn every_required_capability_has_a_stable_name_and_no_duplicates() {
        let mut names: Vec<&str> = RequiredCapability::ALL.iter().map(|c| c.as_str()).collect();
        let count = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), count, "capability names must be unique");
        assert!(names.contains(&"system_dns_fence"));
        assert!(names.contains(&"filesystem_write_confinement"));
    }

    #[test]
    fn capability_refusal_lists_the_missing_capabilities_verbatim() {
        let refusal = BackendRefusal::CapabilitiesNotMet {
            backend: ExecutionBackend::SrtWindows,
            missing: vec![
                RequiredCapability::SystemDnsFence,
                RequiredCapability::DistinctExecutionIdentity,
            ],
        };
        let reason = refusal.reason();
        assert!(reason.contains("srt_windows"), "{reason}");
        assert!(reason.contains("system_dns_fence"), "{reason}");
        assert!(reason.contains("distinct_execution_identity"), "{reason}");
    }

    /// On this Host nothing is provisioned; the probe must say so rather than
    /// assume a backend exists. Written so that a machine where SRT *is*
    /// installed does not fail the suite spuriously.
    #[test]
    fn native_confinement_probe_reports_the_actual_machine_state() {
        let report = probe_native_confinement();

        // The production proof is what the shipped artifact behaves like, and it
        // must refuse regardless of what the probe found. This is a pure value
        // check against `HostVerifiedBackend`: no global state is involved, so
        // no other test can influence it.
        let production = HostVerifiedBackend.availability();
        assert!(
            !production.is_available(),
            "the production backend must never report an available backend"
        );
        assert_eq!(
            production.authorize().unwrap_err(),
            BackendRefusal::NoVerifiedBackend
        );

        // Report the observed state for the log; it is not consulted by any
        // assertion, because a probe result must never be the reason a step
        // passes or fails on its own.
        eprintln!("native confinement probe: {report:?}");

        // A probe result is a diagnostic and must not, by itself, become a
        // verification result.
        assert!(
            !HostVerifiedBackend.availability().is_available(),
            "reading the probe report must not change the production decision"
        );
    }

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
        fn spawn(
            &self,
            proof: &dyn BackendCapabilityProof,
            _spec: &SpawnSpec,
        ) -> io::Result<Box<dyn ChildHandle>> {
            // A scripted spawner stands in for the OS, so it must still honour
            // the decision it was handed: a manager constructed with a refusing
            // proof must not appear to have started anything. This keeps the
            // lifecycle tests honest about which side of the gate they exercise.
            if let Err(refusal) = proof.availability().authorize() {
                return Err(refuse_spawn(&refusal));
            }
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

    // -- fake for the pipe-retention post-mortem ------------------------------

    /// How long a [`HeldPipe`] waits for release before giving up.
    ///
    /// Bounded on purpose: a test that never releases its pipe must not leave a
    /// thread spinning forever.
    const HELD_PIPE_MAX_WAIT: Duration = Duration::from_secs(6);

    /// A reader that does not reach EOF until it is released — the fake
    /// equivalent of a write end held open by a process outside the tree walk.
    struct HeldPipe {
        released: Arc<AtomicBool>,
    }

    impl Read for HeldPipe {
        fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
            let deadline = Instant::now() + HELD_PIPE_MAX_WAIT;
            while Instant::now() < deadline {
                if self.released.load(Ordering::SeqCst) {
                    return Ok(0);
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            Ok(0)
        }
    }

    /// A child whose two pipes stay open until the test releases them.
    struct HeldPipeChild {
        pid: u32,
        terminated: Arc<AtomicUsize>,
        stdout: Option<Box<dyn Read + Send>>,
        stderr: Option<Box<dyn Read + Send>>,
    }

    impl ChildHandle for HeldPipeChild {
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
            self.stdout.take()
        }
        fn take_stderr(&mut self) -> Option<Box<dyn Read + Send>> {
            self.stderr.take()
        }
        fn terminate_tree(&mut self) -> TreeKillReport {
            self.terminated.fetch_add(1, Ordering::SeqCst);
            TreeKillReport {
                walker_exit_code: Some(0),
                walker_visited: 3,
                walker_stderr_bytes: 0,
                walker_error: None,
                direct_kill: true,
            }
        }
    }

    struct HeldPipeSpawner {
        pid: u32,
        terminated: Arc<AtomicUsize>,
        released: Arc<AtomicBool>,
    }

    impl ProcessSpawner for HeldPipeSpawner {
        fn spawn(
            &self,
            proof: &dyn BackendCapabilityProof,
            _spec: &SpawnSpec,
        ) -> io::Result<Box<dyn ChildHandle>> {
            if let Err(refusal) = proof.availability().authorize() {
                return Err(refuse_spawn(&refusal));
            }
            Ok(Box::new(HeldPipeChild {
                pid: self.pid,
                terminated: Arc::clone(&self.terminated),
                stdout: Some(Box::new(HeldPipe { released: Arc::clone(&self.released) })),
                stderr: Some(Box::new(HeldPipe { released: Arc::clone(&self.released) })),
            }))
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
        fn spawn(
            &self,
            proof: &dyn BackendCapabilityProof,
            _spec: &SpawnSpec,
        ) -> io::Result<Box<dyn ChildHandle>> {
            if let Err(refusal) = proof.availability().authorize() {
                return Err(refuse_spawn(&refusal));
            }
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
        // Lifecycle suites run on the authorized path, injected into this one
        // manager; the refusal has its own tests using the production
        // constructor. Nothing global is set, so this cannot affect them.
        ProcessJobManager::with_backend_proof(clock, spawner, probe, config, lifecycle_proof())
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
    fn admission_verifier_rejects_before_manager_state_or_spawn() {
        let spawner = ScriptedSpawner::new(vec![]);
        let manager = manager(Arc::clone(&spawner),
            Arc::new(ScriptedProbe::new(vec![])), Arc::new(ManualClock::default()),
            ManagerConfig::default());
        let error = manager.start_authorized(request("host-dispatch", "echo nope"), |_| {
            Err(JobError::CredentialMismatch("changed authoritative fields".into()))
        }).unwrap_err();
        assert_eq!(error.code(), "credential_mismatch");
        assert_eq!(spawner.spawn_count(), 0);
        assert!(lock(&manager.jobs).is_empty());
        assert!(lock(&manager.keys).is_empty());
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
        let manager = Arc::new(ProcessJobManager::with_backend_proof(
            clock,
            spawner_for_manager,
            probe_for_manager,
            ManagerConfig::default(),
            lifecycle_proof(),
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
        let manager = Arc::new(ProcessJobManager::with_backend_proof(
            Arc::new(ManualClock::default()),
            spawner_for_manager,
            probe_for_manager,
            ManagerConfig::default(),
            lifecycle_proof(),
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
        let manager = Arc::new(ProcessJobManager::with_backend_proof(
            Arc::new(ManualClock::default()),
            spawner_for_manager,
            Arc::new(ScriptedProbe::new(vec![])),
            ManagerConfig { max_jobs: 2, ..ManagerConfig::default() },
            lifecycle_proof(),
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

    // -- what happened to the pipes (C-FINDING-01 evidence path) -------------

    /// An orphaned reader must report *which* pipe was still held, and the
    /// report must be able to observe a later release.
    ///
    /// That second half is the whole point: `readers_orphaned` alone cannot tell
    /// "the grace was 50 ms too short" from "the write end is held by something
    /// we cannot see". Without an observable release time the investigation has
    /// only an opinion.
    #[test]
    fn orphaned_readers_report_which_pipe_is_still_held_and_when_it_is_released() {
        let pid = 940;
        let terminated = Arc::new(AtomicUsize::new(0));
        let released = Arc::new(AtomicBool::new(false));
        let spawner: Arc<dyn ProcessSpawner> = Arc::new(HeldPipeSpawner {
            pid,
            terminated: Arc::clone(&terminated),
            released: Arc::clone(&released),
        });
        let probe: Arc<dyn IdentityProbe> =
            Arc::new(ScriptedProbe::new(vec![(pid, Liveness::Alive { start_marker: Some(5) })]));
        let clock: Arc<dyn Clock> = Arc::new(ManualClock::default());
        let manager = ProcessJobManager::with_backend_proof(
            clock,
            spawner,
            probe,
            ManagerConfig { reader_grace: Duration::from_millis(50), ..ManagerConfig::default() },
            lifecycle_proof(),
        );

        let mut req = request("held", "echo held");
        req.execution_budget = Duration::from_millis(1);
        let job_id = manager.start(req).unwrap().view().job_id.clone();
        let view = wait_terminal(&manager, &job_id);

        assert_eq!(view.state, JobRunState::TimedOut, "{view:?}");
        assert!(
            view.readers_orphaned,
            "a pipe that is still held after the grace must be reported: {view:?}"
        );

        let post = manager.reader_post_mortem(&job_id).expect("post mortem is readable");
        assert_eq!(post.pending_at_orphan, 2, "both streams were still open");
        assert!(post.orphan_observed_at_ms.is_some(), "{post:?}");
        assert_eq!(
            post.stdout_released,
            Some(false),
            "an abandoned reader that is still running must report the pipe as held, \
             not as absent: {post:?}"
        );
        assert_eq!(post.stderr_released, Some(false), "{post:?}");

        // The release is observable afterwards. This is what separates "held
        // forever" from "held until the grace expired".
        released.store(true, Ordering::SeqCst);
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut observed_release = false;
        while Instant::now() < deadline {
            let post = manager.reader_post_mortem(&job_id).unwrap();
            if post.stdout_released == Some(true) && post.stderr_released == Some(true) {
                observed_release = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(
            observed_release,
            "a pipe released after the grace window must be reported as released, \
             otherwise the report cannot distinguish the two cases"
        );
    }

    /// The normal path must not look like a leak: joined readers report no
    /// abandonment at all, which is distinguishable from "held".
    #[test]
    fn joined_readers_report_no_abandonment() {
        let pid = 941;
        let spawner = ScriptedSpawner::new(vec![scripted_plan(pid, "hello")]);
        let probe = Arc::new(ScriptedProbe::new(vec![(pid, Liveness::Gone)]));
        let manager = manager(
            Arc::clone(&spawner),
            Arc::clone(&probe),
            Arc::new(ManualClock::default()),
            ManagerConfig::default(),
        );

        let job_id = manager.start(request("joined", "echo hello")).unwrap().view().job_id.clone();
        let view = wait_terminal(&manager, &job_id);
        assert!(!view.readers_orphaned, "{view:?}");

        let post = manager.reader_post_mortem(&job_id).expect("post mortem is readable");
        assert_eq!(post.pending_at_orphan, 0, "{post:?}");
        assert_eq!(post.orphan_observed_at_ms, None, "{post:?}");
        assert_eq!(
            post.stdout_released, None,
            "nothing was abandoned, so there is no pipe to report on: {post:?}"
        );
        assert_eq!(post.stderr_released, None, "{post:?}");
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
    fn invalid_utf8_keeps_exact_raw_byte_pages_and_monotonic_raw_offsets() {
        let mut window = OutputWindow::new(16);
        window.push(Stream::Stdout, &[0xff, b'A', 0xfe, b'B']);

        let first = window.page(Stream::Stdout, 0, 1);
        assert_eq!(first.raw_bytes, vec![0xff]);
        assert_eq!(first.content, "�");
        assert_eq!(first.offset, 0);
        assert_eq!(first.next_offset, 1);

        let rest = window.page(Stream::Stdout, first.next_offset, 3);
        assert_eq!(rest.raw_bytes, vec![b'A', 0xfe, b'B']);
        assert_eq!(rest.content, "A�B");
        assert_eq!(rest.offset, 1);
        assert_eq!(rest.next_offset, 4);
        assert!(rest.at_end_of_available);
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
