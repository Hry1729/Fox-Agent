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
    assert!(
        outcome.is_err(),
        "the injected death must really interrupt the drive"
    );
    drop(server);

    // Fact 1: the decision committed before the crash, and its mark with it.
    let state = db.kernel_build_full_snapshot(&run_id).unwrap().state;
    assert_eq!(state, "completed", "the injected window is after the commit");
    let staged = db.staged_delivery_rounds().unwrap();
    assert_eq!(staged.len(), 1, "{staged:?}");
    let (staged_run, mark) = staged[0].clone();
    assert_eq!(staged_run, run_id);
    assert!(
        db.decision_mark_committed(&run_id, &mark).unwrap(),
        "the committed decision must have left its mark"
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
    let drive = crate::runtime_host::kernel_host::drive_with_actions(
        &crate::runtime_host::kernel_host::acquire(&root, &run_id).unwrap(),
        &db, &clock, &CancellationRegistry::default(), &run_id, &real_worker_command(),
        "local-test-only", &Allow,
        |_, _, _| Ok((true, json!({"content":[{"type":"text","text":"no tool expected"}]}))),
        |_| Ok(()), |_| Ok(()), &|_: &fox_engine_protocol::KernelModelPreview| {},
    );
    crate::database::finalize_fault::disarm();
    drop(server);
    // The interrupted ledger write must leave the delivery *unfinished*: the
    // drive may end the Run (a failed round is a terminal condition), but it
    // must never report these items as delivered.
    let _ = drive;

    // Nothing was half-written: the stage survives with its committed mark, and
    // no item carries a verdict yet.
    let statuses = item_statuses(&db, &run_id);
    assert_eq!(statuses.len(), 2, "{statuses:?}");
    for (item_key, status) in &statuses {
        assert_eq!(status, "pending", "{item_key} must not be half-finalized");
    }
    assert_eq!(finalized_item_count(&db, &run_id), 0);
    let staged = db.staged_delivery_rounds().unwrap();
    assert_eq!(staged.len(), 1, "{staged:?}");
    assert!(
        db.decision_mark_committed(&staged[0].0, &staged[0].1).unwrap(),
        "the decision committed, so recovery is allowed to finish it"
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
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        crate::runtime_host::kernel_host::drive_with_actions(
            &crate::runtime_host::kernel_host::acquire(&root, &run_id).unwrap(),
            &db, &clock, &CancellationRegistry::default(), &run_id, &real_worker_command(),
            "local-test-only", &Allow,
            |_, _, _| Ok((true, json!({"content":[{"type":"text","text":"no tool expected"}]}))),
            |_| Ok(()), |_| Ok(()), &|_: &fox_engine_protocol::KernelModelPreview| {},
        )
    }));
    assert!(outcome.is_err(), "the injected death must interrupt the drive");
    drop(server);

    // The stage exists, but its mark was never written by a commit.
    let staged = db.staged_delivery_rounds().unwrap();
    assert_eq!(staged.len(), 1, "the stage must be there: {staged:?}");
    let (staged_run, mark) = staged[0].clone();
    assert_eq!(staged_run, run_id);
    assert!(
        !db.decision_mark_committed(&run_id, &mark).unwrap(),
        "the decision never committed, so its mark must be absent"
    );
    // And the decision really did not land: the Run is not completed.
    let state = db.kernel_build_full_snapshot(&run_id).unwrap().state;
    assert_ne!(state, "completed", "the decision must not have committed: {state}");

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
    let drive = crate::runtime_host::kernel_host::drive_with_actions(
        &crate::runtime_host::kernel_host::acquire(&root, &run_id).unwrap(),
        &db, &clock, &CancellationRegistry::default(), &run_id, &real_worker_command(),
        "local-test-only", &Allow,
        |_, _, _| Ok((true, json!({"content":[{"type":"text","text":"no tool expected"}]}))),
        |_| Ok(()), |_| Ok(()), &|_: &fox_engine_protocol::KernelModelPreview| {},
    );
    crate::database::stage_fault::disarm();
    drop(server);
    let _ = drive;

    // The whole batch rolled back: no partial stage, and nothing finalized.
    assert!(
        db.staged_delivery_rounds().unwrap().is_empty(),
        "a failed stage must leave no round behind: {:?}",
        db.staged_delivery_rounds().unwrap()
    );
    for (item_key, status) in item_statuses(&db, &run_id) {
        assert_eq!(status, "pending", "{item_key}");
    }
    assert_eq!(finalized_item_count(&db, &run_id), 0);
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
    let drive = crate::runtime_host::kernel_host::drive_with_actions(
        &crate::runtime_host::kernel_host::acquire(&root, &run_id).unwrap(),
        &db, &clock, &CancellationRegistry::default(), &run_id, &real_worker_command(),
        "local-test-only", &Allow,
        |_, _, _| Ok((true, json!({"content":[{"type":"text","text":"no tool expected"}]}))),
        |_| Ok(()), |_| Ok(()), &|_: &fox_engine_protocol::KernelModelPreview| {},
    );
    drop(server);
    let _ = drive;

    // Whatever the round did, no stage may survive without a committed mark, and
    // the checklist must not contain a verdict no decision authorized.
    for (run, mark) in db.staged_delivery_rounds().unwrap() {
        assert!(
            db.decision_mark_committed(&run, &mark).unwrap(),
            "a lingering stage must belong to a committed decision: {mark}"
        );
    }
    let statuses = item_statuses(&db, &run_id);
    for (item_key, status) in &statuses {
        if !db.staged_delivery_rounds().unwrap().is_empty() {
            assert!(
                status == "pending" || status == "passed" || status == "failed",
                "{item_key} has an impossible status {status}"
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
