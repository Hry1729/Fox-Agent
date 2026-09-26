//! C4: the same two real QuickJS Jobs through the real Host/Node/Pi path,
//! comparing a frozen typed-notice Run with a frozen legacy polling Run.
//! The polling policy is one predetermined pair of status(waitMs=5000) calls.
use super::*;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Arc, Condvar, Mutex,
};
use std::time::{Duration, Instant};

const CHILD: &str = "FOX_TEST_C4_COMPARISON_CHILD";
const SETTLE_DELAY: Duration = Duration::from_millis(600);

#[test]
fn c4_real_host_notice_vs_fixed_polling() {
    let Some(mode) = std::env::var(CHILD).ok() else {
        for mode in ["live-on", "live-off", "round-on", "round-off"] {
            let mut child = std::process::Command::new(std::env::current_exe().unwrap())
                .arg("c4_real_host_notice_vs_fixed_polling")
                .arg("--nocapture")
                .arg("--test-threads=1")
                .env(CHILD, mode)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .unwrap();
            let deadline = Instant::now() + Duration::from_secs(90);
            while child.try_wait().unwrap().is_none() {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!("C4 {mode} exceeded 90 seconds");
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            let output = child.wait_with_output().unwrap();
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            println!("C4 child {mode}:\n{stdout}");
            assert!(
                output.status.success(),
                "C4 {mode} failed:\n{stdout}\n{stderr}"
            );
        }
        return;
    };
    let live = mode.starts_with("live-");
    let notice = mode.ends_with("-on");
    assert!(matches!(
        mode.as_str(),
        "live-on" | "live-off" | "round-on" | "round-off"
    ));
    run_case(&mode, live, notice);
}

struct CompletionGate {
    released: Mutex<bool>,
    cv: Condvar,
    entered: mpsc::Sender<String>,
}

impl CompletionGate {
    fn state(&self) -> std::sync::MutexGuard<'_, bool> {
        match self.released.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
    fn wait_at_settlement(&self, job: &str) {
        self.entered.send(job.to_owned()).unwrap();
        let mut released = self.state();
        let deadline = Instant::now() + Duration::from_secs(35);
        while !*released {
            let (next, _) = match self.cv.wait_timeout(released, Duration::from_millis(50)) {
                Ok(waited) => waited,
                Err(poisoned) => poisoned.into_inner(),
            };
            released = next;
            assert!(Instant::now() < deadline, "C4 Job {job} was not released");
        }
    }
    fn release(&self) {
        *self.state() = true;
        self.cv.notify_all();
    }
}

struct CompletionOwner {
    gate: Arc<CompletionGate>,
    registration: Option<crate::runtime_host::attachment_compute::jobs::test_hooks::SettleGate>,
}
impl Drop for CompletionOwner {
    fn drop(&mut self) {
        self.gate.release();
        drop(self.registration.take());
    }
}

#[derive(Debug)]
struct ProviderRequest {
    body: Value,
    received_at: i64,
}

/// A local HTTP Provider. Request #2 is the sole planned legacy poll; its Job
/// IDs come from the formal result already visible in that request.
fn provider(
    notice: bool,
    done: Arc<AtomicBool>,
) -> (
    std::net::SocketAddr,
    std::thread::JoinHandle<Vec<ProviderRequest>>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();
    let server = std::thread::spawn(move || {
        let mut requests = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(55);
        loop {
            if requests.len() == 3 || (done.load(Ordering::SeqCst) && !requests.is_empty()) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "C4 Provider did not receive its expected requests"
            );
            let mut stream = match listener.accept() {
                Ok((stream, _)) => stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(10));
                    continue;
                }
                Err(error) => panic!("C4 Provider accept: {error}"),
            };
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            stream
                .set_write_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            let mut bytes = Vec::new();
            let mut buffer = [0u8; 8192];
            let request_deadline = Instant::now() + Duration::from_secs(12);
            let (header_end, length) = loop {
                assert!(
                    Instant::now() < request_deadline,
                    "C4 request headers exceeded deadline"
                );
                let size = stream.read(&mut buffer).unwrap();
                assert!(size > 0, "C4 request closed before headers");
                bytes.extend_from_slice(&buffer[..size]);
                assert!(bytes.len() < 1_048_576);
                if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&bytes[..end]).to_ascii_lowercase();
                    let length = headers
                        .lines()
                        .find_map(|line| line.strip_prefix("content-length:"))
                        .unwrap()
                        .trim()
                        .parse::<usize>()
                        .unwrap();
                    assert!(length < 1_048_576, "C4 request body is oversized");
                    break (end + 4, length);
                }
            };
            while bytes.len() < header_end + length {
                assert!(
                    Instant::now() < request_deadline,
                    "C4 request body exceeded deadline"
                );
                let size = stream.read(&mut buffer).unwrap();
                assert!(size > 0, "C4 request closed before body");
                bytes.extend_from_slice(&buffer[..size]);
                assert!(bytes.len() < 1_048_576, "C4 request exceeded byte cap");
            }
            let body: Value =
                serde_json::from_slice(&bytes[header_end..header_end + length]).unwrap();
            let ordinal = requests.len();
            let delta = match ordinal {
                0 => {
                    let code = "function onChunk(c){} function onFinish(){return {answer:42};}";
                    let args = |key: &str| {
                        json!({"idempotencyKey":key,
                        "params":{"processing":"chunked","code":code}})
                        .to_string()
                    };
                    json!({"role":"assistant","tool_calls":[
                        {"index":0,"id":"c4-start-a","type":"function",
                            "function":{"name":"compute_job_start","arguments":args("c4-a")}},
                        {"index":1,"id":"c4-start-b","type":"function",
                            "function":{"name":"compute_job_start","arguments":args("c4-b")}}
                    ]})
                }
                1 if notice => {
                    let count = body["messages"]
                        .to_string()
                        .matches("FOX_HOST_JOB_NOTICE_V1")
                        .count();
                    json!({"role":"assistant","content":if count == 2 {
                        "Both compute results are ready: answer 42."
                    } else { "Wait for both Host Job notices." }})
                }
                1 => {
                    let text = body["messages"].to_string();
                    let ids = job_ids_from_model_input(&text);
                    assert_eq!(
                        ids.len(),
                        2,
                        "the first formal batch must show both Job IDs: {text}"
                    );
                    json!({"role":"assistant","tool_calls":[
                        {"index":0,"id":"c4-poll-a","type":"function",
                            "function":{"name":"compute_job_status","arguments":json!({"jobId":ids[0],"waitMs":5000}).to_string()}},
                        {"index":1,"id":"c4-poll-b","type":"function",
                            "function":{"name":"compute_job_status","arguments":json!({"jobId":ids[1],"waitMs":5000}).to_string()}}
                    ]})
                }
                2 => {
                    json!({"role":"assistant","content":"Both compute results are ready: answer 42."})
                }
                _ => unreachable!(),
            };
            requests.push(ProviderRequest {
                body,
                received_at: crate::database::now_ms(),
            });
            let finish = if delta["tool_calls"].is_array() {
                "tool_calls"
            } else {
                "stop"
            };
            let chunks = [
                json!({"id":format!("c4-{ordinal}"),"object":"chat.completion.chunk","created":1,
                    "model":"kernel-http-test","choices":[{"index":0,"delta":delta,"finish_reason":null}]}),
                json!({"id":format!("c4-{ordinal}"),"object":"chat.completion.chunk","created":1,
                    "model":"kernel-http-test","choices":[{"index":0,"delta":{},"finish_reason":finish}],
                    "usage":{"prompt_tokens":20,"completion_tokens":5,"total_tokens":25}}),
            ];
            let response = format!(
                "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
                chunks[0], chunks[1]
            );
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", response.len(), response).unwrap();
        }
        requests
    });
    (address, server)
}

fn job_ids_from_model_input(text: &str) -> Vec<String> {
    let mut ids = Vec::new();
    let marker = "job:";
    for (index, _) in text.match_indices(marker) {
        let tail = &text[index..];
        let end = tail
            .find(|ch: char| !(ch.is_ascii_alphanumeric() || ":_-".contains(ch)))
            .unwrap_or(tail.len());
        let id = &tail[..end];
        if !ids.iter().any(|seen| seen == id) {
            ids.push(id.to_owned());
        }
    }
    ids
}

fn run_case(mode: &str, live: bool, notice: bool) {
    let done = Arc::new(AtomicBool::new(false));
    let (address, server) = provider(notice, Arc::clone(&done));
    let mut config = worker_configuration();
    config.model_service = json!({"apiType":"openai-completions","modelId":"kernel-http-test",
        "baseUrl":format!("http://{address}/v1")});
    config.proposal_tools = ["compute_job_start", "compute_job_status"]
        .into_iter()
        .map(|name| {
            json!({"name":name,"description":"Local C4 Job operation",
            "parameters":{"type":"object","additionalProperties":true}})
        })
        .collect();
    let clock = crate::runtime_host::shadow_reconcile::ReconcilerClock;
    let (db, root, run) = fixture_with_budgets_notice_opt(
        &clock,
        &config.hash().unwrap(),
        Some(&config),
        true,
        true,
        (0, 1),
        true,
        TimeBudgets::default(),
        notice,
    );
    assert_eq!(db.compute_job_notice_enabled(&run).unwrap(), notice);
    db.freeze_kernel_host_scope(
        &run,
        &crate::database::KernelHostScope {
            schema_version: 1,
            tool_names: ["compute_job_start".into(), "compute_job_status".into()]
                .into_iter()
                .collect(),
            mcp_server_hashes: Default::default(),
            knowledge_reference_hashes: Default::default(),
            knowledge_connection_hashes: Default::default(),
            office_tools: Default::default(),
            lifecycle_hooks: vec![],
        },
    )
    .unwrap();
    std::fs::create_dir_all(root.join("attachments")).unwrap();
    std::fs::create_dir_all(root.join("skills")).unwrap();
    let mut context = tauri::generate_context!();
    for window in &mut context.config_mut().app.windows {
        window.create = false;
    }
    let app = tauri::Builder::default()
        .any_thread()
        .build(context)
        .unwrap();
    let host = crate::runtime_host::RuntimeHost::new(
        app.handle().clone(),
        db.clone(),
        root.clone(),
        root.join("attachments"),
        root.join("skills"),
        crate::yuxi::YuxiClient::new().unwrap(),
    );
    if !live {
        host.force_auto_wake_round_for_test(&run);
    }
    let binding = db.run_control_binding(&run).unwrap().unwrap();
    let (entered_tx, entered_rx) = mpsc::channel();
    let gate = Arc::new(CompletionGate {
        released: Mutex::new(false),
        cv: Condvar::new(),
        entered: entered_tx,
    });
    let hook_gate = Arc::clone(&gate);
    let registration =
        crate::runtime_host::attachment_compute::jobs::test_hooks::gate_at_settle_for_run(
            &run,
            Arc::new(move |job| hook_gate.wait_at_settlement(job)),
        );
    let owner = CompletionOwner {
        gate: Arc::clone(&gate),
        registration: Some(registration),
    };
    // The release clock begins when *both* real executors reach their terminal
    // write. Neither flag mode nor a model/poll request can trigger release.
    let release_thread = std::thread::spawn(move || {
        let first = entered_rx.recv_timeout(Duration::from_secs(30)).unwrap();
        let second = entered_rx.recv_timeout(Duration::from_secs(30)).unwrap();
        assert_ne!(first, second);
        std::thread::sleep(SETTLE_DELAY);
        gate.release();
        crate::database::now_ms()
    });
    let wall_start = crate::database::now_ms();
    let instant_start = Instant::now();
    let ownership = crate::runtime_host::kernel_host::acquire(&root, &run).unwrap();
    let result = if live {
        host.start_kernel_run(ownership, &binding, Value::Null, Value::Null)
    } else {
        host.start_kernel_run_forced_round_for_test(ownership, &binding, Value::Null, Value::Null)
    };
    let release_wall = release_thread.join().unwrap();
    result.unwrap();
    let deadline = Instant::now() + Duration::from_secs(35);
    let final_state = loop {
        let state = db.kernel_host_run_state(&run).unwrap().unwrap();
        if matches!(state.as_str(), "completed" | "failed" | "cancelled") {
            break state;
        }
        assert!(Instant::now() < deadline, "C4 {mode} stayed {state}");
        std::thread::sleep(Duration::from_millis(20));
    };
    let elapsed_ms = instant_start.elapsed().as_millis();
    done.store(true, Ordering::SeqCst);
    drop(owner);
    let requests = server.join().unwrap();
    let jobs = db.kernel_jobs_for_run(&run).unwrap();
    let (run_terminal, last_job_finished): (Option<i64>, Option<i64>) = db
        .with_connection(|conn| {
            conn.query_row(
                "SELECT (SELECT terminal_at FROM kernel_runs WHERE run_id=?1),
                (SELECT MAX(finished_at) FROM kernel_jobs WHERE run_id=?1)",
                [&run],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
        })
        .unwrap();
    let tool_calls: Vec<(String, String, String, Option<String>)> = db.with_connection(|conn| {
        let mut query = conn.prepare("SELECT tool_call_id,tool,state,result_json FROM kernel_tool_calls WHERE run_id=?1 ORDER BY created_at,tool_call_id")?;
        let rows = query.query_map([&run], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }).unwrap();
    let event_counts: Vec<(String, Option<String>, i64)> = db.with_connection(|conn| {
        let mut query = conn.prepare("SELECT event_type,json_extract(payload_json,'$.lane'),COUNT(*)
            FROM kernel_events WHERE run_id=?1 AND
            (event_type LIKE 'engine.%' OR event_type LIKE 'run.jobs_%' OR event_type='run.waiting_jobs')
            GROUP BY event_type,json_extract(payload_json,'$.lane') ORDER BY event_type")?;
        let rows = query.query_map([&run], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }).unwrap();
    let formal_results: Vec<(String, i64)> = db
        .with_connection(|conn| {
            let mut query = conn.prepare(
                "SELECT json_extract(payload_json,'$.toolCallId'),COUNT(*)
            FROM kernel_events WHERE run_id=?1 AND event_type='tool.completed'
            GROUP BY json_extract(payload_json,'$.toolCallId') ORDER BY 1",
            )?;
            let rows = query
                .query_map([&run], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
        .unwrap();
    let usage: Vec<(Option<i64>, Option<i64>, String, Option<f64>, i64)> = db
        .with_connection(|conn| {
            let mut query = conn.prepare(
                "SELECT json_extract(event_json,'$.record.usage.input'),
            json_extract(event_json,'$.record.usage.output'),
            json_extract(event_json,'$.record.usage.completeness'),
            json_extract(event_json,'$.record.cost.knownCost'),
            json_extract(event_json,'$.record.cost.costComplete') FROM run_events
            WHERE run_id=?1 AND event_type='usage.request' ORDER BY seq",
            )?;
            let rows = query
                .query_map([&run], |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
        .unwrap();
    let wire_notices: Vec<usize> = requests
        .iter()
        .map(|request| {
            request.body["messages"]
                .to_string()
                .matches("FOX_HOST_JOB_NOTICE_V1")
                .count()
        })
        .collect();
    let (durable_notices, delivered_notices): (i64, i64) = db
        .with_connection(|conn| {
            conn.query_row(
                "SELECT (SELECT COUNT(*) FROM kernel_job_notices WHERE run_id=?1),
                (SELECT COUNT(*) FROM kernel_job_notice_deliveries d JOIN kernel_job_notices n
                 ON n.job_id=d.job_id WHERE n.run_id=?1 AND d.state='acknowledged')",
                [&run],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
        })
        .unwrap();
    let request_after_finish = last_job_finished.and_then(|finished| {
        requests
            .iter()
            .find(|request| request.received_at >= finished)
            .map(|request| request.received_at - finished)
    });
    let tool_summary: Vec<_> = tool_calls
        .iter()
        .map(|(id, tool, state, _)| (id, tool, state))
        .collect();
    println!("C4_RESULT mode={mode} frozen_notice={notice} poll_policy=one_pair_waitMs5000 \
        requests={} request_notice_markers={wire_notices:?} tools={tool_summary:?} formal={formal_results:?} events={event_counts:?} \
        jobs={:?} notices={durable_notices}/{delivered_notices} state={final_state} \
        usage_records={usage:?} wall_start={wall_start} gate_release={release_wall} \
        last_job_finished={last_job_finished:?} run_terminal={run_terminal:?} \
        start_to_terminal_observed_ms={elapsed_ms} last_job_to_next_provider_request_ms={request_after_finish:?} \
        last_job_to_run_terminal_ms={:?}",
        requests.len(), jobs.iter().map(|job| (&job.state, job.attempts)).collect::<Vec<_>>(),
        run_terminal.zip(last_job_finished).map(|(terminal, finished)| terminal - finished));
    assert_eq!(jobs.len(), 2);
    assert!(jobs
        .iter()
        .all(|job| job.state.as_str() == "completed" && job.attempts == 1));
    for job in &jobs {
        assert!(
            job.result_ref.is_some() && job.result_sha256.is_some() && job.result_bytes.is_some(),
            "C4 {mode} Job {} lacks a durable result identity",
            job.job_id
        );
        let result = db
            .kernel_job_result_value(&binding.conversation_id, &job.job_id)
            .unwrap();
        assert_eq!(
            result["result"]["answer"], 42,
            "C4 {mode} Job {} produced {result}",
            job.job_id
        );
    }
    assert_eq!(
        final_state, "completed",
        "C4 {mode} did not reach a comparable completion"
    );
    assert_eq!(
        requests.len(),
        if notice && wire_notices.get(1) == Some(&2) {
            2
        } else {
            3
        },
        "C4 {mode} took an unplanned model round"
    );
    let starts = tool_calls
        .iter()
        .filter(|(_, tool, _, _)| tool == "compute_job_start")
        .count();
    let polls = tool_calls
        .iter()
        .filter(|(_, tool, _, _)| tool == "compute_job_status")
        .count();
    assert_eq!((starts, polls), (2, if notice { 0 } else { 2 }));
    assert!(tool_calls
        .iter()
        .all(|(_, _, state, _)| state == "completed"));
    let mut expected_formal: Vec<String> =
        tool_calls.iter().map(|(id, _, _, _)| id.clone()).collect();
    expected_formal.sort();
    let actual_formal: Vec<String> = formal_results.iter().map(|(id, _)| id.clone()).collect();
    assert_eq!(
        actual_formal, expected_formal,
        "formal tool.completed IDs differ from settled tool calls"
    );
    assert!(
        formal_results.iter().all(|(_, count)| *count == 1),
        "each toolCallId must have exactly one formal result"
    );
    let mut status_job_ids = Vec::new();
    for (_, tool, _, result) in &tool_calls {
        if tool == "compute_job_status" {
            let envelope: Value =
                serde_json::from_str(result.as_deref().expect("missing status result")).unwrap();
            let body: Value = serde_json::from_str(
                envelope["content"][0]["text"]
                    .as_str()
                    .expect("status result has no JSON text"),
            )
            .unwrap();
            assert_eq!(
                body["state"], "completed",
                "fixed poll returned nonterminal: {body}"
            );
            assert_eq!(body["attempts"], 1);
            status_job_ids.push(body["jobId"].as_str().unwrap().to_owned());
        }
    }
    status_job_ids.sort();
    let mut expected_jobs: Vec<String> = jobs.iter().map(|job| job.job_id.clone()).collect();
    expected_jobs.sort();
    if !notice {
        assert_eq!(
            status_job_ids, expected_jobs,
            "poll must cover both real Jobs once"
        );
    }
    assert_eq!(
        wire_notices.iter().sum::<usize>(),
        if notice { 2 } else { 0 }
    );
    assert_eq!(*wire_notices.last().unwrap(), if notice { 2 } else { 0 });
    assert_eq!(
        (durable_notices, delivered_notices),
        if notice { (2, 2) } else { (0, 0) }
    );
    assert_eq!(
        usage.len(),
        requests.len(),
        "every observed Provider request needs one durable usage record"
    );
    assert!(
        usage
            .iter()
            .all(|(input, output, complete, known_cost, cost_complete)| *input == Some(20)
                && *output == Some(5)
                && complete == "partial"
                && known_cost.is_none()
                && *cost_complete == 0),
        "simulated input/output usage was not durably recorded with partial completeness and unknown cost"
    );
    drop(host);
    drop(app);
}
