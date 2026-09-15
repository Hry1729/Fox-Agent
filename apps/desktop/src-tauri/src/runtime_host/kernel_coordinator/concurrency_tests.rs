//! Host-decided bounded concurrency for independent read-only tool calls
//! (design supplement item 5). The Host — never the model's ordering and never
//! an MCP manifest flag — classifies a maximal source-order prefix of pending
//! dispatches as independent side-effect-free readers, runs at most
//! `MAX_PARALLEL_READ_ONLY` of them concurrently, and keeps every writer,
//! dependent call and unknown tool a serial barrier. These tests pin:
//!   * real concurrent execution with source-order settlement and exactly-once
//!     side effects, durable across reopen;
//!   * a business-failed (`Ok((false, …))`) reader settles inside the group;
//!   * an uncertain (`Err`) or panicking reader leaves exactly its own lease
//!     leased; the certain sibling results still commit, and a reopened Host
//!     never replays the leased call;
//!   * per-tool cancellation stops the remaining calls without duplicates;
//!   * the end-to-end live scheduler fans two reads out and only starts the
//!     following write after both reader closures have returned.
use super::*;
use std::collections::BTreeMap;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

const SPECS_THREE_READS: &[(&str, &str, &str)] = &[
    ("read-a", "read", r#"{"path":"proof.txt"}"#),
    ("read-b", "read", r#"{"path":"proof.txt"}"#),
    ("read-c", "read", r#"{"path":"proof.txt"}"#),
];

fn requests(specs: &[(&str, &str, &str)]) -> Vec<ToolCallRequest> {
    specs
        .iter()
        .enumerate()
        .map(|(source_order, (id, tool, input_json))| ToolCallRequest {
            tool_call_id: (*id).into(),
            tool: (*tool).into(),
            canonical_input_json: (*input_json).to_owned(),
            source_order,
        })
        .collect()
}

fn proposed_batch<'a>(
    clock: &'a TestClock,
    cancellation: &'a CancellationRegistry,
    specs: &[(&str, &str, &str)],
) -> (Database, PathBuf, String) {
    let (db, root, run_id) = fixture(clock);
    let coordinator =
        KernelCoordinator::reopen(&db, clock, &run_id, cancellation).unwrap();
    coordinator
        .propose_tools("batch", requests(specs), &Allow)
        .unwrap();
    drop(coordinator);
    (db, root, run_id)
}

/// Pending DispatchTool effects in the model's source order.
fn pending_group(coordinator: &KernelCoordinator) -> Vec<kernel::OutboxEffect> {
    let snapshot = coordinator.snapshot().unwrap();
    super::super::live::pending_dispatch_ids(&snapshot)
        .iter()
        .map(|id| {
            snapshot
                .pending_effects
                .iter()
                .find(|effect| effect.tool_call_id.as_deref() == Some(id.as_str()))
                .expect("ordered dispatch id has a pending effect")
                .clone()
        })
        .collect()
}

fn effect_status(root: &PathBuf, run_id: &str, tool_call_id: &str) -> String {
    let conn = rusqlite::Connection::open(root.join("facts.db")).unwrap();
    conn.query_row(
        "SELECT status FROM kernel_effect_outbox WHERE run_id=?1 AND tool_call_id=?2",
        rusqlite::params![run_id, tool_call_id],
        |row| row.get(0),
    )
    .unwrap()
}

fn completed_event_order(root: &PathBuf, run_id: &str) -> Vec<String> {
    let conn = rusqlite::Connection::open(root.join("facts.db")).unwrap();
    let mut statement = conn
        .prepare(
            "SELECT payload_json FROM kernel_events
             WHERE run_id=?1 AND event_type='tool.completed' ORDER BY seq",
        )
        .unwrap();
    statement
        .query_map(rusqlite::params![run_id], |row| {
            let payload: String = row.get(0)?;
            Ok(payload)
        })
        .unwrap()
        .map(|row| {
            let payload: Value = serde_json::from_str(&row.unwrap()).unwrap();
            payload["toolCallId"].as_str().unwrap().to_owned()
        })
        .collect()
}

fn classifier_effect(kind: kernel::OutboxEffectKind, payload_json: &str) -> kernel::OutboxEffect {
    kernel::OutboxEffect {
        effect_key: "dispatch:x".into(),
        kind,
        idempotency_key: "idem".into(),
        tool_call_id: Some("x".into()),
        batch_id: Some("batch".into()),
        payload_json: payload_json.to_owned(),
        status: kernel::OutboxStatus::Pending,
        attempts: 0,
    }
}

#[test]
fn classifier_whitelists_only_host_native_readers() {
    let classify = |tool: &str| {
        super::super::live::parallel_read_only_dispatch(&classifier_effect(
            kernel::OutboxEffectKind::DispatchTool,
            &json!({"tool": tool, "input": {}}).to_string(),
        ))
    };
    for tool in ["read", "ls", "find", "grep", "skill_load"] {
        assert!(classify(tool), "{tool} must be parallel-safe");
    }
    for tool in [
        "write_file",
        "edit_file",
        "run_command",
        "read_tool_result",
        "read_attachment",
        "sqlite_read",
        "git_read",
        "graph_readonly_run",
        "web_read",
        "http_request",
        "mcp__external__query",
        "totally_unknown_tool",
    ] {
        assert!(!classify(tool), "{tool} must stay a serial barrier");
    }
    // A malformed payload or a non-dispatch effect is never parallelized.
    assert!(!super::super::live::parallel_read_only_dispatch(&classifier_effect(
        kernel::OutboxEffectKind::DispatchTool,
        "not-json",
    )));
    assert!(!super::super::live::parallel_read_only_dispatch(&classifier_effect(
        kernel::OutboxEffectKind::InitialModel,
        &json!({"tool": "read"}).to_string(),
    )));
}

#[test]
fn independent_readers_run_concurrently_settle_in_source_order_and_never_replay() {
    let clock = TestClock::new(1_000);
    let cancellation = CancellationRegistry::default();
    let (db, root, run_id) = proposed_batch(&clock, &cancellation, SPECS_THREE_READS);
    let coordinator =
        KernelCoordinator::reopen(&db, &clock, &run_id, &cancellation).unwrap();
    let arrivals = Arc::new((Mutex::new(0usize), Condvar::new()));
    let counts: Arc<Mutex<BTreeMap<String, usize>>> =
        Arc::new(Mutex::new(BTreeMap::new()));
    let execute = |binding: &RunControlBinding,
                   effect: &kernel::OutboxEffect,
                   token: &kernel::CancellationToken| {
        let id = effect.tool_call_id.clone().unwrap();
        // Concurrency gate: every reader must be in flight before any of them
        // proceeds. A serial scheduler makes the 5 s wait time out.
        let (lock, cvar) = &*arrivals;
        {
            let mut count = lock.lock().unwrap();
            *count += 1;
            cvar.notify_all();
        }
        let count = lock.lock().unwrap();
        let (count, timeout) = cvar
            .wait_timeout_while(count, Duration::from_secs(5), |count| *count < 3)
            .unwrap();
        assert!(!timeout.timed_out(), "the three reads were not concurrent");
        assert_eq!(*count, 3);
        // Finish strictly out of source order; settlement must reorder.
        let delay = match id.as_str() {
            "read-a" => 150,
            "read-b" => 80,
            _ => 10,
        };
        std::thread::sleep(Duration::from_millis(delay));
        *counts.lock().unwrap().entry(id.clone()).or_insert(0) += 1;
        let payload: Value = serde_json::from_str(&effect.payload_json).unwrap();
        let result =
            crate::resource_gateway::execute(binding, "read", &payload["input"], token)?;
        Ok((true, result))
    };
    coordinator
        .dispatch_read_only_group(&pending_group(&coordinator), "group-owner", &execute)
        .unwrap();

    // Each external effect happened exactly once, through the real bounded
    // Rust reader.
    let counts = counts.lock().unwrap();
    assert_eq!(
        counts.iter().map(|(id, n)| (id.as_str(), *n)).collect::<Vec<_>>(),
        vec![("read-a", 1), ("read-b", 1), ("read-c", 1)]
    );
    drop(counts);
    let snapshot = coordinator.snapshot().unwrap();
    for tool in &snapshot.tool_calls {
        assert_eq!(tool.state, "completed");
        let result: Value = serde_json::from_str(tool.result_json.as_ref().unwrap()).unwrap();
        assert!(result["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("durable coordinator"));
    }
    // Settlement persisted in source order even though execution finished
    // c → b → a.
    assert_eq!(
        completed_event_order(&root, &run_id),
        vec!["read-a", "read-b", "read-c"]
    );
    for id in ["read-a", "read-b", "read-c"] {
        assert_eq!(effect_status(&root, &run_id, id), "completed");
    }
    drop(coordinator);
    drop(db);

    // Reopen: results are durable and the committed calls cannot run again.
    let db = Database::open(root.join("facts.db")).unwrap();
    let coordinator =
        KernelCoordinator::reopen(&db, &clock, &run_id, &cancellation).unwrap();
    for id in ["read-a", "read-b", "read-c"] {
        assert!(!coordinator
            .dispatch_tool(id, "reopened-owner", |_, _, _| panic!("{id} must not replay"))
            .unwrap());
    }
    let snapshot = coordinator.snapshot().unwrap();
    assert!(snapshot
        .pending_effects
        .iter()
        .all(|effect| effect.kind != kernel::OutboxEffectKind::DispatchTool
            || effect.status != kernel::OutboxStatus::Pending));
}

#[test]
fn business_failed_reader_settles_inside_the_group_without_leasing_anything() {
    let clock = TestClock::new(1_000);
    let cancellation = CancellationRegistry::default();
    let specs = &[
        ("read-a", "read", r#"{"path":"proof.txt"}"#),
        ("read-b", "read", r#"{"path":"missing.txt"}"#),
    ];
    let (db, root, run_id) = proposed_batch(&clock, &cancellation, specs);
    let coordinator =
        KernelCoordinator::reopen(&db, &clock, &run_id, &cancellation).unwrap();
    let execute = |_: &RunControlBinding,
                   effect: &kernel::OutboxEffect,
                   _: &kernel::CancellationToken| {
        if effect.tool_call_id.as_deref() == Some("read-b") {
            // A settled business failure (isError), not uncertain execution.
            return Ok((
                false,
                json!({"content":[{"type":"text","text":"file not found"}], "isError": true}),
            ));
        }
        Ok((true, json!({"content":[{"type":"text","text":"ok"}]})))
    };
    coordinator
        .dispatch_read_only_group(&pending_group(&coordinator), "group-owner", &execute)
        .unwrap();
    let snapshot = coordinator.snapshot().unwrap();
    let states: BTreeMap<_, _> = snapshot
        .tool_calls
        .iter()
        .map(|tool| (tool.tool_call_id.as_str(), tool.state.as_str()))
        .collect();
    assert_eq!(states.get("read-a"), Some(&"completed"));
    assert_eq!(states.get("read-b"), Some(&"failed"));
    for id in ["read-a", "read-b"] {
        assert_eq!(effect_status(&root, &run_id, id), "completed");
    }
}

#[test]
fn uncertain_reader_keeps_its_lease_siblings_commit_and_reopen_never_replays() {
    let clock = TestClock::new(1_000);
    let cancellation = CancellationRegistry::default();
    let (db, root, run_id) = proposed_batch(&clock, &cancellation, SPECS_THREE_READS);
    let counts: Arc<Mutex<BTreeMap<String, usize>>> =
        Arc::new(Mutex::new(BTreeMap::new()));
    let execute = |_: &RunControlBinding,
                   effect: &kernel::OutboxEffect,
                   _: &kernel::CancellationToken| {
        let id = effect.tool_call_id.clone().unwrap();
        *counts.lock().unwrap().entry(id.clone()).or_insert(0) += 1;
        if id == "read-b" {
            // Transport lost after possible work: uncertain, never auto-replayed.
            return Err("transport lost after possible side effect".into());
        }
        Ok((true, json!({"content":[{"type":"text","text":"ok"}]})))
    };
    {
        let coordinator =
            KernelCoordinator::reopen(&db, &clock, &run_id, &cancellation).unwrap();
        let error = coordinator
            .dispatch_read_only_group(&pending_group(&coordinator), "dead-owner", &execute)
            .unwrap_err();
        assert!(error.contains("transport lost"), "{error}");
    }
    // Certain sibling results still committed; the uncertain call is leased.
    assert_eq!(effect_status(&root, &run_id, "read-a"), "completed");
    assert_eq!(effect_status(&root, &run_id, "read-b"), "leased");
    assert_eq!(effect_status(&root, &run_id, "read-c"), "completed");

    // Reopen on a fresh Host process: the leased dispatch must not be stolen.
    drop(db);
    let db = Database::open(root.join("facts.db")).unwrap();
    let coordinator =
        KernelCoordinator::reopen(&db, &clock, &run_id, &cancellation).unwrap();
    assert!(!coordinator
        .dispatch_tool("read-b", "new-owner", |_, _, _| panic!("uncertain read reruns"))
        .unwrap());
    let counts = counts.lock().unwrap();
    assert_eq!(*counts.get("read-b").unwrap(), 1);
    assert_eq!(*counts.get("read-a").unwrap(), 1);
    assert_eq!(*counts.get("read-c").unwrap(), 1);
}

#[test]
fn panicking_reader_is_uncertain_without_replaying_or_losing_sibling_results() {
    let clock = TestClock::new(1_000);
    let cancellation = CancellationRegistry::default();
    let specs = &[
        ("read-a", "read", r#"{"path":"proof.txt"}"#),
        ("read-b", "read", r#"{"path":"proof.txt"}"#),
    ];
    let (db, root, run_id) = proposed_batch(&clock, &cancellation, specs);
    let execute = |_: &RunControlBinding,
                   effect: &kernel::OutboxEffect,
                   _: &kernel::CancellationToken| {
        if effect.tool_call_id.as_deref() == Some("read-b") {
            panic!("worker thread exploded");
        }
        Ok((true, json!({"content":[{"type":"text","text":"ok"}]})))
    };
    let error = {
        let coordinator =
            KernelCoordinator::reopen(&db, &clock, &run_id, &cancellation).unwrap();
        coordinator
            .dispatch_read_only_group(&pending_group(&coordinator), "panic-owner", &execute)
            .unwrap_err()
    };
    assert!(error.contains("panicked"), "{error}");
    assert_eq!(effect_status(&root, &run_id, "read-a"), "completed");
    assert_eq!(effect_status(&root, &run_id, "read-b"), "leased");
}

#[test]
fn run_cancel_mid_group_stops_remaining_reads_without_duplicate_execution() {
    let clock = TestClock::new(1_000);
    let cancellation = CancellationRegistry::default();
    let (db, root, run_id) = proposed_batch(&clock, &cancellation, SPECS_THREE_READS);
    let registry = cancellation.clone();
    let run_id_handle = run_id.clone();
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let gate_for_readers = gate.clone();
    let bodies: Arc<Mutex<BTreeMap<String, usize>>> =
        Arc::new(Mutex::new(BTreeMap::new()));
    let bodies_for_readers = bodies.clone();
    let bodies_for_closure = bodies.clone();
    let execute = move |_: &RunControlBinding,
                        effect: &kernel::OutboxEffect,
                        token: &kernel::CancellationToken| {
        let id = effect.tool_call_id.clone().unwrap();
        if id == "read-a" {
            use crate::kernel::CancellationPort;
            // The first reader completes its work and cancels the Run while its
            // siblings are queued in their worker threads.
            *bodies_for_closure
                .lock()
                .unwrap()
                .entry(id.clone())
                .or_insert(0) += 1;
            registry.request_run_cancel(&run_id_handle);
            *gate.0.lock().unwrap() = true;
            gate.1.notify_all();
            return Ok((true, json!({"content":[{"type":"text","text":"a ok"}]})));
        }
        // b/c only reach their first cancellation check AFTER the cancel is
        // durably requested, so the outcome is deterministic.
        let (lock, cvar) = &*gate_for_readers;
        let released = lock.lock().unwrap();
        let (released, timeout) = cvar
            .wait_timeout_while(released, Duration::from_secs(5), |released| !*released)
            .unwrap();
        assert!(!timeout.timed_out() && *released);
        drop(released);
        token.check()?;
        *bodies_for_readers
            .lock()
            .unwrap()
            .entry(id.clone())
            .or_insert(0) += 1;
        Ok((true, json!({"content":[{"type":"text","text":"should not run"}]})))
    };
    let error = {
        let coordinator =
            KernelCoordinator::reopen(&db, &clock, &run_id, &cancellation).unwrap();
        coordinator
            .dispatch_read_only_group(&pending_group(&coordinator), "cancel-owner", &execute)
            .unwrap_err()
    };
    assert!(error.contains("[tool.cancelled]"), "{error}");
    let bodies = bodies.lock().unwrap();
    assert_eq!(*bodies.get("read-a").unwrap(), 1, "the cancelling reader ran once");
    assert!(bodies.get("read-b").is_none(), "b did no work");
    assert!(bodies.get("read-c").is_none(), "c did no work");
    drop(bodies);
    assert_eq!(effect_status(&root, &run_id, "read-a"), "completed");
    assert_eq!(effect_status(&root, &run_id, "read-b"), "leased");
    assert_eq!(effect_status(&root, &run_id, "read-c"), "leased");

    // A reopened Host cannot replay the cancelled, still-leased reads.
    drop(db);
    let db = Database::open(root.join("facts.db")).unwrap();
    let reopened_cancellation = CancellationRegistry::default();
    let coordinator =
        KernelCoordinator::reopen(&db, &clock, &run_id, &reopened_cancellation).unwrap();
    for id in ["read-a", "read-b", "read-c"] {
        assert!(!coordinator
            .dispatch_tool(id, "reopened-owner", |_, _, _| panic!("{id} replayed"))
            .unwrap());
    }
}

#[test]
fn live_scheduler_fans_readers_out_and_never_overlaps_the_following_write() {
    let replies = vec![
        json!({"role":"assistant","tool_calls":[
            {"index":0,"id":"read-a","type":"function",
                "function":{"name":"read","arguments":r#"{"path":"a.txt"}"#}},
            {"index":1,"id":"read-b","type":"function",
                "function":{"name":"read","arguments":r#"{"path":"b.txt"}"#}},
            {"index":2,"id":"write-c","type":"function",
                "function":{"name":"write_file","arguments":r#"{"path":"out.txt","content":"x"}"#}}
        ]}),
        json!({"role":"assistant","content":"I will now analyze the files."}),
        json!({"role":"assistant","content":"Checking the completed reads."}),
        json!({"role":"assistant","content":"任务全部完成。"}),
    ];
    let (address, server) = start_http_model_fixture(replies);
    let mut config = worker_configuration();
    config.proposal_tools = vec![
        json!({"name":"read","description":"Read a file",
            "parameters":{"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}}),
        json!({"name":"write_file","description":"Write a file",
            "parameters":{"type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"}},
                "required":["path","content"]}}),
    ];
    config.model_service = json!({"apiType":"openai-completions","modelId":"kernel-http-test",
        "baseUrl":format!("http://{address}/v1")});
    let clock = TestClock::new(crate::database::now_ms());
    let (db, root, run_id) =
        fixture_with_start_opt(&clock, &config.hash().unwrap(), Some(&config), true, true);
    db.freeze_kernel_host_scope(&run_id, &crate::database::KernelHostScope {
        schema_version: 1,
        tool_names: ["read".into(), "write_file".into()].into_iter().collect(),
        mcp_server_hashes: Default::default(),
        knowledge_reference_hashes: Default::default(),
        knowledge_connection_hashes: Default::default(),
        office_tools: Default::default(),
        lifecycle_hooks: Vec::new(),
    })
    .unwrap();
    let cancellation = CancellationRegistry::default();
    let preview = |_: &fox_engine_protocol::KernelModelPreview| {};
    let coordinator = KernelCoordinator::start_prepared(&db, &clock, &run_id, &cancellation)
        .unwrap()
        .with_preview(&preview);

    // Ordered execution log plus a gate proving the two readers are in flight
    // simultaneously. The write asserts at entry that both reader closures
    // already returned — with a parallel writer this assertion can fail.
    let log = Arc::new(Mutex::new(Vec::<String>::new()));
    let log_for_closure = log.clone();
    let arrivals = Arc::new((Mutex::new(0usize), Condvar::new()));
    let execute = move |_: &RunControlBinding,
                        effect: &kernel::OutboxEffect,
                        _: &kernel::CancellationToken| {
        let payload: Value = serde_json::from_str(&effect.payload_json).unwrap();
        let id = effect.tool_call_id.clone().unwrap();
        let tool = payload["tool"].as_str().unwrap();
        if tool == "write_file" {
            let mut snapshot = log_for_closure.lock().unwrap();
            assert!(
                snapshot.contains(&"end:read-a".to_owned())
                    && snapshot.contains(&"end:read-b".to_owned()),
                "write started before both readers finished: {snapshot:?}"
            );
            snapshot.push("start:write-c".to_owned());
            return Ok((true, json!({"content":[{"type":"text","text":"written"}]})));
        }
        let (lock, cvar) = &*arrivals;
        {
            let mut count = lock.lock().unwrap();
            *count += 1;
            log_for_closure.lock().unwrap().push(format!("start:{id}"));
            cvar.notify_all();
        }
        let count = lock.lock().unwrap();
        let (count, timeout) = cvar
            .wait_timeout_while(count, Duration::from_secs(5), |count| *count < 2)
            .unwrap();
        assert!(!timeout.timed_out(), "the two reads were not fanned out concurrently");
        assert_eq!(*count, 2);
        drop(count);
        std::thread::sleep(Duration::from_millis(120));
        log_for_closure.lock().unwrap().push(format!("end:{id}"));
        Ok((true, json!({"content":[{"type":"text","text":"read ok"}]})))
    };
    coordinator
        .dispatch_initial_live(
            "live-concurrency",
            &Allow,
            &real_worker_command(),
            "local-test-only",
            &execute,
            &|_| Ok(()),
            &|_| Ok(()),
        )
        .unwrap();

    let snapshot = coordinator.snapshot().unwrap();
    assert_eq!(snapshot.state, "completed");
    assert_eq!(snapshot.tool_calls.len(), 3);
    for tool in &snapshot.tool_calls {
        assert_eq!(tool.state, "completed", "{} settled", tool.tool_call_id);
    }
    let log = log.lock().unwrap();
    let end_a = log.iter().position(|entry| entry == "end:read-a").unwrap();
    let end_b = log.iter().position(|entry| entry == "end:read-b").unwrap();
    let write = log
        .iter()
        .position(|entry| entry == "start:write-c")
        .expect("write closure ran");
    assert!(write > end_a && write > end_b, "write started after both reads ended");
    let requests = server.join().unwrap();
    assert_eq!(requests.len(), 4, "tool round plus two bounded reviews and final");
    assert!(
        requests.last().unwrap().to_string().contains("Fox 续答检查"),
        "the final round still goes through the bounded continuation check"
    );
    // "任务全部完成。" is the fourth *response* — the owned final answer.
    let conn = rusqlite::Connection::open(root.join("facts.db")).unwrap();
    let (count, last): (i64, String) = conn
        .query_row(
            "SELECT COUNT(*), COALESCE(MAX(content),'') FROM messages \
             WHERE run_id=?1 AND role='assistant' AND status='completed' AND content<>''",
            [&run_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(count, 1, "exactly one owned final answer");
    assert_eq!(last, "任务全部完成。");
}
