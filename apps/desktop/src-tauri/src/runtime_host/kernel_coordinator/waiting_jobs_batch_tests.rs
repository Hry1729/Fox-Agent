//! Two real compute Jobs in one WaitingJobs park. The typed notice lane must
//! deliver both terminal facts through exactly one automatic continuation once
//! every Job of the Run has settled, and must not deliver an early one while a
//! sibling is still running.
//!
//! Every ordering decision uses a bounded barrier on the real settlement path;
//! no assertion here infers order from a sleep.
use super::*;
use std::collections::{HashMap, HashSet};
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::time::{Duration, Instant};
use tauri::Manager;

const ORDER_ENV: &str = "FOX_TEST_BATCH_NOTICE_ORDER_CHILD";
const TRANSPORT_ENV: &str = "FOX_TEST_BATCH_NOTICE_TRANSPORT_CHILD";

#[test]
fn real_host_merges_two_compute_job_notices_into_one_continuation() {
    let selected = std::env::var(ORDER_ENV).ok();
    let Some(order) = selected else {
        for transport in ["live", "round"] {
            for order in ["a_first", "b_first"] {
                let mode = format!("{transport}-{order}");
                let mut child = std::process::Command::new(std::env::current_exe().unwrap())
                    .arg("real_host_merges_two_compute_job_notices_into_one_continuation")
                    .arg("--test-threads=1")
                    .env(TRANSPORT_ENV, transport)
                    .env(ORDER_ENV, order)
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped())
                    .spawn().unwrap();
                let deadline = Instant::now() + Duration::from_secs(90);
                while child.try_wait().unwrap().is_none() {
                    if Instant::now() >= deadline {
                        let _ = child.kill();
                        let _ = child.wait();
                        panic!("{mode} two-Job notice batch exceeded 90 seconds");
                    }
                    std::thread::sleep(Duration::from_millis(25));
                }
                let output = child.wait_with_output().unwrap();
                assert!(output.status.success(), "{mode} stdout: {}\n{mode} stderr: {}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr));
            }
        }
        return;
    };
    let transport = std::env::var(TRANSPORT_ENV).unwrap();
    assert!(matches!(transport.as_str(), "live" | "round"), "unknown transport {transport}");
    assert!(matches!(order.as_str(), "a_first" | "b_first"), "unknown order {order}");
    run_two_job_notice_batch(transport == "live", order == "a_first");
}

/// Holds every compute Job of one Run at its settlement boundary until the test
/// releases that exact Job. Keyed by Run, removed by its RAII guard, and
/// abandoned (every waiter released) if the guard drops during a panic.
struct BatchGate {
    state: Mutex<GateState>,
    cv: Condvar,
    entered: mpsc::Sender<String>,
}

#[derive(Default)]
struct GateState {
    held: HashSet<String>,
    released: HashSet<String>,
    abandoned: bool,
}

impl BatchGate {
    fn install(run_id: &str) -> (Arc<Self>, mpsc::Receiver<String>,
        crate::runtime_host::attachment_compute::jobs::test_hooks::SettleGate) {
        let (entered, entered_rx) = mpsc::channel();
        let gate = Arc::new(Self {
            state: Mutex::new(GateState::default()), cv: Condvar::new(), entered,
        });
        let hook_gate = Arc::clone(&gate);
        let guard = crate::runtime_host::attachment_compute::jobs::test_hooks::gate_at_settle_for_run(
            run_id, Arc::new(move |job: &str| hook_gate.hold(job)));
        (gate, entered_rx, guard)
    }

    /// Runs on the real execution thread, after the work returned and before
    /// the terminal decision.
    fn hold(&self, job: &str) {
        let mut state = self.state.lock().unwrap();
        assert!(!job.is_empty(), "the settlement gate was reached without a Job identity");
        assert!(state.held.insert(job.to_owned()), "Job {job} reached the settlement gate twice");
        let _ = self.entered.send(job.to_owned());
        let deadline = Instant::now() + Duration::from_secs(60);
        while !state.released.contains(job) && !state.abandoned {
            let (guard, timeout) = self.cv.wait_timeout(state, Duration::from_millis(50)).unwrap();
            state = guard;
            assert!(!(timeout.timed_out() && Instant::now() >= deadline),
                "Job {job} was never released from its bounded settlement gate");
        }
    }

    fn await_held(&self, entered: &mpsc::Receiver<String>, expected: usize) -> Vec<String> {
        let mut held = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(30);
        while held.len() < expected {
            let remaining = deadline.saturating_duration_since(Instant::now());
            assert!(!remaining.is_zero(),
                "only {} of {expected} Jobs reached their settlement gate", held.len());
            held.push(entered.recv_timeout(remaining)
                .expect("a compute Job never reached its settlement gate"));
        }
        held
    }

    fn release(&self, job: &str) {
        let mut state = self.state.lock().unwrap();
        assert!(state.held.contains(job), "Job {job} is not held at its settlement gate");
        state.released.insert(job.to_owned());
        self.cv.notify_all();
    }
}

impl Drop for BatchGate {
    fn drop(&mut self) {
        if let Ok(mut state) = self.state.lock() {
            state.abandoned = true;
            self.cv.notify_all();
        }
    }
}

/// One gated automatic wake decision. The hook runs inside
/// `wake_kernel_waiting_run_owned` after that call acquired the Run OS lock and
/// confirmed the Run is still parked, so blocking there proves the check was
/// really in flight while the assertions below run.
struct WakeGate {
    entered: mpsc::Receiver<()>,
    release: mpsc::Sender<()>,
}

fn arm_wake_entry(run_id: &str) -> WakeGate {
    let (entered_tx, entered) = mpsc::channel();
    let (release, release_rx) = mpsc::channel();
    let release_rx = Arc::new(Mutex::new(release_rx));
    crate::runtime_host::kernel_host::waiting_wake_test_hooks::set_before_wake_account(
        run_id, Arc::new(move || {
            entered_tx.send(()).unwrap();
            release_rx.lock().unwrap().recv_timeout(Duration::from_secs(30))
                .expect("the gated automatic wake check was never released");
        }));
    WakeGate { entered, release }
}

impl WakeGate {
    fn await_entered(&self, what: &str) {
        self.entered.recv_timeout(Duration::from_secs(20))
            .unwrap_or_else(|_| panic!("{what}: the automatic wake check never started"));
    }

    /// The checker holds the Run OS lock for its whole decision, so "the lock
    /// became free again" is a confirmable proof that the check returned, not
    /// merely that nothing had been observed yet.
    fn await_decided(&self, root: &std::path::Path, run_id: &str, what: &str) {
        assert!(matches!(crate::runtime_host::kernel_host::acquire(root, run_id),
            Err(error) if error == crate::runtime_host::kernel_run_lock::KERNEL_RUN_ALREADY_OWNED),
            "{what}: the in-flight check did not hold the Run OS lock");
        self.release.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            match crate::runtime_host::kernel_host::acquire(root, run_id) {
                Ok(ownership) => { drop(ownership); return; }
                Err(error) if error == crate::runtime_host::kernel_run_lock::KERNEL_RUN_ALREADY_OWNED => {}
                Err(error) => panic!("{what}: Run lock acquisition failed: {error}"),
            }
            assert!(Instant::now() < deadline, "{what}: the wake check never returned");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

/// No committed wake intent means no continuation model request: the only
/// dispatcher of a `job_notice` continuation is the outbox effect the wake
/// decision writes together with `run.jobs_woken`.
fn assert_wake_declined(db: &Database, run: &str, what: &str) {
    let (state, wakes, intents, parks): (String, i64, i64, i64) = db.with_connection(|conn| {
        conn.query_row(
            "SELECT (SELECT state FROM kernel_runs WHERE run_id=?1),
                    (SELECT COUNT(*) FROM kernel_events WHERE run_id=?1 AND event_type='run.jobs_woken'),
                    (SELECT COUNT(*) FROM kernel_events WHERE run_id=?1
                       AND event_type='engine.continuation_requested'
                       AND json_extract(payload_json,'$.lane')='job_notice'),
                    (SELECT COUNT(*) FROM kernel_events WHERE run_id=?1 AND event_type='run.waiting_jobs')",
            [run], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))
    }).unwrap();
    assert_eq!(state, "waiting_jobs", "{what} left the park");
    assert_eq!(parks, 1, "{what} changed the park generation");
    assert_eq!(wakes, 0, "{what} woke the Run before every Job of the Run had settled");
    assert_eq!(intents, 0, "{what} armed a job_notice continuation early");
}

/// Every typed notice the Provider actually received, in request order.
fn delivered_notices(value: &Value) -> Vec<fox_engine_protocol::HostJobNotice> {
    fn walk(value: &Value, out: &mut Vec<fox_engine_protocol::HostJobNotice>) {
        match value {
            Value::String(text) => {
                for part in text.split("FOX_HOST_JOB_NOTICE_V1").skip(1) {
                    // The rendered block is the marker line, a human-readable
                    // untrusted-data sentence, then the compact notice JSON on
                    // the final line.
                    println!("host job notice wire block: {part:?}");
                    let body = part.trim_end();
                    let json = body.rfind("\n{").map_or(body, |index| &body[index + 1..]);
                    out.push(serde_json::from_str(json).unwrap_or_else(|error| {
                        panic!("the model-visible Host notice is malformed ({error}): {part:?}")
                    }));
                }
            }
            Value::Array(items) => items.iter().for_each(|item| walk(item, out)),
            Value::Object(fields) => fields.values().for_each(|field| walk(field, out)),
            _ => {}
        }
    }
    let mut notices = Vec::new();
    walk(value, &mut notices);
    notices
}

fn job_key_map(db: &Database, run: &str) -> HashMap<String, String> {
    db.with_connection(|conn| {
        let mut statement = conn.prepare(
            "SELECT idempotency_key,job_id FROM kernel_jobs WHERE run_id=?1")?;
        let rows = statement.query_map([run], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?.collect::<rusqlite::Result<Vec<(String, String)>>>()?;
        Ok(rows)
    }).unwrap().into_iter().collect()
}

fn run_two_job_notice_batch(live: bool, a_first: bool) {
    let code = "function onChunk(c){} function onFinish(){return {answer:42};}";
    let arguments_a = json!({"idempotencyKey":"batch-notice-a",
        "params":{"processing":"chunked","code":code}}).to_string();
    let arguments_b = json!({"idempotencyKey":"batch-notice-b",
        "params":{"processing":"chunked","code":code}}).to_string();
    // Three Provider requests: the initial tool round, the tool-batch round that
    // parks, and the single notice continuation.
    let (address, server) = start_http_model_fixture(vec![
        json!({"role":"assistant","tool_calls":[
            {"index":0,"id":"job-start-a","type":"function",
                "function":{"name":"compute_job_start","arguments":arguments_a}},
            {"index":1,"id":"job-start-b","type":"function",
                "function":{"name":"compute_job_start","arguments":arguments_b}}]}),
        json!({"role":"assistant","content":"Both compute Jobs are still running."}),
        json!({"role":"assistant","content":"I received both Host Job notices."}),
    ]);
    let mut config = worker_configuration();
    config.model_service = json!({"apiType":"openai-completions","modelId":"kernel-http-test",
        "baseUrl":format!("http://{address}/v1")});
    config.proposal_tools = vec![json!({"name":"compute_job_start",
        "description":"Start a bounded background compute Job",
        "parameters":{"type":"object","properties":{"idempotencyKey":{"type":"string"},
            "params":{"type":"object"}},"required":["idempotencyKey","params"]}})];
    let clock = crate::runtime_host::shadow_reconcile::ReconcilerClock;
    let (db, root, run) = fixture_with_retry_opt(&clock, &config.hash().unwrap(),
        Some(&config), true, true, (0, 1));
    super::host_job_notice_tests::enable_notices(&db, &run);
    db.freeze_kernel_host_scope(&run, &crate::database::KernelHostScope {
        schema_version: 1,
        tool_names: ["compute_job_start".into()].into_iter().collect(),
        mcp_server_hashes: Default::default(),
        knowledge_reference_hashes: Default::default(),
        knowledge_connection_hashes: Default::default(),
        office_tools: Default::default(),
        lifecycle_hooks: vec![],
    }).unwrap();
    std::fs::create_dir_all(root.join("attachments")).unwrap();
    std::fs::create_dir_all(root.join("skills")).unwrap();
    let mut context = tauri::generate_context!();
    for window in &mut context.config_mut().app.windows { window.create = false; }
    let app = tauri::Builder::default().any_thread().build(context).unwrap();
    let host = crate::runtime_host::RuntimeHost::new(app.handle().clone(), db.clone(), root.clone(),
        root.join("attachments"), root.join("skills"), crate::yuxi::YuxiClient::new().unwrap());
    if !live { host.force_auto_wake_round_for_test(&run); }
    let binding = db.run_control_binding(&run).unwrap().unwrap();
    let parent_tokens: Vec<_> = {
        let state = host.state.lock().unwrap();
        state.cancellation.register_run(&run).unwrap();
        ["job-start-a", "job-start-b"].into_iter()
            .map(|id| state.cancellation.tool_token(&run, id).unwrap())
            .collect()
    };

    // Gate every settlement of this Run, then drive the real Host. The park
    // happens while both Jobs are held at their settlement boundary.
    let (gate, entered, settle_guard) = BatchGate::install(&run);
    let park_entry = arm_wake_entry(&run);
    let ownership = crate::runtime_host::kernel_host::acquire(&root, &run).unwrap();
    if live {
        host.start_kernel_run(ownership, &binding, Value::Null, Value::Null).unwrap();
    } else {
        host.start_kernel_run_forced_round_for_test(ownership, &binding, Value::Null, Value::Null)
            .unwrap();
    }
    assert_eq!(db.kernel_host_run_state(&run).unwrap().as_deref(), Some("waiting_jobs"),
        "the Run must park while both Jobs are still running");

    let held = gate.await_held(&entered, 2);
    let jobs = db.kernel_jobs_for_run(&run).unwrap();
    assert_eq!(jobs.len(), 2, "two formal job_start results must create exactly two Jobs");
    for job in &jobs {
        assert_eq!(job.attempts, 1, "a Job attempt was replayed at start");
        assert_eq!(job.state.as_str(), "running", "a Job settled before the test released it");
    }
    for token in &parent_tokens {
        assert!(!token.is_cancelled(), "the park retired a child Job token");
    }
    let mut held_sorted = held.clone();
    held_sorted.sort();
    let mut run_jobs: Vec<String> = jobs.iter().map(|job| job.job_id.clone()).collect();
    run_jobs.sort();
    assert_eq!(held_sorted, run_jobs, "the gate must hold exactly this Run's two Jobs");

    let key_map = job_key_map(&db, &run);
    let job_a = key_map["batch-notice-a"].clone();
    let job_b = key_map["batch-notice-b"].clone();
    let (first_job, second_job) = if a_first { (job_a.clone(), job_b.clone()) }
        else { (job_b.clone(), job_a.clone()) };

    // Read a scoped durable notice for one Job, bounded, without naming types.
    let await_notice = |job: &str| {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if let Some(notice) = db.kernel_job_notice(&binding.conversation_id, &run, job).unwrap() {
                return notice;
            }
            assert!(Instant::now() < deadline,
                "the settled Job never committed its scoped durable notice: {job}");
            std::thread::sleep(Duration::from_millis(10));
        }
    };

    // Check 1: the park-time automatic attempt, with both Jobs still running.
    park_entry.await_entered("park-time wake check");
    assert_wake_declined(&db, &run, "park-time wake check");
    park_entry.await_decided(&root, &run, "park-time wake check");
    assert_wake_declined(&db, &run, "park-time wake check (completed)");

    // Check 2: exactly one Job terminal, its sibling still running.
    let sibling_entry = arm_wake_entry(&run);
    gate.release(&first_job);
    let first_notice = await_notice(&first_job);
    sibling_entry.await_entered("one-settled wake check");
    let jobs = db.kernel_jobs_for_run(&run).unwrap();
    let settled = jobs.iter().find(|job| job.job_id == first_job).unwrap();
    let running = jobs.iter().find(|job| job.job_id == second_job).unwrap();
    assert_eq!(settled.state.as_str(), "completed",
        "the released Job must have committed its real terminal state");
    assert_eq!(running.state.as_str(), "running",
        "the sibling Job settled before the wake check ran");
    assert_wake_declined(&db, &run, "one-settled wake check");
    sibling_entry.await_decided(&root, &run, "one-settled wake check");
    assert_wake_declined(&db, &run, "one-settled wake check (completed)");
    let accounted_max: i64 = db.with_connection(|conn| conn.query_row(
        "SELECT MAX(json_extract(payload_json,'$.throughWallMs')) FROM kernel_events
           WHERE run_id=?1 AND event_type='run.jobs_wait_accounted'",
        [&run], |row| row.get(0))).unwrap();
    assert!(accounted_max >= first_notice.finished_at,
        "no wake accounting ran after the first Job published its terminal fact");

    // The sibling settles: the same watcher must now wake once and carry both.
    gate.release(&second_job);
    let deadline = Instant::now() + Duration::from_secs(40);
    loop {
        let state = db.kernel_host_run_state(&run).unwrap();
        if state.as_deref() == Some("completed") { break; }
        assert!(Instant::now() < deadline,
            "the second Job terminal fact did not finish the Run: state={state:?}");
        std::thread::sleep(Duration::from_millis(20));
    }
    drop(settle_guard);

    let second_notice = db.kernel_job_notice(&binding.conversation_id, &run, &second_job)
        .unwrap().expect("the second Job committed no scoped notice");
    let jobs = db.kernel_jobs_for_run(&run).unwrap();
    assert_eq!(jobs.len(), 2);
    for job in &jobs {
        assert_eq!(job.attempts, 1, "a Job attempt was replayed");
        assert_eq!(job.state.as_str(), "completed");
    }
    for token in &parent_tokens {
        assert!(token.is_cancelled(), "the terminal Run kept a child Job token");
    }

    let requests = server.join().unwrap();
    // One notice continuation is not "the whole Run called the model once":
    // the initial tool round and the parked batch round are real requests too.
    assert_eq!(requests.len(), 3,
        "two Jobs in one park need exactly one automatic continuation");
    assert_eq!(requests[1]["messages"].to_string().matches("FOX_HOST_JOB_NOTICE_V1").count(), 0,
        "the parked round delivered a notice before every Job settled");
    assert_eq!(requests[2]["messages"].to_string().matches("FOX_HOST_JOB_NOTICE_V1").count(), 2,
        "the single notice continuation must carry both typed notices");
    let delivered = delivered_notices(&requests[2]["messages"]);
    assert_eq!(delivered.len(), 2, "the Provider must receive exactly two typed notices");

    let mut expected_order = vec![
        (first_notice.finished_at, first_notice.job_id.clone()),
        (second_notice.finished_at, second_notice.job_id.clone()),
    ];
    expected_order.sort();
    let mut observed_order: Vec<(i64, String)> = delivered.iter()
        .map(|notice| (notice.finished_at, notice.job_id.clone())).collect();
    let mut observed_set = observed_order.clone();
    observed_set.sort();
    assert_eq!(observed_set, expected_order, "the batch must carry both Job notices exactly once");
    observed_order.sort();
    assert_eq!(observed_order, expected_order,
        "the batch must follow the durable finished_at/job_id order");
    for wire in &delivered {
        let durable = if wire.job_id == first_notice.job_id { &first_notice } else { &second_notice };
        assert_eq!(wire.run_id, run.as_str());
        assert_eq!(wire.conversation_id, binding.conversation_id.as_str());
        assert_eq!(wire.attempt, durable.attempt);
        assert_eq!(wire.terminal_state, durable.terminal_state.as_str());
        assert_eq!(wire.finished_at, durable.finished_at);
        assert_eq!(wire.error_code, durable.error_code);
        assert_eq!(wire.result_ref, durable.result_ref);
        assert_eq!(wire.result_sha256, durable.result_sha256);
        assert_eq!(wire.result_bytes, durable.result_bytes);
    }

    let (notices, deliveries, acknowledged, dispatch_keys, positions): (i64, i64, i64, i64, i64) =
        db.with_connection(|conn| conn.query_row(
            "SELECT (SELECT COUNT(*) FROM kernel_job_notices WHERE run_id=?1),
                    (SELECT COUNT(*) FROM kernel_job_notice_deliveries d
                       JOIN kernel_job_notices n ON n.job_id=d.job_id WHERE n.run_id=?1),
                    (SELECT COUNT(*) FROM kernel_job_notice_deliveries d
                       JOIN kernel_job_notices n ON n.job_id=d.job_id
                       WHERE n.run_id=?1 AND d.state='acknowledged'),
                    (SELECT COUNT(DISTINCT d.dispatch_key) FROM kernel_job_notice_deliveries d
                       JOIN kernel_job_notices n ON n.job_id=d.job_id WHERE n.run_id=?1),
                    (SELECT MAX(d.history_position)-MIN(d.history_position) FROM kernel_job_notice_deliveries d
                       JOIN kernel_job_notices n ON n.job_id=d.job_id WHERE n.run_id=?1)",
            [&run], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)))
        ).unwrap();
    assert_eq!((notices, deliveries, acknowledged, dispatch_keys, positions), (2, 2, 2, 1, 1),
        "both notices must be delivered and acknowledged once in a single batch");
    // Every model dispatch of the Run gets its own notice-input row (zero
    // notices included), so the batch is identified by the shared dispatch key
    // of its two delivery rows rather than by the Run's row count.
    let (bound_inputs, acknowledged_inputs): (i64, i64) = db.with_connection(|conn| conn.query_row(
        "SELECT (SELECT COUNT(*) FROM kernel_model_notice_inputs WHERE run_id=?1),
                (SELECT COUNT(*) FROM kernel_model_notice_inputs WHERE run_id=?1 AND state='acknowledged')",
        [&run], |row| Ok((row.get(0)?, row.get(1)?)))).unwrap();
    assert!(bound_inputs >= 1 && bound_inputs == acknowledged_inputs,
        "no model notice input may be left bound but unacknowledged ({bound_inputs}/{acknowledged_inputs})");
    let batch_key: String = db.with_connection(|conn| conn.query_row(
        "SELECT d.dispatch_key FROM kernel_job_notice_deliveries d
           JOIN kernel_job_notices n ON n.job_id=d.job_id WHERE n.run_id=?1 GROUP BY d.dispatch_key",
        [&run], |row| row.get(0))).unwrap();
    let batch_inputs: i64 = db.with_connection(|conn| conn.query_row(
        "SELECT COUNT(*) FROM kernel_model_notice_inputs
          WHERE run_id=?1 AND dispatch_key=?2 AND state='acknowledged'",
        rusqlite::params![&run, &batch_key], |row| row.get(0))).unwrap();
    assert_eq!(batch_inputs, 1, "the notice batch must bind exactly one acknowledged model input");

    let (formal_a, formal_b, wakes, intents, continuations, parks): (i64, i64, i64, i64, i64, i64) =
        db.with_connection(|conn| conn.query_row(
            "SELECT (SELECT COUNT(*) FROM kernel_events WHERE run_id=?1 AND event_type='tool.completed'
                       AND json_extract(payload_json,'$.toolCallId')='job-start-a'),
                    (SELECT COUNT(*) FROM kernel_events WHERE run_id=?1 AND event_type='tool.completed'
                       AND json_extract(payload_json,'$.toolCallId')='job-start-b'),
                    (SELECT COUNT(*) FROM kernel_events WHERE run_id=?1 AND event_type='run.jobs_woken'),
                    (SELECT COUNT(*) FROM kernel_events WHERE run_id=?1
                       AND event_type='engine.continuation_requested'
                       AND json_extract(payload_json,'$.lane')='job_notice'),
                    (SELECT COUNT(*) FROM kernel_events WHERE run_id=?1
                       AND event_type='engine.continuation_response'),
                    (SELECT COUNT(*) FROM kernel_events WHERE run_id=?1 AND event_type='run.waiting_jobs')",
            [&run], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?)))
        ).unwrap();
    assert_eq!((formal_a, formal_b), (1, 1),
        "each job_start must have exactly one formal tool result");
    assert_eq!((parks, wakes, intents, continuations), (1, 1, 1, 1),
        "one park, one successful wake, one notice continuation and one continuation response");
    assert_eq!(db.kernel_build_full_snapshot(&run).unwrap().state, "completed");
    assert_eq!(
        crate::runtime_host::kernel_host::waiting_wake_test_hooks::take_continuation_transports_for_test(&run),
        vec![if live { "live" } else { "per_round" }],
        "the automatic continuation must use the requested real Pi transport",
    );
    // Per-scenario durable summary: one line per live/per-round ordering run.
    let (initial_responses, batch_responses): (i64, i64) = db.with_connection(|conn| conn.query_row(
        "SELECT (SELECT COUNT(*) FROM kernel_events WHERE run_id=?1 AND event_type='engine.initial_response'),
                (SELECT COUNT(*) FROM kernel_events WHERE run_id=?1 AND event_type='engine.batch_response')",
        [&run], |row| Ok((row.get(0)?, row.get(1)?)))).unwrap();
    println!("two-job notice batch transport={} order={} run={run} first={first_job} second={second_job} \
requests={} marker_counts={:?} delivered_order={:?} attempts={:?} states={:?} \
notices={notices} deliveries={deliveries} acknowledged={acknowledged} batch_dispatch_keys={dispatch_keys} \
batch_model_inputs={batch_inputs} run_model_inputs={bound_inputs} \
formal=({formal_a},{formal_b}) parks={parks} wakes={wakes} notice_intents={intents} continuations={continuations} \
response_kinds={:?}",
        if live { "live" } else { "per_round" }, if a_first { "a_first" } else { "b_first" },
        requests.len(),
        requests.iter().map(|request| request["messages"].to_string()
            .matches("FOX_HOST_JOB_NOTICE_V1").count()).collect::<Vec<_>>(),
        delivered.iter().map(|notice| (notice.job_id.clone(), notice.finished_at)).collect::<Vec<_>>(),
        jobs.iter().map(|job| job.attempts).collect::<Vec<_>>(),
        jobs.iter().map(|job| job.state.as_str()).collect::<Vec<_>>(),
        vec![("initial", initial_responses), ("batch", batch_responses),
            ("continuation", continuations)],
    );
    drop(host);
    drop(app);
}
