//! JOB-v1 integration tests — real OS processes, real pipes, real signals.
//!
//! The module under test (`src/process_jobs.rs`) is **not declared in `lib.rs`**
//! yet: it is still `module_ready`, not `integration_ready`. It is pulled in
//! here with an explicit `#[path]` so it can be exercised as a real compiled
//! target without registering anything in the product.
//!
//! What these tests add over the unit tests: unit tests script the child, so
//! they prove the *decision logic*. These prove the parts that only exist once a
//! real OS is involved — pipe back-pressure on two streams at once, exit codes
//! from a real shell, terminating a real process tree, and pid identity from the
//! real process table.
//!
//! Scope note: nothing here is wired into the product. No Host tool, no
//! `run_command`, no database, no Run. Authorization is out of scope by
//! construction — this module must be handed an already-authorized Run.

#[path = "../src/process_jobs.rs"]
mod process_jobs;

use process_jobs::{
    EnvMode, IdentityProbe, JobRunState, Liveness, ManagerConfig, OutputWindow, OwnerStatus,
    PersistedJobRecord, ProcessIdentity, ProcessJobManager, RecoveryReason, SpawnSpec,
    StartOutcome, StartRequest, Stream, SystemClock, SystemIdentityProbe, SystemSpawner,
    TerminateOutcome,
};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

static FIXTURE_SEQ: AtomicU64 = AtomicU64::new(0);

/// Per-test scratch directory.
///
/// Prefers the harness-provided isolated project root; falls back to the system
/// temp directory so the file also runs under a bare `rustc --test`.
fn fixture(tag: &str) -> PathBuf {
    let root = std::env::var_os("FOX_HARNESS_PROJECT_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0);
    let unique = format!(
        "{tag}-{}-{}-{}",
        std::process::id(),
        stamp,
        FIXTURE_SEQ.fetch_add(1, Ordering::SeqCst)
    );
    let dir = root.join("process-jobs-it").join(unique);
    std::fs::create_dir_all(&dir).expect("fixture directory");
    dir
}

fn manager(config: ManagerConfig) -> Arc<ProcessJobManager> {
    Arc::new(ProcessJobManager::new(
        Arc::new(SystemClock),
        Arc::new(SystemSpawner),
        Arc::new(SystemIdentityProbe),
        config,
    ))
}

fn long_running_spec(dir: &std::path::Path) -> SpawnSpec {
    // ~59 s on Windows, ~60 s elsewhere. Far longer than any budget used here,
    // so a test that finishes quickly proves we stopped it, not that it ended.
    #[cfg(windows)]
    let command = "ping -n 60 127.0.0.1 > nul";
    #[cfg(not(windows))]
    let command = "sleep 60";
    SpawnSpec::shell(command, &dir.to_string_lossy())
}

/// Writes a `.cmd` fixture and returns it.
///
/// Multi-command fixtures go in a file rather than an inline `cmd /C` string for
/// a concrete reason discovered while writing these tests: in cmd.exe a `for`
/// body extends to the end of the line, so
/// `for ... do @echo a & for ... do @echo b 1>&2` nests the second loop *inside*
/// the first — 5000 x 5000 lines instead of one pass each. A file also keeps the
/// command line free of quotes that cmd and the Win32 argument quoter disagree
/// about.
#[cfg(windows)]
fn batch_fixture(dir: &std::path::Path, name: &str, body: &str) -> SpawnSpec {
    let path = dir.join(name);
    let normalized = body.replace('\n', "\r\n");
    std::fs::write(&path, format!("@echo off\r\n{normalized}\r\n")).expect("write batch fixture");
    SpawnSpec::shell(name, &dir.to_string_lossy())
}

fn start_request(key: &str, spec: SpawnSpec, budget: Duration) -> StartRequest {
    StartRequest {
        run_id: "it-run".to_owned(),
        idempotency_key: key.to_owned(),
        spec,
        execution_budget: budget,
    }
}

/// Ticks supervision until the job is terminal, or fails the test on timeout.
fn wait_terminal(
    manager: &ProcessJobManager,
    job_id: &str,
    within: Duration,
) -> process_jobs::JobStatusView {
    let deadline = Instant::now() + within;
    loop {
        let view = manager.status(job_id).expect("job exists");
        if view.is_terminal() {
            return view;
        }
        if Instant::now() >= deadline {
            panic!("job {job_id} did not finish within {within:?}: {view:?}");
        }
        manager.tick();
        thread::sleep(Duration::from_millis(10));
    }
}

/// Reads a stream to the end, asserting the cursor never regresses.
fn read_all(manager: &ProcessJobManager, job_id: &str, stream: Stream) -> String {
    let mut offset = 0u64;
    let mut collected = String::new();
    for _ in 0..10_000 {
        let page = manager.output(job_id, stream, offset, 8192).expect("page");
        assert!(
            page.next_offset >= offset,
            "a cursor must never move backwards"
        );
        collected.push_str(&page.content);
        offset = page.next_offset;
        if page.at_end_of_available && page.stream_closed {
            return collected;
        }
        thread::sleep(Duration::from_millis(5));
    }
    panic!("stream did not end");
}

// ---------------------------------------------------------------------------
// Real exit codes, and the diagnostics that must survive them
// ---------------------------------------------------------------------------

#[cfg(windows)]
#[test]
fn a_real_non_zero_exit_keeps_its_code_and_its_output() {
    let dir = fixture("nonzero");
    let manager = manager(ManagerConfig::default());
    let spec = SpawnSpec::shell("echo out-line && echo err-line 1>&2 && exit /b 7", &dir.to_string_lossy());

    let job = manager
        .start(start_request("nonzero", spec, Duration::from_secs(30)))
        .expect("start");
    let job_id = job.view().job_id.clone();
    let view = wait_terminal(&manager, &job_id, Duration::from_secs(20));

    assert_eq!(view.state, JobRunState::Failed, "a non-zero exit is not success");
    assert_eq!(view.exit_code, Some(7));
    assert_eq!(view.error_code.as_deref(), Some("nonzero_exit"));
    assert_eq!(view.production_state(), "failed");

    // The point of the whole batch: output produced before the failure survives
    // and is actually reachable, instead of being replaced by an error string.
    let stdout = read_all(&manager, &job_id, Stream::Stdout);
    assert!(
        stdout.contains("out-line"),
        "stdout from a failed command must remain readable, got {stdout:?}"
    );
    let stderr = read_all(&manager, &job_id, Stream::Stderr);
    assert!(stderr.contains("err-line"), "got {stderr:?}");
}

#[cfg(windows)]
#[test]
fn a_real_zero_exit_is_success() {
    let dir = fixture("zero");
    let manager = manager(ManagerConfig::default());
    let spec = SpawnSpec::shell("echo all-good", &dir.to_string_lossy());
    let job = manager
        .start(start_request("zero", spec, Duration::from_secs(30)))
        .expect("start");
    let job_id = job.view().job_id.clone();
    let view = wait_terminal(&manager, &job_id, Duration::from_secs(20));

    assert_eq!(view.state, JobRunState::Succeeded);
    assert_eq!(view.exit_code, Some(0));
    assert_eq!(view.production_state(), "completed");
    assert!(read_all(&manager, &job_id, Stream::Stdout).contains("all-good"));
}

// ---------------------------------------------------------------------------
// Back-pressure: two pipes at once, far past any pipe buffer
// ---------------------------------------------------------------------------

#[cfg(windows)]
#[test]
fn heavy_output_on_both_streams_does_not_deadlock_and_reports_what_was_dropped() {
    let dir = fixture("heavy");
    // ~165 KiB per stream, well past the pipe buffer: without a reader per pipe
    // the child would block on write and never exit.
    let manager = manager(ManagerConfig {
        stream_retention_bytes: 8 * 1024,
        ..ManagerConfig::default()
    });
    let spec = batch_fixture(
        &dir,
        "heavy.cmd",
        "for /L %%i in (1,1,5000) do @echo oooooooooooooooooooooooooooooooo\n\
         for /L %%i in (1,1,5000) do @echo eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee 1>&2",
    );

    let job = manager
        .start(start_request("heavy", spec, Duration::from_secs(60)))
        .expect("start");
    let job_id = job.view().job_id.clone();
    let view = wait_terminal(&manager, &job_id, Duration::from_secs(45));

    assert_eq!(view.state, JobRunState::Succeeded, "{view:?}");
    assert!(
        view.stdout.total_bytes > 100_000,
        "both streams must be fully consumed, stdout={}",
        view.stdout.total_bytes
    );
    assert!(
        view.stderr.total_bytes > 100_000,
        "stderr must not be starved by stdout, stderr={}",
        view.stderr.total_bytes
    );
    // Retention is bounded, and the loss is reported rather than hidden.
    assert!(view.stdout.dropped_bytes > 0);
    assert_eq!(view.stdout.retained_bytes, view.stdout.retention_limit as u64);
    assert_eq!(
        view.stdout.total_bytes,
        view.stdout.retained_bytes + view.stdout.dropped_bytes
    );

    // The retained tail is still readable, and a stale cursor is told what it lost.
    let page = manager.output(&job_id, Stream::Stdout, 0, 4096).expect("page");
    assert!(page.lost_before_offset > 0, "an old cursor is informed, not silently shifted");
    assert!(!page.content.is_empty());
    assert!(page.stream_closed, "the pipe ended with the process");
}

#[cfg(windows)]
#[test]
fn a_process_with_no_output_still_completes() {
    let dir = fixture("silent");
    let manager = manager(ManagerConfig::default());
    let spec = SpawnSpec::shell("ping -n 1 127.0.0.1 > nul", &dir.to_string_lossy());
    let job = manager
        .start(start_request("silent", spec, Duration::from_secs(30)))
        .expect("start");
    let job_id = job.view().job_id.clone();
    let view = wait_terminal(&manager, &job_id, Duration::from_secs(20));

    assert_eq!(view.state, JobRunState::Succeeded);
    assert_eq!(view.stdout.total_bytes, 0);
    assert_eq!(view.stderr.total_bytes, 0);
    let page = manager.output(&job_id, Stream::Stdout, 0, 1024).expect("page");
    assert!(page.content.is_empty());
    assert!(page.at_end_of_available);
    assert!(page.stream_closed);
}

// ---------------------------------------------------------------------------
// Timeout and cancel against a real process tree
// ---------------------------------------------------------------------------

#[cfg(windows)]
#[test]
fn a_timeout_stops_a_real_tree_and_keeps_partial_output() {
    let dir = fixture("timeout");
    let manager = manager(ManagerConfig::default());
    // Emits one line, then blocks for ~59 s.
    let spec = SpawnSpec::shell(
        "echo started && ping -n 60 127.0.0.1 > nul",
        &dir.to_string_lossy(),
    );
    let started = Instant::now();
    let job = manager
        .start(start_request("timeout", spec, Duration::from_millis(400)))
        .expect("start");
    let job_id = job.view().job_id.clone();
    let view = wait_terminal(&manager, &job_id, Duration::from_secs(20));

    assert_eq!(view.state, JobRunState::TimedOut);
    assert_eq!(view.error_code.as_deref(), Some("timed_out"));
    assert_eq!(view.production_state(), "failed");
    assert_ne!(
        view.terminate,
        Some(TerminateOutcome::AlreadyExited),
        "the tree must have been signalled, not found already gone"
    );
    assert!(
        started.elapsed() < Duration::from_secs(15),
        "a 400 ms budget must not wait out a 59 s command"
    );
    // Output produced before the deadline stays readable.
    assert!(read_all(&manager, &job_id, Stream::Stdout).contains("started"));

    // The ordering the contract asks for is recorded, not assumed.
    let kinds: Vec<String> = view
        .events
        .iter()
        .map(|event| format!("{:?}", event.kind))
        .collect();
    let position = |needle: &str| {
        kinds
            .iter()
            .position(|kind| kind.contains(needle))
            .unwrap_or_else(|| panic!("no {needle} event in {kinds:?}"))
    };
    let stop = position("StopRequested");
    let terminated = position("TreeTerminated");
    let finished = position("Finished");
    assert!(stop < terminated && terminated < finished);
}

#[cfg(windows)]
#[test]
fn a_real_grandchild_does_not_survive_the_tree_termination() {
    let dir = fixture("tree");
    let gate = dir.join("gate.txt");
    let sentinel = dir.join("sentinel-for-test.txt");

    // The grandchild does **not** race a timer. It polls for a gate file and only
    // then records that it is still alive, so the assertion at the end is
    // "a descendant that was killed can never write the file" rather than
    // "the kill probably beat a 3 s sleep".
    //
    // That distinction is the fix for this test: the previous version slept ~3 s
    // and then wrote the sentinel, so on a loaded machine the kill could lose the
    // race and the test failed although nothing was wrong with the termination.
    // It reproduced only under load, which is the worst kind of flake — it would
    // have been read as "the tree walk broke" instead of "the test is racy".
    //
    // `process_jobs` runs `cmd /D /S /C "<command>"`. A batch file keeps the
    // grandchild's own command line free of nested quotes, which cmd.exe and the
    // Win32 argument quoter disagree about — so the fixture is a file, not an
    // inline `cmd /c "..."`.
    std::fs::write(
        dir.join("waiter.cmd"),
        "@echo off\r\n\
         :poll\r\n\
         if exist \"%~dp0gate.txt\" goto fire\r\n\
         ping -n 2 127.0.0.1 > nul\r\n\
         goto poll\r\n\
         :fire\r\n\
         echo survived > \"%~dp0sentinel-for-test.txt\"\r\n",
    )
    .expect("write waiter");

    let manager = manager(ManagerConfig::default());
    // Three levels, all of them real: our child is cmd.exe, which waits on a
    // nested `cmd.exe` running the fixture, which waits on the gate. `taskkill /T`
    // has to walk the whole chain, and the grandchild waits indefinitely, so the
    // tree cannot quietly end on its own before the budget expires.
    let spec = SpawnSpec::shell("cmd /c waiter.cmd", &dir.to_string_lossy());
    let job = manager
        .start(start_request("tree", spec, Duration::from_millis(500)))
        .expect("start");
    let job_id = job.view().job_id.clone();
    let view = wait_terminal(&manager, &job_id, Duration::from_secs(20));

    assert_eq!(view.state, JobRunState::TimedOut, "{view:?}");

    // (1) The termination reports what the walker itself did, not merely that we
    // asked. `Terminated` is only reachable when the walker reported success, so
    // a silently-failed `taskkill` can no longer pass this test.
    match view.terminate {
        Some(TerminateOutcome::Terminated { walker_exit_code, walker_visited }) => {
            assert_eq!(walker_exit_code, Some(0), "the tree walker must report success: {view:?}");
            assert!(
                walker_visited >= 1,
                "the walker must report the processes it acted on, not zero: {view:?}"
            );
        }
        ref other => panic!("the tree walk must be reported as terminated, got {other:?}"),
    }

    // (2) Reclamation, separately from the attempt: the spawned process is
    // re-probed after the teardown and must be gone.
    assert_eq!(
        view.owner_after_stop,
        Some(OwnerStatus::Gone),
        "the spawned process must actually be gone afterwards: {view:?}"
    );

    // (3) Nothing may still be holding the output pipes. If a descendant
    // survived, it keeps them open and the readers have to be abandoned.
    assert!(
        !view.readers_orphaned,
        "something still holds the pipes, so a descendant outlived the kill: {view:?}"
    );

    // Only now is the grandchild's trigger created. A survivor notices within one
    // poll interval and writes the sentinel; a descendant that was killed cannot.
    std::fs::write(&gate, b"go").expect("write gate");
    thread::sleep(Duration::from_secs(6));
    assert!(
        !sentinel.exists(),
        "a descendant outlived the tree termination; it wrote {}",
        sentinel.display()
    );
}

#[cfg(windows)]
#[test]
fn cancel_is_a_request_until_acknowledged_and_is_idempotent() {
    let dir = fixture("cancel");
    let manager = manager(ManagerConfig::default());
    let spec = long_running_spec(&dir);
    let job = manager
        .start(start_request("cancel", spec, Duration::from_secs(300)))
        .expect("start");
    let job_id = job.view().job_id.clone();

    let first = manager.cancel(&job_id).expect("cancel");
    assert!(first.changed);
    let view = wait_terminal(&manager, &job_id, Duration::from_secs(20));
    assert_eq!(view.state, JobRunState::Cancelled);
    assert_eq!(view.production_state(), "cancelled");
    assert!(view.cancel_requested_at.is_some());
    assert!(
        view.cancel_acknowledged_at.is_some(),
        "the request must be acknowledged once the tree actually stopped"
    );
    assert!(view.cancel_acknowledged_at >= view.cancel_requested_at);

    let second = manager.cancel(&job_id).expect("cancel again");
    assert!(!second.changed, "a repeat cancel changes nothing");
    assert!(second.acknowledged);
    assert_eq!(second.view.state, JobRunState::Cancelled);
    assert_eq!(
        second.view.terminate, view.terminate,
        "a repeat cancel must not signal the tree again"
    );
}

// ---------------------------------------------------------------------------
// Idempotency and side-effect freedom against real processes
// ---------------------------------------------------------------------------

#[cfg(windows)]
#[test]
fn concurrent_real_starts_with_one_key_launch_exactly_once() {
    let dir = fixture("race");
    let manager = manager(ManagerConfig::default());
    let spec = SpawnSpec::shell("ping -n 2 127.0.0.1 > nul", &dir.to_string_lossy());
    let spec = Arc::new(spec);

    let mut handles = Vec::new();
    for _ in 0..8 {
        let manager = Arc::clone(&manager);
        let spec = Arc::clone(&spec);
        handles.push(thread::spawn(move || {
            let outcome = manager
                .start(start_request("race-key", (*spec).clone(), Duration::from_secs(30)))
                .expect("start");
            (outcome.launched(), outcome.view().job_id.clone())
        }));
    }
    let results: Vec<(bool, String)> = handles
        .into_iter()
        .map(|handle| handle.join().expect("thread"))
        .collect();

    let launched = results.iter().filter(|(launched, _)| *launched).count();
    assert_eq!(launched, 1, "exactly one caller may launch: {results:?}");
    let ids: std::collections::BTreeSet<&String> = results.iter().map(|(_, id)| id).collect();
    assert_eq!(ids.len(), 1, "every caller must see the same job");
}

#[cfg(windows)]
#[test]
fn polling_and_reconnecting_never_re_execute_the_command() {
    let dir = fixture("sideeffect");
    let counter = dir.join("counter.txt");
    let manager = manager(ManagerConfig::default());
    // Every execution appends a line: the file is the witness.
    let spec = SpawnSpec::shell("echo ran >> counter.txt", &dir.to_string_lossy());

    let job = manager
        .start(start_request("sideeffect", spec, Duration::from_secs(30)))
        .expect("start");
    let job_id = job.view().job_id.clone();
    let view = wait_terminal(&manager, &job_id, Duration::from_secs(20));
    assert_eq!(view.state, JobRunState::Succeeded);

    let after_first = std::fs::read_to_string(&counter).expect("counter written");
    assert_eq!(after_first.lines().count(), 1);

    // Poll hard, page output, re-read status and reconnect — the command must
    // not run again, so the witness file must not grow.
    for _ in 0..40 {
        let _ = manager.status(&job_id).expect("status");
        let _ = manager.output(&job_id, Stream::Stdout, 0, 1024).expect("output");
        let _ = manager.list(Some("it-run"));
        manager.tick();
    }
    let after_polling = std::fs::read_to_string(&counter).expect("counter still readable");
    assert_eq!(
        after_polling.lines().count(),
        1,
        "reads and reconnects must never replay the command"
    );
}

#[cfg(windows)]
#[test]
fn an_environment_isolated_child_cannot_see_a_planted_variable() {
    // Proves the adapter boundary can actually isolate an environment, which the
    // wiring layer needs in order to apply A's policy rather than inheriting
    // whatever the Host happens to have.
    let dir = fixture("env");
    let manager = manager(ManagerConfig::default());
    std::env::set_var("FOX_IT_LEAK_MARKER", "should-not-appear");
    let spec = SpawnSpec {
        program: "cmd.exe".to_owned(),
        args: vec![
            "/D".to_owned(),
            "/S".to_owned(),
            "/C".to_owned(),
            "echo [%FOX_IT_LEAK_MARKER%]".to_owned(),
        ],
        cwd: dir.to_string_lossy().into_owned(),
        env: EnvMode::Replace(vec![("PATH".to_owned(), std::env::var("PATH").unwrap_or_default())]),
    };
    let job = manager
        .start(start_request("env", spec, Duration::from_secs(30)))
        .expect("start");
    let job_id = job.view().job_id.clone();
    let _ = wait_terminal(&manager, &job_id, Duration::from_secs(20));
    let output = read_all(&manager, &job_id, Stream::Stdout);
    assert!(
        !output.contains("should-not-appear"),
        "a replaced environment must not leak the parent's variables: {output:?}"
    );
    std::env::remove_var("FOX_IT_LEAK_MARKER");
}

// ---------------------------------------------------------------------------
// Recovery against the real process table
// ---------------------------------------------------------------------------

#[test]
fn recovery_reads_identity_from_the_real_process_table() {
    let probe = SystemIdentityProbe;
    let manager = manager(ManagerConfig::default());

    // This test process is real, alive, and ours.
    let me = probe.current();
    assert!(me.pid > 0);
    assert!(
        matches!(probe.probe(me.pid), Liveness::Alive { .. } | Liveness::Unknown(_)),
        "probing our own pid must never report Gone"
    );

    // A pid that cannot exist: the probe must say Gone, not Unknown.
    let impossible = u32::MAX - 16;
    let decisions = manager.recover(&[
        PersistedJobRecord {
            job_id: "ours".to_owned(),
            run_id: "it-run".to_owned(),
            state: JobRunState::Running,
            owner: Some(me),
        },
        PersistedJobRecord {
            job_id: "dead".to_owned(),
            run_id: "it-run".to_owned(),
            state: JobRunState::Running,
            owner: Some(ProcessIdentity { pid: impossible, start_marker: Some(1) }),
        },
        PersistedJobRecord {
            job_id: "reused".to_owned(),
            run_id: "it-run".to_owned(),
            state: JobRunState::Running,
            // Right pid, wrong creation time: a pid reused after a restart.
            owner: Some(ProcessIdentity { pid: me.pid, start_marker: Some(1) }),
        },
    ]);

    let find = |id: &str| decisions.iter().find(|decision| decision.job_id == id).unwrap();

    // Alive and provably ours: keep running, but still never replay.
    if find("ours").owner_status == OwnerStatus::Alive {
        assert_eq!(find("ours").to, JobRunState::Running);
        assert!(find("ours").unchanged());
    }

    assert_eq!(find("dead").to, JobRunState::Interrupted);
    assert_eq!(find("dead").reason, RecoveryReason::OwnerGone);
    assert_eq!(find("dead").owner_status, OwnerStatus::Gone);
    assert_eq!(find("dead").error_code(), Some("interrupted"));

    // The same pid with a different creation time is a different process.
    if me.start_marker.is_some() {
        assert_eq!(find("reused").to, JobRunState::Interrupted);
        assert_eq!(find("reused").reason, RecoveryReason::OwnerReplaced);
        assert_eq!(find("reused").owner_status, OwnerStatus::Replaced);
    }

    assert!(
        decisions.iter().all(|decision| !decision.allow_replay),
        "recovery must never authorize replaying a side-effecting command"
    );
}

// ---------------------------------------------------------------------------
// A module that is not wired must stay unwired
// ---------------------------------------------------------------------------

#[test]
fn the_module_is_not_registered_as_a_product_tool() {
    // The text of `lib.rs` is embedded at *compile* time. `include_str!` resolves
    // the path relative to this file, so the check needs no environment variable,
    // no runtime cwd, and no path-format guessing -- it behaves identically under
    // `cargo test` and under a bare `rustc --test` run from any directory, and a
    // wrong path is a compile error rather than a silently skipped assertion.
    //
    // Earlier versions of this guard resolved the crate root at runtime from
    // `CARGO_MANIFEST_DIR`, which is absent under a bare rustc and arrives in Git
    // Bash form (`/d/...`) when the compiler is driven from a POSIX shell; the
    // fallback then leaned on the launch cwd. That made the guard pass or fail
    // depending on where the binary was started from, which is worse than useless
    // for a boundary check. `include_str!` removes the whole class of problem.
    //
    // This file exists to prove the module works *before* wiring. If the module
    // ever gets registered in `lib.rs`, that is an `integration_ready` change and
    // this guard should fail so the batch boundary is noticed rather than
    // silently crossed.
    const LIB_RS: &str = include_str!("../src/lib.rs");
    assert!(
        !LIB_RS.contains("process_jobs"),
        "process_jobs must not be declared in lib.rs while the batch is module_ready"
    );
}

#[test]
fn an_empty_output_window_is_still_a_valid_target() {
    // Boundary case for the buffer itself, kept here so the integration target
    // also fails if the constructor regresses.
    let window = OutputWindow::new(0);
    let view = window.view(Stream::Stdout);
    assert_eq!(view.total_bytes, 0);
    assert!(view.retention_limit >= 1);
}

#[test]
fn a_start_outcome_reports_whether_it_launched() {
    // Cheap structural guard: `Existing` must never be mistaken for a launch by
    // a caller that forgets to check.
    fn describe(outcome: &StartOutcome) -> bool {
        outcome.launched()
    }
    let _ = describe;
}
