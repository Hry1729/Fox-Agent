use super::*;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::time::{Duration, Instant};

// Real HTTP SSE provider scripted per model request. Each request is answered
// with either a plain stop ("stop") or a tool-call proposal ("tool").
fn delivery_provider(
    script: Vec<&'static str>,
) -> (std::net::SocketAddr, std::thread::JoinHandle<Vec<Value>>) {
    delivery_provider_with_ids(script.into_iter().map(|reply| (reply, "read-once")).collect())
}

/// Same provider, with an explicit tool-call id per round. Durable tool
/// identity is per call, so a script that proposes tools in several rounds must
/// propose a different call each time.
fn delivery_provider_with_ids(
    script: Vec<(&'static str, &'static str)>,
) -> (std::net::SocketAddr, std::thread::JoinHandle<Vec<Value>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();
    let server = std::thread::spawn(move || {
        let mut requests = Vec::new();
        for (reply, tool_call_id) in script {
            let deadline = Instant::now() + Duration::from_secs(25);
            let (mut stream, request) = loop {
                let mut stream = match listener.accept() {
                    Ok((stream, _)) => stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < deadline, "missing model request for {reply}");
                        std::thread::sleep(Duration::from_millis(10));
                        continue;
                    }
                    Err(error) => panic!("{error}"),
                };
                stream.set_nonblocking(false).unwrap();
                stream.set_read_timeout(Some(Duration::from_millis(300))).unwrap();
                stream.set_write_timeout(Some(Duration::from_secs(2))).unwrap();
                let mut bytes = Vec::new();
                let request = loop {
                    let mut buffer = [0u8; 8192];
                    let n = match stream.read(&mut buffer) { Ok(n) if n > 0 => n, _ => break None };
                    bytes.extend_from_slice(&buffer[..n]);
                    assert!(bytes.len() < 1_048_576);
                    if let Some(end) = bytes.windows(4).position(|p| p == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&bytes[..end]).to_ascii_lowercase();
                        let length = headers
                            .lines()
                            .find_map(|line| line.strip_prefix("content-length:"))
                            .unwrap()
                            .trim()
                            .parse::<usize>()
                            .unwrap();
                        if bytes.len() >= end + 4 + length {
                            break Some(
                                serde_json::from_slice::<Value>(&bytes[end + 4..end + 4 + length])
                                    .unwrap(),
                            );
                        }
                    }
                };
                // Undici may open a replacement idle socket while the worker
                // advances; it is not another model request.
                if let Some(request) = request {
                    break (stream, request);
                }
                assert!(Instant::now() < deadline, "only abandoned sockets for {reply}");
            };
            requests.push(request);
            note_stub_request(address);
            // "drop" answers nothing and closes: the worker sees a transport
            // failure, so this round goes through the Host's model-retry lane
            // before the next scripted reply is served.
            if reply == "drop" {
                drop(stream);
                continue;
            }
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n"
            )
            .unwrap();
            let send = |stream: &mut std::net::TcpStream, delta: Value, finish: Value| {
                let body = format!(
                    "data: {}\n\n",
                    json!({"id":"delivery-live","object":"chat.completion.chunk","created":1,"model":"delivery-live",
                        "choices":[{"index":0,"delta":delta,"finish_reason":finish}]})
                );
                write!(stream, "{:x}\r\n{}\r\n", body.len(), body)
            };
            if reply == "tool" {
                send(
                    &mut stream,
                    json!({"role":"assistant","tool_calls":[{"index":0,"id":tool_call_id,"type":"function",
                        "function":{"name":"read","arguments":"{\"path\":\"proof.txt\"}"}}]}),
                    Value::Null,
                )
                .unwrap();
                send(&mut stream, json!({}), json!("tool_calls")).unwrap();
            } else {
                send(
                    &mut stream,
                    json!({"role":"assistant","content":"我这就给出最终答复。"}),
                    Value::Null,
                )
                .unwrap();
                send(&mut stream, json!({}), json!("stop")).unwrap();
            }
            let done = "data: [DONE]\n\n";
            write!(stream, "{:x}\r\n{}\r\n0\r\n\r\n", done.len(), done).unwrap();
        }
        requests
    });
    (address, server)
}

// ---------------------------------------------------------------------------
// Failure diagnostics for the live delivery drives.
//
// These helpers only *read* state after the drive has ended and format it into
// assertion messages; they never change what a test asserts. They exist because
// `a_crash_while_finalizing_the_nth_item_leaves_recoverable_state` failed on one
// Windows checkout with nothing but `left: 0, right: 1`, and the drive result
// was discarded (`let _ = drive`).
// ---------------------------------------------------------------------------

/// Model requests the scripted stub has received, keyed by its (unique) bound
/// address so parallel tests never read each other's counters.
fn stub_request_counts() -> &'static std::sync::Mutex<std::collections::HashMap<std::net::SocketAddr, usize>> {
    static COUNTS: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<std::net::SocketAddr, usize>>,
    > = std::sync::OnceLock::new();
    COUNTS.get_or_init(Default::default)
}

fn note_stub_request(address: std::net::SocketAddr) {
    if let Ok(mut counts) = stub_request_counts().lock() {
        *counts.entry(address).or_default() += 1;
    }
}

fn stub_requests_seen(address: std::net::SocketAddr) -> usize {
    stub_request_counts()
        .lock()
        .map(|counts| counts.get(&address).copied().unwrap_or(0))
        .unwrap_or(0)
}

fn panic_text(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| payload.downcast_ref::<&str>().map(|text| (*text).to_owned()))
        .unwrap_or_else(|| "<non-string panic payload>".to_owned())
}

/// Consume the stub's join handle the way `drop(server)` did (a still-running
/// stub stays detached), but report what it saw: how many model requests
/// arrived and whether the stub thread itself panicked (e.g. "missing model
/// request" after its 25 s accept deadline).
fn stub_report(
    address: std::net::SocketAddr,
    scripted: usize,
    server: std::thread::JoinHandle<Vec<Value>>,
) -> String {
    let seen = stub_requests_seen(address);
    if server.is_finished() {
        match server.join() {
            Ok(requests) => format!(
                "model stub {address}: finished, {} of {scripted} scripted request(s) received",
                requests.len()
            ),
            Err(payload) => format!(
                "model stub {address}: thread PANICKED after {seen} of {scripted} request(s): {}",
                panic_text(payload.as_ref())
            ),
        }
    } else {
        format!(
            "model stub {address}: still waiting (detached) after {seen} of {scripted} scripted request(s)"
        )
    }
}

fn describe_drive<T>(drive: &Result<T, String>) -> String {
    match drive {
        Ok(_) => "Ok(())".to_owned(),
        Err(error) => format!("Err({error})"),
    }
}

fn describe_caught<T>(outcome: &std::thread::Result<Result<T, String>>) -> String {
    match outcome {
        Ok(drive) => format!("returned without panicking: {}", describe_drive(drive)),
        Err(payload) => format!("panicked: {}", panic_text(payload.as_ref())),
    }
}

/// `finalize_fault`/`stage_fault` are thread-local. Read them on the test
/// thread **before** `disarm`; `counted` is how many guarded updates ran on this
/// thread, so `counted == 0` means the guarded write never ran here (not reached,
/// or reached on another thread where the fault is not armed).
fn describe_fault(name: &str, observed: (Option<usize>, usize)) -> String {
    let (limit, counted) = observed;
    let fired = limit.is_some_and(|limit| counted > limit);
    format!(
        "{name}: armed={limit:?} counted_on_test_thread={counted} fired={fired} (test thread {:?})",
        std::thread::current().id()
    )
}

fn truncate_chars(text: &str, limit: usize) -> String {
    let count = text.chars().count();
    if count <= limit {
        text.to_owned()
    } else {
        format!("{}…(+{} chars)", text.chars().take(limit).collect::<String>(), count - limit)
    }
}

fn diag_rows(db: &Database, sql: &str, run_id: &str) -> String {
    let rows = db.with_connection(|connection| {
        let mut statement = connection.prepare(sql)?;
        let columns = statement.column_count();
        let rows = statement
            .query_map([run_id], |row| {
                let mut cells = Vec::with_capacity(columns);
                for index in 0..columns {
                    let cell = match row.get_ref(index)? {
                        rusqlite::types::ValueRef::Null => "NULL".to_owned(),
                        rusqlite::types::ValueRef::Integer(value) => value.to_string(),
                        rusqlite::types::ValueRef::Real(value) => value.to_string(),
                        rusqlite::types::ValueRef::Text(value) => {
                            truncate_chars(&String::from_utf8_lossy(value), 400)
                        }
                        rusqlite::types::ValueRef::Blob(value) => format!("<blob {} bytes>", value.len()),
                    };
                    cells.push(cell);
                }
                Ok(cells.join(" | "))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    });
    match rows {
        Ok(rows) if rows.is_empty() => "    (none)".to_owned(),
        Ok(rows) => rows.iter().map(|row| format!("    {row}")).collect::<Vec<_>>().join("\n"),
        Err(error) => format!("    <query failed: {error}>"),
    }
}

/// Everything known about a finished live drive, for an assertion message.
fn drive_diagnostics(
    db: &Database,
    root: &Path,
    run_id: &str,
    drive: &str,
    elapsed: Duration,
    stub: &str,
    faults: &[String],
) -> String {
    let worker = real_worker_command();
    let script = worker.script.clone().unwrap_or_default();
    let runtime_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../services/agent-runtime");
    let node_version = std::process::Command::new(&worker.program)
        .arg("--version")
        .output()
        .map(|output| {
            format!(
                "status={} stdout={} stderr={}",
                output.status,
                String::from_utf8_lossy(&output.stdout).trim(),
                truncate_chars(String::from_utf8_lossy(&output.stderr).trim(), 300)
            )
        })
        .unwrap_or_else(|error| format!("<spawn failed: {error}>"));
    let root_text = root.to_string_lossy();
    let canonical = std::fs::canonicalize(root)
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(|error| format!("<canonicalize failed: {error}>"));
    let staged = db
        .staged_delivery_rounds()
        .map(|rounds| format!("{rounds:?}"))
        .unwrap_or_else(|error| format!("<failed: {error}>"));
    let mut out = String::new();
    out.push_str("\n===== live delivery drive diagnostics =====\n");
    out.push_str(&format!("drive result: {drive}\n"));
    out.push_str(&format!("drive elapsed: {} ms\n", elapsed.as_millis()));
    out.push_str(&format!("{stub}\n"));
    for fault in faults {
        out.push_str(&format!("{fault}\n"));
    }
    out.push_str("--- environment (checkout-dependent) ---\n");
    out.push_str(&format!(
        "worker: program={:?} script={} (exists={}, {} chars)\n",
        worker.program,
        script.display(),
        script.is_file(),
        script.to_string_lossy().chars().count()
    ));
    out.push_str(&format!(
        "worker deps: {} exists={} ; node --version: {node_version}\n",
        runtime_dir.join("node_modules").display(),
        runtime_dir.join("node_modules").is_dir()
    ));
    // pnpm links each package into node_modules; a copied/moved checkout can
    // keep links that point into ANOTHER checkout or nowhere. Print where each
    // worker dependency really resolves.
    for package in [
        "@earendil-works/pi-ai",
        "@earendil-works/pi-agent-core",
        "@earendil-works/pi-coding-agent",
        "typebox",
    ] {
        let manifest = runtime_dir.join("node_modules").join(package).join("package.json");
        let resolved = std::fs::canonicalize(&manifest)
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_else(|error| format!("<unresolved: {error}>"));
        let version = std::fs::read_to_string(&manifest)
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .and_then(|manifest| manifest["version"].as_str().map(str::to_owned))
            .unwrap_or_else(|| "?".to_owned());
        out.push_str(&format!("  dep {package}@{version} -> {resolved}\n"));
    }
    let protocol = runtime_dir.join("../../packages/fox-engine-protocol/index.mjs");
    out.push_str(&format!(
        "  packages/fox-engine-protocol/index.mjs exists={}\n",
        protocol.is_file()
    ));
    // Load the worker's own module graph exactly as the Host's `node <script>`
    // would resolve it (no model call, no stdin protocol): a missing or broken
    // dependency shows up here as an import error.
    let import_probe = std::process::Command::new(&worker.program)
        .current_dir(&runtime_dir)
        .args([
            "--input-type=module",
            "-e",
            "for (const m of ['@earendil-works/pi-ai','@earendil-works/pi-agent-core','@earendil-works/pi-coding-agent','typebox']) { await import(m) } console.log('imports ok')",
        ])
        .output()
        .map(|output| {
            format!(
                "status={} stdout={} stderr={}",
                output.status,
                String::from_utf8_lossy(&output.stdout).trim(),
                truncate_chars(String::from_utf8_lossy(&output.stderr).trim(), 800)
            )
        })
        .unwrap_or_else(|error| format!("<spawn failed: {error}>"));
    out.push_str(&format!("  worker import probe (cwd={}): {import_probe}\n", runtime_dir.display()));
    out.push_str(&format!(
        "FOX_KERNEL_WORKER_STDERR={:?} (set to `inherit` to see the Node worker's own stderr)\n",
        std::env::var("FOX_KERNEL_WORKER_STDERR").ok()
    ));
    out.push_str(&format!(
        "fixture root: {root_text} ({} chars, {} bytes); canonical: {canonical}\n",
        root_text.chars().count(),
        root_text.len()
    ));
    out.push_str(&format!(
        "temp_dir: {} ; CARGO_MANIFEST_DIR: {} ({} chars)\n",
        std::env::temp_dir().display(),
        env!("CARGO_MANIFEST_DIR"),
        env!("CARGO_MANIFEST_DIR").chars().count()
    ));
    out.push_str("--- durable state ---\n");
    out.push_str(&format!("run_id: {run_id}\n"));
    out.push_str("kernel_runs (state | last_event_seq | terminal_at | updated_at):\n");
    out.push_str(&diag_rows(db, "SELECT state, last_event_seq, terminal_at, updated_at FROM kernel_runs WHERE run_id=?1", run_id));
    out.push_str("\nruns (status | error_code | error_message):\n");
    out.push_str(&diag_rows(db, "SELECT status, error_code, error_message FROM runs WHERE id=?1", run_id));
    out.push_str("\nrun.failed / budget events (seq | type | code | message):\n");
    out.push_str(&diag_rows(db,
        "SELECT seq, event_type, json_extract(payload_json,'$.code'), json_extract(payload_json,'$.message')
         FROM kernel_events WHERE run_id=?1 AND event_type IN ('run.failed','run.budget_exhausted','run.approval_expired','run.cancelled')
         ORDER BY seq", run_id));
    out.push_str("\nkernel_events tail, newest first (seq | type | payload):\n");
    out.push_str(&diag_rows(db,
        "SELECT seq, event_type, payload_json FROM kernel_events WHERE run_id=?1 ORDER BY seq DESC LIMIT 20", run_id));
    out.push_str("\nkernel_tool_calls (tool_call_id | tool | state):\n");
    out.push_str(&diag_rows(db, "SELECT tool_call_id, tool, state FROM kernel_tool_calls WHERE run_id=?1 ORDER BY created_at", run_id));
    out.push_str("\nkernel_decision_marks (mark | created_at):\n");
    out.push_str(&diag_rows(db, "SELECT mark, created_at FROM kernel_decision_marks WHERE run_id=?1 ORDER BY created_at", run_id));
    out.push_str("\ndelivery_outcome_stage (item_key | passed | decision_mark):\n");
    out.push_str(&diag_rows(db, "SELECT item_key, passed, decision_mark FROM delivery_outcome_stage WHERE run_id=?1 ORDER BY item_key", run_id));
    out.push_str("\ndelivery_checklist_items (item_key | status | has_finding):\n");
    out.push_str(&diag_rows(db, "SELECT item_key, status, finding_json IS NOT NULL FROM delivery_checklist_items WHERE run_id=?1 ORDER BY item_key", run_id));
    out.push_str(&format!("\nstaged_delivery_rounds(): {staged}\n"));
    out.push_str("===== end diagnostics =====\n");
    out
}

struct DeliveryLive {
    db: Database,
    root: PathBuf,
    run_id: String,
}

fn delivery_live_fixture(address: std::net::SocketAddr) -> DeliveryLive {
    let mut config = worker_configuration();
    config.model_service = json!({"apiType":"openai-completions","modelId":"delivery-live",
        "baseUrl":format!("http://{address}/v1")});
    let clock = TestClock::new(crate::database::now_ms());
    let (db, root, run_id) = fixture_with_start_opt(
        &clock,
        &config.hash().unwrap(),
        Some(&config),
        true,
        true,
    );
    freeze_host_scope(&db, &run_id);
    // Fixtures bypass the runtime_host run start, so the authoritative lead's
    // delivery checklist is seeded the same way production start does it.
    let seeds =
        crate::runtime_host::delivery::expectations_from_task("请生成一份 Excel 成果");
    assert_eq!(seeds.len(), 1);
    db.seed_delivery_checklist(&run_id, &seeds, crate::database::now_ms())
        .unwrap();
    DeliveryLive { db, root, run_id }
}

/// Same fixture, but the task also demands a chart per sheet: the checklist is
/// seeded through the production seeding step (task text → seeds → the
/// requirements those items must satisfy).
fn delivery_live_fixture_with_chart_demand(address: std::net::SocketAddr) -> DeliveryLive {
    let mut config = worker_configuration();
    config.model_service = json!({"apiType":"openai-completions","modelId":"delivery-live",
        "baseUrl":format!("http://{address}/v1")});
    let clock = TestClock::new(crate::database::now_ms());
    let (db, root, run_id) = fixture_with_start_opt(
        &clock,
        &config.hash().unwrap(),
        Some(&config),
        true,
        true,
    );
    freeze_host_scope(&db, &run_id);
    let task = "请生成 AGV汇总.xlsx，其中每个 Sheet 配一张图表。";
    let mut seeds = crate::runtime_host::delivery::expectations_from_task(task);
    assert_eq!(seeds.len(), 1);
    let requirements = crate::runtime_host::delivery::requirements_from_task(task);
    assert!(
        !requirements.is_empty(),
        "the chart demand must be parsed from the task"
    );
    for seed in seeds.iter_mut() {
        crate::runtime_host::delivery::attach_requirements(seed, &requirements);
    }
    assert_eq!(seeds[0].requirements.len(), 1);
    db.seed_delivery_checklist(&run_id, &seeds, crate::database::now_ms())
        .unwrap();
    let stored = db.delivery_requirements(&run_id).unwrap();
    assert_eq!(
        stored.len(),
        1,
        "the seeded chart demand must be readable back at the gate"
    );
    DeliveryLive { db, root, run_id }
}

fn real_workbook_bytes() -> Vec<u8> {
    std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/office-reading.xlsx"),
    )
    .unwrap()
}

/// Register the managed-write receipt the production write path records for a
/// file it committed. A live test that stands in for the tool layer must do the
/// same, otherwise the delivery gate has no evidence of authorship — which is
/// exactly what R03 requires it to demand.
fn record_managed_write_receipt(db: &Database, root: &Path, run_id: &str, relative: &str) {
    let path = root.join(relative.replace('/', std::path::MAIN_SEPARATOR_STR));
    let bytes = std::fs::read(&path).unwrap();
    use sha2::Digest;
    let mut hasher = sha2::Sha256::new();
    hasher.update(&bytes);
    let hash = format!("sha256:{}", hex::encode(hasher.finalize()));
    let conversation: String = db
        .with_connection(|connection| {
            Ok(connection.query_row(
                "SELECT conversation_id FROM runs WHERE id=?1",
                [run_id],
                |row| row.get(0),
            )?)
        })
        .unwrap();
    db.register_managed_file_version(
        &crate::database::ManagedFileVersionInput {
            conversation_id: &conversation,
            run_id: Some(run_id),
            tool_call_id: Some("live-write"),
            tool: "write_file",
            storage_path: &path.to_string_lossy(),
            display_name: relative,
            change_kind: "created",
            before_hash: None,
            before_size: None,
            after_hash: Some(&hash),
            after_size: Some(bytes.len() as i64),
            backup_path: None,
            after_backup_path: None,
            restored_from_id: None,
            source: crate::database::ManagedFileSource::HostCapture,
        },
        crate::database::now_ms(),
    )
    .unwrap();
}

/// Seed a repair-round checklist: two promised JSON artifacts that the Run
/// really wrote (each has a managed-write receipt), so every verdict passes.
///
/// The receipt stands for the production write path: `managed_files::record_after`
/// and `record_office_write` register the same row, which is what the delivery
/// gate treats as proof that these bytes are this task's own work.
fn seed_two_receipted_deliverables(db: &Database, root: &Path, run_id: &str) {
    let task = "生成 out/one.json 与 out/two.json。";
    std::fs::create_dir_all(root.join("out")).unwrap();
    let seeds = crate::runtime_host::delivery::expectations_from_task(task);
    assert_eq!(seeds.len(), 2, "{seeds:?}");
    for seed in seeds.iter() {
        let relative = seed.target_path.clone().expect("an explicit target");
        let path = root.join(relative.replace('/', std::path::MAIN_SEPARATOR_STR));
        std::fs::write(&path, format!("{{\"target\":\"{relative}\"}}")).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let mut hasher = sha2::Sha256::new();
        use sha2::Digest;
        hasher.update(&bytes);
        let hash = format!("sha256:{}", hex::encode(hasher.finalize()));
        let conversation: String = db
            .with_connection(|connection| {
                Ok(connection.query_row(
                    "SELECT conversation_id FROM runs WHERE id=?1",
                    [run_id],
                    |row| row.get(0),
                )?)
            })
            .unwrap();
        db.register_managed_file_version(
            &crate::database::ManagedFileVersionInput {
                conversation_id: &conversation,
                run_id: Some(run_id),
                tool_call_id: Some("seed-write"),
                tool: "write_file",
                storage_path: &path.to_string_lossy(),
                display_name: &relative,
                change_kind: "created",
                before_hash: None,
                before_size: None,
                after_hash: Some(&hash),
                after_size: Some(bytes.len() as i64),
                backup_path: None,
                after_backup_path: None,
                restored_from_id: None,
                source: crate::database::ManagedFileSource::HostCapture,
            },
            crate::database::now_ms(),
        )
        .unwrap();
    }
    db.seed_delivery_checklist(run_id, &seeds, crate::database::now_ms())
        .unwrap();
}

fn item_statuses(db: &Database, run_id: &str) -> Vec<(String, String)> {
    db.with_connection(|connection| {
        let mut statement = connection.prepare(
            "SELECT item_key, status FROM delivery_checklist_items
             WHERE run_id=?1 ORDER BY item_key",
        )?;
        let rows = statement
            .query_map([run_id], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    })
    .unwrap()
}

fn finalized_item_count(db: &Database, run_id: &str) -> i64 {
    db.with_connection(|connection| {
        Ok(connection.query_row(
            "SELECT COUNT(*) FROM delivery_checklist_items
             WHERE run_id=?1 AND finding_json IS NOT NULL",
            [run_id],
            |row| row.get(0),
        )?)
    })
    .unwrap()
}

/// Drive one round that proposes a tool call, settle it, and return the batch the
/// retry lane resumes from.
///
/// This reproduces the durable state a provider retry lands in: the tools of the
/// previous round are settled and the model request for the batch is pending, so
/// the next response arrives through `dispatch_batch` — the per-round entry point,
/// not the initial one.
fn settled_tool_batch(
    coordinator: &KernelCoordinator<'_>,
    run_id: &str,
    root: &Path,
) -> String {
    coordinator
        .dispatch_initial("owner", &Ask, |_, frame, _| {
            Ok(fox_engine_protocol::KernelInitialModelResponse {
                schema_version: 1,
                run_id: run_id.to_owned(),
                turn_id: frame.input.turn_id.clone(),
                checkpoint_seq: frame.checkpoint_seq,
                assistant_message: json!({"role":"assistant","stopReason":"toolUse",
                    "content":[{"type":"toolCall","id":"write-scratch","name":"write_file",
                        "arguments":{"path":"scratch.txt","content":"scratch"}}]}),
            })
        })
        .unwrap();
    coordinator
        .resolve_approval("write-scratch", kernel::ApprovalDecision::AllowOnce)
        .unwrap();
    coordinator
        .dispatch_tool("write-scratch", "writer", |_, _, _| {
            std::fs::write(root.join("scratch.txt"), b"scratch").unwrap();
            Ok((true, json!({"content":[{"type":"text","text":"written"}]})))
        })
        .unwrap();
    coordinator
        .snapshot()
        .unwrap()
        .pending_effects
        .iter()
        .find(|effect| effect.kind == kernel::OutboxEffectKind::DeliverToolBatch)
        .expect("the settled tool batch must be delivered to the model")
        .batch_id
        .clone()
        .unwrap()
}

fn final_stop_response(run_id: &str, frame: &fox_engine_protocol::KernelBatchResumeFrame) -> fox_engine_protocol::KernelModelResponse {
    fox_engine_protocol::KernelModelResponse {
        schema_version: 1,
        run_id: run_id.to_owned(),
        turn_id: frame.turn_id.clone(),
        batch_id: frame.batch_id.clone(),
        checkpoint_seq: frame.checkpoint_seq,
        assistant_message: json!({"role":"assistant","stopReason":"stop",
            "content":[{"type":"text","text":"已完成。"}]}),
    }
}

fn committed_marks(db: &Database, run_id: &str) -> i64 {
    db.with_connection(|connection| {
        Ok(connection.query_row(
            "SELECT COUNT(*) FROM kernel_decision_marks WHERE run_id=?1",
            [run_id],
            |row| row.get(0),
        )?)
    })
    .unwrap()
}

/// Regression (2026-10-02 review, P1 completion gap): a Run whose *batch* response
/// completes it must record the delivery verdict.
///
/// This is the entry point a provider retry resumes through: the previous round's
/// tools are settled and the next response arrives as a batch response, so the
/// completing decision is committed by `dispatch_batch` rather than by the live
/// loop. Before the fix that path terminated the Run without ever running the
/// delivery gate, which is how O02 and O07 completed with `pending` rows.
#[test]
fn a_batch_completion_records_a_satisfied_delivery_verdict() {
    let DeliveryLive { db, root, run_id } = delivery_live_fixture("127.0.0.1:9".parse().unwrap());
    let clock = TestClock::new(crate::database::now_ms());
    let cancellation = CancellationRegistry::default();
    // The promised workbook really is produced, with its managed-write receipt.
    std::fs::write(root.join("AGV汇总.xlsx"), real_workbook_bytes()).unwrap();
    record_managed_write_receipt(&db, &root, &run_id, "AGV汇总.xlsx");
    let coordinator =
        KernelCoordinator::start_prepared(&db, &clock, &run_id, &cancellation).unwrap();
    let batch = settled_tool_batch(&coordinator, &run_id, &root);
    coordinator
        .dispatch_batch(&batch, "reply", &Allow, |_, frame, _| {
            Ok(final_stop_response(&run_id, frame))
        })
        .unwrap();
    drop(coordinator);

    assert_eq!(
        item_statuses(&db, &run_id),
        vec![("slot:xlsx:1".to_owned(), "passed".to_owned())],
        "the batch entry point must record the verdict, not leave it pending"
    );
    assert_eq!(
        db.kernel_build_full_snapshot(&run_id).unwrap().state,
        "completed"
    );
    assert_eq!(committed_marks(&db, &run_id), 1);
    assert!(db.staged_delivery_rounds().unwrap().is_empty());
    let _ = std::fs::remove_dir_all(root);
}

/// The other outcome through the same entry point: the promised artifact is
/// missing, so the Run must record a failed verdict and arm the bounded repair
/// round instead of completing.
#[test]
fn a_batch_completion_with_a_missing_artifact_arms_the_repair_round() {
    let DeliveryLive { db, root, run_id } = delivery_live_fixture("127.0.0.1:9".parse().unwrap());
    let clock = TestClock::new(crate::database::now_ms());
    let cancellation = CancellationRegistry::default();
    let coordinator =
        KernelCoordinator::start_prepared(&db, &clock, &run_id, &cancellation).unwrap();
    let batch = settled_tool_batch(&coordinator, &run_id, &root);
    coordinator
        .dispatch_batch(&batch, "reply", &Allow, |_, frame, _| {
            Ok(final_stop_response(&run_id, frame))
        })
        .unwrap();
    drop(coordinator);

    assert_eq!(
        item_statuses(&db, &run_id),
        vec![("slot:xlsx:1".to_owned(), "failed".to_owned())],
        "an artifact that was never produced must be recorded as failed"
    );
    assert_ne!(
        db.kernel_build_full_snapshot(&run_id).unwrap().state,
        "completed",
        "a missing deliverable must not complete the Run"
    );
    assert_eq!(db.delivery_repair_round_count(&run_id).unwrap(), 1);
    let _ = std::fs::remove_dir_all(root);
}

/// Regression (2026-10-02 review, P1): a completion whose decision is refused must
/// not leave its staged verdicts behind.
///
/// The refusal path itself (a terminal decision that loses its write-set to a
/// job-notice competition) needs the notice machinery to trigger; what this test
/// pins is the handling the refusal depends on: `abandon_completion_delivery`
/// withdraws exactly the stage of the refused round, writes no verdict, and leaves
/// nothing for a later recovery to finalize.
#[test]
fn a_completion_stage_is_withdrawn_when_its_decision_is_refused() {
    let DeliveryLive { db, root, run_id } = delivery_live_fixture("127.0.0.1:9".parse().unwrap());
    let clock = TestClock::new(crate::database::now_ms());
    let cancellation = CancellationRegistry::default();
    std::fs::write(root.join("AGV汇总.xlsx"), real_workbook_bytes()).unwrap();
    record_managed_write_receipt(&db, &root, &run_id, "AGV汇总.xlsx");
    let coordinator =
        KernelCoordinator::start_prepared(&db, &clock, &run_id, &cancellation).unwrap();

    // Stage the verdicts this round would commit, exactly as the completing entry
    // point does before its commit.
    let completion = coordinator
        .stage_completion_delivery(&[], &json!({"role":"assistant","content":[]}))
        .unwrap();
    assert_eq!(
        db.staged_delivery_rounds().unwrap().len(),
        1,
        "the completing round must have staged its verdicts"
    );

    // The commit is refused: the round is abandoned, so its stage goes with it.
    coordinator
        .abandon_completion_delivery(Some(&completion))
        .unwrap();
    assert!(
        db.staged_delivery_rounds().unwrap().is_empty(),
        "a refused round must not leave its stage behind"
    );
    assert_eq!(
        item_statuses(&db, &run_id),
        vec![("slot:xlsx:1".to_owned(), "pending".to_owned())],
        "no verdict may be written for a decision that never committed"
    );
    // And a later recovery must not finalize anything either.
    assert_eq!(
        crate::runtime_host::delivery::finalize_staged_outcomes(&db).unwrap(),
        0
    );
    assert_eq!(
        item_statuses(&db, &run_id),
        vec![("slot:xlsx:1".to_owned(), "pending".to_owned())]
    );
    let _ = std::fs::remove_dir_all(root);
}

/// Regression (2026-10-04 review, P1): a Run that ends in failure still records a
/// real verdict for what it promised.
///
/// The F09 evidence was 200/200 artifacts plus progress.csv and summary.md with every
/// checklist row still `pending` and no marks, because the Run died in compaction
/// before any stop evaluation. The verdict below is the ordinary deterministic gate,
/// written through the failing decision's own mark; the Run stays failed, and a
/// missing artifact is still recorded as failed rather than passed.
#[test]
fn a_failed_run_still_records_the_verified_delivery_verdict() {
    for artifact_present in [true, false] {
        let DeliveryLive { db, root, run_id } = delivery_live_fixture("127.0.0.1:9".parse().unwrap());
        let clock = TestClock::new(crate::database::now_ms());
        let cancellation = CancellationRegistry::default();
        if artifact_present {
            std::fs::write(root.join("AGV汇总.xlsx"), real_workbook_bytes()).unwrap();
            record_managed_write_receipt(&db, &root, &run_id, "AGV汇总.xlsx");
        }
        let coordinator =
            KernelCoordinator::start_prepared(&db, &clock, &run_id, &cancellation).unwrap();
        coordinator
            .fail("kernel.context_compaction_failed", "compaction stopped the Run")
            .unwrap();
        drop(coordinator);

        let expected = if artifact_present { "passed" } else { "failed" };
        assert_eq!(
            item_statuses(&db, &run_id),
            vec![("slot:xlsx:1".to_owned(), expected.to_owned())],
            "the failing Run must close its checklist with a real verdict"
        );
        assert_eq!(
            db.kernel_build_full_snapshot(&run_id).unwrap().state,
            "failed",
            "a verdict about files never turns the Run into a pass"
        );
        assert!(
            committed_marks(&db, &run_id) >= 1,
            "the verdict must be owned by the failing decision's committed mark"
        );
        assert!(db.staged_delivery_rounds().unwrap().is_empty());
        let items = db.delivery_checklist(&run_id).unwrap();
        assert!(
            items[0].checked_at.is_some(),
            "a closed verdict carries when it was verified"
        );
        let _ = std::fs::remove_dir_all(root);
    }
}

/// Regression (2026-10-02 review, P0): every entry point that can finish a Run
/// must record the delivery verdict.
///
/// `dispatch_initial` used to terminate the Run without ever running the delivery
/// gate, so a Run that finished through a per-round entry point — which is the
/// path a provider retry falls back to, and the one the O02 and O07 Runs took
/// after their 429s — completed with its checklist rows `pending` for ever. The
/// verdicts are staged before the completing decision, committed with it, and
/// finalized after it.
#[test]
fn a_completion_through_the_per_round_entry_point_finalizes_the_verdict() {
    let DeliveryLive { db, root, run_id } = delivery_live_fixture("127.0.0.1:9".parse().unwrap());
    let clock = TestClock::new(crate::database::now_ms());
    let cancellation = CancellationRegistry::default();
    // The promised workbook really is produced, with the receipt the production
    // write path records for it.
    std::fs::write(root.join("AGV汇总.xlsx"), real_workbook_bytes()).unwrap();
    record_managed_write_receipt(&db, &root, &run_id, "AGV汇总.xlsx");
    let coordinator =
        KernelCoordinator::start_prepared(&db, &clock, &run_id, &cancellation).unwrap();
    coordinator
        .dispatch_initial("owner", &Allow, |_, frame, _| {
            Ok(fox_engine_protocol::KernelInitialModelResponse {
                schema_version: 1,
                run_id: run_id.clone(),
                turn_id: frame.input.turn_id.clone(),
                checkpoint_seq: frame.checkpoint_seq,
                assistant_message: json!({"role":"assistant","stopReason":"stop",
                    "content":[{"type":"text","text":"已完成。"}]}),
            })
        })
        .unwrap();
    drop(coordinator);

    let statuses = item_statuses(&db, &run_id);
    assert_eq!(
        statuses,
        vec![("slot:xlsx:1".to_owned(), "passed".to_owned())],
        "the produced deliverable must be recorded, not left pending"
    );
    assert_eq!(finalized_item_count(&db, &run_id), 1);
    assert_eq!(
        db.kernel_build_full_snapshot(&run_id).unwrap().state,
        "completed"
    );
    let marks: i64 = db
        .with_connection(|connection| {
            Ok(connection.query_row(
                "SELECT COUNT(*) FROM kernel_decision_marks WHERE run_id=?1",
                [&run_id],
                |row| row.get(0),
            )?)
        })
        .unwrap();
    assert_eq!(marks, 1, "the completing decision must carry its delivery mark");
    // Nothing is left waiting for a later resolution.
    assert!(db.staged_delivery_rounds().unwrap().is_empty());
    let _ = std::fs::remove_dir_all(root);
}

/// The other half of the same entry point: a promised deliverable that is missing
/// must arm the bounded repair round instead of letting the Run complete. This is
/// the "retry finished without the artifact" case the review asked to cover.
#[test]
fn a_completion_through_the_per_round_entry_point_repairs_a_missing_deliverable() {
    let DeliveryLive { db, root, run_id } = delivery_live_fixture("127.0.0.1:9".parse().unwrap());
    let clock = TestClock::new(crate::database::now_ms());
    let cancellation = CancellationRegistry::default();
    let coordinator =
        KernelCoordinator::start_prepared(&db, &clock, &run_id, &cancellation).unwrap();
    coordinator
        .dispatch_initial("owner", &Allow, |_, frame, _| {
            Ok(fox_engine_protocol::KernelInitialModelResponse {
                schema_version: 1,
                run_id: run_id.clone(),
                turn_id: frame.input.turn_id.clone(),
                checkpoint_seq: frame.checkpoint_seq,
                assistant_message: json!({"role":"assistant","stopReason":"stop",
                    "content":[{"type":"text","text":"我已完成。"}]}),
            })
        })
        .unwrap();
    drop(coordinator);

    assert_eq!(
        item_statuses(&db, &run_id),
        vec![("slot:xlsx:1".to_owned(), "failed".to_owned())],
        "an artifact that was never produced must be recorded as failed"
    );
    assert_ne!(
        db.kernel_build_full_snapshot(&run_id).unwrap().state,
        "completed",
        "a missing deliverable must not complete the Run"
    );
    assert_eq!(
        db.delivery_repair_round_count(&run_id).unwrap(),
        1,
        "the failed deliverable must arm exactly one bounded repair round"
    );
    let _ = std::fs::remove_dir_all(root);
}

/// Regression, 2026-10-01 review (R05): the round decision and the delivery
/// verdicts live in different write-sets, so the window between them is real.
///
/// A process death right after `commit_round_decision` — modelled by the test
/// barrier that sits exactly there — used to leave a completed Run whose
/// checklist rows stayed `pending` forever. With the two-phase write the
/// verdicts are staged *before* the decision, under that decision's mark, so a
/// crash leaves a stage that recovery can complete **because the mark proves the
/// decision committed** — without re-asking a model or replaying a tool call.
#[test]
fn a_crash_after_the_round_decision_is_recovered_from_the_staged_verdicts() {
    let (address, server) = delivery_provider(vec!["stop"]);
    let clock = crate::runtime_host::shadow_reconcile::ReconcilerClock;
    let mut config = worker_configuration();
    config.model_service = json!({"apiType":"openai-completions","modelId":"delivery-live",
        "baseUrl":format!("http://{address}/v1")});
    let budgets = TimeBudgets {
        model_request_ms: 15_000,
        model_first_response_ms: 15_000,
        model_idle_ms: 15_000,
        run_execution_ms: 45_000,
        ..TimeBudgets::default()
    };
    let (db, root, run_id) = fixture_with_budgets_opt(
        &clock, &config.hash().unwrap(), Some(&config), true, true, (0, 1), true, budgets,
    );
    freeze_host_scope(&db, &run_id);
    seed_two_receipted_deliverables(&db, &root, &run_id);

    // The crash: the decision is durable, the ledger write is not reached.
    super::live::test_barrier::install(
        &format!("{run_id}:delivery:committed"),
        Box::new(|| panic!("injected: process died after the round decision committed")),
    );
    let executions = AtomicUsize::new(0);
    let started = Instant::now();
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        crate::runtime_host::kernel_host::drive_with_actions(
            &crate::runtime_host::kernel_host::acquire(&root, &run_id).unwrap(),
            &db, &clock, &CancellationRegistry::default(), &run_id, &real_worker_command(),
            "local-test-only", &Allow,
            |_, _, _| {
                executions.fetch_add(1, Ordering::SeqCst);
                Ok((true, json!({"content":[{"type":"text","text":"no tool expected"}]})))
            },
            |_| Ok(()), |_| Ok(()), &|_: &fox_engine_protocol::KernelModelPreview| {},
        )
    }));
    let elapsed = started.elapsed();
    let barrier_key = format!("{run_id}:delivery:committed");
    let barrier = format!(
        "test_barrier {barrier_key}: fired={}",
        !super::live::test_barrier::pending(&barrier_key)
    );
    let stub = stub_report(address, 1, server);
    let diagnostics = || {
        drive_diagnostics(&db, &root, &run_id, &describe_caught(&outcome), elapsed, &stub,
            std::slice::from_ref(&barrier))
    };
    assert!(
        outcome.is_err(),
        "the injected death must really interrupt the drive{}",
        diagnostics()
    );

    // Fact 1: the decision committed before the crash, and its mark with it.
    let state = db.kernel_build_full_snapshot(&run_id).unwrap().state;
    assert_eq!(state, "completed", "the injected window is after the commit{}", diagnostics());
    let staged = db.staged_delivery_rounds().unwrap();
    assert_eq!(staged.len(), 1, "{staged:?}{}", diagnostics());
    let (staged_run, mark) = staged[0].clone();
    assert_eq!(staged_run, run_id);
    assert!(
        db.decision_mark_committed(&run_id, &mark).unwrap(),
        "the committed decision must have left its mark{}",
        diagnostics()
    );

    // Fact 2: nothing was finalized, and no item was touched before the decision
    // authorized it.
    let statuses = item_statuses(&db, &run_id);
    assert_eq!(statuses.len(), 2, "{statuses:?}");
    for (item_key, status) in &statuses {
        assert_eq!(
            status, "pending",
            "a verdict is not written until its decision committed ({item_key})"
        );
    }
    assert_eq!(finalized_item_count(&db, &run_id), 0);

    // Fact 3: a new Host — the same startup path the app runs — completes the
    // verdicts from the stage rows, because the mark proves the commit.
    let reopened = Database::open(root.join("facts.db")).unwrap();
    let recovered = crate::runtime_host::delivery::finalize_staged_outcomes(&reopened).unwrap();
    assert_eq!(recovered, 2, "both staged verdicts are recovered");
    let statuses = item_statuses(&reopened, &run_id);
    for (item_key, status) in &statuses {
        assert_eq!(status, "passed", "{item_key} must be recoverable, not pending");
    }
    assert_eq!(finalized_item_count(&reopened, &run_id), 2);
    // The Run is completed and its business rows agree: no completed+pending.
    assert_eq!(
        reopened.kernel_build_full_snapshot(&run_id).unwrap().state,
        "completed"
    );
    assert!(
        recovered == 0 || executions.load(Ordering::SeqCst) == 0,
        "recovery must not replay a tool side effect"
    );
    // Recovery is idempotent: a second startup writes nothing more.
    assert_eq!(
        crate::runtime_host::delivery::finalize_staged_outcomes(&reopened).unwrap(),
        0
    );

    // The delivery repair budget must not have been spent by a crash that never
    // committed a repair round.
    assert_eq!(reopened.delivery_repair_round_count(&run_id).unwrap(), 0);
    let _ = std::fs::remove_dir_all(root);
}

/// Regression, 2026-10-01 review (R05): the same window, entered from the other
/// side — the ledger write starts and dies partway through item N.
#[test]
fn a_crash_while_finalizing_the_nth_item_leaves_recoverable_state() {
    let (address, server) = delivery_provider(vec!["stop"]);
    let clock = crate::runtime_host::shadow_reconcile::ReconcilerClock;
    let mut config = worker_configuration();
    config.model_service = json!({"apiType":"openai-completions","modelId":"delivery-live",
        "baseUrl":format!("http://{address}/v1")});
    let budgets = TimeBudgets {
        model_request_ms: 15_000,
        model_first_response_ms: 15_000,
        model_idle_ms: 15_000,
        run_execution_ms: 45_000,
        ..TimeBudgets::default()
    };
    let (db, root, run_id) = fixture_with_budgets_opt(
        &clock, &config.hash().unwrap(), Some(&config), true, true, (0, 1), true, budgets,
    );
    freeze_host_scope(&db, &run_id);
    seed_two_receipted_deliverables(&db, &root, &run_id);

    // Abort inside the finalize transaction once the first item was updated. The
    // transaction rolls back, which is exactly what a process death does.
    crate::database::finalize_fault::abort_after(1);
    let started = Instant::now();
    let drive = crate::runtime_host::kernel_host::drive_with_actions(
        &crate::runtime_host::kernel_host::acquire(&root, &run_id).unwrap(),
        &db, &clock, &CancellationRegistry::default(), &run_id, &real_worker_command(),
        "local-test-only", &Allow,
        |_, _, _| Ok((true, json!({"content":[{"type":"text","text":"no tool expected"}]}))),
        |_| Ok(()), |_| Ok(()), &|_: &fox_engine_protocol::KernelModelPreview| {},
    );
    let elapsed = started.elapsed();
    // Read the thread-local fault counter before `disarm` resets it.
    let fault = describe_fault(
        "finalize_fault",
        crate::database::finalize_fault::observed(),
    );
    crate::database::finalize_fault::disarm();
    let stub = stub_report(address, 1, server);
    // The interrupted ledger write must leave the delivery *unfinished*: the
    // drive may end the Run (a failed round is a terminal condition), but it
    // must never report these items as delivered. The drive result itself is
    // not asserted (unchanged semantics); it is only reported on failure.
    let diagnostics = || {
        drive_diagnostics(&db, &root, &run_id, &describe_drive(&drive), elapsed, &stub,
            std::slice::from_ref(&fault))
    };

    // Nothing was half-written: the stage survives with its committed mark, and
    // no item carries a verdict yet.
    let statuses = item_statuses(&db, &run_id);
    assert_eq!(statuses.len(), 2, "{statuses:?}{}", diagnostics());
    for (item_key, status) in &statuses {
        assert_eq!(status, "pending", "{item_key} must not be half-finalized{}", diagnostics());
    }
    assert_eq!(finalized_item_count(&db, &run_id), 0, "{}", diagnostics());
    let staged = db.staged_delivery_rounds().unwrap();
    assert_eq!(staged.len(), 1, "{staged:?}{}", diagnostics());
    assert!(
        db.decision_mark_committed(&staged[0].0, &staged[0].1).unwrap(),
        "the decision committed, so recovery is allowed to finish it{}",
        diagnostics()
    );

    // A new Host recovers the whole set, and the repair budget stays unspent.
    let reopened = Database::open(root.join("facts.db")).unwrap();
    assert_eq!(
        crate::runtime_host::delivery::finalize_staged_outcomes(&reopened).unwrap(),
        2
    );
    for (item_key, status) in item_statuses(&reopened, &run_id) {
        assert_eq!(status, "passed", "{item_key}");
    }
    assert_eq!(reopened.delivery_repair_round_count(&run_id).unwrap(), 0);
    let _ = std::fs::remove_dir_all(root);
}

/// Regression (2026-10-02 review, P1): the *other* half of the same window.
///
/// A crash after staging but **before** the decision commits must not let
/// recovery write the staged verdicts. The stage carries a mark the commit never
/// wrote, so the startup resolver withdraws it and the checklist is left exactly
/// as the abandoned round found it — no pass, no failed, no repair round.
#[test]
fn a_crash_before_the_round_decision_never_finalizes_the_staged_verdicts() {
    let (address, server) = delivery_provider(vec!["stop"]);
    let clock = crate::runtime_host::shadow_reconcile::ReconcilerClock;
    let mut config = worker_configuration();
    config.model_service = json!({"apiType":"openai-completions","modelId":"delivery-live",
        "baseUrl":format!("http://{address}/v1")});
    let budgets = TimeBudgets {
        model_request_ms: 15_000,
        model_first_response_ms: 15_000,
        model_idle_ms: 15_000,
        run_execution_ms: 45_000,
        ..TimeBudgets::default()
    };
    let (db, root, run_id) = fixture_with_budgets_opt(
        &clock, &config.hash().unwrap(), Some(&config), true, true, (0, 1), true, budgets,
    );
    freeze_host_scope(&db, &run_id);
    seed_two_receipted_deliverables(&db, &root, &run_id);

    // Die immediately after staging: the decision that would authorize these
    // verdicts is never committed.
    super::live::test_barrier::install(
        &format!("{run_id}:delivery:staged"),
        Box::new(|| panic!("injected: process died after staging, before the decision committed")),
    );
    let started = Instant::now();
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        crate::runtime_host::kernel_host::drive_with_actions(
            &crate::runtime_host::kernel_host::acquire(&root, &run_id).unwrap(),
            &db, &clock, &CancellationRegistry::default(), &run_id, &real_worker_command(),
            "local-test-only", &Allow,
            |_, _, _| Ok((true, json!({"content":[{"type":"text","text":"no tool expected"}]}))),
            |_| Ok(()), |_| Ok(()), &|_: &fox_engine_protocol::KernelModelPreview| {},
        )
    }));
    let elapsed = started.elapsed();
    let barrier_key = format!("{run_id}:delivery:staged");
    let barrier = format!(
        "test_barrier {barrier_key}: fired={}",
        !super::live::test_barrier::pending(&barrier_key)
    );
    let stub = stub_report(address, 1, server);
    let diagnostics = || {
        drive_diagnostics(&db, &root, &run_id, &describe_caught(&outcome), elapsed, &stub,
            std::slice::from_ref(&barrier))
    };
    assert!(outcome.is_err(), "the injected death must interrupt the drive{}", diagnostics());

    // The stage exists, but its mark was never written by a commit.
    let staged = db.staged_delivery_rounds().unwrap();
    assert_eq!(staged.len(), 1, "the stage must be there: {staged:?}{}", diagnostics());
    let (staged_run, mark) = staged[0].clone();
    assert_eq!(staged_run, run_id);
    assert!(
        !db.decision_mark_committed(&run_id, &mark).unwrap(),
        "the decision never committed, so its mark must be absent{}",
        diagnostics()
    );
    // And the decision really did not land: the Run is not completed.
    let state = db.kernel_build_full_snapshot(&run_id).unwrap().state;
    assert_ne!(state, "completed", "the decision must not have committed: {state}{}", diagnostics());

    // Recovery must refuse to finalize those verdicts, and must not leave the
    // stage behind for a later round to pick up either.
    let reopened = Database::open(root.join("facts.db")).unwrap();
    let recovered = crate::runtime_host::delivery::finalize_staged_outcomes(&reopened).unwrap();
    assert_eq!(recovered, 0, "an uncommitted decision authorizes nothing");
    for (item_key, status) in item_statuses(&reopened, &run_id) {
        assert_eq!(
            status, "pending",
            "{item_key} must stay pending: its verdict was never authorized"
        );
    }
    assert_eq!(finalized_item_count(&reopened, &run_id), 0);
    assert!(
        reopened.staged_delivery_rounds().unwrap().is_empty(),
        "the withdrawn round must not linger for a later recovery"
    );
    // No repair round was registered for work that never ran.
    assert_eq!(reopened.delivery_repair_round_count(&run_id).unwrap(), 0);
    // A second recovery stays silent.
    assert_eq!(
        crate::runtime_host::delivery::finalize_staged_outcomes(&reopened).unwrap(),
        0
    );
    let _ = std::fs::remove_dir_all(root);
}

/// Regression (2026-10-02 review, P1): staging a batch is one transaction, so a
/// failure at item N leaves no partial round behind.
#[test]
fn a_stage_failure_at_the_nth_item_leaves_no_partial_round() {
    let (address, server) = delivery_provider(vec!["stop"]);
    let clock = crate::runtime_host::shadow_reconcile::ReconcilerClock;
    let mut config = worker_configuration();
    config.model_service = json!({"apiType":"openai-completions","modelId":"delivery-live",
        "baseUrl":format!("http://{address}/v1")});
    let budgets = TimeBudgets {
        model_request_ms: 15_000,
        model_first_response_ms: 15_000,
        model_idle_ms: 15_000,
        run_execution_ms: 45_000,
        ..TimeBudgets::default()
    };
    let (db, root, run_id) = fixture_with_budgets_opt(
        &clock, &config.hash().unwrap(), Some(&config), true, true, (0, 1), true, budgets,
    );
    freeze_host_scope(&db, &run_id);
    seed_two_receipted_deliverables(&db, &root, &run_id);

    // Abort while inserting the second item's stage row.
    crate::database::stage_fault::abort_after(1);
    let started = Instant::now();
    let drive = crate::runtime_host::kernel_host::drive_with_actions(
        &crate::runtime_host::kernel_host::acquire(&root, &run_id).unwrap(),
        &db, &clock, &CancellationRegistry::default(), &run_id, &real_worker_command(),
        "local-test-only", &Allow,
        |_, _, _| Ok((true, json!({"content":[{"type":"text","text":"no tool expected"}]}))),
        |_| Ok(()), |_| Ok(()), &|_: &fox_engine_protocol::KernelModelPreview| {},
    );
    let elapsed = started.elapsed();
    let fault = describe_fault("stage_fault", crate::database::stage_fault::observed());
    crate::database::stage_fault::disarm();
    let stub = stub_report(address, 1, server);
    // The drive result is not asserted (unchanged semantics); it is reported.
    let diagnostics = || {
        drive_diagnostics(&db, &root, &run_id, &describe_drive(&drive), elapsed, &stub,
            std::slice::from_ref(&fault))
    };

    // The whole batch rolled back: no partial stage, and nothing finalized.
    assert!(
        db.staged_delivery_rounds().unwrap().is_empty(),
        "a failed stage must leave no round behind: {:?}{}",
        db.staged_delivery_rounds().unwrap(),
        diagnostics()
    );
    for (item_key, status) in item_statuses(&db, &run_id) {
        assert_eq!(status, "pending", "{item_key}{}", diagnostics());
    }
    assert_eq!(finalized_item_count(&db, &run_id), 0, "{}", diagnostics());
    assert_eq!(
        crate::runtime_host::delivery::finalize_staged_outcomes(&db).unwrap(),
        0
    );
    let _ = std::fs::remove_dir_all(root);
}

/// Regression (2026-10-02 review, P1): additional user input takes the round
/// over, so the verdicts staged for the abandoned stop are withdrawn instead of
/// surviving as a round whose decision never committed.
#[test]
fn a_steering_takeover_withdraws_the_abandoned_rounds_stage() {
    let (address, server) = delivery_provider_with_ids(vec![
        ("stop", "stop-abandoned"),
        ("stop", "stop-steering"),
    ]);
    let clock = crate::runtime_host::shadow_reconcile::ReconcilerClock;
    let mut config = worker_configuration();
    config.model_service = json!({"apiType":"openai-completions","modelId":"delivery-live",
        "baseUrl":format!("http://{address}/v1")});
    let budgets = TimeBudgets {
        model_request_ms: 15_000,
        model_first_response_ms: 15_000,
        model_idle_ms: 15_000,
        run_execution_ms: 45_000,
        ..TimeBudgets::default()
    };
    let (db, root, run_id) = fixture_with_budgets_opt(
        &clock, &config.hash().unwrap(), Some(&config), true, true, (0, 1), true, budgets,
    );
    freeze_host_scope(&db, &run_id);
    seed_two_receipted_deliverables(&db, &root, &run_id);

    // Inside the window between staging and the decision commit, a request from
    // the user arrives: the round is re-planned as a steering round.
    let request_id = format!("mid-round-{}", uuid::Uuid::new_v4());
    db.enqueue_run_steering(&run_id, &request_id, "补充一句要求", crate::database::now_ms())
        .expect("a request can be accepted");
    super::live::test_barrier::install(
        &format!("{run_id}:delivery:staged"),
        Box::new(move || {
            // The row is already accepted; the commit re-reads the queue and
            // abandons the delivery decision for this round.
            let _ = &request_id;
        }),
    );
    let barrier_key = format!("{run_id}:delivery:staged");
    let started = Instant::now();
    let drive = crate::runtime_host::kernel_host::drive_with_actions(
        &crate::runtime_host::kernel_host::acquire(&root, &run_id).unwrap(),
        &db, &clock, &CancellationRegistry::default(), &run_id, &real_worker_command(),
        "local-test-only", &Allow,
        |_, _, _| Ok((true, json!({"content":[{"type":"text","text":"no tool expected"}]}))),
        |_| Ok(()), |_| Ok(()), &|_: &fox_engine_protocol::KernelModelPreview| {},
    );
    let elapsed = started.elapsed();
    let barrier = format!(
        "test_barrier {barrier_key}: fired={}",
        !super::live::test_barrier::pending(&barrier_key)
    );
    let stub = stub_report(address, 2, server);
    // The drive result is not asserted (unchanged semantics); it is reported.
    let diagnostics = || {
        drive_diagnostics(&db, &root, &run_id, &describe_drive(&drive), elapsed, &stub,
            std::slice::from_ref(&barrier))
    };

    // Whatever the round did, no stage may survive without a committed mark, and
    // the checklist must not contain a verdict no decision authorized.
    for (run, mark) in db.staged_delivery_rounds().unwrap() {
        assert!(
            db.decision_mark_committed(&run, &mark).unwrap(),
            "a lingering stage must belong to a committed decision: {mark}{}",
            diagnostics()
        );
    }
    let statuses = item_statuses(&db, &run_id);
    for (item_key, status) in &statuses {
        if !db.staged_delivery_rounds().unwrap().is_empty() {
            assert!(
                status == "pending" || status == "passed" || status == "failed",
                "{item_key} has an impossible status {status}{}",
                diagnostics()
            );
        }
    }
    // The finalize gate is what matters: a stage without a committed mark can
    // never become a verdict.
    let invented = format!("delivery:{}", uuid::Uuid::new_v4());
    db.stage_delivery_outcome(
        &run_id,
        &invented,
        &[crate::database::StagedDeliveryItem {
            item_key: statuses[0].0.clone(),
            passed: true,
            finding_json: "{\"invented\":true}".into(),
        }],
        None,
        crate::database::now_ms(),
    )
    .unwrap();
    assert_eq!(
        db.finalize_staged_delivery_outcome(&run_id, &invented, crate::database::now_ms())
            .unwrap(),
        0,
        "an uncommitted mark must never finalize"
    );
    assert_eq!(
        crate::runtime_host::delivery::finalize_staged_outcomes(&db).unwrap(),
        0
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn live_delivery_repair_loop_rides_continuation_without_consuming_review_budget_or_replaying_tools() {
    let (address, server) = delivery_provider(vec!["stop", "tool", "stop"]);
    let DeliveryLive { db, root, run_id } = delivery_live_fixture(address);
    let clock = TestClock::new(crate::database::now_ms());
    let cancellation = CancellationRegistry::default();
    let preview = |_: &fox_engine_protocol::KernelModelPreview| {};
    let coordinator =
        KernelCoordinator::start_prepared(&db, &clock, &run_id, &cancellation)
            .unwrap()
            .with_preview(&preview);
    let executions = AtomicUsize::new(0);
    let workbook_bytes = real_workbook_bytes();
    let execute = |_: &RunControlBinding, _: &kernel::OutboxEffect, _: &kernel::CancellationToken| {
        // The single tool call represents the write tool's real external side
        // effect: a parseable workbook lands inside the frozen project root.
        assert_eq!(executions.fetch_add(1, Ordering::SeqCst), 0);
        std::fs::write(root.join("AGV汇总.xlsx"), &workbook_bytes).unwrap();
        Ok((
            true,
            json!({"content":[{"type":"text","text":"workbook written"}]}),
        ))
    };
    coordinator
        .dispatch_initial_live(
            "delivery-live",
            &Allow,
            &real_worker_command(),
            "local-test-only",
            &execute,
            &|_| Ok(()),
            &|_| Ok(()),
        )
        .unwrap();

    // The run completes and its promised file passed deterministic checks.
    assert_eq!(coordinator.snapshot().unwrap().state, "completed");
    let items = db.delivery_checklist(&run_id).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].status, "passed");
    assert_eq!(items[0].target_path.as_deref(), Some("AGV汇总.xlsx"));
    assert!(items[0].checked_at.is_some());

    // Exactly one bounded business repair happened; the stop-review lane and
    // its independent counter stayed at zero.
    assert_eq!(db.delivery_repair_round_count(&run_id).unwrap(), 1);
    assert_eq!(db.kernel_count_delivery_repairs(&run_id).unwrap(), 1);
    assert_eq!(db.kernel_count_continuations(&run_id).unwrap(), 0);

    // One real side effect, never replayed.
    assert_eq!(executions.load(Ordering::SeqCst), 1);

    let requests = server.join().unwrap();
    assert_eq!(requests.len(), 3);
    // The repair directive reached the model as a user turn naming the failed
    // deliverable; it never masquerades as the stop-review prompt.
    let second_request = requests[1].to_string();
    assert!(second_request.contains("Fox 交付核验"));
    assert!(!second_request.contains("Fox 续答检查"));
}

#[test]
fn live_delivery_verdict_is_written_even_when_a_model_retry_precedes_the_final_stop() {
    // Regression, 2026-10-01 fix verification (O02): a Run that completed after
    // the Host's model-retry lane left its delivery rows `pending` with no
    // `checked_at`. The promised artifact was really written by this Run, so a
    // completed Run must always carry a verdict for every checklist row.
    //
    // A real clock is required: the retry lane waits for its durable due time.
    let (address, server) = delivery_provider(vec!["drop", "tool", "stop"]);
    let clock = crate::runtime_host::shadow_reconcile::ReconcilerClock;
    let mut config = worker_configuration();
    config.model_service = json!({"apiType":"openai-completions","modelId":"delivery-live",
        "baseUrl":format!("http://{address}/v1")});
    let budgets = TimeBudgets {
        model_request_ms: 15_000,
        model_first_response_ms: 15_000,
        model_idle_ms: 15_000,
        run_execution_ms: 45_000,
        ..TimeBudgets::default()
    };
    let (db, root, run_id) = fixture_with_budgets_opt(
        &clock, &config.hash().unwrap(), Some(&config), true, true, (2, 1), true, budgets,
    );
    freeze_host_scope(&db, &run_id);
    let seeds = crate::runtime_host::delivery::expectations_from_task("请生成一份 Excel 成果");
    assert_eq!(seeds.len(), 1);
    db.seed_delivery_checklist(&run_id, &seeds, crate::database::now_ms())
        .unwrap();

    let cancellation = CancellationRegistry::default();
    let preview = |_: &fox_engine_protocol::KernelModelPreview| {};
    let executions = AtomicUsize::new(0);
    let workbook_bytes = real_workbook_bytes();
    let execute = |_: &RunControlBinding, _: &kernel::OutboxEffect, _: &kernel::CancellationToken| {
        executions.fetch_add(1, Ordering::SeqCst);
        std::fs::write(root.join("AGV汇总.xlsx"), &workbook_bytes).unwrap();
        Ok((
            true,
            json!({"content":[{"type":"text","text":"workbook written"}]}),
        ))
    };
    crate::runtime_host::kernel_host::drive_with_actions(
        &crate::runtime_host::kernel_host::acquire(&root, &run_id).unwrap(),
        &db,
        &clock,
        &cancellation,
        &run_id,
        &real_worker_command(),
        "local-test-only",
        &Allow,
        execute,
        |_| Ok(()),
        |_| Ok(()),
        &preview,
    )
    .unwrap();

    // The retry lane really ran: one rejected model request was re-dispatched.
    let retries: i64 = db
        .with_connection(|connection| {
            Ok(connection.query_row(
                "SELECT COUNT(*) FROM kernel_events WHERE run_id=?1 AND event_type='run.retrying'",
                [&run_id],
                |row| row.get(0),
            )?)
        })
        .unwrap();
    assert_eq!(retries, 1, "the scripted transport failure must have retried");
    assert_eq!(db.kernel_build_full_snapshot(&run_id).unwrap().state, "completed");

    let items = db.delivery_checklist(&run_id).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(
        items[0].status, "passed",
        "a completed Run must not leave its delivery row pending: {:?}",
        items[0].finding
    );
    assert!(
        items[0].checked_at.is_some(),
        "the verdict must be timestamped, not left unchecked"
    );
    assert_eq!(executions.load(Ordering::SeqCst), 1, "one real side effect");
    assert_eq!(server.join().unwrap().len(), 3);
}

#[test]
fn a_batch_stage_provider_failure_retries_and_still_records_the_verdict() {
    // Regression (2026-10-02 review, P1 acceptance): the retry evidence so far only
    // covered a failure on the FIRST model request, and the re-run batch happened to
    // have no retries at all — so nothing said "a Run completed by a retried batch
    // response still carries its delivery verdict".
    //
    // The script drops the SECOND request: the initial round proposes a tool, the
    // tool batch is delivered, and the transport failure lands on that batch's
    // response. The Host's retry lane then re-dispatches, and the Run completes.
    let (address, server) = delivery_provider(vec!["tool", "drop", "stop"]);
    let clock = crate::runtime_host::shadow_reconcile::ReconcilerClock;
    let mut config = worker_configuration();
    config.model_service = json!({"apiType":"openai-completions","modelId":"delivery-live",
        "baseUrl":format!("http://{address}/v1")});
    let budgets = TimeBudgets {
        model_request_ms: 15_000,
        model_first_response_ms: 15_000,
        model_idle_ms: 15_000,
        run_execution_ms: 45_000,
        ..TimeBudgets::default()
    };
    let (db, root, run_id) = fixture_with_budgets_opt(
        &clock, &config.hash().unwrap(), Some(&config), true, true, (2, 1), true, budgets,
    );
    freeze_host_scope(&db, &run_id);
    let seeds = crate::runtime_host::delivery::expectations_from_task("请生成一份 Excel 成果");
    assert_eq!(seeds.len(), 1);
    db.seed_delivery_checklist(&run_id, &seeds, crate::database::now_ms())
        .unwrap();

    let cancellation = CancellationRegistry::default();
    let preview = |_: &fox_engine_protocol::KernelModelPreview| {};
    let executions = AtomicUsize::new(0);
    let workbook_bytes = real_workbook_bytes();
    let execute = |_: &RunControlBinding, _: &kernel::OutboxEffect, _: &kernel::CancellationToken| {
        executions.fetch_add(1, Ordering::SeqCst);
        std::fs::write(root.join("AGV汇总.xlsx"), &workbook_bytes).unwrap();
        Ok((
            true,
            json!({"content":[{"type":"text","text":"PROOF-READ-OK"}]}),
        ))
    };
    crate::runtime_host::kernel_host::drive_with_actions(
        &crate::runtime_host::kernel_host::acquire(&root, &run_id).unwrap(),
        &db,
        &clock,
        &cancellation,
        &run_id,
        &real_worker_command(),
        "local-test-only",
        &Allow,
        execute,
        |_| Ok(()),
        |_| Ok(()),
        &preview,
    )
    .unwrap();

    let requests = server.join().unwrap();
    // The dropped round really is the post-tool round: the second request carries the
    // settled tool result, and only the retry after it can produce the third.
    assert!(
        requests.len() >= 3,
        "expected initial, batch and retried requests, saw {}",
        requests.len()
    );
    assert!(
        requests[1].to_string().contains("PROOF-READ-OK"),
        "the dropped request must be the batch stage that follows the tool result"
    );
    let retries: i64 = db
        .with_connection(|connection| {
            Ok(connection.query_row(
                "SELECT COUNT(*) FROM kernel_events WHERE run_id=?1 AND event_type='run.retrying'",
                [&run_id],
                |row| row.get(0),
            )?)
        })
        .unwrap();
    assert_eq!(
        retries, 1,
        "the batch-stage transport failure must have gone through the retry lane"
    );
    assert_eq!(
        executions.load(Ordering::SeqCst),
        1,
        "the tool batch ran exactly once, before the dropped round"
    );

    // The retried round completed the Run — and the verdict is durable, not pending.
    assert_eq!(
        db.kernel_build_full_snapshot(&run_id).unwrap().state,
        "completed"
    );
    let items = db.delivery_checklist(&run_id).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(
        items[0].status, "passed",
        "a Run completed by a retried batch response must carry its verdict: {:?}",
        items[0].finding
    );
    assert!(
        items[0].checked_at.is_some(),
        "the verdict must be timestamped, not left unchecked"
    );
    let marks: i64 = db
        .with_connection(|connection| {
            Ok(connection.query_row(
                "SELECT COUNT(*) FROM kernel_decision_marks WHERE run_id=?1",
                [&run_id],
                |row| row.get(0),
            )?)
        })
        .unwrap();
    assert!(
        marks >= 1,
        "the completing decision of the retried round must carry its delivery mark"
    );
    assert!(db.staged_delivery_rounds().unwrap().is_empty());
}

#[test]
fn live_delivery_repair_budget_exhausts_with_completed_run_and_visibly_failed_item() {
    let (address, server) = delivery_provider(vec!["stop", "stop", "stop"]);
    let DeliveryLive { db, root: _root, run_id } = delivery_live_fixture(address);
    let clock = TestClock::new(crate::database::now_ms());
    let cancellation = CancellationRegistry::default();
    let preview = |_: &fox_engine_protocol::KernelModelPreview| {};
    let coordinator =
        KernelCoordinator::start_prepared(&db, &clock, &run_id, &cancellation)
            .unwrap()
            .with_preview(&preview);
    let executions = AtomicUsize::new(0);
    // The model never proposes a tool: no side effect may ever run.
    let execute = |_: &RunControlBinding, _: &kernel::OutboxEffect, _: &kernel::CancellationToken| {
        executions.fetch_add(1, Ordering::SeqCst);
        Ok((true, json!({"content":[{"type":"text","text":"unexpected"}]})))
    };
    coordinator
        .dispatch_initial_live(
            "delivery-exhausted",
            &Allow,
            &real_worker_command(),
            "local-test-only",
            &execute,
            &|_| Ok(()),
            &|_| Ok(()),
        )
        .unwrap();

    // Exhaustion completes the run rather than looping forever, while the
    // business delivery stays visibly failed.
    assert_eq!(coordinator.snapshot().unwrap().state, "completed");
    let items = db.delivery_checklist(&run_id).unwrap();
    assert_eq!(items[0].status, "failed");
    assert_eq!(db.delivery_repair_round_count(&run_id).unwrap(), 2);
    assert_eq!(db.kernel_count_delivery_repairs(&run_id).unwrap(), 2);
    assert_eq!(db.kernel_count_continuations(&run_id).unwrap(), 0);
    assert_eq!(executions.load(Ordering::SeqCst), 0);
    assert_eq!(server.join().unwrap().len(), 3);
}

/// R6: a deliverable that is parseable but does not contain what the task
/// demanded must reach the existing bounded delivery repair through the REAL
/// coordinator path — not only through a test-only checker.
#[test]
fn live_delivery_gate_repairs_a_parseable_artifact_that_misses_a_stated_demand() {
    // Round 1 writes a real but chart-less workbook and stops: the gate must
    // reject it and arm one bounded repair. Round 2 (the repair round) writes the
    // charted workbook and stops: the demand is now met.
    let (address, server) = delivery_provider_with_ids(vec![
        ("tool", "write-plain"),
        ("stop", "stop-after-plain"),
        ("tool", "write-charted"),
        ("stop", "stop-after-charted"),
    ]);
    let DeliveryLive { db, root, run_id } = delivery_live_fixture_with_chart_demand(address);
    let clock = TestClock::new(crate::database::now_ms());
    let cancellation = CancellationRegistry::default();
    let preview = |_: &fox_engine_protocol::KernelModelPreview| {};
    let coordinator =
        KernelCoordinator::start_prepared(&db, &clock, &run_id, &cancellation)
            .unwrap()
            .with_preview(&preview);
    let executions = AtomicUsize::new(0);
    let plain = real_workbook_bytes();
    // The task demands a chart per worksheet, so the repaired deliverable must
    // carry one *worksheet-referenced* chart per sheet — a whole-file chart count
    // could not satisfy (or verify) a per-sheet demand, and an unreferenced chart
    // part is not something the user can see.
    let charted = crate::runtime_host::delivery::zip_attach_chart_per_sheet(&plain);
    let execute = |_: &RunControlBinding, _: &kernel::OutboxEffect, _: &kernel::CancellationToken| {
        // Each tool round represents a real write of the promised artifact: the
        // bytes land, and the managed-write receipt the production write path
        // records is registered for them, because provenance is what the gate
        // requires before it inspects content.
        let round = executions.fetch_add(1, Ordering::SeqCst);
        let bytes = if round == 0 { &plain } else { &charted };
        std::fs::write(root.join("AGV汇总.xlsx"), bytes).unwrap();
        record_managed_write_receipt(&db, &root, &run_id, "AGV汇总.xlsx");
        Ok((
            true,
            json!({"content":[{"type":"text","text":"workbook written"}]}),
        ))
    };
    coordinator
        .dispatch_initial_live(
            "delivery-demands",
            &Allow,
            &real_worker_command(),
            "local-test-only",
            &execute,
            &|_| Ok(()),
            &|_| Ok(()),
        )
        .unwrap();

    // The repair happened through the delivery lane, not the stop-review lane,
    // and the artifact was written twice (once per real tool round).
    let requests = server.join().unwrap();
    let items = db.delivery_checklist(&run_id).unwrap();
    assert_eq!(
        items.first().map(|item| item.status.clone()),
        Some("passed".to_owned()),
        "delivery item: {:?}; requests: {}",
        items.first().map(|item| item.finding.clone()),
        requests.len()
    );
    assert_eq!(coordinator.snapshot().unwrap().state, "completed");
    assert_eq!(db.delivery_repair_round_count(&run_id).unwrap(), 1);
    assert_eq!(db.kernel_count_delivery_repairs(&run_id).unwrap(), 1);
    assert_eq!(db.kernel_count_continuations(&run_id).unwrap(), 0);
    assert_eq!(executions.load(Ordering::SeqCst), 2);

    // The final verdict shows the demand met, with the structural evidence.
    let finding: Value = serde_json::from_str(items[0].finding.as_deref().unwrap()).unwrap();
    let requirement = finding["requirements"]
        .as_array()
        .expect("requirement findings")
        .iter()
        .find(|entry| entry["check"] == json!("charts"))
        .expect("chart requirement reported");
    assert_eq!(requirement["state"], json!("passed"));
    assert_eq!(
        finding["checks"]["charts>=per-sheet:1"]["state"],
        json!("passed"),
        "the per-sheet demand is recorded as such and met: {finding}"
    );

    assert_eq!(requests.len(), 4);
    // The repair prompt names the missing chart requirement and rides the
    // delivery lane rather than masquerading as the stop-review prompt.
    let repair_request = requests[2].to_string();
    assert!(repair_request.contains("Fox 交付核验"));
    assert!(repair_request.contains("图表"), "repair must name the demand");
    assert!(!repair_request.contains("Fox 续答检查"));
}

/// A task that promises files but states no machine-decidable content demand
/// still gets the structural checks, and the run completes without inventing a
/// requirement or charging a repair round.
#[test]
fn live_delivery_gate_stays_quiet_without_a_structured_demand() {
    let (address, server) = delivery_provider(vec!["tool", "stop"]);
    let DeliveryLive { db, root, run_id } = delivery_live_fixture(address);
    let clock = TestClock::new(crate::database::now_ms());
    let cancellation = CancellationRegistry::default();
    let preview = |_: &fox_engine_protocol::KernelModelPreview| {};
    let coordinator =
        KernelCoordinator::start_prepared(&db, &clock, &run_id, &cancellation)
            .unwrap()
            .with_preview(&preview);
    let workbook_bytes = real_workbook_bytes();
    let execute = |_: &RunControlBinding, _: &kernel::OutboxEffect, _: &kernel::CancellationToken| {
        std::fs::write(root.join("AGV汇总.xlsx"), &workbook_bytes).unwrap();
        Ok((true, json!({"content":[{"type":"text","text":"written"}]})))
    };
    coordinator
        .dispatch_initial_live(
            "delivery-plain",
            &Allow,
            &real_worker_command(),
            "local-test-only",
            &execute,
            &|_| Ok(()),
            &|_| Ok(()),
        )
        .unwrap();
    assert_eq!(coordinator.snapshot().unwrap().state, "completed");
    assert_eq!(db.delivery_repair_round_count(&run_id).unwrap(), 0);
    let items = db.delivery_checklist(&run_id).unwrap();
    assert_eq!(items[0].status, "passed");
    let finding: Value = serde_json::from_str(items[0].finding.as_deref().unwrap()).unwrap();
    assert!(
        finding.get("requirements").is_none(),
        "no structured demand was stated, so none may be reported: {finding}"
    );
    assert_eq!(server.join().unwrap().len(), 2);
}
