//! Real-task evaluation entries: ONE synthetic regression and ONE real-model
//! acceptance, kept strictly apart and never substituted for each other.
//!
//! Both entries drive the SAME chain production uses:
//!
//! ```text
//! model endpoint (scripted local provider OR a real cloud model)
//!   -> real Node `pi-runtime` Kernel worker
//!   -> real KernelCoordinator live loop (leases, checkpoints, continuation)
//!   -> real GatewayPolicy Host dispatch
//!   -> real bundled OfficeCLI (`fox-office`) / real project readers
//! ```
//!
//! Tiers, stated honestly:
//!
//! * `synthetic-provider` — `real_task_evaluation_through_host_kernel_runtime_and_real_tools`.
//!   The entry always creates a scripted loopback HTTP/SSE provider
//!   (`fox-real-eval-synthetic`, `http://127.0.0.1:<ephemeral>/v1`, api key
//!   `local-eval-only`) and replays a fixed tool-call plan whose values are
//!   derived from the CURRENT source workbook. It proves the KERNEL / tool
//!   routing / GatewayPolicy / OfficeCLI chain and its independent checker — it
//!   does NOT exercise a cloud model, and it may assert an exact model-round
//!   count only because the plan is authored here.
//! * `real-cloud-model` — `real_task_evaluation_cloud`. The model endpoint comes
//!   from `FOX_EVAL_BASE_URL` / `FOX_EVAL_API_KEY` / `FOX_EVAL_MODEL`
//!   (`FOX_EVAL_API_TYPE` optional). No local provider is invented, the model
//!   decides its own tool calls and its own number of rounds, so the tier asserts
//!   only artifacts plus structural invariants. Without those variables the
//!   acceptance is recorded as 未执行 (`executed: false`, `notRunReason` names the
//!   missing variables) and the entry FAILS — it never falls back to the
//!   synthetic provider and never records a pass.
//!
//! Exact commands (run from `apps/desktop/src-tauri`):
//!
//! ```bash
//! # synthetic regression (the fast contract layer is `cargo test --lib real_eval`)
//! cargo test --lib real_task_evaluation_through_host_kernel_runtime_and_real_tools -- --ignored --nocapture
//! # real cloud-model acceptance (real credentials required; otherwise 未执行 + failure)
//! cargo test --lib real_task_evaluation_cloud -- --ignored --nocapture
//! ```
//!
//! The loose legacy filter `cargo test --lib real_task_evaluation -- --ignored`
//! selects BOTH entries now, so with no credentials it ends in the cloud entry's
//! explicit 未执行 failure; prefer the exact names above.
//!
//! Every entry writes into its own evaluation directory under
//! `output/claude-design-repair-20260915/eval/<tier>/evaluations/<evaluation id>/`
//! (or `FOX_EVAL_OUTPUT_ROOT`): report, scenario databases, snapshots and
//! artifacts of one evaluation are siblings, and a repeated evaluation reserves
//! a new directory instead of deleting or reusing the previous one. Earlier
//! rounds and the historical evidence stay untouched; the historical evidence
//! directory `output/claude-design-impl-20260913/eval` is never written again.
//!
//! The non-ignored tests below are the fast contract layer (derivation, plan
//! determinism, checker negative cases and the cloud not-run/identity contract)
//! and need neither Node, OfficeCLI nor credentials.

use super::*;
// O-REVIEW-03：O 完整编译发现本文件漏了这三个导入（e-compile-glue.patch），
// 导致 E-T-01/02/05 无法编译执行。此处按 O 已验证的形式补齐。
use crate::runtime_host::{
    create_fresh_host_tool_call, finalize_host_tool_execution, HostToolCallExecution,
};
use crate::runtime_host::{kernel_gateway, kernel_host, kernel_model_worker};
use calamine::{open_workbook_auto_from_rs, Data as CellData, Reader};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::io::{Cursor, Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// Fixed inputs, model configuration and output directory.
// ---------------------------------------------------------------------------

const SYNTHETIC_MODEL_ID: &str = "fox-real-eval-synthetic";
/// Literal placeholder of the loopback scripted provider. It is NOT a
/// credential: the synthetic listener accepts any bearer token.
const EVAL_API_KEY: &str = "local-eval-only";

/// This round's evidence root. One directory per round; earlier rounds (including
/// `claude-design-repair-20260914`) and the historical
/// `claude-design-impl-20260913` evidence stay exactly as they were produced.
/// `FOX_EVAL_OUTPUT_ROOT` may point the entry at another round directory; the
/// historical directory is refused either way.
const EVAL_ROUND_DIR: &str = "output/claude-design-repair-20260915/eval";
const EVAL_OUTPUT_ROOT_ENV: &str = "FOX_EVAL_OUTPUT_ROOT";
const HISTORICAL_EVAL_DIR: &str = "output/claude-design-impl-20260913/eval";

/// Real-model acceptance environment. Values are read here and NEVER written to
/// a report — only presence, source variable and the base-URL host are.
const CLOUD_ENV_BASE_URL: &str = "FOX_EVAL_BASE_URL";
const CLOUD_ENV_API_KEY: &str = "FOX_EVAL_API_KEY";
const CLOUD_ENV_MODEL: &str = "FOX_EVAL_MODEL";
const CLOUD_ENV_API_TYPE: &str = "FOX_EVAL_API_TYPE";
const CLOUD_DEFAULT_API_TYPE: &str = "openai-completions";
/// API types the real tier accepts. `faux` (the scripted adapter) is refused on
/// purpose so a "real cloud" run can never silently be a local fake.
const CLOUD_API_TYPES: [&str; 3] = [
    "openai-completions",
    "openai-responses",
    "anthropic-messages",
];
const SYNTHETIC_KEY_SOURCE: &str = "hardcoded loopback placeholder (not a credential)";
const CLOUD_KEY_SOURCE: &str = "FOX_EVAL_API_KEY (value never recorded)";
const DURABLE_ROUNDS_SOURCE: &str = "durable kernel model-response events";

// ---------------------------------------------------------------------------
// Phase trace and bounded watchdog for the real-chain entries.
//
// The synthetic entry runs a real Node worker and the pinned OfficeCLI, so a
// stall can come from a build lock, evaluation initialisation, the database, the
// worker handshake or the sidecar. A long unobserved wait tells nobody which one
// it was, so every step reports a phase, the current phase is printed whenever it
// changes, and a watchdog prints the stuck phase periodically and then ends the
// isolated evaluation at a bound instead of hanging forever. The parent test
// supervises that process; no watchdog may exit the shared test binary.
//
// The watchdog is a **guard**, started by both entries (synthetic and real
// cloud), and it stops when the evaluation it guards ends. A deadline that fires
// after the evaluation finished would kill the whole test binary and take
// unrelated tests down with it, which is a worse outcome than the stall it
// reports.
// ---------------------------------------------------------------------------

/// Deadline for one real-chain entry, in seconds (`FOX_EVAL_DEADLINE_SECS`).
const EVAL_DEADLINE_ENV: &str = "FOX_EVAL_DEADLINE_SECS";
const EVAL_DEADLINE_DEFAULT_SECS: u64 = 900;
/// How often the watchdog reports the phase it is still stuck in.
const EVAL_WATCHDOG_TICK: Duration = Duration::from_secs(15);
/// The watchdog checks for its own cancellation this often, so a finished
/// evaluation is not held by the tick.
const EVAL_WATCHDOG_SLICE: Duration = Duration::from_millis(50);

/// What the watchdog decided when its wait ended.
#[derive(Debug, PartialEq, Eq)]
enum WatchdogAction {
    /// Still inside the deadline: the same phase is held long enough to be
    /// worth a line, and that line is the only observable a stall gets.
    Report { phase: String, held_secs: u64 },
    /// Emit the last diagnostics; the external supervisor owns termination.
    Terminate { phase: String, held_secs: u64 },
}

fn watchdog_action(
    now: Instant,
    started: Instant,
    deadline: Duration,
    phase: &str,
    held_secs: u64,
) -> WatchdogAction {
    let elapsed = now.saturating_duration_since(started);
    if elapsed >= deadline {
        return WatchdogAction::Terminate {
            phase: phase.to_owned(),
            held_secs,
        };
    }
    WatchdogAction::Report {
        phase: phase.to_owned(),
        held_secs,
    }
}

/// The watchdog of one real-chain entry. Dropping it — on a pass, a failure or a
/// panic — stops the thread, so an evaluation that already finished can never
/// end the test process later on and take an unrelated test with it.
struct EvalWatchdog {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Drop for EvalWatchdog {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            // The thread returns within one slice of seeing the flag.
            let _ = thread.join();
        }
    }
}

fn eval_deadline_from_environment() -> Duration {
    Duration::from_secs(
        std::env::var(EVAL_DEADLINE_ENV)
            .ok()
            .and_then(|value| value.trim().parse::<u64>().ok())
            .filter(|value| *value > 0)
            .unwrap_or(EVAL_DEADLINE_DEFAULT_SECS),
    )
}

fn eval_trace_start() -> EvalWatchdog {
    eval_trace_start_with(eval_deadline_from_environment())
}

fn eval_trace_start_with(deadline: Duration) -> EvalWatchdog {
    let started = Instant::now();
    let stop = Arc::new(AtomicBool::new(false));
    let watching = stop.clone();
    let thread = std::thread::spawn(move || {
        loop {
            // Wait for the next report, but never past the deadline.
            let remaining = deadline.saturating_sub(started.elapsed());
            let wait_for = remaining.min(EVAL_WATCHDOG_TICK);
            let mut slept = Duration::ZERO;
            while slept < wait_for {
                if watching.load(Ordering::SeqCst) {
                    return;
                }
                std::thread::sleep(EVAL_WATCHDOG_SLICE.min(wait_for - slept));
                slept += EVAL_WATCHDOG_SLICE;
            }
            if watching.load(Ordering::SeqCst) {
                return;
            }
            let (phase, held) = eval_current_phase();
            let elapsed = started.elapsed();
            match watchdog_action(Instant::now(), started, deadline, &phase, held) {
                WatchdogAction::Report { phase, held_secs } => eprintln!(
                    "[eval-watchdog] t={}s phase={phase} held={held_secs}s deadline={}s",
                    elapsed.as_secs(),
                    deadline.as_secs()
                ),
                WatchdogAction::Terminate { phase, held_secs } => {
                    eprintln!(
                        "[eval-watchdog] DEADLINE: still in phase '{phase}' after {held_secs}s \
                         (total {}s); the isolated-process supervisor will stop this evaluation \
                         instead of waiting without evidence",
                        elapsed.as_secs()
                    );
                    for line in eval_phase_history() {
                        eprintln!("[eval-watchdog] {line}");
                    }
                    let _ = std::io::Write::flush(&mut std::io::stderr());
                    return;
                }
            }
        }
    });
    EvalWatchdog {
        stop,
        thread: Some(thread),
    }
}

const EVAL_CHILD_TEST: &str = "FOX_EVAL_CHILD_TEST";

fn evaluation_test_name(name: &str) -> String {
    // libtest names omit the crate prefix present in module_path!().
    let module = module_path!().split_once("::").unwrap().1;
    format!("{module}::{name}")
}

struct EvaluationChild(std::process::Child);

impl Drop for EvaluationChild {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_some() {
            return;
        }
        // Only this owned test process and its descendants are terminated.
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            let _ = std::process::Command::new("taskkill")
                .args(["/PID", &self.0.id().to_string(), "/T", "/F"])
                .creation_flags(0x08000000)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        }
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn run_evaluation_process(
    name: &str,
    deadline: Duration,
    extra_environment: &[(&str, &str)],
) -> Result<std::process::ExitStatus, String> {
    let test_name = evaluation_test_name(name);
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut command = std::process::Command::new(&executable);
    command.args(["--exact", &test_name, "--include-ignored", "--nocapture", "--test-threads=1"])
        .env(EVAL_CHILD_TEST, &test_name)
        .envs(extra_environment.iter().copied())
        .stdin(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // A Cargo-launched libtest can resolve native test dependencies from
        // target/debug while its directly spawned copy cannot. Preserve the
        // inherited search path and add only the executable's own debug root.
        let debug_dir = executable
            .parent()
            .and_then(std::path::Path::parent)
            .ok_or("evaluation executable is not under target/debug/deps")?;
        let mut search_path = vec![debug_dir.to_path_buf()];
        if let Some(inherited) = std::env::var_os("PATH") {
            search_path.extend(std::env::split_paths(&inherited));
        }
        command.env(
            "PATH",
            std::env::join_paths(search_path).map_err(|error| error.to_string())?,
        );
        command.creation_flags(0x08000000);
    }
    let mut child = EvaluationChild(command.spawn().map_err(|e| e.to_string())?);
    let started = Instant::now();
    loop {
        if let Some(status) = child.0.try_wait().map_err(|e| e.to_string())? {
            return Ok(status);
        }
        if started.elapsed() >= deadline {
            eprintln!("[eval-supervisor] DEADLINE entry={name} pid={} elapsedMs={} (phase trace above; existing evidence retained)",
                child.0.id(), started.elapsed().as_millis());
            return Err(format!("isolated evaluation {name} exceeded its deadline"));
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// Parent entries run only the named evaluation in a child process. A stalled
/// native tool can then be killed without ending unrelated libtest work.
fn supervise_evaluation(name: &str) -> bool {
    if std::env::var(EVAL_CHILD_TEST).ok().as_deref() == Some(evaluation_test_name(name).as_str()) {
        return false;
    }
    let status = run_evaluation_process(name, eval_deadline_from_environment(), &[])
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(status.success(), "isolated evaluation {name} failed: {status}");
    true
}

struct EvalPhaseState {
    phase: String,
    entered: Instant,
    /// Every phase this process entered, so a deadline names the whole path.
    history: Vec<(String, u64)>,
}

fn eval_phase_cell() -> &'static Mutex<EvalPhaseState> {
    static STATE: std::sync::OnceLock<Mutex<EvalPhaseState>> = std::sync::OnceLock::new();
    STATE.get_or_init(|| {
        Mutex::new(EvalPhaseState {
            phase: "start".to_owned(),
            entered: Instant::now(),
            history: Vec::new(),
        })
    })
}

/// Record and print the phase the entry is entering.
fn eval_phase(name: &str) {
    let mut state = match eval_phase_cell().lock() {
        Ok(state) => state,
        Err(_) => return,
    };
    let held = state.entered.elapsed().as_secs();
    let previous = state.phase.clone();
    state.history.push((previous, held));
    state.phase = name.to_owned();
    state.entered = Instant::now();
    eprintln!("[eval-phase] {name} (previous held {held}s)");
    let _ = std::io::Write::flush(&mut std::io::stderr());
}

fn eval_current_phase() -> (String, u64) {
    match eval_phase_cell().lock() {
        Ok(state) => (state.phase.clone(), state.entered.elapsed().as_secs()),
        Err(_) => ("<poisoned>".to_owned(), 0),
    }
}

/// `phase held Xs` for every phase entered so far, oldest first.
fn eval_phase_history() -> Vec<String> {
    match eval_phase_cell().lock() {
        Ok(state) => state
            .history
            .iter()
            .map(|(phase, held)| format!("phase={phase} held={held}s"))
            .collect(),
        Err(_) => vec!["<poisoned>".to_owned()],
    }
}
const PROVIDER_ROUNDS_SOURCE: &str = "loopback provider HTTP requests";
/// Recorded instead of a result whenever an acceptance did not run at all.
const NOT_RUN_MARKER: &str = "未执行";

// ---------------------------------------------------------------------------
// Acceptance modes. A mode fixes the tier name, the entry command, the output
// directory and the model identity; the two modes share the tool contract and
// the independent checker, and nothing may turn one into the other.
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum EvalMode {
    /// Scripted loopback HTTP/SSE provider (`synthetic-provider` tier).
    SyntheticProvider,
    /// Real endpoint from `FOX_EVAL_*` (`real-cloud-model` tier).
    RealCloudModel,
}

impl EvalMode {
    const ALL: [EvalMode; 2] = [EvalMode::SyntheticProvider, EvalMode::RealCloudModel];

    fn tier(self) -> &'static str {
        match self {
            Self::SyntheticProvider => "synthetic-provider",
            Self::RealCloudModel => "real-cloud-model",
        }
    }

    fn entry_command(self) -> &'static str {
        match self {
            Self::SyntheticProvider => {
                "cargo test --lib real_task_evaluation_through_host_kernel_runtime_and_real_tools -- --ignored --nocapture"
            }
            Self::RealCloudModel => {
                "cargo test --lib real_task_evaluation_cloud -- --ignored --nocapture"
            }
        }
    }

    fn report_name(self) -> &'static str {
        "real-task-eval-report.json"
    }

    /// Each tier owns its own output directory, so no run can overwrite another
    /// tier's — or the historical round's — evidence.
    fn run_dir(self) -> std::path::PathBuf {
        round_dir().join(self.tier())
    }

    /// One directory per **evaluation**, under the tier: a repeated run adds
    /// evidence instead of replacing the previous run's.
    fn evaluation_dir(self) -> std::path::PathBuf {
        self.run_dir().join("evaluations").join(evaluation_id())
    }

    fn artifact_dir(self, scenario: &str) -> std::path::PathBuf {
        self.evaluation_dir().join("artifacts").join(scenario)
    }
}

/// Identifies this process's evaluation: sortable, and distinct from every other
/// invocation's (a parallel run gets its own copy of every directory).
fn evaluation_id() -> &'static str {
    static ID: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    ID.get_or_init(|| {
        let seconds = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|value| value.as_secs())
            .unwrap_or(0);
        let random: String = uuid::Uuid::new_v4()
            .simple()
            .to_string()
            .chars()
            .take(8)
            .collect();
        format!("{seconds}-{random}")
    })
}

/// Create one directory that no earlier evaluation owns.
///
/// `create_dir` is the exclusion: a path that already exists is *evidence from
/// another run*, so it is never deleted, never truncated and never reused as a
/// silent fallback. Callers that must not collide (parallel evaluations) get a
/// distinct path from the same call.
fn reserve_directory(base: &std::path::Path, name: &str) -> Result<std::path::PathBuf, String> {
    std::fs::create_dir_all(base).map_err(|error| format!("{base:?}: {error}"))?;
    for attempt in 0..64u32 {
        let candidate = if attempt == 0 {
            base.join(name)
        } else {
            base.join(format!("{name}-retry{attempt}"))
        };
        match std::fs::create_dir(&candidate) {
            Ok(()) => return Ok(candidate),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("{candidate:?}: {error}")),
        }
    }
    Err(format!(
        "every directory under {base:?} named '{name}*' already holds evidence from an \
         earlier evaluation; refusing to reuse or delete it"
    ))
}

/// Model identity as recorded in a report: API type, model id, base URL and its
/// host. The API key is deliberately NOT part of this structure — a report
/// records only whether a key was supplied and from which variable.
#[derive(Clone, Debug, PartialEq)]
struct ModelIdentity {
    api_type: String,
    model_id: String,
    base_url: String,
    base_url_host: String,
    api_key_present: bool,
    api_key_source: &'static str,
    note: &'static str,
}

impl ModelIdentity {
    fn base_url_host_of(url: &str) -> String {
        match reqwest::Url::parse(url) {
            Ok(parsed) => match (parsed.host_str(), parsed.port()) {
                (Some(host), Some(port)) => format!("{host}:{port}"),
                (Some(host), None) => host.to_owned(),
                _ => String::from("<no-host>"),
            },
            Err(_) => String::from("<invalid-url>"),
        }
    }

    fn from_model_service(
        service: &Value,
        api_key_present: bool,
        api_key_source: &'static str,
    ) -> Self {
        let text = |key: &str| {
            service
                .get(key)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned()
        };
        let base_url = text("baseUrl");
        Self {
            api_type: text("apiType"),
            model_id: text("modelId"),
            base_url_host: Self::base_url_host_of(&base_url),
            base_url,
            api_key_present,
            api_key_source,
            note: "",
        }
    }

    /// The synthetic tier's identity: a loopback listener whose port the OS
    /// assigns per scenario.
    fn synthetic_placeholder() -> Self {
        Self {
            note: "loopback listener on 127.0.0.1 with an OS-assigned port (per scenario); no external endpoint",
            ..Self::from_model_service(
                &synthetic_model_service(synthetic_placeholder_address()),
                true,
                SYNTHETIC_KEY_SOURCE,
            )
        }
    }

    /// Identity of a not-run acceptance: empty on purpose, so a report can never
    /// imply a model that was never contacted.
    fn not_configured(reason: &'static str) -> Self {
        Self {
            api_type: String::new(),
            model_id: String::new(),
            base_url: String::new(),
            base_url_host: String::new(),
            api_key_present: false,
            api_key_source: "not read: the acceptance did not execute",
            note: reason,
        }
    }

    fn to_json(&self) -> Value {
        json!({
            "configured": !self.model_id.is_empty(),
            "apiType": self.api_type,
            "modelId": self.model_id,
            "baseUrl": self.base_url,
            "baseUrlHost": self.base_url_host,
            "apiKeyPresent": self.api_key_present,
            "apiKeySource": self.api_key_source,
            "apiKeyRecorded": false,
            "note": self.note,
        })
    }

    /// Every field that differs from the Run's frozen model service, plus any
    /// sign that a credential leaked into the frozen configuration.
    fn mismatches(&self, service: &Value) -> Vec<String> {
        let frozen = Self::from_model_service(service, self.api_key_present, self.api_key_source);
        let mut problems = Vec::new();
        if frozen.api_type != self.api_type {
            problems.push(format!(
                "apiType: entry={} frozen={}",
                self.api_type, frozen.api_type
            ));
        }
        if frozen.model_id != self.model_id {
            problems.push(format!(
                "modelId: entry={} frozen={}",
                self.model_id, frozen.model_id
            ));
        }
        if frozen.base_url != self.base_url {
            problems.push(format!(
                "baseUrl: entry={} frozen={}",
                self.base_url, frozen.base_url
            ));
        }
        if service
            .as_object()
            .is_some_and(|fields| fields.keys().any(|key| key_is_secret(key)))
        {
            problems.push(String::from("the frozen model service carries a credential field"));
        }
        problems
    }
}

fn key_is_secret(key: &str) -> bool {
    matches!(
        key.to_ascii_lowercase().replace('_', "").as_str(),
        "apikey" | "authorization" | "credentials" | "accesstoken" | "password" | "secret"
    )
}

/// Address used only as the documentation placeholder of the synthetic tier
/// (port 0 = the OS assigns one per scenario).
fn synthetic_placeholder_address() -> std::net::SocketAddr {
    std::net::SocketAddr::from(([127, 0, 0, 1], 0))
}

/// The real tier's environment, resolved through an injected lookup so the fast
/// contract tests can prove the not-run behaviour without any credential.
struct CloudEnvironment {
    api_type: String,
    model_id: String,
    base_url: String,
    /// Never recorded anywhere; only passed to the live dispatch.
    api_key: String,
}

impl std::fmt::Debug for CloudEnvironment {
    /// Debug output is safe to print: the API key is never rendered.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CloudEnvironment")
            .field("api_type", &self.api_type)
            .field("model_id", &self.model_id)
            .field("base_url", &self.base_url)
            .field("api_key", &"<not recorded>")
            .finish()
    }
}

/// Why the real tier could not start: the offending variable names plus a
/// human-readable reason per name.
#[derive(Clone, Debug, PartialEq)]
struct CloudEnvironmentError {
    variables: Vec<&'static str>,
    reasons: Vec<String>,
}

impl CloudEnvironmentError {
    fn missing(variables: Vec<&'static str>) -> Self {
        let reasons = variables
            .iter()
            .map(|name| format!("{name} 未设置 (not set)"))
            .collect();
        Self { variables, reasons }
    }

    fn invalid(variable: &'static str, reason: String) -> Self {
        Self {
            variables: vec![variable],
            reasons: vec![format!("{variable} {reason}")],
        }
    }

    /// The 未执行 statement recorded in the report and in the failure message.
    fn summary(&self) -> String {
        format!(
            "{NOT_RUN_MARKER}: real-cloud-model acceptance did not run — {}",
            self.reasons.join("; ")
        )
    }

    fn variables_json(&self) -> Vec<String> {
        self.variables.iter().map(|name| (*name).to_owned()).collect()
    }
}

fn non_blank(raw: Option<String>) -> Option<String> {
    raw.map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

impl CloudEnvironment {
    /// Read the real environment. Absent or unusable values abort the tier.
    fn read_from_process() -> Result<Self, CloudEnvironmentError> {
        Self::resolve(|name| std::env::var(name).ok())
    }

    fn resolve(mut lookup: impl FnMut(&str) -> Option<String>) -> Result<Self, CloudEnvironmentError> {
        let base_url = non_blank(lookup(CLOUD_ENV_BASE_URL));
        let api_key = non_blank(lookup(CLOUD_ENV_API_KEY));
        let model_id = non_blank(lookup(CLOUD_ENV_MODEL));
        let api_type =
            non_blank(lookup(CLOUD_ENV_API_TYPE)).unwrap_or_else(|| CLOUD_DEFAULT_API_TYPE.into());

        let mut missing = Vec::new();
        if base_url.is_none() {
            missing.push(CLOUD_ENV_BASE_URL);
        }
        if api_key.is_none() {
            missing.push(CLOUD_ENV_API_KEY);
        }
        if model_id.is_none() {
            missing.push(CLOUD_ENV_MODEL);
        }
        if !missing.is_empty() {
            return Err(CloudEnvironmentError::missing(missing));
        }
        let base_url = base_url.unwrap_or_default();
        let api_key = api_key.unwrap_or_default();
        let model_id = model_id.unwrap_or_default();

        if !CLOUD_API_TYPES.contains(&api_type.as_str()) {
            return Err(CloudEnvironmentError::invalid(
                CLOUD_ENV_API_TYPE,
                format!(
                    "must be one of {CLOUD_API_TYPES:?}; the scripted `faux` adapter is refused in the real tier (got `{api_type}`)"
                ),
            ));
        }
        let parsed = reqwest::Url::parse(&base_url).map_err(|error| {
            CloudEnvironmentError::invalid(
                CLOUD_ENV_BASE_URL,
                format!("is not an absolute URL: {error}"),
            )
        })?;
        if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
            return Err(CloudEnvironmentError::invalid(
                CLOUD_ENV_BASE_URL,
                String::from("must be an absolute http(s) URL with a host"),
            ));
        }
        if !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.query().is_some()
            || parsed.fragment().is_some()
        {
            return Err(CloudEnvironmentError::invalid(
                CLOUD_ENV_BASE_URL,
                String::from(
                    "must not carry credentials, a query or a fragment (the frozen Kernel model config enforces the same rule)",
                ),
            ));
        }
        if model_id == SYNTHETIC_MODEL_ID {
            return Err(CloudEnvironmentError::invalid(
                CLOUD_ENV_MODEL,
                format!("must not be the synthetic provider id `{SYNTHETIC_MODEL_ID}`"),
            ));
        }
        Ok(Self {
            api_type,
            model_id,
            base_url,
            api_key,
        })
    }

    /// The frozen model configuration of the real tier: only environment values,
    /// and no credential (the key travels beside the config, as in production).
    /// `systemPrompt`/`proposalTools` are not set: `prepare_eval_run` replaces
    /// the whole configuration with the production describe result.
    fn model_config(&self) -> kernel_model_worker::KernelModelConfig {
        let mut config = worker_configuration();
        config.model_service = json!({
            "apiType": self.api_type,
            "modelId": self.model_id,
            "baseUrl": self.base_url,
            "contextWindow": 256_000,
            "maxOutputTokens": 8_192,
        });
        config
    }

    fn identity(&self) -> ModelIdentity {
        ModelIdentity {
            note: "endpoint supplied by FOX_EVAL_* environment variables; no local provider",
            ..ModelIdentity::from_model_service(&self.model_config().model_service, true, CLOUD_KEY_SOURCE)
        }
    }

    /// What the report may say about the environment: variable names and
    /// resolved non-secret identity, never a value that is a credential.
    fn report_json(&self) -> Value {
        json!({
            "required": [CLOUD_ENV_BASE_URL, CLOUD_ENV_API_KEY, CLOUD_ENV_MODEL],
            "optional": [CLOUD_ENV_API_TYPE],
            "apiTypeDefault": CLOUD_DEFAULT_API_TYPE,
            "resolved": self.identity().to_json(),
            "apiKeyRecorded": false,
        })
    }
}

const AGV_SOURCE_NAME: &str = "AGV长时间任务汇总统计表.xlsx";
const AGV_DIST_NAME: &str = "AGV长时间任务统计分布.xlsx";
// NOTE: the production delivery-seed heuristic treats `成` as a prose/name
// boundary (`生成报告.xlsx`), so a deliverable literally named 分析成果.xlsx
// would seed as `果.xlsx`. The indicator workbook therefore uses a name the
// Host can bind exactly; its title sheet still says 分析成果.
const AGV_SUMMARY_NAME: &str = "AGV长时间任务指标汇总.xlsx";
const AGV_REPORT_NAME: &str = "AGV长时间任务分析报告.docx";
const SHORT_DOC_NAME: &str = "会议纪要-评测.docx";
const LARGE_RESULT_NAME: &str = "large-records.json";

// The prompt deliberately avoids spelling out the SOURCE workbook's .xlsx
// name (that would seed the input as a deliverable) and uses deliverable names
// the production seeder can bind without CJK prose-boundary fragmentation.
const AGV_TASK_PROMPT: &str = "请基于项目目录中的 AGV 源数据工作簿完成分析，\
交付三个文件：AGV长时间任务统计分布.xlsx、AGV长时间任务指标汇总.xlsx、\
AGV长时间任务分析报告.docx。";
/// Env var naming a file that holds the user's ORIGINAL full instruction.
///
/// The real acceptance must run the task the user actually wrote, not the
/// shortened harness prompt above. When this is set, the AGV scenarios use the
/// file's contents verbatim; when it is not, the built-in prompt keeps the
/// scripted regression deterministic. The chosen text, its SHA-256 and its
/// length are recorded in the report, so "which instruction ran" is never
/// ambiguous.
const AGV_INSTRUCTION_ENV: &str = "FOX_EVAL_AGV_INSTRUCTION_FILE";

/// The AGV task text for this run: the user's original instruction when one was
/// supplied, otherwise the built-in prompt.
fn agv_task_prompt() -> String {
    match std::env::var(AGV_INSTRUCTION_ENV) {
        Ok(path) if !path.trim().is_empty() => match std::fs::read_to_string(path.trim()) {
            Ok(text) if !text.trim().is_empty() => text,
            Ok(_) => panic!("{AGV_INSTRUCTION_ENV} points at an empty file"),
            Err(error) => panic!("{AGV_INSTRUCTION_ENV} could not be read: {error}"),
        },
        _ => AGV_TASK_PROMPT.to_owned(),
    }
}

/// Identity of the AGV instruction used, for the report and the evidence file.
fn agv_instruction_identity(text: &str) -> Value {
    json!({
        "source": if std::env::var(AGV_INSTRUCTION_ENV).is_ok() {
            "user's original instruction (FOX_EVAL_AGV_INSTRUCTION_FILE)"
        } else {
            "built-in harness prompt"
        },
        "characters": text.chars().count(),
        "bytes": text.len(),
        "sha256": sha256_hex(text.as_bytes()),
    })
}
const SHORT_DOC_TASK_PROMPT: &str =
    "请在项目中创建短文档：会议纪要-评测.docx，并完成一次短文档修改。";
const LARGE_RESULT_TASK_PROMPT: &str =
    "请读取项目中的大结果记录文件，用 read_tool_result 完整续读后给出汇总。";

fn manifest_dir() -> &'static std::path::Path {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn repository_dir() -> std::path::PathBuf {
    manifest_dir().join("../../..")
}

/// This round's evidence root (one directory per round).
///
/// `FOX_EVAL_OUTPUT_ROOT` may override the round directory; the historical
/// evidence directory is refused even then, so no run can overwrite it.
fn round_dir() -> std::path::PathBuf {
    match std::env::var(EVAL_OUTPUT_ROOT_ENV) {
        Ok(value) if !value.trim().is_empty() => {
            let candidate = std::path::PathBuf::from(value.trim());
            let candidate = if candidate.is_absolute() {
                candidate
            } else {
                repository_dir().join(candidate)
            };
            if candidate.starts_with(historical_eval_dir()) {
                panic!(
                    "{EVAL_OUTPUT_ROOT_ENV} must not point at the historical evidence directory"
                );
            }
            candidate
        }
        _ => repository_dir().join(EVAL_ROUND_DIR),
    }
}

/// The previous round's evidence: read-only for this round, kept verbatim.
fn historical_eval_dir() -> std::path::PathBuf {
    repository_dir().join(HISTORICAL_EVAL_DIR)
}

fn source_workbook_path() -> std::path::PathBuf {
    manifest_dir()
        .join("../tests/execl-ceshi")
        .join(AGV_SOURCE_NAME)
}

fn office_resources_dir() -> std::path::PathBuf {
    manifest_dir().join("resources")
}

/// The tool names the acceptance expert's bundled package declares.
///
/// Read from the same library the install uses, so the regression asserts
/// production's own scoping rule instead of a second hand-written list.
fn acceptance_expert_declared_tools() -> Vec<String> {
    let library = office_resources_dir()
        .join("expert-library")
        .join("bundled.json");
    let raw = std::fs::read_to_string(&library).expect("bundled expert library");
    let bundled: Value = serde_json::from_str(&raw).expect("bundled expert library JSON");
    bundled
        .as_array()
        .and_then(|entries| {
            entries
                .iter()
                .find(|entry| entry["expertId"].as_str() == Some(ACCEPTANCE_EXPERT_ID))
        })
        .and_then(|entry| entry["package"]["resources"]["tools"].as_array())
        .map(|tools| {
            tools
                .iter()
                .filter_map(|tool| tool.as_str().map(str::to_owned))
                .collect()
        })
        .expect("the acceptance expert declares its tools")
}

/// The expert this acceptance runs as. `fox-data-analyst` is the bundled
/// 「数据分析专家」 (`resources/expert-library/catalog.json`), i.e. the expert the
/// AGV analysis task belongs to.
const ACCEPTANCE_EXPERT_ID: &str = "fox-data-analyst";

/// Install and bind the acceptance expert through production code.
///
/// The 2026-09-19 review found this tier had never demonstrated an expert
/// selection at all. This uses the same path the application's expert center
/// uses: the bundled package is parsed, previewed against the Host's real
/// resource view, installed by `install_expert_package_version`, and bound with
/// `bind_conversation_expert`. Nothing writes an `agents` row by hand, and the
/// resource guard is *fed the truth* (an empty local-knowledge view) instead of
/// being skipped: a package that requires a local knowledge base is refused.
fn bind_acceptance_expert(
    db: &Database,
    scenario_root: &std::path::Path,
    conversation_id: &str,
) -> Result<(), String> {
    let library = office_resources_dir()
        .join("expert-library")
        .join("bundled.json");
    let raw = std::fs::read_to_string(&library)
        .map_err(|error| format!("bundled expert library unreadable at {library:?}: {error}"))?;
    let bundled: Value = serde_json::from_str(&raw)
        .map_err(|error| format!("bundled expert library is not valid JSON: {error}"))?;
    let skills_dir = eval_host_dir(scenario_root).join("skills");
    std::fs::create_dir_all(&skills_dir).map_err(|error| error.to_string())?;
    crate::skills::install_bundled_office_skills(&skills_dir)?;
    crate::expert_packages::install_bundled_expert(
        db,
        &skills_dir,
        &crate::expert_packages::NoLocalKnowledge,
        &bundled,
        ACCEPTANCE_EXPERT_ID,
    )?;
    db.bind_conversation_expert(conversation_id, ACCEPTANCE_EXPERT_ID, "acceptance_harness")
        .map(|_| ())
        .map_err(|error| format!("binding {ACCEPTANCE_EXPERT_ID} failed: {error:?}"))
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

// ---------------------------------------------------------------------------
// Independent expectation derivation from the CURRENT source workbook.
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct Expectations {
    source_sha256: String,
    /// Structural anchors: the workbook declares which sheet is the dedup
    /// fact table and which is the cantilever exception list. Counts are
    /// still derived, never taken from these labels.
    dedup_sheet: String,
    exception_sheet: Option<String>,
    total: u64,
    /// Ordered (label, count); canonical labels first, extras sorted.
    task_counts: Vec<(String, u64)>,
    status_counts: Vec<(String, u64)>,
    wait_min: i64,
    wait_max: i64,
    wait_avg: f64,
    /// (label, low inclusive, high inclusive, count) with fixed boundaries.
    buckets: Vec<(String, i64, i64, u64)>,
    exception_total: u64,
    exception_tasks: Vec<(String, u64)>,
}

impl Expectations {
    fn ratio_of(&self, count: u64) -> f64 {
        round4(count as f64 / self.total as f64)
    }
}

fn round4(value: f64) -> f64 {
    (value * 10_000.0).round() / 10_000.0
}

fn round2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

/// Percent text written into documents, e.g. `59.1`.
fn percent_text(ratio: f64) -> String {
    format!("{:.1}", (ratio * 10_000.0).round() / 100.0)
}

fn cell_text(cell: &CellData) -> String {
    match cell {
        CellData::String(value) => value.trim().to_owned(),
        CellData::Float(value) => {
            if value.fract() == 0.0 {
                format!("{}", *value as i64)
            } else {
                format!("{value}")
            }
        }
        CellData::Bool(value) => value.to_string(),
        CellData::DateTime(value) => value.to_string(),
        _ => String::new(),
    }
}

fn cell_i64(cell: &CellData) -> Option<i64> {
    match cell {
        CellData::Float(value) if value.fract() == 0.0 => Some(*value as i64),
        CellData::String(value) => value.trim().parse::<i64>().ok(),
        _ => None,
    }
}

/// Prefer canonical labels (卸船/装船, 重车/空车) first so table layout stays
/// stable; any additional category the current source carries is still
/// included, sorted, instead of being dropped or assumed absent.
fn ordered_counts(counts: &BTreeMap<String, u64>, preferred: &[&str]) -> Vec<(String, u64)> {
    let mut ordered: Vec<(String, u64)> = Vec::new();
    for label in preferred {
        if let Some(count) = counts.get(*label) {
            ordered.push(((*label).to_owned(), *count));
        }
    }
    for (label, count) in counts {
        if !preferred.contains(&label.as_str()) {
            ordered.push((label.clone(), *count));
        }
    }
    ordered
}

fn derive_expectations(source_bytes: &[u8]) -> Result<Expectations, String> {
    let mut workbook = open_workbook_auto_from_rs(Cursor::new(source_bytes))
        .map_err(|error| format!("source workbook cannot open: {error:?}"))?;

    // Pick sheets by their declared names AND headers, never by fixed
    // position. Header matching alone is insufficient here: the 分列 raw
    // sheet and the 8-column 非悬臂 sheet carry the same ORDERID/任务类型/
    // 等待时长/AGV状态 headers but different row sets (2213 and 838). The
    // 去重 sheet is the canonical fact table (927 rows in the current file);
    // the 悬臂 sheet (7 columns, 超时原因 in G) is the cantilever exception
    // list, distinct from 非悬臂.
    let mut dedup_sheet: Option<String> = None;
    let mut exception_sheet: Option<String> = None;
    for name in workbook.sheet_names().to_owned() {
        let Ok(range) = workbook.worksheet_range(&name) else {
            continue;
        };
        let Some(header) = range.rows().next() else {
            continue;
        };
        let headers: Vec<String> = header.iter().map(cell_text).collect();
        let has = |wanted: &str| headers.iter().any(|value| value == wanted);
        if dedup_sheet.is_none()
            && name.contains("去重")
            && has("ORDERID")
            && has("任务类型")
            && has("等待时长")
            && has("AGV状态")
        {
            dedup_sheet = Some(name.clone());
        }
        if exception_sheet.is_none()
            && name == "悬臂"
            && headers.len() == 7
            && headers.iter().any(|value| value == "超时原因")
        {
            exception_sheet = Some(name);
        }
    }
    let dedup_sheet = dedup_sheet.ok_or("source has no 去重 sheet with the expected headers")?;

    let range = workbook
        .worksheet_range(&dedup_sheet)
        .map_err(|error| format!("dedup sheet unreadable: {error:?}"))?;
    let header: Vec<String> = range
        .rows()
        .next()
        .map(|row| row.iter().map(cell_text).collect())
        .unwrap_or_default();
    let column_of = |wanted: &str| header.iter().position(|value| value == wanted);
    let col_order = column_of("ORDERID").ok_or("missing ORDERID column")?;
    let col_status = column_of("AGV状态").ok_or("missing AGV状态 column")?;
    let col_task = column_of("任务类型").ok_or("missing 任务类型 column")?;
    let col_wait = column_of("等待时长").ok_or("missing 等待时长 column")?;

    let mut task_counts: BTreeMap<String, u64> = BTreeMap::new();
    let mut status_counts: BTreeMap<String, u64> = BTreeMap::new();
    let mut waits: Vec<i64> = Vec::new();
    let mut total = 0u64;
    for row in range.rows().skip(1) {
        let order = row.get(col_order).map(cell_text).unwrap_or_default();
        if order.is_empty() {
            continue;
        }
        total += 1;
        let task = row.get(col_task).map(cell_text).unwrap_or_default();
        let status = row.get(col_status).map(cell_text).unwrap_or_default();
        if !task.is_empty() {
            *task_counts.entry(task).or_default() += 1;
        }
        if !status.is_empty() {
            *status_counts.entry(status).or_default() += 1;
        }
        if let Some(wait) = row.get(col_wait).and_then(cell_i64) {
            waits.push(wait);
        }
    }
    if total == 0 {
        return Err("source 去重 sheet has no records".into());
    }
    let wait_min = waits.iter().copied().min().unwrap_or(0);
    let wait_max = waits.iter().copied().max().unwrap_or(0);
    let wait_avg = round2(waits.iter().sum::<i64>() as f64 / waits.len() as f64);

    // Fixed, data-independent boundaries.
    let bucket_edges: [(&str, i64, i64); 4] = [
        ("≤20分钟", 0, 20),
        ("21-30分钟", 21, 30),
        ("31-60分钟", 31, 60),
        (">60分钟", 61, i64::MAX),
    ];
    let buckets = bucket_edges
        .iter()
        .map(|(label, low, high)| {
            let count = waits
                .iter()
                .filter(|value| *value >= low && *value <= high)
                .count() as u64;
            ((*label).to_owned(), *low, *high, count)
        })
        .collect::<Vec<_>>();

    let mut exception_total = 0u64;
    let mut exception_tasks: BTreeMap<String, u64> = BTreeMap::new();
    if let Some(name) = &exception_sheet {
        let range = workbook
            .worksheet_range(name)
            .map_err(|error| format!("exception sheet unreadable: {error:?}"))?;
        let header: Vec<String> = range
            .rows()
            .next()
            .map(|row| row.iter().map(cell_text).collect())
            .unwrap_or_default();
        let col_order = header.iter().position(|value| value == "ORDERID");
        let col_task = header.iter().position(|value| value == "任务类型");
        for row in range.rows().skip(1) {
            let has_order = col_order
                .and_then(|index| row.get(index))
                .map(|cell| !cell_text(cell).is_empty())
                .unwrap_or(false);
            if !has_order {
                continue;
            }
            exception_total += 1;
            if let Some(index) = col_task {
                let task = cell_text(row.get(index).unwrap_or(&CellData::Empty));
                if !task.is_empty() {
                    *exception_tasks.entry(task).or_default() += 1;
                }
            }
        }
    }

    Ok(Expectations {
        source_sha256: sha256_hex(source_bytes),
        dedup_sheet,
        exception_sheet,
        total,
        task_counts: ordered_counts(&task_counts, &["卸船", "装船"]),
        status_counts: ordered_counts(&status_counts, &["重车", "空车"]),
        wait_min,
        wait_max,
        wait_avg,
        buckets,
        exception_total,
        exception_tasks: ordered_counts(&exception_tasks, &["卸船", "装船"]),
    })
}

// ---------------------------------------------------------------------------
// The deterministic model plan: scripted tool rounds whose VALUES come from
// the independently derived expectations. Nothing here writes a deliverable;
// every operation is handed to the real Host/Runtime/OfficeCLI chain.
// ---------------------------------------------------------------------------

#[derive(Clone, serde::Serialize)]
enum Reply {
    Tool {
        id: String,
        name: String,
        arguments: Value,
    },
    Stop,
}

fn office_call(tool: &str, arguments: Value) -> Value {
    json!({"serverId": crate::office::SERVER_ID, "tool": tool, "arguments": arguments})
}

fn set_cell(path: &str, value: Value) -> Value {
    json!({"command":"set","path":path,"props":{"value":value}})
}

fn add_sheet(name: &str) -> Value {
    json!({"command":"add","path":"/","type":"sheet","props":{"name":name}})
}

fn bar_chart(sheet: &str, first_data_row: usize, last_row: usize, anchor: &str) -> Value {
    json!({"command":"add","path":format!("/{sheet}"),"type":"chart","props":{
        "dataRange":format!("{sheet}!B{first_data_row}:B{last_row}"),
        "categories":format!("{sheet}!A{first_data_row}:A{last_row}"),
        "chartType":"bar","anchor":anchor}})
}

fn office_edit_same(file: &str, operations: Value) -> (String, Value) {
    (
        "call_mcp_tool".into(),
        office_call(
            "office_edit",
            json!({"file":file,"output":file,"overwrite":true,"operations":operations}),
        ),
    )
}

/// Workbook 1: distribution workbook — task distribution, wait distribution and
/// a real chart part.
fn distribution_workbook_script(expected: &Expectations) -> Vec<(String, Value)> {
    let mut ops = vec![
        set_cell("/Sheet1/A1", json!("AGV 长时间等待任务统计分布（真实链路评测）")),
        add_sheet("任务类型分布"),
        set_cell("/任务类型分布/A1", json!("任务类型")),
        set_cell("/任务类型分布/B1", json!("记录数")),
        set_cell("/任务类型分布/C1", json!("占比")),
    ];
    let task_rows = expected.task_counts.len();
    for (index, (name, count)) in expected.task_counts.iter().enumerate() {
        let row = index + 2;
        ops.push(set_cell(&format!("/任务类型分布/A{row}"), json!(name)));
        ops.push(set_cell(&format!("/任务类型分布/B{row}"), json!(count)));
        ops.push(set_cell(
            &format!("/任务类型分布/C{row}"),
            json!(expected.ratio_of(*count)),
        ));
    }
    ops.push(bar_chart("任务类型分布", 2, task_rows + 1, "E2:N20"));
    ops.push(add_sheet("等待时长分布"));
    ops.push(set_cell("/等待时长分布/A1", json!("等待时长区间")));
    ops.push(set_cell("/等待时长分布/B1", json!("记录数")));
    ops.push(set_cell("/等待时长分布/C1", json!("占比")));
    for (index, (label, _, _, count)) in expected.buckets.iter().enumerate() {
        let row = index + 2;
        ops.push(set_cell(&format!("/等待时长分布/A{row}"), json!(label)));
        ops.push(set_cell(&format!("/等待时长分布/B{row}"), json!(count)));
        ops.push(set_cell(
            &format!("/等待时长分布/C{row}"),
            json!(expected.ratio_of(*count)),
        ));
    }
    vec![
        (
            "call_mcp_tool".into(),
            office_call("office_create", json!({"output": AGV_DIST_NAME})),
        ),
        office_edit_same(AGV_DIST_NAME, json!(ops)),
        (
            "call_mcp_tool".into(),
            office_call("office_validate", json!({"file": AGV_DIST_NAME})),
        ),
    ]
}

/// Workbook 2: indicator workbook — headline numbers plus the same task table.
fn summary_workbook_script(expected: &Expectations) -> Vec<(String, Value)> {
    let mut ops = vec![
        set_cell("/Sheet1/A1", json!("AGV 长时间等待任务分析成果（真实链路评测）")),
        add_sheet("总览指标"),
        set_cell("/总览指标/A1", json!("指标")),
        set_cell("/总览指标/B1", json!("数值")),
    ];
    let mut metrics: Vec<(String, Value)> = vec![("去重记录总数".into(), json!(expected.total))];
    for (name, count) in &expected.task_counts {
        metrics.push((format!("{name}记录数"), json!(count)));
        metrics.push((format!("{name}占比"), json!(expected.ratio_of(*count))));
    }
    for (name, count) in &expected.status_counts {
        metrics.push((format!("{name}记录数"), json!(count)));
    }
    metrics.push((String::from("最短等待时长(分钟)"), json!(expected.wait_min)));
    metrics.push((String::from("最长等待时长(分钟)"), json!(expected.wait_max)));
    metrics.push((String::from("平均等待时长(分钟)"), json!(expected.wait_avg)));
    metrics.push((
        String::from("异常记录数(悬臂)"),
        json!(expected.exception_total),
    ));
    for (index, (label, value)) in metrics.iter().enumerate() {
        let row = index + 2;
        ops.push(set_cell(&format!("/总览指标/A{row}"), json!(label)));
        ops.push(set_cell(&format!("/总览指标/B{row}"), value.clone()));
    }
    ops.push(add_sheet("任务类型分布"));
    ops.push(set_cell("/任务类型分布/A1", json!("任务类型")));
    ops.push(set_cell("/任务类型分布/B1", json!("记录数")));
    ops.push(set_cell("/任务类型分布/C1", json!("占比")));
    let task_rows = expected.task_counts.len();
    for (index, (name, count)) in expected.task_counts.iter().enumerate() {
        let row = index + 2;
        ops.push(set_cell(&format!("/任务类型分布/A{row}"), json!(name)));
        ops.push(set_cell(&format!("/任务类型分布/B{row}"), json!(count)));
        ops.push(set_cell(
            &format!("/任务类型分布/C{row}"),
            json!(expected.ratio_of(*count)),
        ));
    }
    ops.push(bar_chart("任务类型分布", 2, task_rows + 1, "E2:N20"));
    vec![
        (
            "call_mcp_tool".into(),
            office_call("office_create", json!({"output": AGV_SUMMARY_NAME})),
        ),
        office_edit_same(AGV_SUMMARY_NAME, json!(ops)),
        (
            "call_mcp_tool".into(),
            office_call("office_validate", json!({"file": AGV_SUMMARY_NAME})),
        ),
    ]
}

/// Word report: fixed section structure with derived numbers and percentages.
fn report_document_script(expected: &Expectations) -> Vec<(String, Value)> {
    let paragraph = |text: &str| {
        json!({"command":"add","path":"/body","type":"paragraph","props":{"text":text}})
    };
    let heading = |text: &str| {
        json!({"command":"add","path":"/body","type":"paragraph",
            "props":{"text":text,"style":"Heading1"}})
    };
    let mut ops = vec![
        heading("AGV 长时间等待任务分析报告"),
        heading("一、数据概览"),
        paragraph(&format!(
            "本次分析基于去重后的AGV长时间等待任务记录，共{}条；等待时长介于{}-{}分钟，平均约{}分钟。",
            expected.total, expected.wait_min, expected.wait_max, expected.wait_avg
        )),
        heading("二、任务类型构成"),
    ];
    for (name, count) in &expected.task_counts {
        ops.push(paragraph(&format!(
            "{}{}条，占比{}%。",
            name,
            count,
            percent_text(expected.ratio_of(*count))
        )));
    }
    let status_summary = expected
        .status_counts
        .iter()
        .map(|(name, count)| format!("{name}{count}条"))
        .collect::<Vec<_>>()
        .join("、");
    ops.push(paragraph(&format!("按AGV状态，{status_summary}。")));
    ops.push(heading("三、等待时长分布"));
    let bucket_summary = expected
        .buckets
        .iter()
        .map(|(label, _, _, count)| {
            format!(
                "{label}{count}条（{}%）",
                percent_text(expected.ratio_of(*count))
            )
        })
        .collect::<Vec<_>>()
        .join("；");
    ops.push(paragraph(&format!("{bucket_summary}。")));
    ops.push(heading("四、异常与超时"));
    let exception_summary = expected
        .exception_tasks
        .iter()
        .map(|(name, count)| format!("{name}{count}条"))
        .collect::<Vec<_>>()
        .join("、");
    ops.push(paragraph(&format!(
        "悬臂异常记录共{}条，其中{}。",
        expected.exception_total, exception_summary
    )));
    ops.push(heading("五、分析结论"));
    ops.push(paragraph(
        "等待集中在21至30分钟区间，应结合悬臂箱区调度与任务类型占比优化排队。",
    ));
    vec![
        (
            "call_mcp_tool".into(),
            office_call("office_create", json!({"output": AGV_REPORT_NAME})),
        ),
        office_edit_same(AGV_REPORT_NAME, json!(ops)),
        (
            "call_mcp_tool".into(),
            office_call("office_validate", json!({"file": AGV_REPORT_NAME})),
        ),
    ]
}

fn agv_script(expected: &Expectations) -> VecDeque<Reply> {
    let mut rounds = VecDeque::new();
    let mut push = |calls: Vec<(String, Value)>, offset: &mut usize| {
        for (name, arguments) in calls {
            rounds.push_back(Reply::Tool {
                id: format!("agv-call-{offset}"),
                name,
                arguments,
            });
            *offset += 1;
        }
    };
    let mut offset = 1usize;
    push(distribution_workbook_script(expected), &mut offset);
    push(summary_workbook_script(expected), &mut offset);
    push(report_document_script(expected), &mut offset);
    rounds.push_back(Reply::Stop);
    rounds
}

// ---------------------------------------------------------------------------
// Independent checker.
// ---------------------------------------------------------------------------

#[derive(Default)]
struct CheckResults {
    rows: Vec<(String, bool, String)>,
}

impl CheckResults {
    fn check(&mut self, id: &str, passed: bool, detail: impl Into<String>) {
        self.rows.push((id.into(), passed, detail.into()));
    }

    fn ok(&self) -> bool {
        self.rows.iter().all(|(_, passed, _)| *passed)
    }
}

fn workbook_rows(bytes: &[u8], sheet: &str) -> Result<Vec<Vec<String>>, String> {
    let mut workbook = open_workbook_auto_from_rs(Cursor::new(bytes))
        .map_err(|error| format!("{sheet}: workbook cannot open: {error:?}"))?;
    let range = workbook
        .worksheet_range(sheet)
        .map_err(|error| format!("{sheet}: {error:?}"))?;
    Ok(range
        .rows()
        .map(|row| row.iter().map(cell_text).collect())
        .collect())
}

fn sheet_names_of(bytes: &[u8]) -> Result<Vec<String>, String> {
    let workbook = open_workbook_auto_from_rs(Cursor::new(bytes))
        .map_err(|error| format!("workbook cannot open: {error:?}"))?;
    Ok(workbook.sheet_names().to_owned())
}

fn has_chart_part(bytes: &[u8]) -> Result<bool, String> {
    // The pinned OfficeCLI packages chart XML under xl/drawings/charts/.
    Ok(crate::local_knowledge_import::zip_entry_names(bytes)?
        .iter()
        .any(|name| {
            (name.starts_with("xl/drawings/charts/chart")
                || name.starts_with("xl/charts/chart"))
                && name.ends_with(".xml")
        }))
}

fn parse_number(value: &str) -> Option<f64> {
    value
        .trim()
        .replace(['=', '，', ','], "")
        .parse::<f64>()
        .ok()
}

fn nearly(left: f64, right: f64, tolerance: f64) -> bool {
    (left - right).abs() <= tolerance
}

/// Read a label/value sheet ("指标/数值" or table rows) into an ordered map.
fn label_map(rows: &[Vec<String>]) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for row in rows {
        if let (Some(label), Some(value)) = (row.first(), row.get(1)) {
            if !label.is_empty() {
                map.insert(label.clone(), value.clone());
            }
        }
    }
    map
}

fn number_after(haystack: &str, marker: &str) -> Option<f64> {
    let start = haystack.find(marker)? + marker.len();
    let tail = &haystack[start..];
    let end = tail
        .find(|character: char| !character.is_ascii_digit() && character != '.')
        .unwrap_or(tail.len());
    if end == 0 {
        return None;
    }
    tail[..end].parse::<f64>().ok()
}

fn percent_after(haystack: &str, marker: &str) -> Option<f64> {
    Some(number_after(haystack, marker)? / 100.0)
}

/// Canonical metrics shared across deliverables.
type MetricMap = BTreeMap<String, f64>;

fn workbook_metrics(bytes: &[u8], expected: &Expectations) -> Result<MetricMap, String> {
    let mut metrics = MetricMap::new();
    // Distribution tables appear identically in both workbooks.
    let rows = workbook_rows(bytes, "任务类型分布")?;
    let table = label_map(&rows);
    for (name, _count) in &expected.task_counts {
        let stored_count = table
            .get(name)
            .and_then(|value| parse_number(value))
            .ok_or_else(|| format!("missing task row {name}"))?;
        metrics.insert(format!("task:{name}:count"), stored_count);
        let row = rows
            .iter()
            .find(|row| row.first().is_some_and(|label| label == name))
            .ok_or_else(|| format!("missing task ratio row {name}"))?;
        let ratio = row
            .get(2)
            .and_then(|value| parse_number(value))
            .ok_or_else(|| format!("missing task ratio {name}"))?;
        metrics.insert(format!("task:{name}:ratio"), ratio);
    }
    Ok(metrics)
}

fn distribution_workbook_checks(
    bytes: &[u8],
    expected: &Expectations,
    results: &mut CheckResults,
) -> MetricMap {
    let names = match sheet_names_of(bytes) {
        Ok(names) => names,
        Err(error) => {
            results.check("dist:parseable", false, error);
            return MetricMap::new();
        }
    };
    results.check(
        "dist:sheets",
        names.contains(&"任务类型分布".to_owned())
            && names.contains(&"等待时长分布".to_owned()),
        format!("sheets={names:?}"),
    );
    let metrics = match workbook_metrics(bytes, expected) {
        Ok(metrics) => metrics,
        Err(error) => {
            results.check("dist:task-table", false, error);
            MetricMap::new()
        }
    };
    for (name, count) in &expected.task_counts {
        let ok = metrics
            .get(&format!("task:{name}:count"))
            .is_some_and(|value| *value as u64 == *count);
        results.check(
            &format!("dist:task-count:{name}"),
            ok,
            format!("expected={count} stored={:?}", metrics.get(&format!("task:{name}:count"))),
        );
        let ok = metrics
            .get(&format!("task:{name}:ratio"))
            .is_some_and(|value| nearly(*value, expected.ratio_of(*count), 0.0005));
        results.check(
            &format!("dist:task-ratio:{name}"),
            ok,
            format!(
                "expected={} stored={:?}",
                expected.ratio_of(*count),
                metrics.get(&format!("task:{name}:ratio"))
            ),
        );
    }
    let buckets = match workbook_rows(bytes, "等待时长分布") {
        Ok(rows) => label_map(&rows),
        Err(error) => {
            results.check("dist:bucket-table", false, error);
            BTreeMap::new()
        }
    };
    let mut bucket_sum = 0u64;
    for (label, _, _, count) in &expected.buckets {
        let stored = buckets.get(label).and_then(|value| parse_number(value));
        bucket_sum += stored.unwrap_or(0.0) as u64;
        results.check(
            &format!("dist:bucket:{label}"),
            stored.is_some_and(|value| value as u64 == *count),
            format!("expected={count} stored={stored:?}"),
        );
    }
    results.check(
        "dist:bucket-sum",
        bucket_sum == expected.total,
        format!("bucket sum {bucket_sum} vs total {}", expected.total),
    );
    match has_chart_part(bytes) {
        Ok(present) => results.check(
            "dist:chart-part",
            present,
            "xl/charts/chart*.xml in the OOXML package",
        ),
        Err(error) => results.check("dist:chart-part", false, error),
    }
    metrics
}

fn summary_workbook_checks(
    bytes: &[u8],
    expected: &Expectations,
    results: &mut CheckResults,
) -> MetricMap {
    let rows = match workbook_rows(bytes, "总览指标") {
        Ok(rows) => rows,
        Err(error) => {
            results.check("summary:overview-sheet", false, error);
            return MetricMap::new();
        }
    };
    let map = label_map(&rows);
    let expect_number = |results: &mut CheckResults,
                         id: &str,
                         label: &str,
                         wanted: f64,
                         tolerance: f64| {
        let stored = map.get(label).and_then(|value| parse_number(value));
        results.check(
            id,
            stored.is_some_and(|value| nearly(value, wanted, tolerance)),
            format!("{label}: expected={wanted} stored={stored:?}"),
        );
    };
    expect_number(results, "summary:total", "去重记录总数", expected.total as f64, 0.0);
    for (name, count) in &expected.task_counts {
        expect_number(
            results,
            &format!("summary:task-count:{name}"),
            &format!("{name}记录数"),
            *count as f64,
            0.0,
        );
        expect_number(
            results,
            &format!("summary:task-ratio:{name}"),
            &format!("{name}占比"),
            expected.ratio_of(*count),
            0.0005,
        );
    }
    for (name, count) in &expected.status_counts {
        expect_number(
            results,
            &format!("summary:status:{name}"),
            &format!("{name}记录数"),
            *count as f64,
            0.0,
        );
    }
    expect_number(
        results,
        "summary:wait-min",
        "最短等待时长(分钟)",
        expected.wait_min as f64,
        0.0,
    );
    expect_number(
        results,
        "summary:wait-max",
        "最长等待时长(分钟)",
        expected.wait_max as f64,
        0.0,
    );
    expect_number(
        results,
        "summary:wait-avg",
        "平均等待时长(分钟)",
        expected.wait_avg,
        0.01,
    );
    expect_number(
        results,
        "summary:exceptions",
        "异常记录数(悬臂)",
        expected.exception_total as f64,
        0.0,
    );
    let metrics = match workbook_metrics(bytes, expected) {
        Ok(metrics) => metrics,
        Err(error) => {
            results.check("summary:task-table", false, error);
            MetricMap::new()
        }
    };
    match has_chart_part(bytes) {
        Ok(present) => results.check(
            "summary:chart-part",
            present,
            "xl/charts/chart*.xml in the OOXML package",
        ),
        Err(error) => results.check("summary:chart-part", false, error),
    }
    metrics
}

fn report_document_checks(
    bytes: &[u8],
    expected: &Expectations,
    results: &mut CheckResults,
) -> MetricMap {
    let text = match crate::local_knowledge_import::extract_office_text(bytes, AGV_REPORT_NAME) {
        Ok(Some(text)) => text,
        Ok(None) => {
            results.check(
                "report:parseable",
                false,
                String::from("no document text"),
            );
            return MetricMap::new();
        }
        Err(error) => {
            results.check("report:parseable", false, error);
            return MetricMap::new();
        }
    };
    let mut metrics = MetricMap::new();
    for section in ["数据概览", "任务类型构成", "等待时长分布", "异常与超时", "分析结论"] {
        results.check(
            &format!("report:section:{section}"),
            text.contains(section),
            format!("heading containing {section}"),
        );
    }
    if let Some(total) = number_after(&text, "共") {
        results.check(
            "report:total",
            total as u64 == expected.total,
            format!("expected={} stored={total}", expected.total),
        );
        metrics.insert("total".into(), total);
    } else {
        results.check("report:total", false, "no total after 共");
    }
    for (name, count) in &expected.task_counts {
        let stored_count = number_after(&text, name.as_str());
        results.check(
            &format!("report:task-count:{name}"),
            stored_count.is_some_and(|value| value as u64 == *count),
            format!("expected={count} stored={stored_count:?}"),
        );
        let ratio = percent_after(&text, &format!("{name}{count}条，占比"));
        results.check(
            &format!("report:task-ratio:{name}"),
            ratio.is_some_and(|value| nearly(value, expected.ratio_of(*count), 0.002)),
            format!("expected={} stored={ratio:?}", expected.ratio_of(*count)),
        );
        if let Some(value) = stored_count {
            metrics.insert(format!("task:{name}:count"), value);
        }
        if let Some(value) = ratio {
            metrics.insert(format!("task:{name}:ratio"), value);
        }
    }
    if let Some(avg) = number_after(&text, "平均约") {
        results.check(
            "report:wait-avg",
            nearly(avg, expected.wait_avg, 0.05),
            format!("expected={} stored={avg}", expected.wait_avg),
        );
    } else {
        results.check("report:wait-avg", false, "no average after 平均约");
    }
    if let Some(total) = number_after(&text, "悬臂异常记录共") {
        results.check(
            "report:exceptions",
            total as u64 == expected.exception_total,
            format!("expected={} stored={total}", expected.exception_total),
        );
    } else {
        results.check("report:exceptions", false, "no exception total");
    }
    metrics
}

/// Pure cross-document consistency core so the fast contract tests can feed it
/// fabricated readings without producing any file.
fn inconsistent_metrics(per_doc: &[(String, &MetricMap)], tolerance: f64) -> Vec<String> {
    let mut keys: BTreeSet<&String> = BTreeSet::new();
    for (_, metrics) in per_doc {
        keys.extend(metrics.keys());
    }
    let mut failures = Vec::new();
    for key in keys {
        let values: Vec<(String, f64)> = per_doc
            .iter()
            .filter_map(|(name, metrics)| metrics.get(key).map(|value| (name.clone(), *value)))
            .collect();
        if values.len() < 2 {
            continue;
        }
        let min = values
            .iter()
            .map(|(_, value)| *value)
            .fold(f64::INFINITY, f64::min);
        let max = values
            .iter()
            .map(|(_, value)| *value)
            .fold(f64::NEG_INFINITY, f64::max);
        if max - min > tolerance {
            failures.push(format!(
                "{key}: {}",
                values
                    .iter()
                    .map(|(name, value)| format!("{name}={value}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    }
    failures
}

// ---------------------------------------------------------------------------
// Scripted synthetic OpenAI-compatible provider over real HTTP/SSE.
// ---------------------------------------------------------------------------

struct ProviderHandle {
    address: std::net::SocketAddr,
    join: Mutex<Option<std::thread::JoinHandle<Vec<Value>>>>,
    shutdown: Arc<AtomicBool>,
}

impl Drop for ProviderHandle {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
    }
}

/// Spawn the scripted provider. `decide(request_index, current_request_body)`
/// returns the next reply and whether to keep serving (`false` ends the
/// script). When the Run ends before the script does, dropping the handle
/// unblocks the server thread.
fn spawn_provider(
    mut decide: impl FnMut(usize, &str) -> (Reply, bool) + Send + 'static,
) -> ProviderHandle {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();
    let shutdown = Arc::new(AtomicBool::new(false));
    let server_shutdown = shutdown.clone();
    let join = std::thread::spawn(move || {
        let mut requests = Vec::new();
        let mut index = 0usize;
        loop {
            // Wait for the next real request, polling shutdown so an early Run
            // failure never leaves a detached thread with a live deadline.
            let deadline = Instant::now() + Duration::from_secs(300);
            let (mut stream, request, body_text) = loop {
                if server_shutdown.load(Ordering::SeqCst) {
                    return requests;
                }
                let mut stream = match listener.accept() {
                    Ok((stream, _)) => stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < deadline, "missing model request #{index}");
                        std::thread::sleep(Duration::from_millis(10));
                        continue;
                    }
                    Err(error) => panic!("{error}"),
                };
                stream.set_nonblocking(false).unwrap();
                stream.set_read_timeout(Some(Duration::from_millis(500))).unwrap();
                stream.set_write_timeout(Some(Duration::from_secs(10))).unwrap();
                let mut bytes = Vec::new();
                let request = loop {
                    let mut buffer = [0u8; 8192];
                    let n = match stream.read(&mut buffer) {
                        Ok(n) if n > 0 => n,
                        _ => break None,
                    };
                    bytes.extend_from_slice(&buffer[..n]);
                    assert!(bytes.len() < 8_388_608, "eval request #{index} exceeded 8 MiB");
                    if let Some(end) = bytes.windows(4).position(|p| p == b"\r\n\r\n") {
                        let headers =
                            String::from_utf8_lossy(&bytes[..end]).to_ascii_lowercase();
                        let length = headers
                            .lines()
                            .find_map(|line| line.strip_prefix("content-length:"))
                            .unwrap()
                            .trim()
                            .parse::<usize>()
                            .unwrap();
                        if bytes.len() >= end + 4 + length {
                            let body = &bytes[end + 4..end + 4 + length];
                            break serde_json::from_slice::<Value>(body).ok().map(|value| {
                                (value, String::from_utf8_lossy(body).into_owned())
                            });
                        }
                    }
                };
                // The Node worker may open a replacement idle socket; it is
                // not another model request.
                if let Some(pair) = request {
                    break (stream, pair.0, pair.1);
                }
                assert!(
                    Instant::now() < deadline,
                    "only abandoned sockets for request #{index}"
                );
            };
            requests.push(request);
            let (reply, more) = decide(index, &body_text);

            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n"
            )
            .unwrap();
            // One closure owns the stream borrow; the second SSE event of each
            // round carries deterministic synthetic usage plus finish_reason.
            let mut send_chunk = |id_suffix: &str, delta: Value, finish: Value| {
                let mut delta = delta;
                if !finish.is_null() {
                    // Deterministic synthetic accounting, emitted on every round.
                    let prompt_tokens = (body_text.len() / 4).max(1) as u64;
                    delta["usage"] = json!({"prompt_tokens":prompt_tokens,"completion_tokens":32,
                        "total_tokens":prompt_tokens + 32});
                }
                let body = format!(
                    "data: {}\n\n",
                    json!({"id":format!("eval-{index}{id_suffix}"),"object":"chat.completion.chunk","created":1,
                        "model":SYNTHETIC_MODEL_ID,
                        "choices":[{"index":0,"delta":delta,"finish_reason":finish}]})
                );
                write!(stream, "{:x}\r\n{}\r\n", body.len(), body).unwrap();
            };
            match &reply {
                Reply::Tool { id, name, arguments } => {
                    send_chunk(
                        "",
                        json!({"role":"assistant","tool_calls":[{"index":0,"id":id,"type":"function",
                            "function":{"name":name,"arguments":arguments.to_string()}}]}),
                        Value::Null,
                    );
                    send_chunk("-usage", json!({}), json!("tool_calls"));
                }
                Reply::Stop => {
                    send_chunk(
                        "",
                        json!({"role":"assistant","content":"已按要求完成全部交付，并完成自检。"}),
                        Value::Null,
                    );
                    send_chunk("-usage", json!({}), json!("stop"));
                }
            }
            drop(send_chunk);
            let done = "data: [DONE]\n\n";
            write!(stream, "{:x}\r\n{}\r\n0\r\n\r\n", done.len(), done).unwrap();
            index += 1;
            if !more {
                break;
            }
        }
        requests
    });
    ProviderHandle {
        address,
        join: Mutex::new(Some(join)),
        shutdown,
    }
}

/// Fixed-script provider: if the Run unexpectedly asks more rounds than the
/// plan (a repair or stop-review nudge), keep serving Stop instead of letting
/// the request hit a dead listener; the exact-rounds check exposes the
/// discrepancy. The accept loop exits when the ProviderHandle drops.
fn scripted_provider(script: VecDeque<Reply>) -> ProviderHandle {
    let replies = Mutex::new(script);
    spawn_provider(move |_, _| match replies.lock().unwrap().pop_front() {
        Some(reply) => (reply, true),
        None => (Reply::Stop, true),
    })
}

/// The large-result paging model: read the big file once, then walk
/// `read_tool_result` ranges using the cursor the model received in history.
fn paging_decide(_request_index: usize, body: &str) -> (Reply, bool) {
    if !body.contains("fox-result://") {
        return (
            Reply::Tool {
                id: "read-once".into(),
                name: "read".into(),
                arguments: json!({"path": LARGE_RESULT_NAME}),
            },
            true,
        );
    }
    let reference = extract_result_reference(body).unwrap_or_default();
    let pages_seen = body.matches("range-read-").count();
    if pages_seen == 0 {
        return (
            Reply::Tool {
                id: "range-read-1".into(),
                name: "read_tool_result".into(),
                arguments: json!({"reference":reference,"offset":0,"limit":7_000}),
            },
            true,
        );
    }
    // The trailing model-view cursor block in the LAST range tool result is
    // the navigation truth. Inner JSON is escaped in the request.
    if let Some(true) = last_json_bool_after(body, "complete") {
        return (Reply::Stop, true);
    }
    let offset = last_json_number_after(body, "nextOffset").unwrap_or(0);
    let page = pages_seen + 1;
    (
        Reply::Tool {
            id: format!("range-read-{page}"),
            name: "read_tool_result".into(),
            arguments: json!({"reference":reference,"offset":offset,"limit":7_000}),
        },
        true,
    )
}

fn extract_result_reference(body: &str) -> Option<String> {
    let start = body.find("fox-result://")?;
    let rest = &body[start..];
    let end = rest
        .char_indices()
        .find(|(_, character)| {
            !(character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '/' | '.' | ':'))
        })
        .map(|(index, _)| index)
        .unwrap_or(rest.len());
    Some(rest[..end].to_owned())
}

fn skip_cursor_separator(tail: &str) -> &str {
    let mut rest = tail;
    loop {
        let trimmed = rest
            .trim_start_matches(|character: char| character.is_whitespace())
            .strip_prefix(['\\', '"'])
            .unwrap_or(rest);
        if trimmed == rest {
            return rest;
        }
        rest = trimmed;
    }
}

fn last_json_number_after(body: &str, key: &str) -> Option<u64> {
    let positions: Vec<usize> = body.match_indices(key).map(|(index, _)| index).collect();
    for position in positions.into_iter().rev() {
        let tail = skip_cursor_separator(&body[position + key.len()..]);
        let Some(tail) = tail.strip_prefix(':') else {
            continue;
        };
        let tail = tail.trim_start_matches(|character: char| character.is_whitespace());
        let digits: String = tail
            .chars()
            .take_while(|character| character.is_ascii_digit())
            .collect();
        if let Ok(value) = digits.parse::<u64>() {
            return Some(value);
        }
    }
    None
}

fn last_json_bool_after(body: &str, key: &str) -> Option<bool> {
    let positions: Vec<usize> = body.match_indices(key).map(|(index, _)| index).collect();
    for position in positions.into_iter().rev() {
        let tail = skip_cursor_separator(&body[position + key.len()..]);
        let Some(tail) = tail.strip_prefix(':') else {
            continue;
        };
        let tail = tail.trim_start_matches(|character: char| character.is_whitespace());
        if tail.starts_with("true") {
            return Some(true);
        }
        if tail.starts_with("false") {
            return Some(false);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Real Host/Runtime execution through the frozen GatewayPolicy.
// ---------------------------------------------------------------------------

#[derive(Default)]
struct EvalState {
    last_tool: Option<String>,
    last_error: Option<String>,
}

/// The frozen model service of the synthetic tier: a loopback HTTP/SSE provider
/// whose port the OS assigns. Nothing here is a credential.
fn synthetic_model_service(address: std::net::SocketAddr) -> Value {
    json!({
        "apiType":"openai-completions",
        "modelId":SYNTHETIC_MODEL_ID,
        "baseUrl":format!("http://{address}/v1"),
        "contextWindow": 256_000,
        "maxOutputTokens": 2_048,
    })
}

/// The evaluation's model configuration *template*.
///
/// Only `modelService` matters here: `prepare_eval_run` replaces the whole
/// configuration with the production `kernel.describe` result before anything is
/// frozen, so the `systemPrompt` and `proposalTools` fields of this template are
/// never what the model is offered. The reduced three-tool contract that used to
/// live here has been deleted outright, so this tier cannot silently fall back to
/// a harness-only tool set again; `the_evaluation_describes_the_production_tool_
/// surface` fails loudly if it does.
/// The scripted provider's calibrated tool contract.
///
/// Only the scripted (synthetic) tier uses this: its loopback provider replays a
/// fixed sequence of proposals, so the tool surface is part of the fixture rather
/// than something under test. The real tier is described through
/// `production_run_context` and never sees this list.
fn scripted_provider_tool_contract() -> Vec<Value> {
    vec![
        json!({"name":"read","description":"Read a project file",
            "parameters":{"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}}),
        json!({"name":"read_tool_result","description":"Read a byte range of a stored tool result",
            "parameters":{"type":"object","properties":{
                "reference":{"type":"string"},"offset":{"type":"integer"},"limit":{"type":"integer"}},
                "required":["reference"]}}),
        json!({"name":"call_mcp_tool","description":"Call a frozen MCP/Office tool",
            "parameters":{"type":"object","properties":{
                "serverId":{"type":"string"},"tool":{"type":"string"},
                "arguments":{"type":"object","additionalProperties":true}},
                "required":["serverId","tool","arguments"]}}),
    ]
}

fn eval_model_config(address: std::net::SocketAddr) -> kernel_model_worker::KernelModelConfig {
    let mut config = worker_configuration();
    config.model_service = synthetic_model_service(address);
    config
}

/// Default-run regression for V1: this tier must describe the PRODUCTION tool
/// surface and system prompt, and must never again be able to run against a
/// hand-written three-tool contract or a one-line system prompt.
///
/// It is deliberately not `#[ignore]`d: the review found the reduced
/// configuration only in the real (cloud) tier, which is never run by default, so
/// nothing caught it. This test builds the same configuration the cloud tier
/// builds — production prompt context, production `kernel.describe` handshake,
/// production frozen scope — and fails if any of it regresses.
#[test]
fn the_evaluation_describes_the_production_tool_surface() {
    // A private temp directory: this regression runs by default and must not
    // write into the evaluation output root.
    let root = std::env::temp_dir().join(format!(
        "fox-model-visible-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let config = eval_model_config(synthetic_placeholder_address());
    let (db, run_id) = prepare_eval_run(
        &root,
        AGV_TASK_PROMPT,
        &config,
        "model-visible-regression",
        PermissionMode::Allow,
        // The regression exercises the real tier's production description.
        true,
    )
    .expect("prepare the evaluation run");

    // 1. The frozen configuration is the production one, read back from the
    //    database exactly as the worker will read it.
    let frozen = db
        .kernel_model_config(&run_id)
        .expect("read the frozen model configuration");
    let names: Vec<String> = frozen
        .proposal_tools
        .iter()
        .filter_map(|tool| tool["name"].as_str().map(str::to_owned))
        .collect();
    // The capabilities the review found missing, plus the basics a data-analysis
    // Run cannot work without. The Office connector is reached through the
    // wrapper, so `call_mcp_tool`/`list_mcp_tools` are the discovery entry points.
    for required in [
        "read",
        "write_file",
        "list_mcp_tools",
        "call_mcp_tool",
        "attachment_compute",
        "compute_job_start",
        "tabular_data",
        "read_attachment",
    ] {
        assert!(
            names.iter().any(|name| name == required),
            "the model must be offered `{required}`; offered {names:?}"
        );
    }
    assert!(
        names.len() >= 20,
        "a production tool catalogue is not three tools; offered {} : {names:?}",
        names.len()
    );
    // The tool set is exactly the expert package's declared scope: production
    // narrows a Run to what the selected expert declares, so equality with the
    // package (rather than a hand-written list) is the contract to assert.
    let mut declared = acceptance_expert_declared_tools();
    declared.sort();
    let mut offered_sorted = names.clone();
    offered_sorted.sort();
    assert_eq!(
        offered_sorted, declared,
        "the model-visible tools must be exactly the expert package's declared tools"
    );
    assert!(
        frozen.system_prompt.chars().count() > 500,
        "the system prompt must be the production one, not the previous {} character placeholder",
        frozen.system_prompt.chars().count()
    );
    assert_ne!(
        frozen.system_prompt, "Use the durable supplied history only.",
        "the placeholder system prompt must never reach the model"
    );
    // The production prompt tells the model how the Office connector is reached.
    assert!(
        frozen.system_prompt.contains("office_") || frozen.system_prompt.contains("Office"),
        "the system prompt must carry the connector guidance the application ships"
    );

    // 2. The frozen scope is derived from the same tool set, so what the gateway
    //    admits and what the model can discover cannot disagree.
    let scope = db.kernel_host_scope(&run_id).expect("read the frozen scope");
    let mut scope_tools: Vec<String> = scope.tool_names.iter().cloned().collect();
    scope_tools.sort();
    let mut offered = names.clone();
    offered.sort();
    assert_eq!(
        scope_tools, offered,
        "the frozen scope must match the model-visible tool set"
    );
    assert!(
        !scope.office_tools.is_empty(),
        "the enabled Office connector must be discoverable in the frozen scope"
    );

    // 3. The initial input is the production one: the submitted text is present
    //    and the system prompt travels in the frozen configuration beside it.
    let initial = db
        .kernel_initial_input(&run_id)
        .expect("read the frozen initial input");
    let encoded = serde_json::to_string(&initial.messages).unwrap();
    assert!(
        encoded.contains("AGV"),
        "the submitted task text must reach the model: {encoded}"
    );

    // 4. The evidence record names what the model was offered, not just what the
    //    Host keeps internally.
    let prompt = production_prompt_for_test(&db, &root, &run_id, &frozen, AGV_TASK_PROMPT);
    let evidence = model_visible_request_evidence(&db, &run_id, &frozen, &prompt);
    assert_eq!(
        evidence["frozenScopeMatchesModelVisible"], true,
        "evidence must record scope/model agreement: {evidence}"
    );
    assert!(
        evidence["modelVisibleToolCount"].as_u64().unwrap_or(0) >= 20,
        "{evidence}"
    );
    std::fs::remove_dir_all(&root).ok();
}

/// Rebuild the prompt payload for an already-frozen Run, so the delivered
/// evidence can be reproduced without re-running the describe handshake.
fn production_prompt_for_test(
    db: &Database,
    scenario_root: &std::path::Path,
    run_id: &str,
    _config: &kernel_model_worker::KernelModelConfig,
    text: &str,
) -> Value {
    let binding = db.run_control_binding(run_id).unwrap().unwrap();
    let skills_dir = eval_host_dir(scenario_root).join("skills");
    let context = crate::runtime_host::conversation_prompt_context(
        db,
        &skills_dir,
        &binding.conversation_id,
        text,
        "primary",
        crate::runtime_host::PromptScopeOverrides { tool_allowlist: None, mcp_server_scope: None },
    )
    .expect("prompt context");
    let messages = db
        .runtime_prompt_context_budget(&binding.conversation_id, 240, 240_000)
        .expect("history");
    let project_context = crate::runtime_host::project_context_for(
        db,
        &eval_host_dir(scenario_root).join("runtime-sessions"),
        &binding.conversation_id,
        &binding.permission,
    );
    let work_snapshot =
        crate::runtime_host::work_tools::snapshot(db, &binding.conversation_id).expect("snapshot");
    let memory_context = db
        .recall_memories(&binding.conversation_id, Some(run_id), text, 8, 6_000)
        .expect("memory");
    crate::runtime_host::kernel_prompt_payload(crate::runtime_host::KernelPromptPayload {
        text,
        messages: &messages,
        images: &Vec::<Value>::new(),
        context: &context,
        project_context: &project_context,
        work_snapshot: &work_snapshot,
        memory_context: &memory_context,
        max_prompt_tokens: 240_000,
    })
}

/// The production model configuration and initial input for one evaluation Run.
/// The 2026-09-19 review found the tier had been described against a hand-written
/// three-tool contract (`read`, `read_tool_result`, `call_mcp_tool`) and a
/// one-line system prompt, so a failure could not be attributed to the model, the
/// Host or the wiring. This calls the SAME production entry points the desktop
/// Host calls:
///
/// * `conversation_prompt_context` builds the system prompt, the assistant and
///   expert packages, the expert binding and the skill plan from Host facts;
/// * `kernel_prompt_payload` builds the reference data the model receives
///   (project context, work snapshot, memory, prompt budget);
/// * `kernel_model_worker::describe` runs the production `kernel.describe`
///   handshake against the full `kernel_gateway::supported_tools()` catalog, so
///   the returned system prompt and tool definitions — including
///   `list_mcp_tools`, `attachment_compute` and the Office tools — are the ones
///   a real Run sees.
///
/// Nothing here is hand-written for the harness, and no call sequence, answer or
/// provider name is injected.
fn production_run_context(
    db: &Database,
    scenario_root: &std::path::Path,
    binding: &RunControlBinding,
    model_service: Value,
    text: &str,
) -> Result<(Value, kernel_model_worker::KernelModelConfig), String> {
    let conversation_id = binding.conversation_id.as_str();
    // The same skill store the application installs into
    // (`skills::install_bundled_office_skills`), created on demand so the
    // acceptance Run sees the production skill plan.
    let skills_dir = eval_host_dir(scenario_root).join("skills");
    std::fs::create_dir_all(&skills_dir).map_err(|error| error.to_string())?;
    crate::skills::install_bundled_office_skills(&skills_dir)?;
    let context = crate::runtime_host::conversation_prompt_context(
        db,
        &skills_dir,
        conversation_id,
        text,
        "primary",
        crate::runtime_host::PromptScopeOverrides {
            tool_allowlist: None,
            mcp_server_scope: None,
        },
    )?;
    let messages = db.runtime_prompt_context_budget(conversation_id, 240, 240_000)?;
    let project_context = crate::runtime_host::project_context_for(
        db,
        &eval_host_dir(scenario_root).join("runtime-sessions"),
        conversation_id,
        &binding.permission,
    );
    let work_snapshot = crate::runtime_host::work_tools::snapshot(db, conversation_id)?;
    let memory_context = db.recall_memories(conversation_id, Some(&binding.run_id), text, 8, 6_000)?;
    let prompt = crate::runtime_host::kernel_prompt_payload(
        crate::runtime_host::KernelPromptPayload {
            text,
            messages: &messages,
            images: &Vec::<Value>::new(),
            context: &context,
            project_context: &project_context,
            work_snapshot: &work_snapshot,
            memory_context: &memory_context,
            max_prompt_tokens: 240_000,
        },
    );
    // The runtime command the production Host resolves: the same Node worker
    // entry point the application spawns (`RuntimeHost::runtime_command`).
    let runtime = crate::runtime_host::RuntimeCommand {
        program: "node".into(),
        script: Some(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../../services/agent-runtime/src/pi-runtime.mjs"),
        ),
    };
    let registry = CancellationRegistry::default();
    registry.register_run(&binding.run_id)?;
    let token = registry.run_token(&binding.run_id)?;
    let config = kernel_model_worker::describe(
        &runtime,
        binding,
        model_service,
        prompt.clone(),
        kernel_gateway::supported_tools(),
        &token,
    )?;
    Ok((prompt, config))
}

/// Every model-visible fact about one prepared Run, recorded as desensitized
/// evidence. The review's point was precise: the Host's internal frozen scope is
/// not proof of what the *model* was offered, so this captures the request side
/// (system prompt and tool definitions) as well as the frozen scope.
fn model_visible_request_evidence(
    db: &Database,
    run_id: &str,
    config: &kernel_model_worker::KernelModelConfig,
    prompt: &Value,
) -> Value {
    let scope = db.kernel_host_scope(run_id).ok();
    let tool_names: Vec<String> = config
        .proposal_tools
        .iter()
        .filter_map(|tool| tool["name"].as_str().map(str::to_owned))
        .collect();
    let scope_tools: Vec<String> = scope
        .as_ref()
        .map(|scope| scope.tool_names.iter().cloned().collect())
        .unwrap_or_default();
    let office_tools: Vec<String> = scope
        .as_ref()
        .map(|scope| scope.office_tools.iter().cloned().collect())
        .unwrap_or_default();
    let office_exposed: Vec<String> = config
        .proposal_tools
        .iter()
        .filter_map(|tool| {
            let name = tool["name"].as_str()?;
            // The gateway reaches Office through the wrapper plus discovery.
            (name == "call_mcp_tool" || name == "list_mcp_tools").then(|| name.to_owned())
        })
        .collect();
    // Computed before the literal: a block expression inside `json!` does not
    // survive the macro's token munching.
    let scope_matches_model_visible = {
        let mut left = tool_names.clone();
        let mut right = scope_tools.clone();
        left.sort();
        right.sort();
        left == right
    };
    let prompt_keys = prompt
        .as_object()
        .map(|object| object.keys().cloned().collect::<Vec<_>>());
    json!({
        "runId": run_id,
        "expertBinding": prompt["expertBinding"],
        "expertPackagePresent": !prompt["expertPackage"].is_null(),
        "systemPromptChars": config.system_prompt.chars().count(),
        "systemPromptHash": format!("sha256:{}", hex::encode(Sha256::digest(config.system_prompt.as_bytes()))),
        "systemPromptMentionsOffice": config.system_prompt.contains("office_"),
        "modelVisibleTools": tool_names,
        "modelVisibleToolCount": tool_names.len(),
        "frozenScopeTools": scope_tools,
        "frozenScopeMatchesModelVisible": scope_matches_model_visible,
        "frozenOfficeTools": office_tools,
        "officeDiscoveryEntryPoints": office_exposed,
        "promptKeys": prompt_keys,
        "configHash": config.hash().ok(),
        "userTextChars": prompt["text"].as_str().map(|text| text.chars().count()),
    })
}

/// Prepared Run under a frozen binding of the caller's permission mode (with the
/// project-scoped Office grant) and the bundled Office connector registered,
/// mirroring the production freeze order.
fn eval_host_dir(project_root: &std::path::Path) -> std::path::PathBuf {
    // A sibling, never a child of the model's authorized project root.
    project_root.with_file_name(format!("{}.host", project_root.file_name().unwrap().to_string_lossy()))
}

fn prepare_eval_run(
    scenario_root: &std::path::Path,
    prompt: &str,
    config: &kernel_model_worker::KernelModelConfig,
    run_label: &str,
    permission_mode: PermissionMode,
    // Whether to describe through the production path (`conversation_prompt_context`
    // + `kernel.describe` against the full supported-tool catalogue).
    //
    // The real tier always does: the 2026-09-19 review found it had been running
    // against a hand-written three-tool contract and a one-line system prompt.
    // The synthetic tier keeps its previous configuration, because its model is a
    // *scripted* loopback provider whose responses are calibrated to a fixed tool
    // surface; changing that surface invalidates the script rather than testing
    // anything. `the_evaluation_describes_the_production_tool_surface` fails
    // loudly if the real tier ever regresses to the reduced contract.
    production_context: bool,
) -> Result<(Database, String), (String, String)> {
    prepare_eval_run_with_budgets(scenario_root, prompt, config, run_label,
        permission_mode, production_context, TimeBudgets::default())
}

fn prepare_eval_run_with_budgets(
    scenario_root: &std::path::Path, prompt: &str,
    config: &kernel_model_worker::KernelModelConfig, run_label: &str,
    permission_mode: PermissionMode, production_context: bool, budgets: TimeBudgets,
) -> Result<(Database, String), (String, String)> {
    eval_phase("prepare:database");
    std::fs::create_dir_all(eval_host_dir(scenario_root))
        .map_err(|error| ("prepare:host-directory".into(), error.to_string()))?;
    let db = Database::open(eval_host_dir(scenario_root).join("facts.db"))
        .map_err(|error| ("prepare:database".into(), error.to_string()))?;
    eval_phase("prepare:office-connector");
    crate::office::setup(&db, &office_resources_dir())
        .map_err(|error| ("prepare:office-connector".into(), error))?;
    eval_phase("prepare:conversation");
    let conversation = db
        .create_conversation(
            db.default_agent_id(),
            None,
            scenario_root.to_str(),
            Some(permission_mode.as_str()),
        )
        .map_err(|error| ("prepare:conversation".into(), error))?;
    // The real tier runs as the bundled 数据分析专家, installed and bound through
    // the application's own expert path. It has to happen before the Run exists:
    // production locks an expert binding once the conversation has user or
    // assistant messages, and the Run's own bookkeeping would lock it here. The
    // scripted tier deliberately skips this — its package scope would change the
    // calibrated fixture.
    if production_context {
        eval_phase("prepare:expert");
        bind_acceptance_expert(&db, scenario_root, &conversation.id)
            .map_err(|error| ("prepare:expert".into(), error))?;
    }
    eval_phase("prepare:run");
    let run_id = db
        .create_run(&conversation.id, run_label, None)
        .map_err(|error| ("prepare:run".into(), error.to_string()))?
        .run
        .id;
    // No blanket grants: Office mutations carry approval="always" and are
    // bound to their exact request (office-request scope hashes), so under
    // production they wait for a per-request human decision. The real chain is
    // therefore driven by an approval watcher that presses "allow once"
    // through the same durable command queue the Tauri UI uses.
    let permission = FrozenPermission {
        mode: permission_mode,
        project_root: Some(scenario_root.to_string_lossy().into_owned()),
        grants: vec![],
        approval_epoch: None,
    };
    let binding = RunControlBinding {
        schema_version: 1,
        run_id: run_id.clone(),
        conversation_id: conversation.id,
        engine_id: "pi".into(),
        execution_profile_id: "legacy".into(),
        authority: ExecutionAuthority::Authoritative,
        read_only_executor: ResourceExecutor::Rust,
        permission_snapshot_id: Database::run_control_permission_hash(&permission)
            .map_err(|error| ("prepare:permission".into(), error))?,
        permission,
        budgets,
    };
    db.freeze_run_control(&binding)
        .map_err(|error| ("prepare:freeze-binding".into(), error))?;
    // The real tier describes through the production path *before* the frozen
    // configuration is built, because `prompt_config_hash` must be the hash of
    // the configuration the worker is actually handed. The synthetic tier keeps
    // its calibrated configuration: its model is a scripted loopback provider
    // whose responses are tied to a fixed tool surface, so changing that surface
    // would invalidate the script instead of testing anything.
    let described = if production_context {
        Some(
            production_run_context(
                &db,
                scenario_root,
                &binding,
                config.model_service.clone(),
                prompt,
            )
            .map_err(|error| ("prepare:describe".into(), error))?,
        )
    } else {
        None
    };
    // The scripted tier's *calibrated* contract, applied only when the tier is
    // not using the production description. A focused fixture may explicitly
    // append a tool to the template; preserve that addition while keeping the
    // three calibrated tools and their order stable for every ordinary run.
    //
    // Its model is a loopback provider replaying a fixed script, so its tool
    // surface is part of the fixture: the scenario's scripted proposals name
    // `read`, `read_tool_result` and `call_mcp_tool`, and the frozen scope is
    // derived from this list. Removing these (as an earlier revision of this
    // round did) silently reduced the scripted tier's scope to one tool and the
    // scenario stopped converging. The name says who it is for, and the real
    // tier cannot reach it: `production_context` selects the branch, and
    // `the_evaluation_describes_the_production_tool_surface` fails if the real
    // tier ever ends up with a contract this small.
    let scripted_config = if described.is_none() {
        let mut scripted = config.clone();
        let mut proposal_tools = scripted_provider_tool_contract();
        for tool in &config.proposal_tools {
            let Some(name) = tool["name"].as_str() else { continue };
            if !proposal_tools.iter().any(|existing| existing["name"].as_str() == Some(name)) {
                proposal_tools.push(tool.clone());
            }
        }
        scripted.proposal_tools = proposal_tools;
        Some(scripted)
    } else {
        None
    };
    let effective_config = described
        .as_ref()
        .map(|(_, described)| described)
        .or(scripted_config.as_ref())
        .unwrap_or(config);
    let config_hash = effective_config
        .hash()
        .map_err(|error| ("prepare:config".into(), error))?;
    let frozen = kernel::RunFrozenConfig {
        engine_id: "pi".into(),
        kernel_mode: "authoritative".into(),
        capability_manifest_version: 2,
        capability_manifest_hash: "real-eval-manifest".into(),
        permission_snapshot_id: binding.permission_snapshot_id.clone(),
        execution_profile_id: binding.execution_profile_id.clone(),
        prompt_config_hash: config_hash.clone(),
        model_request_timeout_ms: binding.budgets.model_request_ms,
        model_first_response_ms: binding.budgets.model_first_response_ms,
        model_idle_ms: binding.budgets.model_idle_ms,
        tool_execution_timeout_ms: binding.budgets.tool_execution_ms,
        run_execution_budget_ms: binding.budgets.run_execution_ms,
        run_execution_limited: binding.budgets.run_execution_limited,
        approval_wait_timeout_ms: binding.budgets.approval_wait_ms,
        provider_max_retries: 0,
        turn_max_retries: 0,
    };
    db.kernel_create_run(
        &run_id,
        "pi",
        "authoritative",
        2,
        &binding.permission_snapshot_id,
        "legacy",
        &config_hash,
        &serde_json::to_string(&frozen)
            .map_err(|error| ("prepare:config".into(), error.to_string()))?,
    )
    .map_err(|error| ("prepare:kernel-run".into(), error))?;
    db.freeze_kernel_model_config(&run_id, effective_config)
        .map_err(|error| ("prepare:model-config".into(), error))?;
    let scope = match &described {
        Some((prompt_payload, _)) => {
            // The production initial input: the durable history plus the
            // submitted text, built by the same function the Host uses. The
            // system prompt travels beside it in the frozen model configuration
            // (that is where the worker takes it from), so this tier can no
            // longer run with the history and nothing else.
            let initial = kernel_host::initial_input(&binding, prompt_payload, &config_hash)
                .map_err(|error| ("prepare:initial-input".into(), error))?;
            db.freeze_kernel_initial_input(&initial)
                .map_err(|error| ("prepare:initial-input".into(), error))?;
            // The frozen scope is derived from the SAME prompt and configuration
            // the model was described against, so `assistantPackage` /
            // `expertPackage` scope connectors and Office tools exactly as they
            // do in production.
            let scope = kernel_gateway::freeze_scope(&db, &binding, prompt_payload, effective_config)
                .map_err(|error| ("prepare:scope".into(), error))?;
            scope
        }
        None => {
            let initial = fox_engine_protocol::KernelInitialModelInput {
                schema_version: 1,
                run_id: run_id.clone(),
                turn_id: "turn-1".into(),
                prompt_config_hash: config_hash,
                messages: vec![json!({"role":"user","content":prompt})],
            };
            db.freeze_kernel_initial_input(&initial)
                .map_err(|error| ("prepare:initial-input".into(), error))?;
            // The scripted tier's prompt carries no package manifest, so the
            // frozen scope is exactly the calibrated tool set.
            kernel_gateway::freeze_scope(&db, &binding, &json!({}), effective_config)
                .map_err(|error| ("prepare:scope".into(), error))?
        }
    };
    db.freeze_kernel_host_scope(&run_id, &scope)
        .map_err(|error| ("prepare:scope".into(), error))?;
    // Desensitized evidence of what the MODEL was offered, recorded *after* both
    // the model configuration and the frozen scope exist, so the two can be
    // compared rather than assumed equal.
    if let Some((prompt_payload, described)) = &described {
        let evidence = model_visible_request_evidence(&db, &run_id, described, prompt_payload);
        if let Ok(encoded) = serde_json::to_string_pretty(&evidence) {
            let _ = std::fs::write(eval_host_dir(scenario_root).join("model-visible-request.json"), encoded);
        }
        println!("EVAL-MODEL-VISIBLE {evidence}");
    }
    let mut seeds = crate::runtime_host::delivery::expectations_from_task(prompt);
    // Bind the task's structured demands exactly as the production start does,
    // including the cross-artifact ones (a stated ratio, a statistic recomputed
    // from the source data this Run names).
    let requirements = crate::runtime_host::delivery::requirements_from_task(prompt)
        .into_iter()
        .map(|requirement| {
            crate::runtime_host::delivery::bind_source_distribution(scenario_root, requirement)
        })
        .collect::<Vec<_>>();
    crate::runtime_host::delivery::attach_requirements_to_seeds(&mut seeds, &requirements);
    db.seed_delivery_checklist(&run_id, &seeds, crate::database::now_ms())
        .map_err(|error| ("prepare:checklist".into(), error))?;
    Ok((db, run_id))
}

struct RunObservations {
    state: String,
    model_requests: usize,
    /// Where `model_requests` was counted: the loopback provider's HTTP
    /// requests (synthetic tier) or the Run's durable kernel model-response
    /// events (real tier, where no local endpoint is involved).
    model_requests_source: &'static str,
    dispatch_error: Option<String>,
    /// Per-request human approvals pressed through the durable command queue
    /// (every mutating Office request carries approval="always").
    approvals: u64,
}

/// The model side of one dispatch: the loopback provider of the synthetic tier,
/// or nothing at all for the real tier (which talks to the cloud endpoint).
///
/// The session **owns** the provider handle for the whole dispatch. Dropping a
/// handle shuts the scripted provider down, so copying the shutdown flag and join
/// handle out of it (and letting the handle drop) killed the listener before the
/// worker could connect — every real-chain scenario then failed with
/// `ECONNREFUSED` and a `model_transport_failure`, which looked like a worker or
/// sidecar problem instead of a harness one.
struct ProviderSession {
    handle: Option<ProviderHandle>,
    shutdown: Arc<AtomicBool>,
}

impl ProviderSession {
    /// Real tier: no local listener exists, so a "cloud" run can never degrade
    /// into a scripted provider.
    fn detached() -> Self {
        Self {
            handle: None,
            shutdown: Arc::new(AtomicBool::new(false)),
        }
    }

    fn from_handle(handle: ProviderHandle) -> Self {
        let shutdown = handle.shutdown.clone();
        Self {
            handle: Some(handle),
            shutdown,
        }
    }

    /// The provider thread's own handle, taken once the dispatch is over.
    fn take_join(&self) -> Option<std::thread::JoinHandle<Vec<Value>>> {
        let handle = self.handle.as_ref()?;
        handle.join.lock().ok()?.take()
    }
}

/// Stand-in for the human at the Tauri UI: watches the durable
/// `kernel_approvals` queue on a separate connection and enqueues an
/// `allow_once` command per request via the SAME production API the UI uses.
/// It never fabricates grants or executes tools; the Kernel records the
/// decision itself when it drains the queue.
fn spawn_approval_watcher(
    root: &std::path::Path,
    run_id: &str,
    stop: Arc<AtomicBool>,
) -> std::thread::JoinHandle<u64> {
    let root = root.to_path_buf();
    let run_id = run_id.to_owned();
    std::thread::spawn(move || {
        let writer = Database::open(eval_host_dir(&root).join("facts.db")).unwrap();
        let reader = rusqlite::Connection::open(eval_host_dir(&root).join("facts.db")).unwrap();
        reader.busy_timeout(Duration::from_secs(5)).unwrap();
        let mut approved: BTreeSet<String> = BTreeSet::new();
        // The approver must outlive the run it serves, so it takes the same
        // configured deadline as the entry instead of a fixed 900s that would
        // cap a longer run: a real cloud AGV task reached 23 model rounds and
        // was still waiting for its first Office write approval when a fixed
        // deadline expired under it.
        let deadline = Instant::now() + eval_deadline_from_environment();
        while Instant::now() < deadline && !stop.load(Ordering::SeqCst) {
            let terminal = reader
                .query_row(
                    "SELECT COALESCE(k.state, legacy.status)
                     FROM run_control_bindings b
                     JOIN runs legacy ON legacy.id = b.run_id
                     LEFT JOIN kernel_runs k ON k.run_id = b.run_id
                     WHERE b.run_id = ?1",
                    [&run_id],
                    |row| row.get::<_, String>(0),
                )
                .is_ok_and(|state| {
                    matches!(
                        state.as_str(),
                        "completed" | "failed" | "cancelled" | "budget_exhausted"
                            | "approval_expired"
                    )
                });
            if terminal {
                break;
            }
            let mut statement = reader
                .prepare(
                    "SELECT tool_call_id FROM kernel_approvals
                     WHERE run_id = ?1 AND state = 'pending' ORDER BY rowid",
                )
                .unwrap();
            let pending: Vec<String> = statement
                .query_map([&run_id], |row| row.get::<_, String>(0))
                .unwrap()
                .flatten()
                .collect();
            drop(statement);
            for id in pending {
                if approved.contains(&id) {
                    continue;
                }
                // The Kernel only accepts the command while the request is
                // actionable; "not yet actionable" is retried on the next tick.
                // A busy/locked database is likewise retried, never surfaced: this
                // watcher is a *second writer* standing in for the UI, and it must
                // not turn its own contention into a Run failure.
                match writer.queue_kernel_host_command(&run_id, Some((&id, "allow_once"))) {
                    Ok(true) => {
                        approved.insert(id);
                    }
                    Ok(false) => {}
                    Err(_) => {}
                }
            }
            // Human-paced, not a write storm: polling faster than a person can
            // click only adds contention with the coordinator's own writes.
            std::thread::sleep(Duration::from_millis(150));
        }
        approved.len() as u64
    })
}

/// Drive one prepared Run end to end with the REAL production tool routing:
/// kernel_host.rs classifies context resources (`read_tool_result`) and sends
/// everything else (`read`, `call_mcp_tool`) through the GatewayPolicy, whose
/// `fox-office` branch executes the pinned OfficeCLI.
///
/// `session` is the model side of the dispatch: the loopback provider of the
/// synthetic tier, or `ProviderSession::detached()` when a real endpoint is
/// configured. `api_key` is passed here only, never into a config or a report.
fn drive_real_run(
    db: &Database,
    root: &std::path::Path,
    run_id: &str,
    _owner: &str,
    session: ProviderSession,
    api_key: &str,
    state: Arc<Mutex<EvalState>>,
) -> RunObservations {
    drive_real_run_with_approvals(db, root, run_id, session, api_key, state, true)
}

fn drive_real_run_with_approvals(
    db: &Database, root: &std::path::Path, run_id: &str,
    session: ProviderSession, api_key: &str, state: Arc<Mutex<EvalState>>,
    approve: bool,
) -> RunObservations {
    let shutdown = session.shutdown.clone();
    // The approval watcher is a stand-in for the human at the UI: it must stop as
    // soon as this scenario is done, otherwise a Run that never reached a terminal
    // state would hold the entry for its full deadline with nothing to show.
    let watcher_stop = Arc::new(AtomicBool::new(false));
    let clock = crate::runtime_host::shadow_reconcile::ReconcilerClock;
    let cancellation = CancellationRegistry::default();
    let preview = |_: &fox_engine_protocol::KernelModelPreview| {};
    // The human-at-UI stand-in starts approving before the first request
    // reaches `waiting_approval`; it only writes to the durable command queue.
    let approver = approve.then(|| spawn_approval_watcher(root, run_id, watcher_stop.clone()));
    let binding = db.run_control_binding(run_id).unwrap().unwrap();
    let scope = db.kernel_host_scope(run_id).unwrap();
    let host_dir = eval_host_dir(root);
    let sessions_dir = host_dir.join("runtime-sessions");
    let attachments_dir = host_dir.join("attachments");
    let skills_dir = host_dir.join("skills");
    let policy = kernel_gateway::GatewayPolicy {
        binding, scope, database: Some(db.clone()),
        sessions_dir: Some(sessions_dir.clone()), artifacts_dir: Some(host_dir.clone()),
    };
    let proposal_policy = kernel_gateway::GatewayProposalPolicy { gateway: &policy, database: db };
    // The evaluation keeps its own isolated Host data directory (its own
    // database, artifacts and managed-file snapshots), never the user's.
    let managed_dir = eval_host_dir(root).join("managed-files");
    let execute = |_: &RunControlBinding,
                   effect: &kernel::OutboxEffect,
                   token: &kernel::CancellationToken| {
        let payload: Value = serde_json::from_str(&effect.payload_json)
            .map_err(|_| "invalid frozen tool dispatch")?;
        let tool = payload["tool"].as_str().ok_or("missing tool name")?;
        {
            state.lock().unwrap().last_tool = Some(tool.into());
        }
        // Every real tool call is a phase: an Office round that never returns is
        // then attributable to the tool (sidecar) rather than to the worker.
        eval_phase(&format!("tool:{tool}"));
        let result = if kernel_gateway::is_context_resource(tool) {
            policy.execute_context_resource(
                db, &attachments_dir, &sessions_dir, &skills_dir, tool, &payload["input"], token,
            )
        } else {
            // Drive the SAME production execution seam the desktop Host uses, so
            // a real Office save performed here is registered as a restorable
            // content version exactly as it would be in the app. Calling the
            // gateway directly would execute the write and register nothing.
            crate::runtime_host::managed_files::execute_with_managed_versions(
                crate::runtime_host::managed_files::ManagedExecutionContext {
                    database: db,
                    backups_dir: &managed_dir,
                    conversation_id: &policy.binding.conversation_id,
                    run_id: &policy.binding.run_id,
                    project_root: policy.binding.permission.project_root.as_deref(),
                    permission_mode: policy.binding.permission.mode.as_str(),
                    scope: &policy.scope,
                    sessions_dir: Some(&sessions_dir),
                },
                tool,
                &payload["input"],
                // Production identifies the call by the durable effect, not by a
                // payload key; using the same source keeps the evaluation's
                // registration rows as traceable as the app's.
                effect.tool_call_id.as_deref(),
                || policy.execute(db, tool, &payload["input"], token),
            )
        };
        match result {
            Ok(value) => Ok((
                value.get("isError").and_then(Value::as_bool) != Some(true),
                value,
            )),
            Err(error) => {
                state.lock().unwrap().last_error = Some(error.clone());
                // Mirrors the production executor: non-work/non-delegation
                // failures while the Run is alive become ordinary tool-failure
                // results; cancellation surfaces the error.
                if token.check().is_ok() {
                    Ok((
                        false,
                        kernel_host::resource_failure_result(tool, &error),
                    ))
                } else {
                    Err(error)
                }
            }
        }
    };
    eval_phase("drive:worker-dispatch");
    let dispatch_error = kernel_host::acquire(&sessions_dir, run_id)
        .and_then(|ownership| kernel_host::drive_with_actions(
            &ownership, db, &clock, &cancellation, run_id,
            &real_worker_command(), api_key, &proposal_policy,
            &execute, |_| Ok(()), |_| Ok(()), &preview,
        )).err();
    eval_phase("drive:after-dispatch");
    if let Some(error) = &dispatch_error {
        // Printed immediately: the entry must not carry a fast dispatch failure
        // all the way to the report before anybody can see why it failed.
        eprintln!("[eval] dispatch_error: {error}");
    }
    shutdown.store(true, Ordering::SeqCst);
    watcher_stop.store(true, Ordering::SeqCst);
    eval_phase("post:snapshot");
    let state = db.kernel_host_run_state(run_id).ok().flatten()
        .unwrap_or_else(|| "unknown".into());
    // The synthetic tier counts the loopback provider's real HTTP requests; the
    // real tier counts the Run's durable model-response events instead, because
    // it has no local endpoint to observe.
    eval_phase("post:provider-join");
    let (model_requests, model_requests_source) = match session.take_join() {
        Some(join) => (
            join.join().expect("provider thread").len(),
            PROVIDER_ROUNDS_SOURCE,
        ),
        None => (durable_model_rounds(root, run_id), DURABLE_ROUNDS_SOURCE),
    };
    eval_phase("post:approver-join");
    let approvals = approver.map(|thread| thread.join().unwrap()).unwrap_or(0);
    eval_phase("post:observations");
    RunObservations {
        state,
        model_requests,
        model_requests_source,
        dispatch_error,
        approvals,
    }
}

/// Durable count of model rounds for a Run, read straight from the kernel event
/// stream (`run_id` is bound as a parameter, never interpolated). The event set
/// is exactly the one `kernel_projection.rs` treats as the Run's model
/// responses (initial / per-batch / continuation), so a rename there must be
/// mirrored here — the real tier then fails loudly instead of passing silently.
fn durable_model_rounds(root: &std::path::Path, run_id: &str) -> usize {
    let Ok(connection) = rusqlite::Connection::open(eval_host_dir(&root).join("facts.db")) else {
        return 0;
    };
    connection
        .query_row(
            "SELECT COUNT(*) FROM kernel_events
             WHERE run_id = ?1
               AND event_type IN ('engine.initial_response','engine.batch_response','engine.continuation_response')",
            [run_id],
            |row| row.get::<_, i64>(0),
        )
        .map(|count| count.max(0) as usize)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Report gathering
// ---------------------------------------------------------------------------

struct ScenarioOutcome {
    id: &'static str,
    title: &'static str,
    /// Which tier produced this scenario; recorded in the scenario JSON so a
    /// single scenario can never be mistaken for the other tier.
    mode: EvalMode,
    status: &'static str,
    run_id: Option<String>,
    duration_ms: u128,
    model_requests: Option<usize>,
    model_requests_source: &'static str,
    failure_stage: Option<String>,
    failure: Option<Value>,
    checks: Vec<(String, bool, String)>,
    delivery: Value,
    usage: Value,
    tool_executions: Value,
    artifacts: Vec<String>,
    /// (name, sha256, bytes) of every deliverable/artifact file, so a report
    /// names the exact bytes it verified.
    artifact_hashes: Vec<(String, String, u64)>,
    source_sha256: Option<String>,
}

impl ScenarioOutcome {
    fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "title": self.title,
            "tier": self.mode.tier(),
            "status": self.status,
            "executed": true,
            "runId": self.run_id,
            "durationMs": self.duration_ms,
            "modelRequests": self.model_requests,
            "modelRequestsSource": self.model_requests_source,
            "failureStage": self.failure_stage,
            "failure": self.failure,
            "checks": self.checks.iter().map(|(id, passed, detail)|
                json!({"id":id,"passed":passed,"detail":detail})).collect::<Vec<_>>(),
            "deliveryChecks": self.delivery,
            "usage": self.usage,
            "toolExecutions": self.tool_executions,
            "artifacts": self.artifacts,
            "artifactHashes": self.artifact_hashes.iter().map(|(name, sha256, bytes)|
                json!({"name":name,"sha256":sha256,"bytes":bytes})).collect::<Vec<_>>(),
            "sourceSha256": self.source_sha256,
        })
    }

    fn with_duration(mut self, duration_ms: u128) -> Self {
        self.duration_ms = duration_ms;
        self
    }
}

fn usage_report_for(root: &std::path::Path, run_id: &str) -> Value {
    let connection = match rusqlite::Connection::open(eval_host_dir(&root).join("facts.db")) {
        Ok(connection) => connection,
        Err(error) => return json!({"hostUsageRows":0,"error":error.to_string()}),
    };
    let mut statement = match connection.prepare(
        "SELECT event_json FROM run_events WHERE run_id=?1 AND event_type IN ('usage.updated','usage.request') ORDER BY rowid",
    ) {
        Ok(statement) => statement,
        Err(error) => return json!({"hostUsageRows":0,"error":error.to_string()}),
    };
    let rows = statement
        .query_map([run_id], |row| row.get::<_, String>(0))
        .unwrap();
    let mut count = 0usize;
    let mut events: Vec<Value> = Vec::new();
    for row in rows {
        if let Ok(json_text) = row {
            if let Ok(value) = serde_json::from_str::<Value>(&json_text) {
                events.push(value);
                if events.last().is_some_and(|event|event["type"]=="usage.updated") {count += 1;}
            }
        }
    }
    json!({"hostUsageRows":count,"events":events})
}

fn delivery_report(db: &Database, run_id: &str) -> Value {
    match db.delivery_checklist(run_id) {
        Ok(items) => json!(items
            .iter()
            .map(|item| json!({
                "itemKey": item.item_key,
                "targetPath": item.target_path,
                "status": item.status,
                "finding": item.finding,
                "checkedAt": item.checked_at,
            }))
            .collect::<Vec<_>>()),
        Err(error) => json!({"error": error}),
    }
}

fn tool_execution_report(
    db: &Database,
    run_id: &str,
) -> (Value, Vec<crate::database::ToolCallRecord>) {
    let records = db.list_runtime_tool_calls_for_run(run_id).unwrap_or_default();
    let value = json!(records
        .iter()
        .map(|record| json!({
            "toolCallId": record.runtime_tool_call_id,
            "tool": record.tool_name,
            // The inner Office/MCP tool for `call_mcp_tool` rounds, so a report
            // shows which frozen connector tool really ran.
            "mcpTool": record.input.get("tool").and_then(Value::as_str),
            "status": record.status,
        }))
        .collect::<Vec<_>>());
    (value, records)
}

/// Office mutation rounds must wait for a per-request human decision; every
/// other tool (`read`, `read_tool_result`, `office_validate`, `office_read`)
/// must not. Read from the durable tool-call records, never from a plan.
fn office_mutations<'a>(
    records: &'a [crate::database::ToolCallRecord],
) -> Vec<&'a crate::database::ToolCallRecord> {
    records
        .iter()
        .filter(|record| {
            record.tool_name == "call_mcp_tool"
                && matches!(
                    record.input.get("tool").and_then(Value::as_str),
                    Some("office_create" | "office_edit")
                )
        })
        .collect()
}

fn list_artifacts(root: &std::path::Path) -> Vec<String> {
    let mut artifacts = Vec::new();
    if let Ok(entries) = std::fs::read_dir(root) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name == "facts.db" {
                continue;
            }
            artifacts.push(name);
        }
    }
    artifacts.sort();
    artifacts
}

/// sha256 of every produced file (SQLite's own files are listed by name in
/// `artifacts` but not hashed: they are runtime state, not deliverables).
fn artifact_hashes(root: &std::path::Path) -> Vec<(String, String, u64)> {
    let mut hashes = Vec::new();
    if let Ok(entries) = std::fs::read_dir(root) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if matches!(name.as_str(), "facts.db" | "facts.db-wal" | "facts.db-shm") {
                continue;
            }
            let Ok(bytes) = std::fs::read(entry.path()) else {
                continue;
            };
            hashes.push((name, sha256_hex(&bytes), bytes.len() as u64));
        }
    }
    hashes.sort_by(|left, right| left.0.cmp(&right.0));
    hashes
}

fn fail_outcome(
    mode: EvalMode,
    id: &'static str,
    title: &'static str,
    stage: String,
    detail: String,
    run_id: Option<String>,
    root: Option<&std::path::Path>,
) -> ScenarioOutcome {
    ScenarioOutcome {
        id,
        title,
        mode,
        status: "failed",
        run_id: run_id.clone(),
        duration_ms: 0,
        model_requests: None,
        model_requests_source: match mode {
            EvalMode::SyntheticProvider => PROVIDER_ROUNDS_SOURCE,
            EvalMode::RealCloudModel => DURABLE_ROUNDS_SOURCE,
        },
        failure_stage: Some(stage.clone()),
        failure: Some(json!({"stage":stage,"error":detail})),
        checks: vec![],
        delivery: Value::Null,
        usage: root
            .map(|root| usage_report_for(root, run_id.as_deref().unwrap_or("-")))
            .unwrap_or(Value::Null),
        tool_executions: Value::Null,
        artifacts: root.map(list_artifacts).unwrap_or_default(),
        artifact_hashes: root.map(artifact_hashes).unwrap_or_default(),
        source_sha256: None,
    }
}

// ---------------------------------------------------------------------------
// Scenario A: AGV — two real Excel files and one real Word document.
//
// Two drivers share the SAME checks: the synthetic tier replays a plan we
// authored, the real tier hands the very same prompt to a cloud model that
// decides its own plan.
// ---------------------------------------------------------------------------

fn scenario_root(mode: EvalMode, scenario: &str) -> Result<std::path::PathBuf, String> {
    // This evaluation's own namespace, and an exclusive `create_dir` on top: the
    // previous evaluation's database, snapshots and documents stay exactly where
    // they were, and a directory that already exists is never reused as if it
    // were empty.
    reserve_directory(&mode.evaluation_dir().join("artifacts"), scenario)
}

/// Copy the CURRENT source workbook into the scenario root and derive the
/// expectations from those exact bytes. Shared by both tiers so a real run is
/// checked against the same, never hardcoded, numbers.
fn stage_source_workbook(root: &std::path::Path) -> Result<(Vec<u8>, Expectations), String> {
    let source_path = source_workbook_path();
    let source_bytes = std::fs::read(&source_path)
        .map_err(|error| format!("{}: {error}", source_path.display()))?;
    std::fs::write(root.join(AGV_SOURCE_NAME), &source_bytes)
        .map_err(|error| format!("source copy failed: {error}"))?;
    let expected = derive_expectations(&source_bytes)?;
    println!(
        "EVAL-AGV expectations derived from current source: total={} tasks={:?} statuses={:?} waits={}..{} avg={} buckets={:?} exceptions={} sha256={}",
        expected.total,
        expected.task_counts,
        expected.status_counts,
        expected.wait_min,
        expected.wait_max,
        expected.wait_avg,
        expected.buckets,
        expected.exception_total,
        expected.source_sha256,
    );
    Ok((source_bytes, expected))
}

/// Synthetic tier: scripted loopback provider, fixed plan, exact round count.
fn run_agv_scenario() -> ScenarioOutcome {
    let mode = EvalMode::SyntheticProvider;
    let id = "agv-deliverables";
    let title = "AGV：真实链路生成两份 Excel 与一份 Word";
    let started = Instant::now();
    eval_phase("agv:scenario-root");
    let root = match scenario_root(mode, "agv") {
        Ok(root) => root,
        Err(error) => {
            return fail_outcome(
                mode,
                id,
                title,
                "environment:output-dir".into(),
                error,
                None,
                None,
            )
        }
    };
    eval_phase("agv:stage-source-workbook");
    let expected = match stage_source_workbook(&root) {
        Ok((_bytes, expected)) => expected,
        Err(error) => {
            return fail_outcome(
                mode,
                id,
                title,
                "environment:source-workbook".into(),
                error,
                None,
                Some(&root),
            )
        }
    };
    eval_phase("agv:scripted-provider");
    let script = agv_script(&expected);
    let planned_rounds = script.len();
    let handle = scripted_provider(script);
    let config = eval_model_config(handle.address);
    eval_phase("agv:prepare-run");
    let (db, run_id) =
        match prepare_eval_run(
            &root,
            AGV_TASK_PROMPT,
            &config,
            "real-task-eval",
            PermissionMode::Allow,
            // Scripted-provider regression: its calibrated tool surface is the
            // point of the scenario, so it keeps the legacy description.
            false,
        ) {
            Ok(prepared) => prepared,
            Err((stage, error)) => {
                return fail_outcome(mode, id, title, stage, error, None, Some(&root));
            }
        };
    let eval_state = Arc::new(Mutex::new(EvalState::default()));
    eval_phase("agv:drive-run");
    let observations = drive_real_run(
        &db,
        &root,
        &run_id,
        "real-eval-agv",
        ProviderSession::from_handle(handle),
        EVAL_API_KEY,
        eval_state.clone(),
    );
    run_agv_checks(
        mode,
        &ModelIdentity::synthetic_placeholder(),
        &db,
        &root,
        &run_id,
        &expected,
        observations,
        eval_state,
        Some(planned_rounds),
        started,
    )
}

/// Real tier: the endpoint comes from `FOX_EVAL_*`, no local provider exists,
/// and the model chooses its own tool calls and round count.
fn run_agv_cloud_scenario(env: &CloudEnvironment, identity: &ModelIdentity) -> ScenarioOutcome {
    let mode = EvalMode::RealCloudModel;
    let id = "agv-deliverables-cloud";
    let title = "AGV（真实云模型）：真实模型驱动同一链路生成两份 Excel 与一份 Word";
    let started = Instant::now();
    eval_phase("agv-cloud:scenario-root");
    let root = match scenario_root(mode, "agv") {
        Ok(root) => root,
        Err(error) => {
            return fail_outcome(
                mode,
                id,
                title,
                "environment:output-dir".into(),
                error,
                None,
                None,
            )
        }
    };
    eval_phase("agv-cloud:stage-source-workbook");
    let expected = match stage_source_workbook(&root) {
        Ok((_bytes, expected)) => expected,
        Err(error) => {
            return fail_outcome(
                mode,
                id,
                title,
                "environment:source-workbook".into(),
                error,
                None,
                Some(&root),
            )
        }
    };
    eval_phase("agv-cloud:prepare-run");
    let config = env.model_config();
    // The user's ORIGINAL full instruction when one was supplied; the shortened
    // harness prompt otherwise. Which one ran is recorded in the report.
    let agv_instruction = agv_task_prompt();
    println!(
        "EVAL-AGV-INSTRUCTION {}",
        agv_instruction_identity(&agv_instruction)
    );
    let (db, run_id) =
        match prepare_eval_run(
            &root,
            &agv_instruction,
            &config,
            "real-cloud-model-eval",
            PermissionMode::Allow,
            // The real tier is the one the review found running against a
            // hand-written three-tool contract: always describe through
            // production here.
            true,
        ) {
            Ok(prepared) => prepared,
            Err((stage, error)) => {
                return fail_outcome(mode, id, title, stage, error, None, Some(&root));
            }
        };
    let eval_state = Arc::new(Mutex::new(EvalState::default()));
    eval_phase("agv-cloud:drive-run");
    let observations = drive_real_run(
        &db,
        &root,
        &run_id,
        "real-eval-agv-cloud",
        ProviderSession::detached(),
        &env.api_key,
        eval_state.clone(),
    );
    run_agv_checks(
        mode,
        identity,
        &db,
        &root,
        &run_id,
        &expected,
        observations,
        eval_state,
        None,
        started,
    )
}

/// Invariants of the real tier: plan-independent and round-count independent.
/// A real model decides its own tool calls and how many rounds it takes, so this
/// layer asserts only that the model really ran, that the Run's frozen identity
/// is exactly the environment-configured endpoint (never a local stand-in), and
/// that every Office mutation was gated by its own per-request approval.
fn real_tier_structural_checks(
    observations: &RunObservations,
    mutations: &[&crate::database::ToolCallRecord],
    identity: &ModelIdentity,
    frozen_service: &Value,
    results: &mut CheckResults,
) {
    results.check(
        "model:at-least-one-round",
        observations.model_requests >= 1,
        format!(
            "rounds={} source={}",
            observations.model_requests, observations.model_requests_source
        ),
    );
    results.check(
        "model:rounds-counted-durably",
        observations.model_requests_source == DURABLE_ROUNDS_SOURCE,
        format!("source={}", observations.model_requests_source),
    );
    let problems = identity.mismatches(frozen_service);
    results.check(
        "model:identity-is-environment-configured",
        problems.is_empty(),
        if problems.is_empty() {
            format!(
                "modelId={} host={} apiType={}",
                identity.model_id, identity.base_url_host, identity.api_type
            )
        } else {
            problems.join("; ")
        },
    );
    results.check(
        "model:not-the-synthetic-provider",
        identity.model_id != SYNTHETIC_MODEL_ID
            && !frozen_service.to_string().contains(SYNTHETIC_MODEL_ID),
        format!("modelId={} frozen={frozen_service}", identity.model_id),
    );
    results.check(
        "tools:office-mutations",
        !mutations.is_empty(),
        format!("mutating call_mcp_tool rounds={}", mutations.len()),
    );
    let unfinished: Vec<&str> = mutations
        .iter()
        .filter(|record| record.status != "completed")
        .map(|record| record.runtime_tool_call_id.as_str())
        .collect();
    results.check(
        "tools:mutations-completed",
        unfinished.is_empty(),
        format!("unfinished mutations={unfinished:?}"),
    );
    results.check(
        "approvals:every-mutation-gated",
        observations.approvals == mutations.len() as u64,
        format!(
            "allow_once decisions={} mutations={}",
            observations.approvals,
            mutations.len()
        ),
    );
}

#[allow(clippy::too_many_arguments)]
/// N1 helper: the managed-file versions this Run registered for the documents it
/// really saved through the frozen Office connector.
fn managed_office_versions(
    db: &Database,
    run_id: &str,
    names: &[&str],
) -> Vec<crate::database::ManagedFileVersion> {
    let Ok(Some(binding)) = db.run_control_binding(run_id) else {
        return Vec::new();
    };
    let Ok(rows) = db.managed_file_versions(&binding.conversation_id, None) else {
        return Vec::new();
    };
    rows.into_iter()
        .filter(|row| row.run_id.as_deref() == Some(run_id))
        .filter(|row| matches!(row.tool.as_str(), "office_create" | "office_edit"))
        .filter(|row| names.iter().any(|name| row.display_name.contains(name)))
        .collect()
}

/// What the positive chain cannot prove: what must **not** become a managed
/// version. No row is ever fabricated here: the checks assert absence, plus the
/// absence of any file a rejected write would have produced.
///
/// Layers, stated per check so a reader never mistakes one for another:
/// * `office-gate:*` on the Run's own `GatewayPolicy` — the production
///   permission/dispatch entry `kernel_host.rs` uses, so a refusal is the
///   gateway's decision rather than a test callback's. This proves admission and
///   the absence of any side effect, because the gateway refuses in `validate`
///   before the Office executor exists.
/// * A dispatch after the Run went terminal is refused for want of an active
///   Kernel owner (recorded as its own check). That fence bounds the gateway:
///   the two cases that must **really reach** the pinned OfficeCLI therefore run
///   the same executor the gateway's `fox-office` branch calls
///   (`office::execute_with_cancellation`) inside the shared registration seam,
///   and say so in their detail.
/// * `office-gate:real-read-only-office-produced-no-version` is read from the
///   live Run's own durable tool records, which is the only place a real
///   read-only Office call actually happened.
/// * `managed:generic-mcp-declaration-not-registered@seam` is the shared
///   registration seam alone. It needs a second enabled connector, which a Run
///   cannot have without changing its frozen scope; what it proves is that a
///   result-shaped declaration never registers by itself, not what a live
///   generic connector would do.
fn managed_office_gate_checks(
    mode: EvalMode,
    db: &Database,
    root: &std::path::Path,
    run_id: &str,
    records: &[crate::database::ToolCallRecord],
    registered_versions: usize,
    checks: &mut CheckResults,
) {
    use crate::runtime_host::managed_files::{
        execute_with_managed_versions, ManagedExecutionContext,
    };
    let Ok(Some(binding)) = db.run_control_binding(run_id) else {
        checks.check("managed:negatives-setup", false, "no control binding");
        return;
    };
    let Ok(scope) = db.kernel_host_scope(run_id) else {
        checks.check("managed:negatives-setup", false, "no frozen scope");
        return;
    };
    // This Run's real gateway, and a token that is never cancelled: a refusal
    // below must be attributable to the gate, never to a cancelled Run.
    let policy = kernel_gateway::GatewayPolicy {
        binding: binding.clone(),
        scope: scope.clone(),

        database: None,
        sessions_dir: None, artifacts_dir: None,
    };
    let registry = CancellationRegistry::default();
    if registry.register_run(run_id).is_err() {
        checks.check("managed:negatives-setup", false, "no cancellation scope");
        return;
    }
    let Ok(token) = registry.run_token(run_id) else {
        checks.check("managed:negatives-setup", false, "no run token");
        return;
    };
    let managed_dir = eval_host_dir(root).join("managed-files");
    let context = ManagedExecutionContext {
        database: db,
        backups_dir: &managed_dir,
        conversation_id: &binding.conversation_id,
        run_id,
        project_root: binding.permission.project_root.as_deref(),
        permission_mode: binding.permission.mode.as_str(),
        scope: &scope,
        sessions_dir: None,
    };
    let count = || {
        db.managed_file_versions(&binding.conversation_id, None)
            .map(|rows| rows.len())
            .unwrap_or(usize::MAX)
    };
    let before = count();
    let write = |name: &str, body: &[u8]| {
        let path = root.join(name);
        std::fs::write(&path, body).expect("a gate case seeds its own input file");
    };

    // One dispatch through the production seam, counting gateway entries.
    let executions = std::cell::Cell::new(0u64);
    let dispatch = |context: &ManagedExecutionContext<'_>,
                    tool: &str,
                    input: &Value,
                    tool_call_id: &str| {
        executions.set(executions.get() + 1);
        execute_with_managed_versions(*context, tool, input, Some(tool_call_id), || {
            policy.execute(db, tool, input, &token)
        })
    };

    // 1. A connector this Run never froze is outside the production scope. The
    //    gateway refuses before dispatching anything, so the real Office
    //    executor never runs: no document appears and no version is registered.
    let executor_before = crate::office::executor_entries_for_test();
    let unfrozen = dispatch(
        &context,
        "call_mcp_tool",
        &json!({"serverId": "not-a-frozen-connector", "tool": "office_create",
            "arguments": {"output": "neg-unfrozen.docx"}}),
        "gate-unfrozen",
    );
    let executor_calls = crate::office::executor_entries_for_test() - executor_before;
    let unfrozen_file = root.join("neg-unfrozen.docx").exists();
    checks.check(
        "office-gate:unfrozen-connector-refused",
        matches!(unfrozen.as_ref(), Err(error) if error.contains("outside the frozen scope"))
            && executor_calls == 0 && !unfrozen_file
            && count() == before,
        format!(
            "gatewayError={:?} executorCalls={executor_calls} targetFileExists={unfrozen_file} gatewayDispatches={} \
             versionsBefore={before} versionsAfter={}",
            unfrozen.as_ref().err(),
            executions.get(),
            count(),
        ),
    );

    // 2. An Office operation the frozen scope does not authorize: same connector,
    //    same project root, operation outside the frozen catalog.
    let executor_before = crate::office::executor_entries_for_test();
    let unauthorized = dispatch(
        &context,
        "call_mcp_tool",
        &json!({"serverId": crate::office::SERVER_ID, "tool": "office_drop_document",
            "arguments": {"output": "neg-unauthorized-op.docx"}}),
        "gate-unauthorized-op",
    );
    let executor_calls = crate::office::executor_entries_for_test() - executor_before;
    let unauthorized_file = root.join("neg-unauthorized-op.docx").exists();
    let after_unauthorized = count();
    checks.check(
        "office-gate:unauthorized-office-operation-refused",
        matches!(unauthorized.as_ref(), Err(error) if error.contains("Office operation is outside frozen scope"))
            && executor_calls == 0 && !unauthorized_file
            && after_unauthorized == before,
        format!(
            "gatewayError={:?} executorCalls={executor_calls} targetFileExists={unauthorized_file} versionsBefore={before} \
             versionsAfter={after_unauthorized} (the scope gate runs before any OfficeCLI execution)",
            unauthorized.as_ref().err(),
        ),
    );

    // 3. A Run frozen **read-only** may not mutate a document even through the
    //    authorized connector: the production permission gate refuses the real
    //    office_create, and the target document is never written.
    match prepare_read_only_gate_run(mode) {
        Ok(readonly) => {
            let executor_before = crate::office::executor_entries_for_test();
            let refused = readonly.dispatch(
                "call_mcp_tool",
                &json!({"serverId": crate::office::SERVER_ID, "tool": "office_create",
                    "arguments": {"output": "neg-readonly.docx"}}),
                "gate-readonly-create",
            );
            let executor_calls = crate::office::executor_entries_for_test() - executor_before;
            let written = readonly.root.join("neg-readonly.docx").exists();
            let after = readonly.count();
            checks.check(
                "office-gate:read-only-project-refuses-office-write",
                matches!(refused.as_ref(), Err(error) if error.contains("只读"))
                    && executor_calls == 0 && !written
                    && after == readonly.before,
                format!(
                    "gatewayError={:?} executorCalls={executor_calls} targetFileExists={written} versionsBefore={} versionsAfter={}",
                    refused.as_ref().err(),
                    readonly.before,
                    after,
                ),
            );
        }
        Err(error) => checks.check(
            "office-gate:read-only-project-refuses-office-write",
            false,
            format!("could not prepare the read-only Run: {error}"),
        ),
    }

    // 4. The ownership fence, recorded because it bounds every case below: a
    //    dispatch through the Run's gateway after the Run went terminal is
    //    refused for want of an active Kernel owner, so a post-run dispatch can
    //    prove admission but cannot reach the sidecar.
    let fenced = dispatch(
        &context,
        "call_mcp_tool",
        &json!({"serverId": crate::office::SERVER_ID, "tool": "office_read",
            "arguments": {"file": AGV_REPORT_NAME}}),
        "gate-after-terminal",
    );
    checks.check(
        "office-gate:terminal-run-dispatch-needs-an-active-owner",
        matches!(fenced.as_ref(), Err(error) if error.contains("active Kernel owner"))
            && count() == before,
        format!(
            "gatewayError={:?} versionsBefore={before} versionsAfter={} \
             (the sidecar is never reached once the Run is terminal)",
            fenced.as_ref().err(),
            count(),
        ),
    );

    // 5. A real read-only Office operation must not produce a write version.
    //    Proven from the live Run's own durable records: the Run really called
    //    non-mutating Office operations through the production gateway, and the
    //    registry holds exactly one version per mutation and none per read.
    fn inner_tool(record: &crate::database::ToolCallRecord) -> &str {
        record
            .input
            .get("tool")
            .and_then(Value::as_str)
            .unwrap_or_default()
    }
    let mutating: Vec<_> = records
        .iter()
        .filter(|record| {
            record.tool_name == "call_mcp_tool"
                && matches!(inner_tool(record), "office_create" | "office_edit")
        })
        .collect();
    let reading: Vec<_> = records
        .iter()
        .filter(|record| {
            record.tool_name == "call_mcp_tool"
                && matches!(
                    inner_tool(record),
                    "office_read" | "office_validate" | "office_help"
                )
        })
        .collect();
    checks.check(
        "office-gate:real-read-only-office-produced-no-version",
        !mutating.is_empty() && reading.len() >= 1 && registered_versions == mutating.len(),
        format!(
            "liveRunMutations={} liveRunReads={:?} registeredVersions={registered_versions} \
             (one version per real write, none for any read)",
            mutating.len(),
            reading.iter().map(|record| inner_tool(record)).collect::<Vec<_>>(),
        ),
    );

    // 6. A real Office write that fails: the pinned OfficeCLI runs against a
    //    document it cannot open, the failure is reported as it happened, the
    //    target's bytes are untouched, and nothing is registered as a success.
    //    Layer: the real executor `office::execute_with_cancellation` — the very
    //    call the gateway's fox-office branch makes — inside the shared
    //    registration seam; it is not a test callback pretending to be Office.
    let broken = b"not a document at all, and no OfficeCLI can open this";
    write("neg-broken.docx", broken);
    let before_bytes = sha256_hex(broken);
    let arguments = json!({"file": "neg-broken.docx", "output": "neg-broken.docx",
        "overwrite": true,
        "operations": [{"command": "add", "path": "/body", "type": "paragraph",
            "props": {"text": "a write that cannot succeed"}}]});
    let wrapper = json!({"serverId": crate::office::SERVER_ID, "tool": "office_edit",
        "arguments": arguments.clone()});
    let server = db.get_mcp_server(crate::office::SERVER_ID).ok().flatten();
    let executor_before = crate::office::executor_entries_for_test();
    let failed = execute_with_managed_versions(
        context,
        "call_mcp_tool",
        &wrapper,
        Some("gate-failed-write"),
        || match &server {
            Some(server) => crate::office::execute_with_cancellation(
                server,
                "office_edit",
                &arguments,
                Some(root.to_str().unwrap_or_default()),
                binding.permission.mode.as_str(),
                Some(&token),
                Duration::from_secs(120),
                None,
            ),
            None => Err("the built-in Office connector is not registered".into()),
        },
    );
    let executor_calls = crate::office::executor_entries_for_test() - executor_before;
    let after_bytes = std::fs::read(root.join("neg-broken.docx"))
        .map(|bytes| sha256_hex(&bytes))
        .unwrap_or_default();
    let reported_failure = match &failed {
        Err(_) => true,
        Ok(value) => value.get("isError").and_then(Value::as_bool) == Some(true),
    };
    checks.check(
        "office-gate:real-failed-office-write-not-registered",
        executor_calls == 1 && reported_failure && after_bytes == before_bytes && count() == before,
        format!(
            "layer=real-OfficeCLI-executor+shared-registration-seam executorCalls={executor_calls} dispatchFailed={reported_failure} \
             bytesUnchanged={after_bytes} (expected {before_bytes}) versionsBefore={before} \
             versionsAfter={} result={:?}",
            count(),
            failed.as_ref().err().cloned().or_else(|| failed.as_ref().ok().map(|value| {
                value.to_string().chars().take(240).collect::<String>()
            })),
        ),
    );

    // 7. `@seam` layer: an ordinary MCP connector whose result carries a
    //    byte-identical `foxManagedFile` declaration. The seam decides from Host
    //    facts, so a declaration is data, not authority. See the doc comment for
    //    why this one case cannot go through the gateway.
    let mut seam_executed = false;
    let generic = execute_with_managed_versions(
        context,
        "call_mcp_tool",
        &json!({"serverId": "other-connector", "tool": "office_create",
            "arguments": {"output": "neg-generic.docx"}}),
        Some("neg-generic"),
        || {
            seam_executed = true;
            write("neg-generic.docx", b"generic connector output");
            Ok(json!({"details": {"foxManagedFile": {
                "path": "neg-generic.docx",
                "operation": "office_create",
                "bytes": 24
            }}}))
        },
    );
    let after_generic = count();
    checks.check(
        "managed:generic-mcp-declaration-not-registered@seam",
        seam_executed && generic.is_ok() && after_generic == before,
        format!(
            "layer=shared-registration-seam executed={seam_executed} \
             versionsBefore={before} versionsAfter={after_generic} \
             (declaration is data, not authority; not a production gateway test)",
        ),
    );
}

/// One Run frozen **read-only**, used only to ask its real gateway what it does
/// with a document-mutating Office call. It never runs a model round, so its
/// model service is the loopback placeholder; everything that is tested here is
/// the frozen permission and the dispatch behind it.
struct ReadOnlyGateRun {
    db: Database,
    root: std::path::PathBuf,
    managed_dir: std::path::PathBuf,
    conversation_id: String,
    run_id: String,
    before: usize,
}

impl ReadOnlyGateRun {
    fn count(&self) -> usize {
        self.db
            .managed_file_versions(&self.conversation_id, None)
            .map(|rows| rows.len())
            .unwrap_or(usize::MAX)
    }

    /// The same wiring `kernel_host.rs` gives the desktop Host: the Run's own
    /// frozen `GatewayPolicy` inside the shared managed-file registration seam.
    fn dispatch(&self, tool: &str, input: &Value, tool_call_id: &str) -> Result<Value, String> {
        use crate::runtime_host::managed_files::{
            execute_with_managed_versions, ManagedExecutionContext,
        };
        let binding = self
            .db
            .run_control_binding(&self.run_id)?
            .ok_or("read-only gate Run has no control binding")?;
        let scope = self.db.kernel_host_scope(&self.run_id)?;
        let policy = kernel_gateway::GatewayPolicy { binding, scope, database: None, sessions_dir: None, artifacts_dir: None };
        let registry = CancellationRegistry::default();
        registry.register_run(&self.run_id)?;
        let token = registry.run_token(&self.run_id)?;
        execute_with_managed_versions(
            ManagedExecutionContext {
                database: &self.db,
                backups_dir: &self.managed_dir,
                conversation_id: &self.conversation_id,
                run_id: &self.run_id,
                project_root: Some(self.root.to_str().unwrap_or_default()),
                permission_mode: PermissionMode::ReadOnly.as_str(),
                scope: &policy.scope,
                sessions_dir: None,
            },
            tool,
            input,
            Some(tool_call_id),
            || policy.execute(&self.db, tool, input, &token),
        )
    }
}

fn prepare_read_only_gate_run(mode: EvalMode) -> Result<ReadOnlyGateRun, String> {
    let root = reserve_directory(&mode.evaluation_dir().join("artifacts"), "gate-readonly")?;
    let config = eval_model_config(synthetic_placeholder_address());
    let task = "请只读取现有台账并说明其中有多少条记录。";
    let (db, run_id) = prepare_eval_run(
        &root,
        task,
        &config,
        "real-eval-gate-readonly",
        PermissionMode::ReadOnly,
        // Scripted-provider regression.
        false,
    )
    .map_err(|(stage, error)| format!("{stage}: {error}"))?;
    let conversation_id = db
        .run_control_binding(&run_id)?
        .ok_or("read-only gate Run has no control binding")?
        .conversation_id;
    let before = db
        .managed_file_versions(&conversation_id, None)
        .map_err(|error| error.to_string())?
        .len();
    Ok(ReadOnlyGateRun {
        managed_dir: eval_host_dir(&root).join("managed-files"),
        db,
        root,
        conversation_id,
        run_id,
        before,
    })
}

/// Does every row's registered content really exist and hash to what the row
/// claims? Returns `(verified, failures)`; a row is verified **only** when both
/// the snapshot resolves and its bytes match, so a hash failure can never be
/// counted as a restorable version.
fn restore_source_audit(
    db: &Database,
    rows: &[crate::database::ManagedFileVersion],
) -> (usize, Vec<String>) {
    use crate::runtime_host::managed_files::{hash_file, resolve_restore_source};
    let mut verified = 0usize;
    let mut failures: Vec<String> = Vec::new();
    for row in rows {
        match resolve_restore_source(db, row) {
            Ok(source) => {
                let actual = hash_file(&source.path).map(|(hash, _)| hash);
                if actual.as_deref() == row.after_hash.as_deref() {
                    verified += 1;
                } else {
                    failures.push(format!(
                        "{}: snapshot {} hashes to {actual:?}, row claims {:?}",
                        row.id,
                        source.path.display(),
                        row.after_hash
                    ));
                }
            }
            Err(reason) => failures.push(format!("{}: {reason}", row.id)),
        }
    }
    (verified, failures)
}

/// The recovery the registry is for, through the **production restore entry**:
/// `managed_files::restore_version`, the only implementation behind the
/// `managed_file_restore` command the UI calls. No tool is replayed, no model is
/// asked, and no script copies the file — the restore itself writes the target,
/// and the target is then read back from disk.
fn restored_version_evidence(
    db: &Database,
    root: &std::path::Path,
    run_id: &str,
    registered: &[crate::database::ManagedFileVersion],
    checks: &mut CheckResults,
) {
    use crate::runtime_host::managed_files::restore_version;
    const ID: &str = "managed:restore-selected-version-actually-written";
    let Ok(Some(binding)) = db.run_control_binding(run_id) else {
        checks.check(ID, false, "no control binding");
        return;
    };
    // An older version of a document that has at least two: restoring a file's
    // newest version onto itself would prove nothing.
    let Some(older) = registered.iter().find(|row| {
        registered
            .iter()
            .any(|other| other.storage_path == row.storage_path && other.version_no > row.version_no)
    }) else {
        checks.check(ID, false, "no document has two registered versions to restore between");
        return;
    };
    let latest = registered
        .iter()
        .filter(|row| row.storage_path == older.storage_path)
        .max_by_key(|row| row.version_no)
        .expect("the group has a newest row");
    let target = std::path::PathBuf::from(&older.storage_path);
    let backups_dir = eval_host_dir(root).join("managed-files");
    let Some((current_hash, current_size)) =
        crate::runtime_host::managed_files::hash_file(&target)
    else {
        checks.check(ID, false, format!("{} is unreadable before the restore", target.display()));
        return;
    };
    let Some(older_hash) = older.after_hash.as_deref() else {
        checks.check(ID, false, format!("version {} records no content hash", older.id));
        return;
    };
    let outcome = restore_version(db, &backups_dir, &binding.conversation_id, &older.id, false);
    let restored = match outcome {
        Ok(row) => row,
        Err(error) => {
            checks.check(
                ID,
                false,
                format!("production restore of {} failed: {error}", older.id),
            );
            return;
        }
    };
    // 1. The file on disk now holds exactly the selected version's bytes.
    let (after_hash, after_size) = crate::runtime_host::managed_files::hash_file(&target)
        .unwrap_or_else(|| ("<unreadable>".to_owned(), 0));
    let bytes_match = after_hash == older_hash && Some(after_size) == older.after_size;
    // 2. The restore is itself an auditable version, pointing at what it chose.
    let row_is_correct = restored.change_kind == "restored"
        && restored.tool == "restore"
        && restored.restored_from_id.as_deref() == Some(older.id.as_str())
        && restored.after_hash.as_deref() == Some(older_hash)
        && restored.before_hash.as_deref() == Some(current_hash.as_str());
    // 3. What the restore overwrote is still reachable: the `replaced` row that
    //    precedes it carries those exact bytes and resolves to a real snapshot.
    let replaced = db
        .managed_file_versions(&binding.conversation_id, None)
        .unwrap_or_default()
        .into_iter()
        .find(|row| {
            row.storage_path == older.storage_path
                && row.change_kind == "replaced"
                && row.after_hash.as_deref() == Some(current_hash.as_str())
        });
    let replaced_recoverable = replaced
        .as_ref()
        .is_some_and(|row| restore_source_audit(db, std::slice::from_ref(row)).0 == 1);
    checks.check(
        ID,
        bytes_match && row_is_correct && replaced_recoverable,
        format!(
            "target={} selectedVersion={}@v{} selectedHash={older_hash} \
             targetHashAfterRestore={after_hash} sizeBefore={current_size} sizeAfter={after_size} \
             bytesMatch={bytes_match} restoredRow={{id:{},kind:{},from:{:?},hash:{:?},before:{:?}}} \
             replacedContentRecoverable={replaced_recoverable} \
             [layer: real OfficeCLI content + production restore function; not the UI command]",
            target.display(),
            older.id,
            older.version_no,
            restored.id,
            restored.change_kind,
            restored.restored_from_id,
            restored.after_hash,
            restored.before_hash,
        ),
    );
    // Leave the delivered document holding the content the Run finished with, so
    // this check cannot be what made the later artifact hashes look right.
    match restore_version(db, &backups_dir, &binding.conversation_id, &latest.id, false) {
        Ok(_) => {
            let (hash, _) = crate::runtime_host::managed_files::hash_file(&target)
                .unwrap_or_else(|| ("<unreadable>".to_owned(), 0));
            checks.check(
                "managed:latest-version-restored-back-after-audit",
                Some(hash.as_str()) == latest.after_hash.as_deref(),
                format!(
                    "target={} expected={:?} actual={hash}",
                    target.display(),
                    latest.after_hash,
                ),
            );
        }
        Err(error) => checks.check(
            "managed:latest-version-restored-back-after-audit",
            false,
            format!("restoring the latest version back failed: {error}"),
        ),
    }
}

fn run_agv_checks(
    mode: EvalMode,
    identity: &ModelIdentity,
    db: &Database,
    root: &std::path::Path,
    run_id: &str,
    expected: &Expectations,
    observations: RunObservations,
    eval_state: Arc<Mutex<EvalState>>,
    planned_rounds: Option<usize>,
    started: Instant,
) -> ScenarioOutcome {
    let (id, title) = match mode {
        EvalMode::SyntheticProvider => (
            "agv-deliverables",
            "AGV：真实链路生成两份 Excel 与一份 Word",
        ),
        EvalMode::RealCloudModel => (
            "agv-deliverables-cloud",
            "AGV（真实云模型）：真实模型驱动同一链路生成两份 Excel 与一份 Word",
        ),
    };
    let mut checks = CheckResults::default();
    checks.check(
        "run:completed",
        observations.state == "completed",
        format!(
            "state={} error={:?}",
            observations.state, observations.dispatch_error
        ),
    );
    match planned_rounds {
        // The synthetic tier authored the plan, so its round count is a real
        // assertion. The real tier must never assert one.
        Some(planned_rounds) => checks.check(
            "run:exact-model-rounds",
            observations.model_requests == planned_rounds,
            format!(
                "planned={planned_rounds} observed={}",
                observations.model_requests
            ),
        ),
        None => {}
    }

    // Independent verification: reopen every deliverable with non-OfficeCLI parsers.
    let mut metrics_per_doc: Vec<(String, MetricMap)> = Vec::new();
    for (file, kind) in [
        (AGV_DIST_NAME, "distribution"),
        (AGV_SUMMARY_NAME, "summary"),
        (AGV_REPORT_NAME, "report"),
    ] {
        let path = root.join(file);
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) => {
                checks.check(
                    &format!("{kind}:exists"),
                    false,
                    format!("{}: {error}", path.display()),
                );
                continue;
            }
        };
        checks.check(
            &format!("{kind}:exists"),
            bytes.len() > 100,
            format!("{} bytes", bytes.len()),
        );
        let metrics = match kind {
            "distribution" => distribution_workbook_checks(&bytes, expected, &mut checks),
            "summary" => summary_workbook_checks(&bytes, expected, &mut checks),
            _ => report_document_checks(&bytes, expected, &mut checks),
        };
        if !metrics.is_empty() {
            metrics_per_doc.push((kind.into(), metrics));
        }
    }
    // Cross-document count/ratio consistency.
    let borrowed: Vec<(String, &MetricMap)> = metrics_per_doc
        .iter()
        .map(|(name, metrics)| (name.clone(), metrics))
        .collect();
    for failure in inconsistent_metrics(&borrowed, 0.002) {
        checks.check(&format!("cross:{failure}"), false, failure.clone());
    }
    checks.check(
        "cross:all-three-documents",
        borrowed.len() == 3,
        format!("metrics compared across {} of 3 documents", borrowed.len()),
    );

    let (tool_executions, records) = tool_execution_report(db, run_id);
    match mode {
        EvalMode::SyntheticProvider => {
            let office_calls = records
                .iter()
                .filter(|record| record.tool_name == "call_mcp_tool")
                .count();
            let failed_calls = records
                .iter()
                .filter(|record| record.status != "completed")
                .count();
            checks.check(
                "tools:office-rounds",
                office_calls >= 9,
                format!("call_mcp_tool completed rounds={office_calls}"),
            );
            checks.check(
                "tools:no-failed",
                failed_calls == 0,
                format!("non-completed calls={failed_calls}"),
            );
            // create + edit for each of the three documents = 6 per-request
            // approvals; validate and reads never wait for approval.
            checks.check(
                "approvals:six-per-request-mutations",
                observations.approvals == 6,
                format!("allow_once decisions={}", observations.approvals),
            );
        }
        EvalMode::RealCloudModel => {
            let mutations = office_mutations(&records);
            match db.kernel_model_config(run_id) {
                Ok(frozen) => real_tier_structural_checks(
                    &observations,
                    &mutations,
                    identity,
                    &frozen.model_service,
                    &mut checks,
                ),
                Err(error) => checks.check(
                    "model:identity-is-environment-configured",
                    false,
                    format!("the Run's frozen model config is unreadable: {error}"),
                ),
            }
        }
    }

    let delivery = delivery_report(db, run_id);
    let passed = delivery.as_array().is_some_and(|items| {
        items.len() == 3 && items.iter().all(|item| item["status"] == "passed")
    });
    checks.check(
        "delivery:three-artifacts-passed",
        passed,
        delivery.to_string(),
    );

    // N1: every document this Run really saved through the frozen built-in
    // Office connector must be registered as a user-restorable content version.
    // The dispatch is the MCP wrapper (`call_mcp_tool` → `fox-office`), so the
    // check also proves the wrapper is unwrapped to the real operation name.
    let registered = managed_office_versions(
        db,
        run_id,
        &[AGV_DIST_NAME, AGV_SUMMARY_NAME, AGV_REPORT_NAME],
    );
    checks.check(
        "managed:office-writes-registered",
        registered.len() >= 6,
        format!(
            "registered office versions={}, artifacts={}",
            registered.len(),
            registered
                .iter()
                .map(|row| format!("{}:{}", row.tool, row.display_name))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    );
    checks.check(
        "managed:every-artifact-has-a-version",
        [AGV_DIST_NAME, AGV_SUMMARY_NAME, AGV_REPORT_NAME]
            .iter()
            .all(|name| registered.iter().any(|row| row.display_name.contains(name))),
        "each delivered document appears in the managed-file registry",
    );
    // Every registered version's own content snapshot must resolve *and* hash to
    // what that row claims. A mismatch fails the check; it is not a line in a
    // log next to a pass.
    let (restorable, restore_failures) = restore_source_audit(db, &registered);
    checks.check(
        "managed:every-version-restorable",
        restorable == registered.len() && restore_failures.is_empty(),
        format!(
            "restorable={restorable}/{} failures={restore_failures:?}",
            registered.len()
        ),
    );
    // A registry nobody can restore from is a ledger, not a feature. Restore one
    // real version through the production entry and read the file back.
    restored_version_evidence(db, root, run_id, &registered, &mut checks);
    // The chain above proves what *does* register; these prove what must not.
    managed_office_gate_checks(
        mode,
        db,
        root,
        run_id,
        &records,
        registered.len(),
        &mut checks,
    );

    let status = if checks.ok() && observations.state == "completed" {
        "passed"
    } else {
        "failed"
    };
    let guard = eval_state.lock().unwrap();
    ScenarioOutcome {
        id,
        title,
        mode,
        status,
        run_id: Some(run_id.to_owned()),
        duration_ms: started.elapsed().as_millis(),
        model_requests: Some(observations.model_requests),
        model_requests_source: observations.model_requests_source,
        failure_stage: (status == "failed")
            .then(|| {
                guard
                    .last_tool
                    .clone()
                    .unwrap_or_else(|| "post-run checks".into())
            }),
        failure: guard
            .last_error
            .clone()
            .map(|error| json!({"lastError":error})),
        checks: checks.rows,
        delivery,
        usage: usage_report_for(root, run_id),
        tool_executions,
        artifacts: list_artifacts(root),
        artifact_hashes: artifact_hashes(root),
        source_sha256: Some(expected.source_sha256.clone()),
    }
}

// ---------------------------------------------------------------------------
// Scenario B: short document create + modify.
// ---------------------------------------------------------------------------

fn short_doc_script() -> VecDeque<Reply> {
    let paragraph = |text: &str| {
        json!({"command":"add","path":"/body","type":"paragraph","props":{"text":text}})
    };
    let first = "会议时间：2026年9月13日；议题：AGV 等待时长复盘。";
    let second = "【修订】补充结论：优先处理高频超时车道。";
    VecDeque::from([
        Reply::Tool {
            id: "short-create".into(),
            name: "call_mcp_tool".into(),
            arguments: office_call("office_create", json!({"output": SHORT_DOC_NAME})),
        },
        Reply::Tool {
            id: "short-edit-1".into(),
            name: "call_mcp_tool".into(),
            arguments: office_call(
                "office_edit",
                json!({"file":SHORT_DOC_NAME,"output":SHORT_DOC_NAME,"overwrite":true,
                    "operations":[paragraph(first)]}),
            ),
        },
        Reply::Tool {
            id: "short-edit-2".into(),
            name: "call_mcp_tool".into(),
            arguments: office_call(
                "office_edit",
                json!({"file":SHORT_DOC_NAME,"output":SHORT_DOC_NAME,"overwrite":true,
                    "operations":[
                        {"command":"add","path":"/body","type":"paragraph",
                         "props":{"text":"修订记录","style":"Heading1"}},
                        paragraph(second)]}),
            ),
        },
        Reply::Tool {
            id: "short-read".into(),
            name: "call_mcp_tool".into(),
            arguments: office_call("office_read", json!({"file": SHORT_DOC_NAME})),
        },
        Reply::Stop,
    ])
}

fn run_short_document_scenario() -> ScenarioOutcome {
    let mode = EvalMode::SyntheticProvider;
    let id = "short-document-edit";
    let title = "短文档修改：创建、原地修改与备份";
    let started = Instant::now();
    let root = match scenario_root(mode, "short-document") {
        Ok(root) => root,
        Err(error) => {
            return fail_outcome(
                mode,
                id,
                title,
                "environment:output-dir".into(),
                error,
                None,
                None,
            )
        }
    };
    let script = short_doc_script();
    let planned_rounds = script.len();
    let handle = scripted_provider(script);
    let config = eval_model_config(handle.address);
    let (db, run_id) =
        match prepare_eval_run(
            &root,
            SHORT_DOC_TASK_PROMPT,
            &config,
            "real-task-eval",
            PermissionMode::Allow,
            // Scripted-provider regression: its calibrated tool surface is the
            // point of the scenario, so it keeps the legacy description.
            false,
        ) {
            Ok(prepared) => prepared,
            Err((stage, error)) => {
                return fail_outcome(mode, id, title, stage, error, None, Some(&root));
            }
        };
    let eval_state = Arc::new(Mutex::new(EvalState::default()));
    let observations = drive_real_run(
        &db,
        &root,
        &run_id,
        "real-eval-short",
        ProviderSession::from_handle(handle),
        EVAL_API_KEY,
        eval_state.clone(),
    );

    let mut checks = CheckResults::default();
    checks.check(
        "run:completed",
        observations.state == "completed",
        format!(
            "state={} error={:?}",
            observations.state, observations.dispatch_error
        ),
    );
    checks.check(
        "run:exact-model-rounds",
        observations.model_requests == planned_rounds,
        format!("planned={planned_rounds} observed={}", observations.model_requests),
    );
    let doc_path = root.join(SHORT_DOC_NAME);
    let bytes = match std::fs::read(&doc_path) {
        Ok(bytes) => bytes,
        Err(error) => {
            return fail_outcome(
                mode,
                id,
                title,
                "check:document-missing".into(),
                error.to_string(),
                Some(run_id),
                Some(&root),
            )
            .with_duration(started.elapsed().as_millis());
        }
    };
    let text = crate::local_knowledge_import::extract_office_text(&bytes, SHORT_DOC_NAME)
        .ok()
        .flatten()
        .unwrap_or_default();
    let first = "会议时间：2026年9月13日";
    let second = "优先处理高频超时车道";
    checks.check("doc:contains-original", text.contains(first), first);
    checks.check("doc:contains-revision", text.contains(second), second);
    if let (Some(first_at), Some(second_at)) = (text.find(first), text.find(second)) {
        checks.check(
            "doc:revision-order",
            first_at < second_at,
            "original paragraph precedes the revision",
        );
    } else {
        checks.check("doc:revision-order", false, "markers missing");
    }

    // OfficeCLI keeps pre-overwrite versions; the pre-revision backup must not
    // contain the revision yet.
    // Every pre-write backup the Host took for this document. Backups are part
    // of the managed version store, **not** files next to the document: the
    // project folder must never accumulate `*.fox-backup-*` (that was the
    // source of the watcher EBUSY this change removed). Read them from the
    // registry instead of by filename pattern. Rows are selected by the Run's
    // own conversation and the document name, exactly like the other managed
    // checks, so a path-form difference (canonicalized vs. composed) cannot make
    // the lookup silently miss.
    let conversation = db
        .run_control_binding(&run_id)
        .ok()
        .flatten()
        .map(|binding| binding.conversation_id)
        .unwrap_or_default();
    let registered: Vec<crate::database::ManagedFileVersion> = db
        .managed_file_versions(&conversation, None)
        .unwrap_or_default()
        .into_iter()
        .filter(|row| row.display_name.contains(SHORT_DOC_NAME))
        .collect();
    let backups: Vec<PathBuf> = registered
        .iter()
        .filter_map(|row| row.backup_path.clone())
        .map(PathBuf::from)
        .filter(|path| path.is_file())
        .collect();
    // A backup living in the project folder would be the regression this check
    // exists to catch, so assert the placement directly.
    let stray_backups = std::fs::read_dir(&root)
        .unwrap()
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().contains(".fox-backup-"))
        .count();
    checks.check(
        "backup:at-least-two",
        backups.len() >= 2,
        format!("backups={}", backups.len()),
    );
    checks.check(
        "backup:not-in-the-project-folder",
        stray_backups == 0,
        format!("stray .fox-backup-* files in the project root={stray_backups}"),
    );
    let pre_revision_backup = backups.iter().any(|path| {
        std::fs::read(path)
            .ok()
            .and_then(|bytes| {
                crate::local_knowledge_import::extract_office_text(&bytes, SHORT_DOC_NAME)
                    .ok()
                    .flatten()
            })
            .is_some_and(|text| text.contains(first) && !text.contains(second))
    });
    checks.check(
        "backup:pre-revision-content",
        pre_revision_backup,
        "one backup carries the original paragraph but not the revision",
    );

    let delivery = delivery_report(&db, &run_id);
    let delivery_passed = delivery
        .as_array()
        .is_some_and(|items| items.len() == 1 && items[0]["status"] == "passed");
    checks.check(
        "delivery:one-passed",
        delivery_passed,
        delivery.to_string(),
    );

    let (tool_executions, records) = tool_execution_report(&db, &run_id);
    let failed_calls = records
        .iter()
        .filter(|record| record.status != "completed")
        .count();
    checks.check(
        "tools:no-failed",
        failed_calls == 0,
        format!("non-completed calls={failed_calls}"),
    );
    // create + two edits wait for a per-request decision; office_read does not.
    checks.check(
        "approvals:three-per-request-mutations",
        observations.approvals == 3,
        format!("allow_once decisions={}", observations.approvals),
    );

    let status = if checks.ok() && observations.state == "completed" {
        "passed"
    } else {
        "failed"
    };
    let usage = usage_report_for(&root, &run_id);
    let guard = eval_state.lock().unwrap();
    ScenarioOutcome {
        id,
        title,
        mode,
        status,
        run_id: Some(run_id),
        duration_ms: started.elapsed().as_millis(),
        model_requests: Some(observations.model_requests),
        model_requests_source: observations.model_requests_source,
        failure_stage: (status == "failed")
            .then(|| {
                guard
                    .last_tool
                    .clone()
                    .unwrap_or_else(|| "post-run checks".into())
            }),
        failure: guard
            .last_error
            .clone()
            .map(|error| json!({"lastError":error})),
        checks: checks.rows,
        delivery,
        usage,
        tool_executions,
        artifacts: list_artifacts(&root),
        artifact_hashes: artifact_hashes(&root),
        source_sha256: None,
    }
}

// ---------------------------------------------------------------------------
// Scenario C: large-result continuation read.
// ---------------------------------------------------------------------------

/// ~49 KiB JSON: over the 9 000-byte model-view bound (so the Host projects it
/// with a retrievable `fox-result://` reference) but under the 128 KiB stored
/// result cap, so every original byte is persisted and the range walk must
/// rebuild the file byte-for-byte.
fn large_result_text() -> String {
    let records = (0..300)
        .map(|index| {
            json!({
                "id": index,
                "name": format!("row-{index:04}"),
                "note": "中".repeat(40),
            })
        })
        .collect::<Vec<_>>();
    serde_json::to_string(&json!({"records": records, "total": 300})).unwrap()
}

fn run_large_result_scenario() -> ScenarioOutcome {
    let mode = EvalMode::SyntheticProvider;
    let id = "large-result-continuation-read";
    let title = "大结果续读：完整存储与 read_tool_result 多页续读";
    let started = Instant::now();
    let root = match scenario_root(mode, "large-result") {
        Ok(root) => root,
        Err(error) => {
            return fail_outcome(
                mode,
                id,
                title,
                "environment:output-dir".into(),
                error,
                None,
                None,
            )
        }
    };
    let text = large_result_text();
    if let Err(error) = std::fs::write(root.join(LARGE_RESULT_NAME), &text) {
        return fail_outcome(
            mode,
            id,
            title,
            "environment:large-input".into(),
            error.to_string(),
            None,
            Some(&root),
        );
    }
    let wanted_sha = sha256_hex(text.as_bytes());
    let handle = spawn_provider(move |index, body| paging_decide(index, body));
    let config = eval_model_config(handle.address);
    let (db, run_id) =
        match prepare_eval_run(
            &root,
            LARGE_RESULT_TASK_PROMPT,
            &config,
            "real-task-eval",
            PermissionMode::Allow,
            // Scripted-provider regression: its calibrated tool surface is the
            // point of the scenario, so it keeps the legacy description.
            false,
        ) {
            Ok(prepared) => prepared,
            Err((stage, error)) => {
                return fail_outcome(mode, id, title, stage, error, None, Some(&root));
            }
        };
    let eval_state = Arc::new(Mutex::new(EvalState::default()));
    let observations = drive_real_run(
        &db,
        &root,
        &run_id,
        "real-eval-large",
        ProviderSession::from_handle(handle),
        EVAL_API_KEY,
        eval_state.clone(),
    );

    let mut checks = CheckResults::default();
    checks.check(
        "run:completed",
        observations.state == "completed",
        format!(
            "state={} error={:?}",
            observations.state, observations.dispatch_error
        ),
    );
    let records = db.list_runtime_tool_calls_for_run(&run_id).unwrap_or_default();
    let original_reads = records
        .iter()
        .filter(|record| record.tool_name == "read" && record.runtime_tool_call_id == "read-once")
        .count();
    checks.check(
        "read:executed-once",
        original_reads == 1,
        format!("read-once executions={original_reads}"),
    );
    let mut ranges: Vec<(usize, String)> = records
        .iter()
        .filter(|record| record.tool_name == "read_tool_result")
        .map(|record| {
            let page = record
                .runtime_tool_call_id
                .strip_prefix("range-read-")
                .and_then(|value| value.parse::<usize>().ok())
                .unwrap_or(0);
            let fragment = record
                .result
                .as_ref()
                .and_then(|result| result["content"][0]["text"].as_str())
                .unwrap_or_default()
                .to_owned();
            (page, fragment)
        })
        .collect();
    ranges.sort_by_key(|(page, _)| *page);
    checks.check(
        "ranges:multi-page",
        ranges.len() >= 5,
        format!("range pages={}", ranges.len()),
    );
    let rebuilt: String = ranges
        .iter()
        .map(|(_, fragment)| fragment.as_str())
        .collect();
    checks.check(
        "ranges:rebuild-exact-bytes",
        sha256_hex(rebuilt.as_bytes()) == wanted_sha,
        format!(
            "rebuilt={} bytes sha={} original={} bytes sha={}",
            rebuilt.len(),
            sha256_hex(rebuilt.as_bytes()),
            text.len(),
            wanted_sha
        ),
    );
    let all_completed = records
        .iter()
        .filter(|record| matches!(record.tool_name.as_str(), "read" | "read_tool_result"))
        .all(|record| record.status == "completed");
    checks.check("ranges:all-completed", all_completed, "tool call statuses");
    checks.check(
        "model:multi-round",
        observations.model_requests >= 6,
        format!("model requests={}", observations.model_requests),
    );
    let delivery = delivery_report(&db, &run_id);
    checks.check(
        "delivery:no-checklist-for-read-task",
        delivery.as_array().is_some_and(|items| items.is_empty()),
        delivery.to_string(),
    );
    // Pure read scenario: nothing mutates, so no human approval is ever asked.
    checks.check(
        "approvals:none-for-reads",
        observations.approvals == 0,
        format!("allow_once decisions={}", observations.approvals),
    );

    let (tool_executions, _records) = tool_execution_report(&db, &run_id);
    let status = if checks.ok() && observations.state == "completed" {
        "passed"
    } else {
        "failed"
    };
    let usage = usage_report_for(&root, &run_id);
    let guard = eval_state.lock().unwrap();
    ScenarioOutcome {
        id,
        title,
        mode,
        status,
        run_id: Some(run_id),
        duration_ms: started.elapsed().as_millis(),
        model_requests: Some(observations.model_requests),
        model_requests_source: observations.model_requests_source,
        failure_stage: (status == "failed")
            .then(|| {
                guard
                    .last_tool
                    .clone()
                    .unwrap_or_else(|| "post-run checks".into())
            }),
        failure: guard
            .last_error
            .clone()
            .map(|error| json!({"lastError":error})),
        checks: checks.rows,
        delivery,
        usage,
        tool_executions,
        artifacts: vec![],
        artifact_hashes: vec![(
            LARGE_RESULT_NAME.into(),
            wanted_sha.clone(),
            text.len() as u64,
        )],
        source_sha256: Some(wanted_sha),
    }
}

// ---------------------------------------------------------------------------
// Report file
// ---------------------------------------------------------------------------

/// Environment block of a not-run real-tier report: variable NAMES and defaults
/// only, never a value. Shared by the entry and its contract test so the
/// recorded artifact is exactly what the entry writes.
fn cloud_not_run_environment() -> Value {
    json!({
        "required": [CLOUD_ENV_BASE_URL, CLOUD_ENV_API_KEY, CLOUD_ENV_MODEL],
        "optional": [CLOUD_ENV_API_TYPE],
        "apiTypeDefault": CLOUD_DEFAULT_API_TYPE,
        "resolved": Value::Null,
        "apiKeyRecorded": false,
    })
}

/// One acceptance report. `executed: false` means the tier did not run at all:
/// then `not_run_reason` is present, the overall result is `not-run` (未执行) and
/// the report can never read as a pass.
struct ReportInput {
    mode: EvalMode,
    executed: bool,
    not_run_reason: Option<String>,
    /// Environment variable names an operator must fix before a retry.
    missing_environment: Vec<String>,
    identity: ModelIdentity,
    report_run_id: String,
    scenarios: Vec<ScenarioOutcome>,
    /// Environment/mode description (variable names and non-secret identity).
    environment: Value,
}

fn new_report_run_id(mode: EvalMode) -> String {
    format!("{}-{}", mode.tier(), crate::database::now_ms())
}

/// Honest status of every tier this report knows about. Tiers other than the
/// one that produced this report are explicitly `not-run-in-this-entry`.
fn tiers_block(mode: EvalMode, executed: bool, not_run_reason: Option<&str>, overall: &str) -> Value {
    let mut tiers = serde_json::Map::new();
    for candidate in EvalMode::ALL {
        let entry = if candidate != mode {
            json!({
                "status": "not-run-in-this-entry",
                "marker": NOT_RUN_MARKER,
                "entry": candidate.entry_command(),
                "reason": "select this tier explicitly by running its own entry",
            })
        } else if !executed {
            json!({
                "status": "not-run",
                "marker": NOT_RUN_MARKER,
                "entry": candidate.entry_command(),
                "reason": not_run_reason.unwrap_or("the acceptance did not execute"),
            })
        } else {
            json!({
                "status": if overall == "passed" { "verified-by-this-entry" } else { "failed-in-this-entry" },
                "entry": candidate.entry_command(),
                "chain": "model endpoint -> real pi-runtime worker -> KernelCoordinator live loop -> GatewayPolicy -> bundled OfficeCLI/readers",
            })
        };
        tiers.insert(candidate.tier().into(), entry);
    }
    tiers.insert(
        "gui".into(),
        json!({
            "status": "unverified",
            "marker": NOT_RUN_MARKER,
            "reason": "desktop UI presentation/interaction is not exercised by any cargo entry; GUI verification is separate and was NOT executed in this round.",
        }),
    );
    tiers.insert(
        "install".into(),
        json!({
            "status": "unverified",
            "marker": NOT_RUN_MARKER,
            "reason": "packaged installer/resource bundling is not exercised; the pinned resources/officecli binary is used in place. Install verification is separate and was NOT executed in this round.",
        }),
    );
    Value::Object(tiers)
}

fn build_report(input: ReportInput) -> Value {
    let ReportInput {
        mode,
        executed,
        not_run_reason,
        missing_environment,
        identity,
        report_run_id,
        scenarios,
        environment,
    } = input;
    let failed = scenarios
        .iter()
        .filter(|scenario| scenario.status != "passed")
        .count();
    // An "executed" report without a single scenario is not a pass either.
    let overall = if !executed {
        "not-run"
    } else if scenarios.is_empty() || failed > 0 {
        "failed"
    } else {
        "passed"
    };
    let duration_ms: u128 = scenarios.iter().map(|scenario| scenario.duration_ms).sum();
    let usage_rows: u64 = scenarios
        .iter()
        .map(|scenario| {
            scenario
                .usage
                .get("hostUsageRows")
                .and_then(Value::as_u64)
                .unwrap_or(0)
        })
        .sum();
    let tool_total: usize = scenarios
        .iter()
        .map(|scenario| {
            scenario
                .tool_executions
                .as_array()
                .map(|rows| rows.len())
                .unwrap_or(0)
        })
        .sum();
    let artifact_hashes: Vec<Value> = scenarios
        .iter()
        .flat_map(|scenario| {
            scenario.artifact_hashes.iter().map(move |(name, sha256, bytes)| {
                json!({"scenario":scenario.id,"name":name,"sha256":sha256,"bytes":bytes})
            })
        })
        .collect();
    let source_sha256 = scenarios
        .iter()
        .find_map(|scenario| scenario.source_sha256.clone());
    json!({
        "schemaVersion": 2,
        "generatedAtMs": crate::database::now_ms(),
        "runId": report_run_id,
        "entry": mode.entry_command(),
        "mode": mode.tier(),
        "tier": mode.tier(),
        // Execution is explicit: a tier that did not run says so and says why.
        "executed": executed,
        "notRunReason": not_run_reason,
        "missingEnvironment": missing_environment,
        "overall": overall,
        "durationMs": duration_ms,
        "modelIdentity": identity.to_json(),
        // The synthetic tier binds a loopback listener; the real tier never does.
        "syntheticProviderSpawned": mode == EvalMode::SyntheticProvider && executed,
        "environment": environment,
        "tiers": tiers_block(mode, executed, not_run_reason.as_deref(), overall),
        "fixedInputs": {
            "agvSourcePath": source_workbook_path().to_string_lossy(),
            "outputDir": mode.evaluation_dir().to_string_lossy(),
            "evaluationId": evaluation_id(),
            "prompts": {
                "agv": AGV_TASK_PROMPT,
                "shortDocument": SHORT_DOC_TASK_PROMPT,
                "largeResult": LARGE_RESULT_TASK_PROMPT,
            },
            // Credentials are never recorded; only presence and origin.
            "credential": {
                "apiKeyPresent": identity.api_key_present,
                "apiKeyRecorded": false,
                "apiKeySource": identity.api_key_source,
            },
        },
        "sourceWorkbookSha256": source_sha256,
        "artifactHashes": artifact_hashes,
        "usage": {
            "hostUsageRows": usage_rows,
            "scenarios": scenarios.iter().map(|scenario|
                json!({"id":scenario.id,"usage":scenario.usage})).collect::<Vec<_>>(),
        },
        "toolExecutions": {
            "total": tool_total,
            "scenarios": scenarios.iter().map(|scenario|
                json!({"id":scenario.id,"toolExecutions":scenario.tool_executions})).collect::<Vec<_>>(),
        },
        "scenarios": scenarios.iter().map(ScenarioOutcome::to_json).collect::<Vec<_>>(),
    })
}

/// Persist a report. Refuses to write a document that leaks the credential it
/// was given, and refuses the historical evidence directory outright.
fn write_report_file(
    dir: &std::path::Path,
    name: &str,
    report: &Value,
    secret: Option<&str>,
) -> std::path::PathBuf {
    match try_write_report(dir, name, report, secret) {
        Ok(path) => path,
        Err(error) => panic!("{error}"),
    }
}

/// Fallible form, so an unwritable output directory cannot hide the real
/// verdict (an entry still aborts with its own reason, e.g. 未执行).
fn try_write_report(
    dir: &std::path::Path,
    name: &str,
    report: &Value,
    secret: Option<&str>,
) -> Result<std::path::PathBuf, String> {
    let serialized = serde_json::to_string_pretty(report)
        .map_err(|error| format!("the report does not serialize: {error}"))?;
    if let Some(secret) = secret.filter(|secret| secret.trim().len() >= 4) {
        if serialized.contains(secret) {
            return Err(String::from("a report must never contain the API key"));
        }
    }
    if dir.starts_with(historical_eval_dir()) {
        return Err(format!(
            "the historical evidence directory is read-only for this round: {}",
            historical_eval_dir().display()
        ));
    }
    std::fs::create_dir_all(dir).map_err(|error| {
        format!(
            "the report directory {} could not be created: {error}",
            dir.display()
        )
    })?;
    let path = dir.join(name);
    std::fs::write(&path, serialized)
        .map_err(|error| format!("the report {} could not be written: {error}", path.display()))?;
    Ok(path)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Fast contract layer: expectations are derived, not hardcoded.
#[test]
fn real_eval_derives_current_expectations_without_historical_constants() {
    let bytes = std::fs::read(source_workbook_path()).unwrap();
    let expected = derive_expectations(&bytes).unwrap();
    assert_eq!(expected.dedup_sheet, "去重");
    assert_eq!(expected.exception_sheet.as_deref(), Some("悬臂"));
    assert!(expected.total > 0, "current source must have records");
    let task_sum: u64 = expected.task_counts.iter().map(|(_, count)| *count).sum();
    let status_sum: u64 = expected.status_counts.iter().map(|(_, count)| *count).sum();
    let bucket_sum: u64 = expected.buckets.iter().map(|(_, _, _, count)| *count).sum();
    assert_eq!(task_sum, expected.total, "task categories partition records");
    assert_eq!(status_sum, expected.total, "AGV statuses partition records");
    assert_eq!(bucket_sum, expected.total, "wait buckets partition records");
    assert!(expected.wait_min <= expected.wait_max);
    assert!(expected.wait_avg >= expected.wait_min as f64);
    assert!(expected.wait_avg <= expected.wait_max as f64);
    let ratio_sum: f64 = expected
        .task_counts
        .iter()
        .map(|(_, count)| expected.ratio_of(*count))
        .sum();
    assert!(nearly(ratio_sum, 1.0, 0.001), "ratio sum {ratio_sum}");
    assert_eq!(expected.source_sha256.len(), 64);
    println!(
        "real-eval current source: total={} tasks={:?} exceptions={} sha256={}",
        expected.total,
        expected.task_counts,
        expected.exception_total,
        expected.source_sha256
    );

    // The fixed prompts seed exactly the promised deliverables.
    let agv_seeds = crate::runtime_host::delivery::expectations_from_task(AGV_TASK_PROMPT);
    assert_eq!(agv_seeds.len(), 3, "{agv_seeds:?}");
    let targets: BTreeSet<_> = agv_seeds
        .iter()
        .filter_map(|seed| seed.target_path.clone())
        .collect();
    assert!(targets.contains(AGV_DIST_NAME));
    assert!(targets.contains(AGV_SUMMARY_NAME));
    assert!(targets.contains(AGV_REPORT_NAME));
    let short_seeds =
        crate::runtime_host::delivery::expectations_from_task(SHORT_DOC_TASK_PROMPT);
    assert_eq!(short_seeds.len(), 1, "{short_seeds:?}");
    let large_seeds =
        crate::runtime_host::delivery::expectations_from_task(LARGE_RESULT_TASK_PROMPT);
    assert!(large_seeds.is_empty(), "{large_seeds:?}");
}

#[test]
fn real_eval_plan_is_deterministic_and_carries_charts_sections_and_derived_values() {
    let bytes = std::fs::read(source_workbook_path()).unwrap();
    let expected = derive_expectations(&bytes).unwrap();
    let first = serde_json::to_value(agv_script(&expected).iter().collect::<Vec<_>>()).unwrap();
    let second = serde_json::to_value(agv_script(&expected).iter().collect::<Vec<_>>()).unwrap();
    assert_eq!(first, second, "the synthetic plan must be reproducible");
    let rendered = first.to_string();
    assert!(rendered.contains("office_create"));
    assert!(rendered.contains("office_validate"));
    assert!(rendered.contains("\"bar\""));
    assert!(rendered.contains("任务类型分布"));
    assert!(rendered.contains("等待时长分布"));
    for heading in ["数据概览", "任务类型构成", "等待时长分布", "异常与超时", "分析结论"] {
        assert!(rendered.contains(heading), "missing report section {heading}");
    }
    // The derived total really drives the script, never a literal constant.
    assert!(rendered.contains(&expected.total.to_string()));
}

#[test]
fn real_eval_checker_flags_fabricated_cross_document_inconsistency() {
    let mut good = MetricMap::new();
    good.insert("task:卸船:count".into(), 548.0);
    good.insert("task:卸船:ratio".into(), 0.5912);
    good.insert("total".into(), 927.0);
    let mut fabricated_count = good.clone();
    fabricated_count.insert("task:卸船:count".into(), 600.0);
    let mut fabricated_ratio = good.clone();
    fabricated_ratio.insert("task:卸船:ratio".into(), 0.71);
    let failures = inconsistent_metrics(
        &[
            ("dist".to_owned(), &good),
            ("summary".to_owned(), &fabricated_count),
            ("report".to_owned(), &good),
        ],
        0.002,
    );
    assert!(failures
        .iter()
        .any(|failure| failure.contains("task:卸船:count")));
    let failures = inconsistent_metrics(
        &[
            ("dist".to_owned(), &good),
            ("summary".to_owned(), &good),
            ("report".to_owned(), &fabricated_ratio),
        ],
        0.002,
    );
    assert!(failures
        .iter()
        .any(|failure| failure.contains("task:卸船:ratio")));
    assert!(
        inconsistent_metrics(&[("a".to_owned(), &good), ("b".to_owned(), &good)], 0.002).is_empty()
    );
}

#[test]
fn real_eval_independent_zip_lister_opens_ooxml_package() {
    let bytes = std::fs::read(manifest_dir().join("tests/fixtures/office-reading.xlsx")).unwrap();
    let names = crate::local_knowledge_import::zip_entry_names(&bytes).unwrap();
    assert!(names.iter().any(|name| name.starts_with("xl/worksheets/")));
    assert!(
        !has_chart_part(&bytes).unwrap(),
        "the plain reading fixture has no chart"
    );
}

#[test]
fn real_eval_cursor_parsing_reads_escaped_model_view_blocks() {
    // The tool result is embedded as an escaped JSON string inside the
    // request body the provider receives.
    let body = r#"{"messages":[{"role":"tool","content":"prefix {\"reference\":\"fox-result://run/read-once\",\"offset\":7000,\"returnedBytes\":7000,\"nextOffset\":14000,\"complete\":false} suffix"}]}"#;
    assert_eq!(
        extract_result_reference(body).as_deref(),
        Some("fox-result://run/read-once")
    );
    assert_eq!(last_json_number_after(body, "nextOffset"), Some(14000));
    assert_eq!(last_json_bool_after(body, "complete"), Some(false));
    let done = r#"...nextOffset\":21000,\"complete\":true}"#;
    assert_eq!(last_json_number_after(done, "nextOffset"), Some(21000));
    assert_eq!(last_json_bool_after(done, "complete"), Some(true));
}

// ---------------------------------------------------------------------------
// Contract layer for the two tiers. None of these tests needs Node, OfficeCLI
// or a credential: they exercise the mode/identity/report contract directly,
// including the empty-environment path the real entry hits.
// ---------------------------------------------------------------------------

fn contract_temp_dir(label: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("fox-r8-eval-{}-{label}", std::process::id()))
}

fn mutation_fixture(tool: &str, status: &str) -> crate::database::ToolCallRecord {
    crate::database::ToolCallRecord {
        id: format!("record-{tool}-{status}"),
        runtime_tool_call_id: format!("call-{tool}"),
        run_id: "run-fixture".into(),
        conversation_id: "conversation-fixture".into(),
        tool_name: "call_mcp_tool".into(),
        input: json!({"serverId": crate::office::SERVER_ID, "tool": tool, "arguments": {}}),
        status: status.into(),
        result: None,
        error_message: None,
        execution_location: "host".into(),
        requires_approval: true,
        started_at: 0,
        completed_at: None,
        updated_at: 0,
        trace_id: None,
        span_id: None,
    }
}

fn cloud_observations(rounds: usize) -> RunObservations {
    RunObservations {
        state: "completed".into(),
        model_requests: rounds,
        model_requests_source: DURABLE_ROUNDS_SOURCE,
        dispatch_error: None,
        approvals: 2,
    }
}

fn scenario_fixture(mode: EvalMode, id: &'static str, status: &'static str) -> ScenarioOutcome {
    ScenarioOutcome {
        id,
        title: "fixture",
        mode,
        status,
        run_id: Some("run-fixture".into()),
        duration_ms: 12,
        model_requests: Some(3),
        model_requests_source: DURABLE_ROUNDS_SOURCE,
        failure_stage: None,
        failure: None,
        checks: vec![(
            "run:completed".into(),
            status == "passed",
            "fixture".into(),
        )],
        delivery: json!([]),
        usage: json!({"hostUsageRows":2,"events":[]}),
        tool_executions: json!([{"toolCallId":"call-fixture","tool":"call_mcp_tool","status":"completed"}]),
        artifacts: vec![],
        artifact_hashes: vec![],
        source_sha256: Some("a".repeat(64)),
    }
}

fn cloud_identity_fixture() -> ModelIdentity {
    ModelIdentity {
        api_type: "openai-completions".into(),
        model_id: "cloud-acceptance-model".into(),
        base_url: "https://cloud.example.com/v1".into(),
        base_url_host: "cloud.example.com".into(),
        api_key_present: true,
        api_key_source: CLOUD_KEY_SOURCE,
        note: "fixture",
    }
}

fn cloud_frozen_service_fixture() -> Value {
    json!({
        "apiType":"openai-completions",
        "modelId":"cloud-acceptance-model",
        "baseUrl":"https://cloud.example.com/v1",
        "contextWindow":256_000,
        "maxOutputTokens":8_192,
    })
}

/// (a) With no credentials the real tier records 未执行 with the missing variable
/// names and can never be mistaken for a pass or for the synthetic provider.
#[test]
fn real_eval_cloud_mode_reports_not_run_and_names_missing_environment() {
    let error = CloudEnvironment::resolve(|_| None)
        .expect_err("an absent environment must not resolve into a model endpoint");
    assert_eq!(
        error.variables,
        vec![CLOUD_ENV_BASE_URL, CLOUD_ENV_API_KEY, CLOUD_ENV_MODEL]
    );
    let summary = error.summary();
    assert!(summary.contains(NOT_RUN_MARKER), "{summary}");
    for name in &error.variables {
        assert!(summary.contains(name), "{summary} must name {name}");
    }

    let mode = EvalMode::RealCloudModel;
    let report = build_report(ReportInput {
        mode,
        executed: false,
        not_run_reason: Some(summary.clone()),
        missing_environment: error.variables_json(),
        identity: ModelIdentity::not_configured("the acceptance did not run"),
        report_run_id: new_report_run_id(mode),
        scenarios: vec![],
        environment: cloud_not_run_environment(),
    });
    assert_eq!(report["tier"], "real-cloud-model");
    assert_eq!(report["mode"], "real-cloud-model");
    assert_eq!(report["executed"], false);
    assert_eq!(report["overall"], "not-run");
    assert_eq!(report["notRunReason"].as_str(), Some(summary.as_str()));
    assert_eq!(
        report["missingEnvironment"],
        json!([CLOUD_ENV_BASE_URL, CLOUD_ENV_API_KEY, CLOUD_ENV_MODEL])
    );
    assert_eq!(report["syntheticProviderSpawned"], false);
    assert_eq!(report["modelIdentity"]["configured"], false);
    assert_eq!(report["modelIdentity"]["modelId"], "");
    assert_eq!(report["modelIdentity"]["apiKeyPresent"], false);
    assert_eq!(report["tiers"]["real-cloud-model"]["status"], "not-run");
    assert_eq!(report["tiers"]["real-cloud-model"]["marker"], NOT_RUN_MARKER);
    assert_eq!(
        report["tiers"]["synthetic-provider"]["status"],
        "not-run-in-this-entry"
    );
    assert_eq!(report["tiers"]["gui"]["status"], "unverified");
    assert_eq!(report["tiers"]["install"]["status"], "unverified");
    let rendered = serde_json::to_string(&report).unwrap();
    assert!(
        !rendered.contains(SYNTHETIC_MODEL_ID),
        "a not-run cloud report must not present the synthetic provider as its model"
    );
    assert!(
        !rendered.contains("127.0.0.1"),
        "a not-run cloud report must not invent a local endpoint"
    );

    // The report really lands on disk with the same content. The file is left
    // in place (the directory is named after this process) so the exact bytes
    // this contract produced can be inspected or copied as evidence.
    let dir = contract_temp_dir("not-run");
    let _ = std::fs::remove_dir_all(&dir);
    let path = write_report_file(&dir, mode.report_name(), &report, None);
    let written: Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(written["executed"], false);
    assert_eq!(written["overall"], "not-run");
    assert_eq!(written["missingEnvironment"].as_array().unwrap().len(), 3);
    println!(
        "EVAL-CLOUD not-run report written to {} (executed=false, overall=not-run)",
        path.display()
    );
}

/// A partial environment names exactly what is missing, blank values count as
/// missing, the scripted adapter is refused, and the key never reaches a config
/// or a report.
#[test]
fn real_eval_cloud_environment_resolution_is_explicit_and_never_records_the_key() {
    const SECRET: &str = "sk-contract-test-should-never-appear";
    let full = |name: &str| -> Option<String> {
        match name {
            CLOUD_ENV_BASE_URL => Some("https://models.example.com/v1".into()),
            CLOUD_ENV_API_KEY => Some(SECRET.into()),
            CLOUD_ENV_MODEL => Some("gpt-contract-test".into()),
            _ => None,
        }
    };
    let env = CloudEnvironment::resolve(full).expect("a complete environment resolves");
    assert_eq!(env.api_type, CLOUD_DEFAULT_API_TYPE, "FOX_EVAL_API_TYPE is optional");
    let config = env.model_config();
    assert_eq!(config.model_service["modelId"], "gpt-contract-test");
    assert_eq!(config.model_service["baseUrl"], "https://models.example.com/v1");
    assert!(
        config.model_service.get("apiKey").is_none(),
        "credentials stay out of the frozen model config"
    );
    assert!(
        config.hash().is_ok(),
        "the resolved cloud config must satisfy the production config contract"
    );
    assert_eq!(env.identity().base_url_host, "models.example.com");

    // Partial: only the model id is set.
    let error = CloudEnvironment::resolve(|name| {
        (name == CLOUD_ENV_MODEL).then(|| "gpt-contract-test".to_owned())
    })
    .expect_err("a partial environment must not resolve");
    assert_eq!(error.variables, vec![CLOUD_ENV_BASE_URL, CLOUD_ENV_API_KEY]);

    // Blank is absent, not configured.
    let error = CloudEnvironment::resolve(|name| match name {
        CLOUD_ENV_API_KEY => Some("   ".into()),
        CLOUD_ENV_BASE_URL => Some("https://models.example.com/v1".into()),
        CLOUD_ENV_MODEL => Some("gpt-contract-test".into()),
        _ => None,
    })
    .expect_err("a blank API key must not count as a credential");
    assert_eq!(error.variables, vec![CLOUD_ENV_API_KEY]);

    // The scripted adapter is refused in the real tier.
    let error = CloudEnvironment::resolve(|name| match name {
        CLOUD_ENV_BASE_URL => Some("https://models.example.com/v1".into()),
        CLOUD_ENV_API_KEY => Some(SECRET.into()),
        CLOUD_ENV_MODEL => Some("gpt-contract-test".into()),
        CLOUD_ENV_API_TYPE => Some("faux".into()),
        _ => None,
    })
    .expect_err("`faux` must never be accepted as a cloud API type");
    assert_eq!(error.variables, vec![CLOUD_ENV_API_TYPE]);

    // The synthetic provider id is refused as a "cloud" model id.
    let error = CloudEnvironment::resolve(|name| match name {
        CLOUD_ENV_BASE_URL => Some("https://models.example.com/v1".into()),
        CLOUD_ENV_API_KEY => Some(SECRET.into()),
        CLOUD_ENV_MODEL => Some(SYNTHETIC_MODEL_ID.into()),
        _ => None,
    })
    .expect_err("the synthetic model id must never be accepted in the real tier");
    assert_eq!(error.variables, vec![CLOUD_ENV_MODEL]);

    // A URL carrying credentials is refused too.
    let error = CloudEnvironment::resolve(|name| match name {
        CLOUD_ENV_BASE_URL => Some("https://user:pass@models.example.com/v1".into()),
        CLOUD_ENV_API_KEY => Some(SECRET.into()),
        CLOUD_ENV_MODEL => Some("gpt-contract-test".into()),
        _ => None,
    })
    .expect_err("a credential-bearing base URL must be refused");
    assert_eq!(error.variables, vec![CLOUD_ENV_BASE_URL]);

    // An executed report records identity (id + host) but never the key.
    let report = build_report(ReportInput {
        mode: EvalMode::RealCloudModel,
        executed: true,
        not_run_reason: None,
        missing_environment: vec![],
        identity: env.identity(),
        report_run_id: new_report_run_id(EvalMode::RealCloudModel),
        scenarios: vec![scenario_fixture(
            EvalMode::RealCloudModel,
            "agv-deliverables-cloud",
            "passed",
        )],
        environment: env.report_json(),
    });
    assert_eq!(report["executed"], true);
    assert_eq!(report["tier"], "real-cloud-model");
    assert_eq!(report["modelIdentity"]["modelId"], "gpt-contract-test");
    assert_eq!(report["modelIdentity"]["baseUrlHost"], "models.example.com");
    assert_eq!(report["modelIdentity"]["apiKeyRecorded"], false);
    assert_eq!(report["fixedInputs"]["credential"]["apiKeyRecorded"], false);
    assert_eq!(report["syntheticProviderSpawned"], false);
    let rendered = serde_json::to_string(&report).unwrap();
    assert!(
        !rendered.contains(SECRET),
        "the API key must never reach a report"
    );

    // The writer itself refuses to persist a leaking document.
    let dir = contract_temp_dir("redaction");
    let _ = std::fs::remove_dir_all(&dir);
    let leaky = json!({"fixedInputs": {"apiKey": SECRET}});
    println!("EXPECTED PANIC (asserted by this test): writing a credential-bearing report");
    let refused = std::panic::catch_unwind(|| {
        write_report_file(&dir, "leak.json", &leaky, Some(SECRET))
    })
    .is_err();
    assert!(refused, "write_report_file must refuse to persist a credential");
    assert!(
        !dir.join("leak.json").exists(),
        "the refused report must not be written"
    );
    let _ = std::fs::remove_dir_all(contract_temp_dir("redaction"));
}

/// The real tier asserts artifacts and structural invariants only: it holds for
/// any number of model rounds a real model chooses, and every check it makes is
/// non-vacuous.
#[test]
fn real_eval_cloud_checks_are_plan_and_round_count_independent() {
    let identity = cloud_identity_fixture();
    let frozen = cloud_frozen_service_fixture();
    let records = vec![
        mutation_fixture("office_create", "completed"),
        mutation_fixture("office_edit", "completed"),
    ];
    let borrowed: Vec<&crate::database::ToolCallRecord> = records.iter().collect();
    for rounds in [1usize, 2, 5, 23] {
        let mut results = CheckResults::default();
        real_tier_structural_checks(
            &cloud_observations(rounds),
            &borrowed,
            &identity,
            &frozen,
            &mut results,
        );
        assert!(results.ok(), "rounds={rounds}: {:?}", results.rows);
        assert!(results
            .rows
            .iter()
            .any(|(id, _, _)| id == "model:at-least-one-round"));
        for (id, _, _) in &results.rows {
            assert!(
                !id.contains("exact-model-rounds") && !id.contains("planned"),
                "{id} must not pin a model-authored plan"
            );
        }
    }

    // The layer is not vacuous: each invariant really fails on a violation.
    let mut zero_rounds = CheckResults::default();
    real_tier_structural_checks(&cloud_observations(0), &borrowed, &identity, &frozen, &mut zero_rounds);
    assert!(!zero_rounds.ok(), "zero model rounds must fail");

    let mut no_mutations = CheckResults::default();
    real_tier_structural_checks(
        &cloud_observations(4),
        &[],
        &identity,
        &frozen,
        &mut no_mutations,
    );
    assert!(!no_mutations.ok(), "no Office mutation must fail");

    let unfinished = vec![mutation_fixture("office_create", "failed")];
    let borrowed_unfinished: Vec<&crate::database::ToolCallRecord> = unfinished.iter().collect();
    let mut failed_mutation = CheckResults::default();
    real_tier_structural_checks(
        &cloud_observations(4),
        &borrowed_unfinished,
        &identity,
        &frozen,
        &mut failed_mutation,
    );
    assert!(!failed_mutation.ok(), "an unfinished mutation must fail");

    let mut wrong_approvals = cloud_observations(4);
    wrong_approvals.approvals = 7;
    let mut approvals = CheckResults::default();
    real_tier_structural_checks(&wrong_approvals, &borrowed, &identity, &frozen, &mut approvals);
    assert!(
        !approvals.ok(),
        "an ungated mutation must fail the approval invariant"
    );

    let stale = json!({
        "apiType":"openai-completions",
        "modelId":SYNTHETIC_MODEL_ID,
        "baseUrl":"http://127.0.0.1:9/v1",
        "contextWindow":256_000,
        "maxOutputTokens":2_048,
    });
    let mut identity_mismatch = CheckResults::default();
    real_tier_structural_checks(
        &cloud_observations(4),
        &borrowed,
        &identity,
        &stale,
        &mut identity_mismatch,
    );
    assert!(
        !identity_mismatch.ok(),
        "a frozen identity that is not the environment endpoint must fail"
    );

    let mut wrong_source = cloud_observations(4);
    wrong_source.model_requests_source = PROVIDER_ROUNDS_SOURCE;
    let mut source = CheckResults::default();
    real_tier_structural_checks(&wrong_source, &borrowed, &identity, &frozen, &mut source);
    assert!(
        !source.ok(),
        "the real tier must count rounds durably, not from a loopback provider"
    );

    let mut keyed = frozen.clone();
    keyed["apiKey"] = json!("should-never-be-here");
    let mut leaked = CheckResults::default();
    real_tier_structural_checks(&cloud_observations(4), &borrowed, &identity, &keyed, &mut leaked);
    assert!(!leaked.ok(), "a credential inside the frozen config must fail");
}

/// (b) The report writer labels each tier and records identity + artifact hashes,
/// and an executed report without a scenario is not a pass.
#[test]
fn real_eval_report_writer_records_tier_and_identity_for_both_modes() {
    let mut synthetic = scenario_fixture(EvalMode::SyntheticProvider, "agv-deliverables", "passed");
    synthetic.artifact_hashes = vec![("报表.xlsx".into(), sha256_hex(b"payload"), 7)];
    let report = build_report(ReportInput {
        mode: EvalMode::SyntheticProvider,
        executed: true,
        not_run_reason: None,
        missing_environment: vec![],
        identity: ModelIdentity::synthetic_placeholder(),
        report_run_id: new_report_run_id(EvalMode::SyntheticProvider),
        scenarios: vec![synthetic],
        environment: json!({"provider":"scripted loopback HTTP/SSE provider"}),
    });
    assert_eq!(report["tier"], "synthetic-provider");
    assert_eq!(report["mode"], "synthetic-provider");
    assert_eq!(report["executed"], true);
    assert_eq!(report["notRunReason"], Value::Null);
    assert_eq!(report["overall"], "passed");
    assert_eq!(report["syntheticProviderSpawned"], true);
    assert_eq!(report["modelIdentity"]["modelId"], SYNTHETIC_MODEL_ID);
    assert_eq!(report["modelIdentity"]["baseUrlHost"], "127.0.0.1:0");
    assert_eq!(report["modelIdentity"]["apiKeyRecorded"], false);
    assert_eq!(report["modelIdentity"]["apiKeyPresent"], true);
    assert_eq!(report["scenarios"][0]["tier"], "synthetic-provider");
    assert_eq!(report["scenarios"][0]["executed"], true);
    assert_eq!(report["artifactHashes"][0]["name"], "报表.xlsx");
    assert_eq!(report["artifactHashes"][0]["sha256"], sha256_hex(b"payload"));
    assert_eq!(report["artifactHashes"][0]["bytes"], 7);
    assert_eq!(report["artifactHashes"][0]["scenario"], "agv-deliverables");
    assert_eq!(report["sourceWorkbookSha256"], "a".repeat(64));
    assert_eq!(report["usage"]["hostUsageRows"], 2);
    assert_eq!(report["toolExecutions"]["total"], 1);
    assert_eq!(
        report["tiers"]["synthetic-provider"]["status"],
        "verified-by-this-entry"
    );
    assert_eq!(
        report["tiers"]["real-cloud-model"]["status"],
        "not-run-in-this-entry"
    );

    let cloud = build_report(ReportInput {
        mode: EvalMode::RealCloudModel,
        executed: true,
        not_run_reason: None,
        missing_environment: vec![],
        identity: cloud_identity_fixture(),
        report_run_id: new_report_run_id(EvalMode::RealCloudModel),
        scenarios: vec![scenario_fixture(
            EvalMode::RealCloudModel,
            "agv-deliverables-cloud",
            "failed",
        )],
        environment: json!({"required":[CLOUD_ENV_BASE_URL]}),
    });
    assert_eq!(cloud["tier"], "real-cloud-model");
    assert_eq!(cloud["executed"], true);
    assert_eq!(cloud["overall"], "failed");
    assert_eq!(cloud["syntheticProviderSpawned"], false);
    assert_eq!(cloud["modelIdentity"]["modelId"], "cloud-acceptance-model");
    assert_eq!(cloud["modelIdentity"]["baseUrlHost"], "cloud.example.com");
    assert_eq!(cloud["scenarios"][0]["tier"], "real-cloud-model");
    assert_eq!(
        cloud["tiers"]["real-cloud-model"]["status"],
        "failed-in-this-entry"
    );
    assert_eq!(
        cloud["tiers"]["synthetic-provider"]["status"],
        "not-run-in-this-entry"
    );

    // An "executed" report with no scenario is never a pass.
    let vacuous = build_report(ReportInput {
        mode: EvalMode::RealCloudModel,
        executed: true,
        not_run_reason: None,
        missing_environment: vec![],
        identity: cloud_identity_fixture(),
        report_run_id: new_report_run_id(EvalMode::RealCloudModel),
        scenarios: vec![],
        environment: json!({}),
    });
    assert_eq!(vacuous["overall"], "failed");
}

/// (2) Every entry owns a round-scoped, tier-scoped output directory; no entry
/// can write the historical evidence or another tier's report.
#[test]
fn real_eval_entries_write_round_scoped_directories_only() {
    let round = round_dir();
    let historical = historical_eval_dir();
    assert!(
        round.ends_with(std::path::Path::new("output/claude-design-repair-20260915/eval"))
            || std::env::var(EVAL_OUTPUT_ROOT_ENV).is_ok(),
        "{}",
        round.display()
    );
    assert_ne!(round, historical);
    assert!(
        !round.starts_with(&historical) && !historical.starts_with(&round),
        "the round directory must be independent of the historical evidence"
    );
    for mode in EvalMode::ALL {
        let dir = mode.run_dir();
        let report = dir.join(mode.report_name());
        assert!(dir.starts_with(&round), "{}", dir.display());
        assert!(!dir.starts_with(&historical), "{}", dir.display());
        assert!(!report.starts_with(&historical), "{}", report.display());
        // (2b) One evaluation owns a directory of its own, inside its tier: its
        // report, scenario databases, snapshots and artifacts are siblings, and
        // no evaluation writes into the tier directory that holds them.
        let evaluation = mode.evaluation_dir();
        assert!(evaluation.starts_with(&dir), "{}", evaluation.display());
        assert_ne!(evaluation, dir);
        assert!(!evaluation.starts_with(&historical), "{}", evaluation.display());
        assert!(!evaluation.join(mode.report_name()).starts_with(&historical));
        assert!(
            mode.artifact_dir("agv").starts_with(&evaluation),
            "a scenario's artifacts live inside its own evaluation: {:?}",
            mode.artifact_dir("agv")
        );
        let id = evaluation_id();
        assert!(
            !id.is_empty() && id.contains('-'),
            "unexpected evaluation id {id}"
        );
        assert!(
            mode.entry_command().contains("--ignored"),
            "{:?} must stay an explicitly selected entry",
            mode
        );
        assert_eq!(mode.report_name(), "real-task-eval-report.json");
    }
    assert_ne!(
        EvalMode::SyntheticProvider.evaluation_dir(),
        EvalMode::RealCloudModel.evaluation_dir(),
        "tiers must not share an evaluation directory"
    );
    assert_ne!(
        EvalMode::SyntheticProvider.run_dir(),
        EvalMode::RealCloudModel.run_dir(),
        "tiers must not share an output directory"
    );
    assert_ne!(
        EvalMode::SyntheticProvider.artifact_dir("agv"),
        EvalMode::RealCloudModel.artifact_dir("agv"),
        "tiers must not share a scenario directory"
    );
    assert_eq!(EvalMode::SyntheticProvider.tier(), "synthetic-provider");
    assert_eq!(EvalMode::RealCloudModel.tier(), "real-cloud-model");
    // The historical evidence file, if this checkout carries one, is untouched
    // by this round's writers (they refuse that directory outright).
    let historical_report = historical.join("real-task-eval-report.json");
    println!("EXPECTED PANIC (asserted by this test): writing into the historical evidence directory");
    let refused = std::panic::catch_unwind(|| {
        write_report_file(&historical, "should-never-exist.json", &json!({}), None)
    })
    .is_err();
    assert!(
        refused,
        "writing into the historical evidence directory must be refused"
    );
    assert!(
        !historical.join("should-never-exist.json").exists(),
        "the historical evidence directory must stay untouched"
    );
    if historical_report.exists() {
        let previous: Value =
            serde_json::from_str(&std::fs::read_to_string(&historical_report).unwrap()).unwrap();
        assert_eq!(
            previous["tier"], "synthetic-provider",
            "the historical report is preserved as it was produced"
        );
    }
}

/// C3: an evaluation that has finished must not be able to end the test process
/// later on. The guard is dropped with the entry — on a pass, a failure or a
/// panic — and the thread stops within one slice of that.
#[test]
fn the_watchdog_ends_with_the_evaluation_not_with_the_test_process() {
    // A deadline of a few hundred milliseconds: were the watchdog still alive
    // after the evaluation, this very process would be terminated here and every
    // other test in the same binary would die with it.
    let watchdog = eval_trace_start_with(Duration::from_millis(400));
    eval_phase("watchdog:evaluation-finished");
    // The phase cell is process-wide; even an immediate read can race another
    // test's write. Guard shutdown below is the behaviour this test owns.
    drop(watchdog);
    std::thread::sleep(Duration::from_millis(1_600));
    // Reaching this point, past four times the watchdog's deadline, is the real
    // assertion: a watchdog still alive after the evaluation ended would have
    // terminated this process and every other test in the binary with it.

    // The decision itself, both ways: a stall past the deadline is the only
    // thing that may end a process, and inside the deadline it only reports.
    let started = Instant::now();
    assert_eq!(
        watchdog_action(
            started + Duration::from_secs(10),
            started,
            Duration::from_secs(1),
            "prepare:database",
            9
        ),
        WatchdogAction::Terminate {
            phase: "prepare:database".to_owned(),
            held_secs: 9
        }
    );
    assert_eq!(
        watchdog_action(
            started + Duration::from_millis(500),
            started,
            Duration::from_secs(60),
            "prepare:database",
            9
        ),
        WatchdogAction::Report {
            phase: "prepare:database".to_owned(),
            held_secs: 9
        }
    );
    // A stall reports every phase it went through, not just the last one: the
    // history records each phase when the entry leaves it, and the phase still
    // being held is the current one.
    eval_phase("watchdog:history-a");
    eval_phase("watchdog:history-b");
    let history = eval_phase_history();
    assert!(
        history.iter().any(|line| line.contains("watchdog:history-a")),
        "{history:?}"
    );
    let (current, _) = eval_current_phase();
    assert_eq!(current, "watchdog:history-b", "{history:?}");
    assert!(
        history.iter().any(|line| line.contains("watchdog:evaluation-finished")),
        "the finished evaluation's phase must still be on the trail: {history:?}"
    );
}

/// C4: a repeated evaluation of the same scenario reserves its own directory.
/// Nothing is deleted, nothing is reused, and an old directory the operating
/// system will not give up cannot pull a new evaluation into it.
#[test]
fn evaluation_supervisor_child_fixture() {
    if std::env::var(EVAL_CHILD_TEST).ok().as_deref()
        != Some(evaluation_test_name("evaluation_supervisor_child_fixture").as_str()) {
        return;
    }
    let phase = std::env::var("FOX_EVAL_SUPERVISION_CASE").unwrap();
    let _watchdog = eval_trace_start_with(Duration::from_millis(100));
    eval_phase(&format!("{phase}:intentional-stall"));
    let marker = std::env::var("FOX_EVAL_SUPERVISION_MARKER").unwrap();
    std::fs::write(marker, &phase).unwrap();
    if phase == "completed" { return; }
    loop { std::thread::sleep(Duration::from_millis(50)); }
}

#[test]
fn evaluation_deadlines_kill_only_the_owned_process_and_keep_evidence() {
    let marker = std::env::temp_dir().join(format!("fox-eval-supervisor-{}.txt", uuid::Uuid::new_v4()));
    let marker_text = marker.to_str().unwrap();
    for tier in ["synthetic-provider", "real-cloud-model"] {
        let result = run_evaluation_process("evaluation_supervisor_child_fixture", Duration::from_secs(3), &[
            ("FOX_EVAL_SUPERVISION_CASE", tier), ("FOX_EVAL_SUPERVISION_MARKER", marker_text),
        ]);
        assert!(result.unwrap_err().contains("exceeded its deadline"));
        assert_eq!(std::fs::read_to_string(&marker).unwrap(), tier,
            "completed evidence survives timeout; the shared test process remains alive");
    }
    // Following work still completes after both timed-out processes were reaped.
    let status = run_evaluation_process("evaluation_supervisor_child_fixture", Duration::from_secs(10), &[
        ("FOX_EVAL_SUPERVISION_CASE", "completed"), ("FOX_EVAL_SUPERVISION_MARKER", marker_text),
    ]).unwrap();
    assert!(status.success());
    assert_eq!(std::fs::read_to_string(&marker).unwrap(), "completed");
    std::fs::remove_file(marker).unwrap();
}

#[test]
fn a_scenario_directory_is_reserved_exclusively_and_old_evidence_survives() {
    let base = std::env::temp_dir().join(format!(
        "fox-eval-reserve-{}",
        uuid::Uuid::new_v4().simple()
    ));
    std::fs::create_dir_all(&base).unwrap();
    let first = reserve_directory(&base, "agv").unwrap();
    // Hold a file inside it open: on Windows that makes the directory
    // undeletable, which is the condition a "clean up first" design breaks on.
    let _lock = std::fs::File::create(first.join("facts.db")).unwrap();
    std::fs::write(first.join("分布.xlsx"), b"evaluation-1 bytes").unwrap();
    std::fs::write(first.join("real-task-eval-report.json"), b"{\"run\":1}").unwrap();
    let before = artifact_hashes(&first);
    assert_eq!(before.len(), 2, "{before:?}");

    for _ in 0..3 {
        let next = reserve_directory(&base, "agv").unwrap();
        assert_ne!(next, first, "an existing directory is never reused");
        assert!(
            std::fs::read_dir(&next).map(|mut entries| entries.next().is_none()).unwrap_or(false),
            "a fresh evaluation starts with no artifacts at all: {next:?}"
        );
        // The old evaluation's artifacts cannot leak into the new one's checks:
        // they are not in this directory, and nothing copied them.
        assert!(!next.join("分布.xlsx").exists());
        assert_eq!(artifact_hashes(&first), before, "the earlier evidence changed");
    }

    // Two evaluations at once reserve different directories too.
    let threads: Vec<_> = (0..4)
        .map(|_| {
            let base = base.clone();
            std::thread::spawn(move || reserve_directory(&base, "agv"))
        })
        .collect();
    let mut taken = BTreeSet::new();
    for thread in threads {
        let path = thread.join().unwrap().unwrap();
        let shown = path.display().to_string();
        assert!(taken.insert(path), "two evaluations shared {shown}");
    }
    assert_eq!(artifact_hashes(&first), before, "the earlier evidence changed");
    let _ = std::fs::remove_dir_all(base);
}

/// C5: the registry audit must be able to fail. A snapshot whose bytes are not
/// what its row recorded is a failure, not a restorable version — otherwise
/// `restorable=6/6` says nothing about whether anybody can get their file back.
#[test]
fn the_restore_audit_fails_when_a_snapshot_does_not_match_its_row() {
    use crate::runtime_host::managed_files::{hash_file, record_office_write, OfficeWriteDetails};
    let root = std::env::temp_dir().join(format!(
        "fox-eval-restore-audit-{}",
        uuid::Uuid::new_v4().simple()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let db = Database::open(root.join("facts.db")).unwrap();
    let conversation = db
        .create_conversation(
            db.default_agent_id(),
            None,
            Some(root.to_str().unwrap()),
            Some("allow"),
        )
        .unwrap();
    let run_id = db.create_run(&conversation.id, "restore-audit", None).unwrap().run.id;
    let backups = root.join("managed-files");
    std::fs::create_dir_all(&backups).unwrap();
    let target = root.join("纪要.docx");
    let storage_path = target.to_string_lossy().into_owned();

    let register = |version: usize| {
        std::fs::write(
            &target,
            format!("document content generation {version}").as_bytes(),
        )
        .unwrap();
        let (after_hash, after_size) = hash_file(&target).unwrap();
        let snapshot = backups.join(format!("snapshot-v{version}"));
        std::fs::write(&snapshot, std::fs::read(&target).unwrap()).unwrap();
        record_office_write(
            &db,
            &conversation.id,
            &run_id,
            Some(&format!("tc-{version}")),
            &OfficeWriteDetails {
                tool: "office_create",
                storage_path: &storage_path,
                display_name: "纪要.docx",
                change_kind: "created",
                before_hash: None,
                before_size: None,
                after_hash: &after_hash,
                after_size,
                backup_path: None,
                after_backup_path: Some(&snapshot.to_string_lossy()),
            },
        )
        .unwrap()
    };
    let v1 = register(1);
    let v2 = register(2);
    let rows = || {
        db.managed_file_versions(&conversation.id, None)
            .unwrap()
            .into_iter()
            .filter(|row| row.run_id.as_deref() == Some(run_id.as_str()))
            .collect::<Vec<_>>()
    };

    // Positive control: intact snapshots audit clean.
    let (verified, failures) = restore_source_audit(&db, &rows());
    assert_eq!(failures, Vec::<String>::new(), "{failures:?}");
    assert_eq!(verified, 2);

    // The failure sample: version 2's snapshot no longer holds its bytes.
    let tampered = backups.join("snapshot-v2");
    std::fs::write(&tampered, b"someone replaced these bytes").unwrap();
    let (verified, failures) = restore_source_audit(&db, &rows());
    assert_eq!(verified, 1, "a tampered snapshot was still counted: {failures:?}");
    assert_eq!(failures.len(), 1, "{failures:?}");
    assert!(failures[0].contains("不一致"), "{failures:?}");
    assert!(failures[0].contains(&v2), "the wrong row was blamed: {failures:?}");
    assert!(!failures[0].contains(&v1), "the wrong row was blamed: {failures:?}");
    // The exact boolean the acceptance uses must be false — a count match alone
    // is what used to let this pass.
    let all = rows();
    assert!(
        !(verified == all.len() && failures.is_empty()),
        "the audit still declared a pass after tampering"
    );
    let _ = std::fs::remove_dir_all(root);
}

/// The repeatable synthetic regression (`synthetic-provider` tier). Requires
/// Node + the pinned OfficeCLI. It does NOT contact any cloud model.
#[test]
#[ignore = "synthetic regression: requires the Node runtime and the pinned OfficeCLI sidecar (no cloud credentials used)"]
fn real_task_evaluation_through_host_kernel_runtime_and_real_tools() {
    if supervise_evaluation("real_task_evaluation_through_host_kernel_runtime_and_real_tools") { return; }
    let mode = EvalMode::SyntheticProvider;
    // Phase trace + bounded watchdog: a stall must name its stage and end at a
    // deadline instead of waiting unobserved (see the trace section above).
    let _watchdog = eval_trace_start();
    eval_phase("synthetic:start");
    let scenarios = vec![
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(run_agv_scenario)) {
            Ok(outcome) => outcome,
            Err(payload) => panic_outcome(
                mode,
                "agv-deliverables",
                "AGV：真实链路生成两份 Excel 与一份 Word",
                payload,
            ),
        },
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(
            run_short_document_scenario,
        )) {
            Ok(outcome) => outcome,
            Err(payload) => panic_outcome(
                mode,
                "short-document-edit",
                "短文档修改：创建、原地修改与备份",
                payload,
            ),
        },
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(
            run_large_result_scenario,
        )) {
            Ok(outcome) => outcome,
            Err(payload) => panic_outcome(
                mode,
                "large-result-continuation-read",
                "大结果续读：完整存储与 read_tool_result 多页续读",
                payload,
            ),
        },
    ];
    let report = build_report(ReportInput {
        mode,
        executed: true,
        not_run_reason: None,
        missing_environment: vec![],
        identity: ModelIdentity::synthetic_placeholder(),
        report_run_id: new_report_run_id(mode),
        scenarios,
        environment: json!({
            "required": [],
            "provider": "scripted loopback HTTP/SSE provider created by this entry",
            "apiKeyRecorded": false,
        }),
    });
    let report_path = write_report_file(
        mode.evaluation_dir().as_path(),
        mode.report_name(),
        &report,
        None,
    );
    let scenarios = report["scenarios"].as_array().cloned().unwrap_or_default();
    for scenario in &scenarios {
        println!(
            "EVAL {} {} in {}ms — {}",
            scenario["id"],
            scenario["status"],
            scenario["durationMs"],
            report_path.display()
        );
        for check in scenario["checks"].as_array().into_iter().flatten() {
            println!(
                "  [{}] {} — {}",
                if check["passed"] == json!(true) { "PASS" } else { "FAIL" },
                check["id"],
                check["detail"]
            );
        }
    }
    let failed: Vec<&Value> = scenarios
        .iter()
        .filter(|scenario| scenario["status"] != json!("passed"))
        .collect();
    if failed.is_empty() {
        return;
    }
    for scenario in &failed {
        eprintln!(
            "FAILED {} stage={:?} failure={:?}",
            scenario["id"], scenario["failureStage"], scenario["failure"]
        );
    }
    panic!(
        "synthetic real-task evaluation failed; report at {}",
        report_path.display()
    );
}

/// Real cloud-model acceptance (`real-cloud-model` tier). Needs Node + the
/// pinned OfficeCLI AND real credentials. Without credentials it writes a
/// 未执行 report naming the missing variables and FAILS: it never falls back to
/// the synthetic provider and never records a pass.
#[test]
#[ignore = "real cloud-model acceptance: requires FOX_EVAL_* credentials, the Node runtime and the pinned OfficeCLI sidecar"]
fn real_task_evaluation_cloud() {
    if supervise_evaluation("real_task_evaluation_cloud") { return; }
    let mode = EvalMode::RealCloudModel;
    // The same guard the synthetic tier gets: phases recorded from the first
    // statement, a deadline that stops with the evaluation, and a bounded end
    // for a cloud round that never returns.
    let _watchdog = eval_trace_start();
    eval_phase("cloud:start");
    let env = match CloudEnvironment::read_from_process() {
        Ok(env) => env,
        Err(error) => {
            let reason = error.summary();
            let report = build_report(ReportInput {
                mode,
                executed: false,
                not_run_reason: Some(reason.clone()),
                missing_environment: error.variables_json(),
                identity: ModelIdentity::not_configured(
                    "the acceptance did not run, so no model endpoint was contacted",
                ),
                report_run_id: new_report_run_id(mode),
                scenarios: vec![],
                environment: cloud_not_run_environment(),
            });
            let report_path = try_write_report(
                mode.evaluation_dir().as_path(),
                mode.report_name(),
                &report,
                None,
            );
            let written = match report_path {
                Ok(path) => format!("report at {}", path.display()),
                Err(error) => format!("the {NOT_RUN_MARKER} report could not be written: {error}"),
            };
            panic!(
                "{reason}; {written} (this entry never substitutes the synthetic provider)",
            );
        }
    };
    let identity = env.identity();
    println!(
        "EVAL-CLOUD endpoint modelId={} apiType={} host={} (key not recorded)",
        identity.model_id, identity.api_type, identity.base_url_host
    );
    let scenario =
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run_agv_cloud_scenario(&env, &identity)
        })) {
            Ok(outcome) => outcome,
            Err(payload) => panic_outcome(
                mode,
                "agv-deliverables-cloud",
                "AGV（真实云模型）：真实模型驱动同一链路生成两份 Excel 与一份 Word",
                payload,
            ),
        };
    let status = scenario.status;
    let report = build_report(ReportInput {
        mode,
        executed: true,
        not_run_reason: None,
        missing_environment: vec![],
        identity: identity.clone(),
        report_run_id: new_report_run_id(mode),
        scenarios: vec![scenario],
        environment: env.report_json(),
    });
    let report_path = write_report_file(
        mode.evaluation_dir().as_path(),
        mode.report_name(),
        &report,
        Some(&env.api_key),
    );
    for scenario in report["scenarios"].as_array().into_iter().flatten() {
        println!(
            "EVAL {} {} in {}ms — {}",
            scenario["id"],
            scenario["status"],
            scenario["durationMs"],
            report_path.display()
        );
        for check in scenario["checks"].as_array().into_iter().flatten() {
            println!(
                "  [{}] {} — {}",
                if check["passed"] == json!(true) { "PASS" } else { "FAIL" },
                check["id"],
                check["detail"]
            );
        }
    }
    if status == "passed" {
        return;
    }
    panic!(
        "real-cloud-model acceptance failed; report at {}",
        report_path.display()
    );
}

fn panic_outcome(
    mode: EvalMode,
    id: &'static str,
    title: &'static str,
    payload: Box<dyn std::any::Any + Send>,
) -> ScenarioOutcome {
    let detail = payload
        .downcast_ref::<&str>()
        .map(|value| (*value).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "panic with unknown payload".into());
    ScenarioOutcome {
        id,
        title,
        mode,
        status: "failed",
        run_id: None,
        duration_ms: 0,
        model_requests: None,
        model_requests_source: match mode {
            EvalMode::SyntheticProvider => PROVIDER_ROUNDS_SOURCE,
            EvalMode::RealCloudModel => DURABLE_ROUNDS_SOURCE,
        },
        failure_stage: Some("panic".into()),
        failure: Some(json!({"error": detail})),
        checks: vec![],
        delivery: Value::Null,
        usage: Value::Null,
        tool_executions: Value::Null,
        artifacts: vec![],
        artifact_hashes: vec![],
        source_sha256: None,
    }
}

// Exercise the SAME driver used by the cloud tier, with real wall/monotonic
// time and a local scripted endpoint. No cloud credentials or OfficeCLI needed.
fn exercise_eval_approval_clock(approve: bool) {
    let root = std::env::temp_dir().join(format!("fox-eval-clock-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("proof.txt"), "observed before approval").unwrap();
    let expected_version = crate::tool_host::file_version(b"observed before approval");
    let proposed_version = expected_version.clone();
    let provider = spawn_provider(move |index, _| {
        if index == 0 {
            // Deliberately exceed the approval window before an approval exists:
            // each later proposal must receive a fresh window, not the Run start.
            if approve { std::thread::sleep(Duration::from_millis(1_000)); }
            (Reply::Tool { id: "write-proof".into(), name: "write_file".into(),
                arguments: json!({"path":"proof.txt","content":"authorized once",
                    "expectedVersion": proposed_version}) }, true)
        } else { (Reply::Stop, true) }
    });
    let mut config = eval_model_config(provider.address);
    config.proposal_tools.push(json!({"name":"write_file","description":"Write a project file",
        "parameters":{"type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"},
        "expectedVersion":{"type":"string"}},"required":["path","content","expectedVersion"]}}));
    let (db, run_id) = prepare_eval_run_with_budgets(&root, "Perform the requested operation.",
        &config, "clock-regression", PermissionMode::Ask, false,
        TimeBudgets { approval_wait_ms: 600, ..TimeBudgets::default() }).unwrap();
    // The write gate consumes only Host-opened file evidence. Drive the same
    // reader seam as production so this fixture reaches Ask instead of being
    // correctly rejected for a missing observation.
    let binding = db.run_control_binding(&run_id).unwrap().unwrap();
    let cancellation = CancellationRegistry::default();
    cancellation.register_run(&run_id).unwrap();
    let token = cancellation.tool_token(&run_id, "clock-observe-proof").unwrap();
    let read_input = json!({"path":"proof.txt"});
    let observed = crate::runtime_host::managed_files::execute_observed_reader(
        &db,
        &binding,
        "read",
        &read_input,
        "clock-observe-proof",
        &token,
        Duration::from_secs(5),
    )
    .expect("the real Host reader records the approval target baseline");
    assert_eq!(observed["content"][0]["text"], "observed before approval");
    assert_eq!(observed["details"]["readVersion"], expected_version);
    let observed = drive_real_run_with_approvals(&db, &root, &run_id,
        ProviderSession::from_handle(provider), EVAL_API_KEY,
        Arc::new(Mutex::new(EvalState::default())), approve);
    assert_eq!(observed.dispatch_error, None);
    if approve {
        assert_eq!(observed.state, "completed");
        assert_eq!(observed.approvals, 1);
        assert_eq!(std::fs::read_to_string(root.join("proof.txt")).unwrap(), "authorized once");
        let records = db.list_runtime_tool_calls_for_run(&run_id).unwrap();
        assert_eq!(records.iter().filter(|row| row.status == "completed").count(), 1);
    } else {
        assert_eq!(observed.state, "approval_expired");
        assert_eq!(observed.approvals, 0);
        assert_eq!(
            std::fs::read_to_string(root.join("proof.txt")).unwrap(),
            "observed before approval",
            "an expired approval must leave the observed target unchanged",
        );
        assert!(db.queue_kernel_host_command(&run_id, Some(("write-proof", "allow_once"))).is_err());
        assert!(db.queue_kernel_host_command(&run_id, None).is_err());
    }
    assert!(!root.join("facts.db").exists());
    assert!(!root.join("model-visible-request.json").exists());
    assert!(eval_host_dir(&root).join("facts.db").exists());
}

#[test]
fn real_eval_driver_expires_unanswered_approval_with_wall_time() {
    exercise_eval_approval_clock(false);
}

#[test]
fn real_eval_driver_gives_later_proposals_fresh_approval_windows() {
    exercise_eval_approval_clock(true);
}

// ---------------------------------------------------------------------------
// E-T-01 / E-T-02 / E-T-05 — the three rust-host acceptance cases.
//
// Each one drives the SAME chain a Legacy Runtime drives in production:
//
//   the frozen RunControlBinding (Legacy authority, Rust read-only executor)
//   -> a real durable Host ToolCall row (`create_fresh_host_tool_call`)
//   -> the real executor (`tool_host::prepare` / `execute_with_cancellation`
//      for file/command tools, `resource_gateway` for readers, reached through
//      `execute_rust_reader_request`)
//   -> `finalize_host_tool_execution`, which persists the terminal status and
//      the result the model later re-reads
//   -> `Database::tool_result_range` over `fox-result://<run>/<call>`, the only
//      route by which a model reaches those stored bytes back
//
// No model, no cloud credentials, no Node runtime and no OfficeCLI are needed,
// so these are plain (non-ignored) tests: the exact `cargo test` filters in
// `services/agent-runtime/evals/harness-v1/tasks.mjs` name each one.
//
// A contract requirement the CURRENT product base does not satisfy is
// RECORDED (`passed: false`, with expected vs actual) in the evidence file and
// in `tasks.mjs` as `expectedOutcomeAtBaseline` — it is never hidden and never
// relabelled as a product improvement when a later base satisfies it. Only a
// broken measurement chain (the executor did not really run, the row was not
// persisted, the re-read did not resolve) fails the test itself, because that
// would mean the case measured nothing.
// ---------------------------------------------------------------------------

/// Mirrors `HARNESS_VERSION` in `evals/harness-v1/tasks.mjs` so an evidence
/// file names the harness revision that produced it.
const RUST_HOST_HARNESS: &str = "harness-v1.2/rust-host";
/// Mirrors `CONTRACT_VERSION` in the same file.
const RUST_HOST_CONTRACT: &str = "CONTRACTS v1.2";

/// A frozen Legacy Run with the Rust read-only executor, project root and an
/// explicit permission mode — the shape the Legacy protocol gives Host when a
/// model calls `read` / `write_file` / `edit_file` / `run_command`. Legacy
/// authority is not optional here: `complete_host_tool_call` refuses to
/// finalize a tool call on an Authoritative-owned Run, and the re-read half of
/// every case below depends on that finalization.
fn rust_host_run(label: &str, permission_mode: &str) -> (Database, std::path::PathBuf, String, String) {
    let root = std::env::temp_dir().join(format!(
        "fox-eval-rust-host-{label}-{}",
        uuid::Uuid::new_v4().simple()
    ));
    std::fs::create_dir_all(&root).expect("an isolated project root");
    let db = Database::open(root.join("facts.db")).expect("a facts database");
    let conversation = db
        .create_conversation(
            db.default_agent_id(),
            None,
            Some(root.to_str().unwrap()),
            Some(permission_mode),
        )
        .expect("a conversation bound to the project root");
    let run = db
        .create_run(&conversation.id, "rust-host-eval", None)
        .expect("a Run")
        .run;
    db.freeze_legacy_run_control_with_executor(&run.id, "legacy", ResourceExecutor::Rust)
        .expect("a frozen Legacy binding with the Rust executor");
    db.apply_runtime_event(&run.id, 1, &json!({ "type": "run.started" }))
        .expect("a fresh Host ToolCall needs a running Run");
    (db, root, conversation.id, run.id)
}

/// The Legacy Host dispatch core for a file or command tool, in the exact order
/// `execute_host_tool_request` uses: admission (`tool_host::prepare`), then the
/// durable ToolCall row, then real execution, then `finalize_host_tool_execution`.
///
/// `run_command` always waits for a one-shot human approval in production, so
/// this entry corresponds to the post-approval state (the same state an
/// `allow_once` decision produces); the approval UI is not what E-T-01
/// measures. For `write_file` / `edit_file` under an `allow` project this IS the
/// production path verbatim.
fn dispatch_host_tool(
    db: &Database,
    root: &std::path::Path,
    run_id: &str,
    tool: &str,
    input: &Value,
    tool_call_id: &str,
) -> Result<Value, String> {
    let registry = CancellationRegistry::default();
    registry.register_run(run_id)?;
    let token = registry.tool_token(run_id, tool_call_id)?;
    let prepared = crate::tool_host::prepare(
        tool,
        input,
        root.to_str().ok_or("the project root is not valid UTF-8")?,
    )?;
    match create_fresh_host_tool_call(db, run_id, tool_call_id, tool, input, "running", false)? {
        HostToolCallExecution::Replay(response) => Ok(response),
        HostToolCallExecution::Execute(_) => {
            let outcome = crate::tool_host::execute_with_cancellation(prepared, Some(&token));
            finalize_host_tool_execution(db, run_id, tool_call_id, tool, input, Vec::new(), outcome)
        }
    }
}

/// E-T-01 measures command diagnostics, not production backend availability.
/// Its authorization is an explicit cfg(test)-only dependency, so the shipped
/// Host remains fail-closed while this low-level fixture can run a real child.
fn dispatch_command_with_test_backend(
    db: &Database,
    root: &std::path::Path,
    run_id: &str,
    input: &Value,
    tool_call_id: &str,
) -> Result<Value, String> {
    let prepared = crate::tool_host::prepare(
        "run_command",
        input,
        root.to_str().ok_or("the project root is not valid UTF-8")?,
    )?;
    match create_fresh_host_tool_call(
        db,
        run_id,
        tool_call_id,
        "run_command",
        input,
        "running",
        false,
    )? {
        HostToolCallExecution::Replay(response) => Ok(response),
        HostToolCallExecution::Execute(_) => {
            let proof = crate::process_jobs::AvailableInTests::host_user_unconfined(
                "real-eval-command-fixture",
            );
            let outcome = crate::tool_host::execute_command_with_test_backend(prepared, &proof);
            finalize_host_tool_execution(
                db,
                run_id,
                tool_call_id,
                "run_command",
                input,
                Vec::new(),
                outcome,
            )
        }
    }
}

/// The reader route: a real `tool.readonly_execute` envelope admitted by
/// `execute_rust_reader_request`, which is the Legacy+Rust seam a Runtime
/// reaches. Identical to the envelope `legacy_read_result` builds.
fn dispatch_reader(
    db: &Database,
    run_id: &str,
    tool: &str,
    path: &str,
    tool_call_id: &str,
) -> Result<Value, String> {
    use crate::runtime_host::protocol::{timestamp, RuntimeEnvelope, PROTOCOL_NAME, PROTOCOL_VERSION};
    let binding = db
        .run_control_binding(run_id)?
        .ok_or("the reader Run has no frozen control binding")?;
    let registry = CancellationRegistry::default();
    registry.register_run(run_id)?;
    let token = registry.tool_token(run_id, tool_call_id)?;
    let envelope: RuntimeEnvelope = serde_json::from_value(json!({
        "protocol": PROTOCOL_NAME, "version": PROTOCOL_VERSION,
        "kind": "request", "type": "tool.readonly_execute",
        "id": format!("reader-{tool_call_id}"),
        "timestamp": timestamp(),
        "runId": binding.run_id, "conversationId": binding.conversation_id,
        "payload": { "toolCallId": tool_call_id, "tool": tool,
                     "input": { "path": path },
                     "permissionSnapshotId": binding.permission_snapshot_id },
    }))
    .expect("the reader envelope is well-formed");
    crate::runtime_host::execute_rust_reader_request(db, &envelope, &token)
}

/// What the Host hands the model for this call, built exactly as
/// `handle_host_tool_request` builds it: an `Ok` payload carries the result; an
/// `Err` becomes `{"isError":true,"error":...}`. That envelope is the failure
/// receipt EX-v1 requires, and the checker never trusts the model's own reading
/// of it.
fn model_receipt(outcome: &Result<Value, String>) -> Value {
    match outcome {
        Ok(value) => value.clone(),
        Err(error) => json!({ "isError": true, "error": error }),
    }
}

/// The receipt reports a failure when it says so at the top level (an `Err`
/// dispatch) or through the nested business-failure result EX-v1 allows (an
/// `Ok` whose result carries `isError`). Either is a legal failure receipt;
/// neither may read as a success.
fn receipt_reports_failure(receipt: &Value) -> bool {
    receipt.get("isError").and_then(Value::as_bool) == Some(true)
        || receipt
            .get("result")
            .and_then(|result| result.get("isError"))
            .and_then(Value::as_bool)
            == Some(true)
}

/// A persisted row is a business-failure outcome only when it ended `failed`.
///
/// CONTRACTS v1.2 §6: an execution that has already ended in business failure
/// may be returned as `Ok(Value)` with a top-level `isError=true`, but Host
/// **must persist `failed`**. `completed` is therefore never a failure outcome,
/// whatever `isError` the row carries — accepting `completed + isError` would let
/// a mutant row read as a failure it never was, and would broaden the allowed
/// set beyond the frozen contract to accommodate an old baseline. This is the
/// strict criterion; `persisted_completed_is_error_mutant_must_not_be_a_failure`
/// pins it.
fn persisted_failure(record: &crate::database::ToolCallRecord) -> bool {
    record.status.as_str() == "failed"
}

/// Compile-time source bytes of the product files a rust-host case exercises.
/// An old executable must never label itself with newly edited checkout bytes.
/// The importer compares these digests with the current checkout. Mirrors
/// `MEASURED_PRODUCT_FILES` in `evals/harness-v1/evidence-identity.mjs`
/// verbatim; both sides must list the same paths in the same order.
const MEASURED_PRODUCT_FILES: &[(&str, &[u8])] = &[
    ("apps/desktop/src-tauri/src/tool_host.rs", include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/tool_host.rs"))),
    ("apps/desktop/src-tauri/src/resource_gateway.rs", include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/resource_gateway.rs"))),
    ("apps/desktop/src-tauri/src/tool_guard.rs", include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/tool_guard.rs"))),
    ("apps/desktop/src-tauri/src/runtime_host/mod.rs", include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/runtime_host/mod.rs"))),
    ("apps/desktop/src-tauri/src/database/repositories.rs", include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/database/repositories.rs"))),
    ("apps/desktop/src-tauri/src/runtime_host/kernel_coordinator/real_eval_tests.rs", include_bytes!("real_eval_tests.rs")),
];

/// sha256 of one file's bytes, or `None` when unreadable.
fn file_sha256(path: &std::path::Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    Some(format!("{:x}", Sha256::digest(&bytes)))
}

/// Content hashes of the measured product files, relative to the repository
/// root. Mirrors `productFileDigests` in `evidence-identity.mjs`.
fn product_file_digests() -> Vec<Value> {
    MEASURED_PRODUCT_FILES
        .iter()
        .map(|(relative, compiled_bytes)| {
            json!({ "path": relative, "sha256": format!("{:x}", Sha256::digest(compiled_bytes)) })
        })
        .collect()
}

#[test]
fn rust_host_evidence_compiled_sources_match_the_checkout() {
    for (relative, compiled_bytes) in MEASURED_PRODUCT_FILES {
        let current = std::fs::read(repository_dir().join(relative)).expect("measured source is readable");
        assert_eq!(Sha256::digest(compiled_bytes), Sha256::digest(&current), "stale test executable: {relative}");
    }
}

/// sha256 over the harness sources, mirroring `hashHarnessDir` in
/// `evidence-identity.mjs`: walk the harness dir, collect `*.mjs`/`*.js`/`*.md`,
/// sort by full path, digest each file's bytes, join `basename:sha256` with
/// `|`, sha256 the result. Returns `None` when the harness sources are absent
/// (a run without the checker sources present cannot prove which checker
/// version judged it — the harness then rejects the evidence, by design).
fn harness_source_sha256() -> Option<String> {
    let dir = harness_dir()?;
    let mut files = Vec::new();
    fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if let Some(extension) = path.extension() {
                if matches!(extension.to_str(), Some("mjs") | Some("js") | Some("md")) {
                    out.push(path);
                }
            }
        }
    }
    walk(&dir, &mut files);
    files.sort();
    let joined = files
        .iter()
        .filter_map(|path| {
            let name = path.file_name()?.to_str()?;
            let digest = file_sha256(path)?;
            Some(format!("{name}:{digest}"))
        })
        .collect::<Vec<_>>()
        .join("|");
    Some(format!("{:x}", Sha256::digest(joined.as_bytes())))
}

/// The harness directory, next to the measured product sources. Mirrors the
/// default the Node runner resolves for the same product root.
fn harness_dir() -> Option<std::path::PathBuf> {
    let dir = repository_dir().join("services/agent-runtime/evals/harness-v1");
    dir.is_dir().then_some(dir)
}

/// The product commit this test compiled against, resolved by reading git's
/// files (no subprocess). Mirrors `gitMetadataViaFiles` in `run-harness.mjs`:
/// `.git` → gitdir → `HEAD` → `refs/heads/<branch>` (via commondir) with a
/// packed-refs fallback. Returns `None` when unresolvable — the harness then
/// refuses to accept the evidence, which is the correct outcome for a run whose
/// product identity cannot be established.
fn product_sha() -> Option<String> {
    let gitdir = read_gitdir(&repository_dir())?;
    let head = read_trim(&gitdir.join("HEAD"))?;
    if let Some(branch) = head.strip_prefix("ref:") {
        let branch = branch.trim().strip_prefix("refs/heads/")?;
        let commondir = read_trim(&gitdir.join("commondir"))
            .map(|value| {
                if value.contains(':') || value.starts_with('/') {
                    std::path::PathBuf::from(value)
                } else {
                    gitdir.join(value)
                }
            })
            .unwrap_or_else(|| gitdir.clone());
        if let Some(commit) = read_trim(&commondir.join("refs").join("heads").join(branch)) {
            return Some(commit);
        }
        return read_packed_ref(&commondir, branch);
    }
    Some(head)
}

fn read_gitdir(root: &std::path::Path) -> Option<std::path::PathBuf> {
    if root.join(".git").is_dir() {
        return Some(root.join(".git"));
    }
    let dot_git = read_trim(&root.join(".git"))?;
    if let Some(relative) = dot_git.strip_prefix("gitdir:") {
        let path = relative.trim();
        return Some(if path.contains(':') || path.starts_with('/') {
            std::path::PathBuf::from(path)
        } else {
            root.join(path)
        });
    }
    Some(root.join(".git"))
}

fn read_trim(path: &std::path::Path) -> Option<String> {
    std::fs::read_to_string(path).ok().map(|value| value.trim().to_owned())
}

fn read_packed_ref(commondir: &std::path::Path, branch: &str) -> Option<String> {
    let packed = read_trim(&commondir.join("packed-refs"))?;
    packed
        .lines()
        .find(|line| line.ends_with(&format!("refs/heads/{branch}")))
        .and_then(|line| line.split_whitespace().next().map(|value| value.to_owned()))
}

/// The execution identity: product + harness + contract + case. Mirrors
/// `executionStampOf` in `evidence-identity.mjs`; a plain join, no hashing, so
/// both languages produce the identical string.
fn execution_stamp(product_sha: &str, harness_source_sha256: &str, case: &str) -> String {
    format!("{product_sha}|{harness_source_sha256}|{RUST_HOST_CONTRACT}|{case}")
}

/// Persist one case's checks so a verdict survives the test process: a baseline
/// contract failure is evidence, not a console line next to a pass.
///
/// O-REVIEW-03 gate 1: the record binds production (product commit + measured
/// source content hashes), evaluation (harness source hash), contract, case and
/// execution identity, with a real timestamp. The Node harness re-derives every
/// one of these from its own run and rejects the record on any mismatch, any
/// missing field, an empty check set, a missing required check, or a
/// contradictory `overall`. Older records are never overwritten: each run writes
/// `<case>-<generatedAtMs>.json` and the harness takes the newest.
fn record_rust_host_case(case: &str, problems: &[&str], tool: &str, checks: &CheckResults) {
    let rows = checks
        .rows
        .iter()
        .map(|(id, passed, detail)| json!({ "id": id, "passed": *passed, "detail": detail }))
        .collect::<Vec<_>>();
    let generated_at_ms = crate::database::now_ms();
    let product_sha = product_sha().unwrap_or_default();
    let harness_source_sha256 = harness_source_sha256().unwrap_or_default();
    let report = json!({
        "case": case,
        "harness": RUST_HOST_HARNESS,
        "contractVersion": RUST_HOST_CONTRACT,
        "productSha": product_sha,
        "productRoot": repository_dir().to_string_lossy(),
        "harnessSourceSha256": harness_source_sha256,
        "executionStamp": execution_stamp(&product_sha, &harness_source_sha256, case),
        "executionId": std::env::var("FOX_HARNESS_EXECUTION_ID").unwrap_or_default(),
        "problemIds": problems,
        "tool": tool,
        "generatedAtMs": generated_at_ms,
        // Verdicts are per product base; a later base may satisfy a requirement
        // the current one does not, and tasks.mjs records which.
        "verdictScope": "the product base this cargo test compiled against",
        "productFiles": product_file_digests(),
        "requiredChecks": rows.iter().map(|row| row["id"].clone()).collect::<Vec<_>>(),
        "overall": if checks.ok() { "passed" } else { "failed" },
        "checks": rows,
    });
    // Newest file for a case is used; old ones stay on disk, untouched.
    let name = format!(
        "{}-{generated_at_ms}.json",
        case.to_lowercase().replace('-', "_")
    );
    let dir = rust_host_evidence_dir();
    let path = match dir {
        Ok(dir) => {
            let path = dir.join(&name);
            // Each run writes its own timestamped file; earlier runs' files are
            // never overwritten, only superseded by a newer timestamp.
            let _ = std::fs::create_dir_all(&dir);
            let _ = std::fs::write(&path, serde_json::to_string_pretty(&report).unwrap_or_default());
            path
        }
        Err(reason) => {
            // The round directory is unusable (or is the historical one): keep
            // the verdict out of temp rather than writing it somewhere nobody
            // looks, and say so.
            println!("[rust-host:{case}] evidence could not be persisted: {reason}");
            std::path::PathBuf::from(format!("<unwritable: {reason}>/{name}"))
        }
    };
    println!(
        "[rust-host:{case}] {} — evidence at {}",
        if checks.ok() { "PASS" } else { "FAIL" },
        path.display()
    );
    println!(
        "[rust-host:{case}] identity productSha={product_sha} harnessSourceSha256={}",
        if harness_source_sha256.is_empty() { "<unavailable>" } else { &harness_source_sha256[..16] }
    );
    for (id, passed, detail) in &checks.rows {
        println!(
            "  [{}] {id} — {}",
            if *passed { "PASS" } else { "FAIL" },
            detail.chars().take(300).collect::<String>()
        );
    }
}

/// Where the rust-host cases persist their verdicts. Mirrors
/// `rustHostEvidenceDir` in `evals/harness-v1/run-harness.mjs` and reuses the
/// same round-directory rules as the other evaluation entries.
fn rust_host_evidence_dir() -> Result<std::path::PathBuf, String> {
    let dir = round_dir().join("rust-host");
    if dir.starts_with(historical_eval_dir()) {
        return Err(format!(
            "the historical evidence directory is read-only for this round: {}",
            historical_eval_dir().display()
        ));
    }
    Ok(dir)
}

/// E-T-01 (A01, rust-host): a command that writes a unique marker to stdout and
/// then exits non-zero. EX-v1 requires the exit code AND the stdout diagnostics
/// to reach the model with a failure receipt, and the persisted ToolCall to end
/// as a failure.
#[test]
fn real_eval_command_stdout_only_non_zero_exit_preserves_diagnostics() {
    let mut checks = CheckResults::default();
    let (db, root, conversation, run_id) = rust_host_run("stdout-only", "allow");
    // A marker only the real child process can produce; the expectation is
    // recomputed from it, never from the tool's own output.
    let marker = format!("fox-eval-stdout-{}", uuid::Uuid::new_v4().simple());
    #[cfg(windows)]
    let command = format!("echo {marker}>side-effect.txt& echo {marker}& exit /b 7");
    #[cfg(not(windows))]
    let command = format!("printf '{marker}' > side-effect.txt; printf '{marker}'; exit 7");
    let input = json!({ "command": command, "timeoutSeconds": 20 });
    let outcome = dispatch_command_with_test_backend(
        &db,
        &root,
        &run_id,
        &input,
        "cmd-stdout-only",
    );
    let receipt = model_receipt(&outcome);
    let blob = receipt.to_string();

    // 1. The executor really ran: the side-effect file exists and holds the
    //    marker. Independent of anything the tool returned.
    let side_effect = std::fs::read_to_string(root.join("side-effect.txt")).unwrap_or_default();
    checks.check(
        "et01:real-command-executed",
        side_effect.contains(&marker),
        format!("sideEffectFile={} bytes markerFound={}", side_effect.len(), side_effect.contains(&marker)),
    );
    // 2. A non-zero exit is reported as a failure, never as a success.
    let reported_failure = receipt_reports_failure(&receipt);
    checks.check(
        "et01:non-zero-exit-is-a-failure-receipt",
        reported_failure,
        format!("receipt={}", blob.chars().take(240).collect::<String>()),
    );
    // 3. The exit code reaches the model.
    let exit_code_present = blob.contains("\"exitCode\":7") || blob.contains("exited with code 7");
    checks.check(
        "et01:exit-code-reaches-the-model",
        exit_code_present,
        format!("exitCodeInReceipt={exit_code_present}"),
    );
    // 4. The stdout produced before the non-zero exit survives the failure.
    //    This is the A01 defect the case exists for: the current base drops it.
    let stdout_preserved = blob.contains(&marker);
    checks.check(
        "et01:stdout-diagnostics-preserved",
        stdout_preserved,
        if stdout_preserved {
            "the stdout marker reached the model".to_owned()
        } else {
            "BASELINE: the stdout produced before the non-zero exit was dropped from the failure receipt".to_owned()
        },
    );
    // 5. Persisted state: the Host ToolCall row ended as a failure with a reason.
    let row = db
        .get_runtime_tool_call(&run_id, "cmd-stdout-only")
        .expect("the persisted ToolCall row must be readable")
        .expect("the Host ToolCall row must exist");
    checks.check(
        "et01:persisted-tool-call-ended-as-a-failure",
        persisted_failure(&row),
        format!("status={} error={:?}", row.status, row.error_message),
    );
    // 6. Historical re-read: the stored outcome is reachable by reference under
    //    conversation authorization.
    let reference = format!("fox-result://{run_id}/cmd-stdout-only");
    let re_read = db.tool_result_range(&reference, &conversation, 0, 65_536);
    checks.check(
        "et01:stored-outcome-is-re-readable-by-reference",
        re_read.as_ref().is_ok_and(|range| {
            range.status == row.status && range.tool_name == "run_command"
        }),
        match &re_read {
            Ok(range) => format!("status={} tool={} bytes={}", range.status, range.tool_name, range.returned_bytes),
            Err(error) => format!("re-read failed: {error}"),
        },
    );

    record_rust_host_case("E-T-01", &["A01"], "run_command", &checks);
    let _ = std::fs::remove_dir_all(&root);

    // Only the measurement invariants are asserted here: the chain really ran,
    // really persisted, and really re-reads. The contract verdicts above are the
    // recorded evidence, and tasks.mjs carries the baseline expectation.
    assert!(side_effect.contains(&marker), "the real command never produced its side-effect file; the case measured nothing");
    assert!(persisted_failure(&row), "the Host ToolCall row was not persisted as a failure outcome");
    assert!(
        re_read.as_ref().is_ok_and(|range| range.tool_name == "run_command"),
        "the stored outcome could not be re-read by reference"
    );
}

/// E-T-02 (A03, rust-host): deleting text with a non-empty `oldText` and an
/// empty `newText` is a legal edit and must succeed; an empty `oldText` must
/// still be rejected.
#[test]
fn real_eval_edit_file_allows_deleting_matched_text_to_empty() {
    let mut checks = CheckResults::default();
    let (db, root, conversation, run_id) = rust_host_run("legal-deletion", "allow");
    std::fs::write(root.join("note.txt"), "keep this line\nremove this line\n").unwrap();

    // Positive case: a real deletion to empty.
    let input = json!({ "path": "note.txt", "oldText": "remove this line\n", "newText": "" });
    let outcome = dispatch_host_tool(&db, &root, &run_id, "edit_file", &input, "edit-delete");
    let receipt = model_receipt(&outcome);
    let accepted = outcome.is_ok() && !receipt_reports_failure(&receipt);
    let disk = std::fs::read_to_string(root.join("note.txt")).unwrap_or_default();
    checks.check(
        "et02:legal-deletion-is-accepted",
        accepted,
        format!("accepted={accepted} receipt={}", receipt.to_string().chars().take(200).collect::<String>()),
    );
    checks.check(
        "et02:deleted-text-is-gone-from-disk",
        disk == "keep this line\n",
        format!("disk={disk:?}"),
    );
    // Negative case: an empty oldText must be rejected — it is not a deletion.
    let bad = json!({ "path": "note.txt", "oldText": "", "newText": "anything" });
    let rejected = dispatch_host_tool(&db, &root, &run_id, "edit_file", &bad, "edit-empty-oldtext");
    let rejection_is_a_rejection = match &rejected {
        Err(error) => !error.contains("cannot be resolved") && !error.contains("is not a file"),
        Ok(value) => value.get("isError").and_then(Value::as_bool) == Some(true),
    };
    checks.check(
        "et02:empty-oldtext-is-rejected",
        rejection_is_a_rejection,
        match &rejected {
            Err(error) => format!("rejected with: {error}"),
            Ok(value) => format!("accepted (business failure): {}", value.to_string().chars().take(200).collect::<String>()),
        },
    );
    // The rejected call must not have mutated the file.
    let after_reject = std::fs::read_to_string(root.join("note.txt")).unwrap_or_default();
    checks.check(
        "et02:rejection-left-the-file-untouched",
        after_reject == disk,
        format!("diskBefore={disk:?} diskAfter={after_reject:?}"),
    );
    // Persisted state + historical re-read for the accepted edit.
    let row = db.get_runtime_tool_call(&run_id, "edit-delete");
    checks.check(
        "et02:accepted-edit-persisted-its-outcome",
        row.as_ref().is_ok_and(|row| {
            row.as_ref().is_some_and(|row| !persisted_failure(row) && row.status == "completed")
        }),
        match &row {
            Ok(Some(row)) => format!("status={} error={:?}", row.status, row.error_message),
            Ok(None) => "no ToolCall row was persisted".to_owned(),
            Err(error) => format!("the row could not be read: {error}"),
        },
    );
    let reference = format!("fox-result://{run_id}/edit-delete");
    let re_read = db.tool_result_range(&reference, &conversation, 0, 65_536);
    checks.check(
        "et02:accepted-edit-is-re-readable-by-reference",
        re_read.as_ref().is_ok_and(|range| range.tool_name == "edit_file"),
        match &re_read {
            Ok(range) => format!("status={} tool={} bytes={}", range.status, range.tool_name, range.returned_bytes),
            Err(error) => format!("re-read failed: {error}"),
        },
    );

    record_rust_host_case("E-T-02", &["A03"], "edit_file", &checks);
    let _ = std::fs::remove_dir_all(&root);

    // Measurement invariants only: the negative case was really exercised — an
    // empty oldText was either rejected with an identifiable reason or reported
    // as a business failure, never silently accepted as a successful edit.
    assert!(
        rejection_is_a_rejection,
        "an empty oldText was neither accepted nor rejected with an identifiable reason"
    );
}

/// E-T-05 (S01, rust-host): reading a path outside the frozen project root must
/// be denied with a reason distinguishable from an ordinary read error, and the
/// frozen root itself must never become the basis for escaping it.
#[test]
fn real_eval_read_outside_the_frozen_project_root_is_denied_and_re_readable() {
    let mut checks = CheckResults::default();
    let (db, root, conversation, run_id) = rust_host_run("out-of-root", "read_only");
    let allowed_content = "authorized content";
    let allowed_version = crate::tool_host::file_version(allowed_content.as_bytes());
    std::fs::write(root.join("allowed.txt"), allowed_content).unwrap();
    // A file outside the frozen root: a sibling directory, never a child.
    let outside_dir = root
        .parent()
        .unwrap()
        .join(format!("fox-eval-outside-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&outside_dir).unwrap();
    let outside_file = outside_dir.join("secret.txt");
    std::fs::write(&outside_file, "must never be read").unwrap();

    // Positive control: an in-root read executes, persists completed and is
    // re-readable. Without it a refusal below could be an infrastructure fault.
    let inside = dispatch_reader(&db, &run_id, "read", "allowed.txt", "read-inside");
    let inside_ok = inside.as_ref().is_ok_and(|value| {
        !receipt_reports_failure(value)
            && value["result"]["content"][0]["text"].as_str() == Some(allowed_content)
            && value["result"]["details"]["readVersion"].as_str()
                == Some(allowed_version.as_str())
    });
    checks.check(
        "et05:in-root-read-succeeds",
        inside_ok,
        match &inside {
            Ok(value) => format!("receipt={}", value.to_string().chars().take(200).collect::<String>()),
            Err(error) => format!("in-root read failed: {error}"),
        },
    );
    let inside_row = db
        .get_runtime_tool_call(&run_id, "read-inside")
        .expect("the in-root ToolCall row must exist");
    checks.check(
        "et05:in-root-read-persisted-completed",
        inside_row.as_ref().is_some_and(|row| row.status == "completed"),
        format!("status={:?}", inside_row.as_ref().map(|row| row.status.as_str())),
    );
    let inside_reference = format!("fox-result://{run_id}/read-inside");
    let inside_re_read = db.tool_result_range(&inside_reference, &conversation, 0, 65_536);
    let expected_readback = format!(
        "{allowed_content}\nreadVersion: {allowed_version}. Use this as expectedVersion for write_file/edit_file."
    );
    checks.check(
        "et05:in-root-read-is-re-readable-by-reference",
        inside_re_read.as_ref().is_ok_and(|range| range.content == expected_readback),
        match &inside_re_read {
            Ok(range) => format!("content={:?} bytes={}", range.content, range.returned_bytes),
            Err(error) => format!("re-read failed: {error}"),
        },
    );

    // The denial: an absolute path outside the frozen root.
    let denied = dispatch_reader(
        &db,
        &run_id,
        "read",
        outside_file.to_str().unwrap(),
        "read-outside",
    );
    let denial = denied.as_ref().err().is_some_and(|error| error.contains("outside"));
    checks.check(
        "et05:out-of-root-read-is-denied",
        denial,
        match &denied {
            Err(error) => format!("denied with: {error}"),
            Ok(value) => format!("ACCEPTED (leak): {}", value.to_string().chars().take(200).collect::<String>()),
        },
    );
    // The refusal must be distinguishable from an ordinary read failure: a file
    // that is missing *inside* the root resolves differently.
    let missing = dispatch_reader(&db, &run_id, "read", "does-not-exist.txt", "read-missing");
    let distinguishable = match (denied.as_ref().err(), missing.as_ref().err()) {
        (Some(denied_error), Some(missing_error)) => {
            denied_error.contains("outside") && !missing_error.contains("outside")
        }
        _ => false,
    };
    checks.check(
        "et05:denial-is-distinguishable-from-an-ordinary-read-error",
        distinguishable,
        format!(
            "denial={:?} missing={:?}",
            denied.as_ref().err().map(|error| error.chars().take(80).collect::<String>()),
            missing.as_ref().err().map(|error| error.chars().take(80).collect::<String>()),
        ),
    );
    // The frozen root must not become the basis for escaping it: a path built
    // FROM the root but leaving it is denied for the same reason.
    let escape_path = format!(
        "{root}{sep}..{sep}{dir}{sep}{file}",
        root = root.to_str().unwrap(),
        sep = std::path::MAIN_SEPARATOR,
        dir = outside_dir.file_name().unwrap().to_str().unwrap(),
        file = outside_file.file_name().unwrap().to_str().unwrap(),
    );
    let denied_from_root = dispatch_reader(&db, &run_id, "read", &escape_path, "read-escape");
    let denial_from_root = denied_from_root
        .as_ref()
        .err()
        .is_some_and(|error| error.contains("outside"));
    checks.check(
        "et05:frozen-root-is-not-a-basis-for-escaping-it",
        denial_from_root,
        match &denied_from_root {
            Err(error) => format!("denied with: {error}"),
            Ok(value) => format!("ACCEPTED (leak): {}", value.to_string().chars().take(200).collect::<String>()),
        },
    );
    // Persisted state: the denial is a failed ToolCall with the refusal reason,
    // reachable by reference — the model learns it was refused, not skipped.
    let row = db
        .get_runtime_tool_call(&run_id, "read-outside")
        .expect("the denied ToolCall row must exist")
        .expect("the denied ToolCall row must be persisted");
    checks.check(
        "et05:denial-persisted-as-a-failed-tool-call",
        row.status == "failed"
            && row.error_message.as_deref().is_some_and(|message| message.contains("outside")),
        format!("status={} error={:?}", row.status, row.error_message),
    );
    let reference = format!("fox-result://{run_id}/read-outside");
    let re_read = db.tool_result_range(&reference, &conversation, 0, 65_536);
    checks.check(
        "et05:denial-outcome-is-re-readable-by-reference",
        re_read.as_ref().is_ok_and(|range| range.status == "failed" && range.tool_name == "read"),
        match &re_read {
            Ok(range) => format!("status={} tool={} bytes={}", range.status, range.tool_name, range.returned_bytes),
            Err(error) => format!("re-read failed: {error}"),
        },
    );

    record_rust_host_case("E-T-05", &["S01"], "read", &checks);
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&outside_dir);

    // E-T-05 is satisfied by the current base, so every check is a gate.
    assert!(
        checks.ok(),
        "E-T-05 (out-of-root read denial) failed; see the evidence file and the printed checks"
    );
}

/// O-REVIEW-03 gate 2: a `completed` row carrying `isError=true` is NOT a
/// business failure, and must not be counted as one by `persisted_failure`.
///
/// CONTRACTS v1.2 §6 lets an ended business failure be returned as
/// `Ok(Value)` with a top-level `isError=true`, but requires Host to persist
/// `failed`. A mutant row that persists `completed` while claiming `isError`
/// therefore violates the contract, and any acceptance criterion that accepted
/// it would be silently tolerant of the exact regression the criterion exists
/// to catch. This mutant must fail the strict criterion — no widening of v1.2
/// to accommodate an old baseline is permitted.
#[test]
fn persisted_completed_is_error_mutant_must_not_be_a_failure() {
    let mutant = crate::database::ToolCallRecord {
        id: "mutant-row".to_owned(),
        runtime_tool_call_id: "mutant-call".to_owned(),
        run_id: "mutant-run".to_owned(),
        conversation_id: "mutant-conversation".to_owned(),
        tool_name: "run_command".to_owned(),
        input: json!({}),
        status: "completed".to_owned(),
        // The mutant: a business-failure receipt attached to a completed row.
        result: Some(json!({ "isError": true, "error": "command exited with code 7" })),
        error_message: Some("command exited with code 7".to_owned()),
        execution_location: "host".to_owned(),
        requires_approval: false,
        started_at: 0,
        completed_at: Some(0),
        updated_at: 0,
        trace_id: None,
        span_id: None,
    };
    assert!(
        !persisted_failure(&mutant),
        "a completed+isError mutant row must not read as a persisted failure"
    );

    // The strict criterion still recognises the real failure shape.
    let mut real = mutant.clone();
    real.status = "failed".to_owned();
    assert!(
        persisted_failure(&real),
        "a failed row must read as a persisted failure"
    );

    // Any other terminal status is neither.
    for status in ["running", "pending", "cancelled", "interrupted"] {
        let mut other = mutant.clone();
        other.status = status.to_owned();
        assert!(
            !persisted_failure(&other),
            "a {status} row must not read as a persisted failure"
        );
    }
}
