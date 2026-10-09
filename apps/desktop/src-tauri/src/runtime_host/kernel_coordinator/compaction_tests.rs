use super::*;
use crate::kernel_compaction as context;

#[test]
fn acceptance_task3_long_history_reopens_between_every_compaction_pass() {
    for count in [12, 24, 120] {
        let clock = TestClock::new(1000);
        let cancellation = CancellationRegistry::default();
        let (mut db, root, run_id) = compaction_fixture_count(&clock, count);
        let original = db.kernel_initial_input(&run_id).unwrap();
        drop(KernelCoordinator::start_prepared(&db, &clock, &run_id, &cancellation).unwrap());
        let mut passes = 0;
        let insufficient = loop {
            let coordinator = KernelCoordinator::reopen(&db, &clock, &run_id, &cancellation).unwrap();
            match coordinator.prepare_context_if_needed("initial", 4096) {
                Ok(false) => break false,
                Err(error) => { assert_eq!(error, context::INSUFFICIENT); break true; }
                Ok(true) => {
                    coordinator.dispatch_pending_compaction("acceptance-owner", |_, request, _, _| Ok(response(request))).unwrap();
                    passes += 1;
                    assert!(passes <= context::MAX_PASSES);
                    let view = coordinator.context_view("initial", &original.messages).unwrap();
                    assert_eq!(view[0], original.messages[0]);
                    assert_eq!(&view[view.len()-8..], &original.messages[original.messages.len()-8..]);
                    assert_eq!(db.kernel_initial_input(&run_id).unwrap(), original);
                }
            }
            drop(coordinator);
            drop(db);
            db = Database::open(root.join("facts.db")).unwrap();
        };
        assert_eq!(insufficient, count == 120);
        assert!(passes > 0);
        println!("TASK3 old_messages={count} passes={passes} insufficient={insufficient} source_unchanged=true");
    }
}

fn compaction_fixture(clock: &TestClock) -> (Database, PathBuf, String) {
    compaction_fixture_count(clock, 12)
}

fn compaction_fixture_count(clock: &TestClock, old_count: usize) -> (Database, PathBuf, String) {
    let mut config = worker_configuration();
    config.model_service["contextWindow"] = json!(8192);
    config.model_service["maxOutputTokens"] = json!(512);
    let (db, root, run_id) =
        fixture_with_start_opt(clock, &config.hash().unwrap(), Some(&config), false, true);
    let mut input = initial_input(&run_id, &config.hash().unwrap());
    input.messages.extend((0..old_count).map(|index| {
        json!({"role":if index%2==0 {"user"} else {"assistant"},
        "content":format!("old-{index}: {}", "bounded earlier discussion ".repeat(38))})
    }));
    input.messages.extend(
        (0..8).map(
            |index| json!({"role":"user","content":format!("recent-{index}: preserve exactly")}),
        ),
    );
    db.freeze_kernel_initial_input(&input).unwrap();
    freeze_host_scope(&db, &run_id);
    (db, root, run_id)
}

fn compaction_fixture_with_messages(clock: &TestClock, messages: Vec<Value>) -> (Database, PathBuf, String) {
    let mut config = worker_configuration();
    config.model_service["contextWindow"] = json!(8192);
    config.model_service["maxOutputTokens"] = json!(512);
    let (db, root, run_id) =
        fixture_with_start_opt(clock, &config.hash().unwrap(), Some(&config), false, true);
    let mut input = initial_input(&run_id, &config.hash().unwrap());
    input.messages.extend(messages);
    db.freeze_kernel_initial_input(&input).unwrap();
    freeze_host_scope(&db, &run_id);
    (db, root, run_id)
}

#[test]
fn kernel_usage_calibration_pairs_the_dispatched_view_with_full_occupancy_usage() {
    let clock = TestClock::new(1000);
    let cancellation = CancellationRegistry::default();
    let (db, _root, run_id) = compaction_fixture(&clock);
    let original = db.kernel_initial_input(&run_id).unwrap();
    let config = db.kernel_model_config(&run_id).unwrap();
    let estimated = context::estimate_components_tokens(&config, &original.messages).unwrap() as u64;
    let coordinator = KernelCoordinator::start_prepared(&db, &clock, &run_id, &cancellation).unwrap();
    // Cached input still occupies the window: input + cacheRead + cacheWrite
    // is the full occupancy, and the pair must not apply a billing discount.
    coordinator
        .dispatch_initial("usage-owner", &Allow, |binding, frame, _| {
            Ok(fox_engine_protocol::KernelInitialModelResponse {
                schema_version: 1,
                run_id: binding.run_id.clone(),
                turn_id: frame.input.turn_id.clone(),
                checkpoint_seq: frame.checkpoint_seq,
                assistant_message: json!({"role":"assistant","stopReason":"stop",
                    "content":[{"type":"text","text":"done"}],
                    "usage":{"input":estimated/2,"output":4,
                        "cacheRead":estimated - estimated/2,"cacheWrite":0}}),
            })
        })
        .unwrap();
    let data = db.kernel_usage_calibration(&run_id).unwrap();
    assert_eq!(
        data.latest_usage_input_tokens,
        Some(estimated),
        "cached tokens count at full occupancy"
    );
    let calibration = data
        .calibration
        .expect("a pair matching the dispatched view calibrates the estimator");
    assert!(
        (900_000..=1_100_000).contains(&calibration.ratio_ppm),
        "ratio near 1.0 for a faithful estimate: {}",
        calibration.ratio_ppm
    );
    // A pair that can no longer match what was sent is discarded, and the
    // usage figure stays visible as a labelled estimate fallback.
    let (db2, _root2, run_id2) = compaction_fixture(&clock);
    let coordinator = KernelCoordinator::start_prepared(&db2, &clock, &run_id2, &cancellation).unwrap();
    coordinator
        .dispatch_initial("usage-owner", &Allow, |binding, frame, _| {
            Ok(fox_engine_protocol::KernelInitialModelResponse {
                schema_version: 1,
                run_id: binding.run_id.clone(),
                turn_id: frame.input.turn_id.clone(),
                checkpoint_seq: frame.checkpoint_seq,
                assistant_message: json!({"role":"assistant","stopReason":"stop",
                    "content":[{"type":"text","text":"done"}],
                    "usage":{"input":5,"output":2,"cacheRead":0,"cacheWrite":0}}),
            })
        })
        .unwrap();
    let data = db2.kernel_usage_calibration(&run_id2).unwrap();
    assert_eq!(data.latest_usage_input_tokens, Some(5));
    assert_eq!(data.calibration, None, "a 100x outlier pair is rejected");
}

#[test]
fn kernel_compaction_stop_classes_distinguish_single_item_from_no_candidates() {
    let clock = TestClock::new(1000);
    let cancellation = CancellationRegistry::default();
    // One protected message larger than the whole window: no summary can fix
    // that, and the error must say so distinctly.
    let (db, _root, run_id) = compaction_fixture_with_messages(&clock, vec![
        json!({"role":"user","content":"中".repeat(30_000)}),
    ]);
    let coordinator = KernelCoordinator::start_prepared(&db, &clock, &run_id, &cancellation).unwrap();
    assert_eq!(
        coordinator.prepare_context_if_needed("initial", 4096).unwrap_err(),
        context::SINGLE_TOO_LARGE
    );
    drop(coordinator);
    // Over budget with only unremovable material: nothing is a prose
    // candidate and nothing forms a complete tool round, and no single item
    // alone blows the window — a genuinely different stop class.
    let mut filler = Vec::new();
    for index in 0..30 {
        filler.push(json!({"role":"assistant","content":[
            {"type":"thinking","thinking":format!("reasoning {index} {}", "材料 ".repeat(700))}]}));
    }
    filler.push(json!({"role":"user","content":"continue"}));
    let (db, _root, run_id) = compaction_fixture_with_messages(&clock, filler);
    let coordinator = KernelCoordinator::start_prepared(&db, &clock, &run_id, &cancellation).unwrap();
    assert_eq!(
        coordinator.prepare_context_if_needed("initial", 4096).unwrap_err(),
        context::NO_CANDIDATES
    );
}

fn response(
    request: &fox_engine_protocol::KernelCompactionRequest,
) -> fox_engine_protocol::KernelCompactionResponse {
    fox_engine_protocol::KernelCompactionResponse {schema_version:1,run_id:request.run_id.clone(),turn_id:request.turn_id.clone(),
        compaction_id:request.compaction_id.clone(),input_hash:request.input_hash.clone(),
        summary:"Prior discussion is context only; preserve the current goal and do not infer approvals.".into(),
        usage:json!({"input":100,"output":20,"totalTokens":120})}
}

    fn settled_provider_rejection(request: &fox_engine_protocol::KernelCompactionRequest, status: u16) -> String {
        format!("kernel.settled_model_failure:{}", json!({
            "schemaVersion": 1, "runId": request.run_id, "turnId": request.turn_id,
            "checkpointSeq": 1, "category": "provider_unavailable",
            "httpStatus": status, "retryAfterMs": 2000,
        }))
    }

    fn compaction_failure_events(db: &Database, run_id: &str) -> Vec<Value> {
        db.with_connection(|conn| {
            let mut statement = conn.prepare(
                "SELECT payload_json FROM kernel_events
                  WHERE run_id=?1 AND event_type='context.compaction.failed' ORDER BY seq",
            )?;
            let rows = statement
                .query_map([run_id], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<String>>>()?;
            Ok(rows
                .into_iter()
                .map(|body| serde_json::from_str::<Value>(&body).unwrap_or(Value::Null))
                .collect::<Vec<Value>>())
        })
        .unwrap()
    }

    fn model_request_since(db: &Database, run_id: &str) -> Option<i64> {
        db.with_connection(|conn| {
            Ok(conn.query_row(
                "SELECT model_request_since_wall_ms FROM kernel_runs WHERE run_id=?1",
                [run_id],
                |row| row.get::<_, Option<i64>>(0),
            )?)
        })
        .unwrap()
    }

    /// A settled provider rejection is safe to re-ask: the provider answered, so it
    /// cannot already be generating this summary. One retry, inside the SAME window.
    #[test]
    fn a_settled_provider_rejection_is_retried_once_inside_the_same_deadline() {
        let clock = TestClock::new(1000);
        let cancellation = CancellationRegistry::default();
        let (db, _root, run_id) = compaction_fixture(&clock);
        let coordinator =
            KernelCoordinator::start_prepared(&db, &clock, &run_id, &cancellation).unwrap();
        assert!(coordinator
            .prepare_context_if_needed("initial", 4096)
            .unwrap());
        let deadline_before = model_request_since(&db, &run_id);
        let attempts = std::cell::Cell::new(0u32);
        coordinator
            .dispatch_pending_compaction("retry-owner", |_, request, _, remaining| {
                attempts.set(attempts.get() + 1);
                assert!(remaining > 0, "every attempt shares the original window");
                if attempts.get() == 1 {
                    Err(settled_provider_rejection(request, 429))
                } else {
                    Ok(response(request))
                }
            })
            .unwrap();
        assert_eq!(attempts.get(), 2, "exactly one retry was spent");
        // The window is never renewed: the durable request start is unchanged.
        assert_eq!(
            model_request_since(&db, &run_id),
            deadline_before,
            "a retry must not reset the request deadline"
        );
        // The compaction really completed, so the Run is not stuck.
        assert!(coordinator.snapshot().unwrap().compaction.pending.is_none());
        let failures = compaction_failure_events(&db, &run_id);
        assert_eq!(failures.len(), 1, "{failures:?}");
        assert_eq!(failures[0]["failure"], "settled_rejection");
        assert_eq!(failures[0]["attempt"], 1);
        assert_eq!(failures[0]["category"], "provider_unavailable");
        assert_eq!(failures[0]["httpStatus"], 429);
        assert_eq!(failures[0]["retryable"], true);
        assert!(failures[0]["remainingMs"].as_i64().unwrap() > 0);
        // The event carries no request or summary content.
        let body = failures[0].to_string();
        assert!(!body.contains("Prior discussion is context only"));
    }

    /// The retry is bounded: a second settled rejection stops with one classified
    /// reason, and the pending request keeps its owner so recovery cannot replay it.
    #[test]
    fn an_exhausted_retry_reports_one_classified_reason_and_stops() {
        let clock = TestClock::new(1000);
        let cancellation = CancellationRegistry::default();
        let (db, _root, run_id) = compaction_fixture(&clock);
        let coordinator =
            KernelCoordinator::start_prepared(&db, &clock, &run_id, &cancellation).unwrap();
        assert!(coordinator
            .prepare_context_if_needed("initial", 4096)
            .unwrap());
        let attempts = std::cell::Cell::new(0u32);
        let error = coordinator
            .dispatch_pending_compaction("exhausted-owner", |_, request, _, _| {
                attempts.set(attempts.get() + 1);
                Err(settled_provider_rejection(request, 503))
            })
            .unwrap_err();
        assert_eq!(error, context::FAILED);
        assert_eq!(attempts.get(), 2, "bounded: initial attempt plus one retry");
        let failures = compaction_failure_events(&db, &run_id);
        assert_eq!(failures.len(), 2, "{failures:?}");
        assert_eq!(failures[0]["retryable"], true);
        assert_eq!(failures[1]["retryable"], false, "the last attempt must not claim a retry");
        assert_eq!(failures[1]["attempt"], 2);
        // The durable owner stays set, so a restart never re-dispatches this request.
        let pending = coordinator.snapshot().unwrap().compaction.pending.expect("pending kept");
        assert_eq!(pending.owner.as_deref(), Some("exhausted-owner"));
    }

    /// An unknown outcome is never replayed: the provider may be generating the
    /// summary right now, so a second request could duplicate it.
    #[test]
    fn an_unknown_compaction_outcome_is_never_replayed() {
        let clock = TestClock::new(1000);
        let cancellation = CancellationRegistry::default();
        let (db, _root, run_id) = compaction_fixture(&clock);
        let coordinator =
            KernelCoordinator::start_prepared(&db, &clock, &run_id, &cancellation).unwrap();
        assert!(coordinator
            .prepare_context_if_needed("initial", 4096)
            .unwrap());
        let attempts = std::cell::Cell::new(0u32);
        let error = coordinator
            .dispatch_pending_compaction("unknown-owner", |_, _, _, _| {
                attempts.set(attempts.get() + 1);
                Err(crate::runtime_host::kernel_model_worker::MODEL_WINDOW_EXPIRED.to_owned())
            })
            .unwrap_err();
        assert_eq!(error, context::FAILED);
        assert_eq!(attempts.get(), 1, "an unknown outcome is never replayed");
        let failures = compaction_failure_events(&db, &run_id);
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0]["failure"], "unknown_outcome");
        assert_eq!(failures[0]["retryable"], false);
        assert_eq!(failures[0]["category"],"transport_outcome_unknown");
        assert_eq!(failures[0]["outcomeKnown"],false);
        assert!(failures[0]["upstreamMessage"].is_string());
        assert_eq!(failures[0]["retryTimeline"],"no_run_retry_event_on_compaction_path");
    }

    /// Only a settled rejection may be retried; the classifier is the single place
    /// that decides, so a cancelled Run or an unclassified error never re-asks.
    #[test]
    fn only_a_settled_rejection_is_retryable() {
        assert!(context::CompactionFailure::SettledRejection.is_retryable());
        for kind in [
            context::CompactionFailure::UnknownOutcome,
            context::CompactionFailure::InvalidResult,
            context::CompactionFailure::Cancelled,
            context::CompactionFailure::Unclassified,
        ] {
            assert!(!kind.is_retryable(), "{kind:?} must not be retried");
            assert!(!kind.as_str().is_empty());
        }
        assert_eq!(context::RETRY_LIMIT, 1, "bounded recovery only");
        assert!(context::MIN_RETRY_BUDGET_MS > 0);
    }

#[test]
fn kernel_compaction_prepared_and_committed_views_survive_reopen_without_repeating_model() {
    let clock = TestClock::new(1000);
    let cancellation = CancellationRegistry::default();
    let (db, root, run_id) = compaction_fixture(&clock);
    let original = db.kernel_initial_input(&run_id).unwrap();
    let coordinator =
        KernelCoordinator::start_prepared(&db, &clock, &run_id, &cancellation).unwrap();
    assert!(coordinator
        .prepare_context_if_needed("initial", 4096)
        .unwrap());
    assert_eq!(coordinator.snapshot().unwrap().state, "compacting");
    drop(coordinator);
    drop(db);
    let db = Database::open(root.join("facts.db")).unwrap();
    let coordinator = KernelCoordinator::reopen(&db, &clock, &run_id, &cancellation).unwrap();
    let calls = AtomicUsize::new(0);
    coordinator
        .dispatch_pending_compaction("summary-owner", |_, request, _, remaining| {
            calls.fetch_add(1, Ordering::SeqCst);
            assert!(remaining > 0);
            Ok(response(request))
        })
        .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(db.kernel_initial_input(&run_id).unwrap(), original);
    let view = coordinator
        .context_view("initial", &original.messages)
        .unwrap();
    assert!(context::bytes(&view).unwrap() < context::bytes(&original.messages).unwrap());
    assert_eq!(view[0], original.messages[0]);
    assert_eq!(
        &view[view.len() - 8..],
        &original.messages[original.messages.len() - 8..]
    );
    assert_eq!(coordinator.snapshot().unwrap().compaction.compactions, 1);
    let conn = rusqlite::Connection::open(root.join("facts.db")).unwrap();
    let visible: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM messages WHERE run_id=?1 AND role='assistant'",
            [&run_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(visible, 0, "summaries are not visible assistant answers");
    let usage: String = conn
        .query_row(
            "SELECT event_json FROM run_events WHERE run_id=?1 AND event_type='usage.updated'",
            [&run_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&usage).unwrap()["totalTokens"],
        120
    );
    drop(coordinator);
    drop(db);
    let db = Database::open(root.join("facts.db")).unwrap();
    let coordinator = KernelCoordinator::reopen(&db, &clock, &run_id, &cancellation).unwrap();
    assert!(!coordinator
        .prepare_context_if_needed("initial", 4096)
        .unwrap());
    coordinator.dispatch_initial("normal-model",&Allow,|binding,frame,_| {
        assert_eq!(frame.input.messages,view);
        Ok(fox_engine_protocol::KernelInitialModelResponse {schema_version:1,run_id:binding.run_id.clone(),turn_id:frame.input.turn_id.clone(),
            checkpoint_seq:frame.checkpoint_seq,assistant_message:json!({"role":"assistant","stopReason":"stop","content":[{"type":"text","text":"final"}]})})
    }).unwrap();
    assert_eq!(coordinator.snapshot().unwrap().state, "completed");
}

#[test]
fn kernel_compaction_failed_result_commit_keeps_source_and_never_replays_after_restart() {
    let clock = TestClock::new(1000);
    let cancellation = CancellationRegistry::default();
    let (db, root, run_id) = compaction_fixture(&clock);
    let original = db.kernel_initial_input(&run_id).unwrap();
    let coordinator =
        KernelCoordinator::start_prepared(&db, &clock, &run_id, &cancellation).unwrap();
    coordinator
        .prepare_context_if_needed("initial", 4096)
        .unwrap();
    let conn = rusqlite::Connection::open(root.join("facts.db")).unwrap();
    conn.execute_batch("CREATE TRIGGER reject_compaction BEFORE INSERT ON kernel_events WHEN NEW.event_type='context.compaction.result' BEGIN SELECT RAISE(ABORT,'injected compaction commit failure'); END;").unwrap();
    assert!(coordinator
        .dispatch_pending_compaction("owner", |_, request, _, _| Ok(response(request)))
        .is_err());
    assert_eq!(coordinator.snapshot().unwrap().state, "compacting");
    assert!(db
        .kernel_compaction_results(&run_id, "initial")
        .unwrap()
        .is_empty());
    assert_eq!(db.kernel_initial_input(&run_id).unwrap(), original);
    conn.execute_batch("DROP TRIGGER reject_compaction;")
        .unwrap();
    drop(conn);
    drop(coordinator);
    drop(db);
    let db = Database::open(root.join("facts.db")).unwrap();
    let coordinator = KernelCoordinator::reopen(&db, &clock, &run_id, &cancellation).unwrap();
    assert_eq!(
        coordinator
            .dispatch_pending_compaction("new-owner", |_, _, _, _| panic!(
                "unknown request must not repeat"
            ))
            .unwrap_err(),
        context::UNCERTAIN
    );
    drop(coordinator);
    super::super::super::kernel_host::drive(
        super::super::super::kernel_host::acquire(&root, &run_id).unwrap(),
        &db,
        &clock,
        &cancellation,
        &run_id,
        &real_worker_command(),
        "",
        &Allow,
        |_, _, _| panic!("no resources"),
    )
    .unwrap();
    let conn = rusqlite::Connection::open(root.join("facts.db")).unwrap();
    let error: String = conn
        .query_row(
            "SELECT error_code FROM runs WHERE id=?1",
            [&run_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(error, context::UNCERTAIN);
}

#[test]
fn kernel_compaction_queued_cancel_wins_late_summary_transaction() {
    let clock = TestClock::new(1000);
    let cancellation = CancellationRegistry::default();
    let (db, root, run_id) = compaction_fixture(&clock);
    let coordinator =
        KernelCoordinator::start_prepared(&db, &clock, &run_id, &cancellation).unwrap();
    coordinator
        .prepare_context_if_needed("initial", 4096)
        .unwrap();
    assert!(coordinator
        .dispatch_pending_compaction("owner", |_, request, _, _| {
            db.queue_kernel_host_command(&run_id, None).unwrap();
            Ok(response(request))
        })
        .is_err());
    assert!(db
        .kernel_compaction_results(&run_id, "initial")
        .unwrap()
        .is_empty());
    drop(coordinator);
    super::super::super::kernel_host::drive(
        super::super::super::kernel_host::acquire(&root, &run_id).unwrap(),
        &db,
        &clock,
        &cancellation,
        &run_id,
        &real_worker_command(),
        "",
        &Allow,
        |_, _, _| panic!("no resources"),
    )
    .unwrap();
    assert_eq!(
        db.kernel_build_full_snapshot(&run_id).unwrap().state,
        "cancelled"
    );
}

#[test]
fn kernel_compaction_single_owner_and_invalid_response_never_publish_a_view() {
    let clock = TestClock::new(1000);
    let cancellation = CancellationRegistry::default();
    let (db, _root, run_id) = compaction_fixture(&clock);
    let coordinator =
        KernelCoordinator::start_prepared(&db, &clock, &run_id, &cancellation).unwrap();
    coordinator
        .prepare_context_if_needed("initial", 4096)
        .unwrap();
    let other = KernelCoordinator::reopen(&db, &clock, &run_id, &cancellation).unwrap();
    coordinator
        .dispatch_pending_compaction("owner", |_, request, _, _| {
            assert!(other
                .dispatch_pending_compaction("rival", |_, _, _, _| panic!("only one model request"))
                .is_err());
            let stale = coordinator.controller.lock().unwrap().persist_command(&[]);
            clock.advance(200);
            coordinator.tick().unwrap();
            assert!(
                db.kernel_commit_decision(&run_id, clock.now_wall_ms(), &stale)
                    .is_err(),
                "replayed ticks cannot refund compaction execution time"
            );
            Ok(response(request))
        })
        .unwrap();
    assert_eq!(
        db.kernel_compaction_results(&run_id, "initial")
            .unwrap()
            .len(),
        1
    );
    assert!(
        other.tick().is_err(),
        "stale tick cannot restore a completed compaction"
    );
    let (db, _root, run_id) = compaction_fixture(&clock);
    let coordinator =
        KernelCoordinator::start_prepared(&db, &clock, &run_id, &cancellation).unwrap();
    coordinator
        .prepare_context_if_needed("initial", 4096)
        .unwrap();
    assert!(coordinator
        .dispatch_pending_compaction("owner", |_, request, _, _| {
            let mut invalid = response(request);
            invalid.input_hash = "sha256:foreign".into();
            Ok(invalid)
        })
        .is_err());
    assert!(db
        .kernel_compaction_results(&run_id, "initial")
        .unwrap()
        .is_empty());
}

#[test]
fn kernel_compaction_host_drives_real_isolated_summarizer_then_normal_model() {
    let clock = TestClock::new(1000);
    let cancellation = CancellationRegistry::default();
    let (db, root, run_id) = compaction_fixture(&clock);
    let original = db.kernel_initial_input(&run_id).unwrap();
    super::super::super::kernel_host::drive(
        super::super::super::kernel_host::acquire(&root, &run_id).unwrap(),
        &db,
        &clock,
        &cancellation,
        &run_id,
        &real_worker_command(),
        "",
        &Allow,
        |_, _, _| panic!("summarizer cannot execute resources"),
    )
    .unwrap();
    let snapshot = db.kernel_build_full_snapshot(&run_id).unwrap();
    assert_eq!(snapshot.state, "completed");
    assert!(snapshot.compaction.compactions > 0);
    assert!(snapshot.tool_calls.is_empty());
    assert_eq!(db.kernel_initial_input(&run_id).unwrap(), original);
    let conn = rusqlite::Connection::open(root.join("facts.db")).unwrap();
    let visible: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM messages WHERE run_id=?1 AND role='assistant'",
            [&run_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(visible, 1, "only the normal model answer is visible");
}

#[test]
fn kernel_compaction_input_transaction_failure_leaves_no_intent_and_can_prepare_again() {
    let clock = TestClock::new(1000);
    let cancellation = CancellationRegistry::default();
    let (db, root, run_id) = compaction_fixture(&clock);
    let coordinator =
        KernelCoordinator::start_prepared(&db, &clock, &run_id, &cancellation).unwrap();
    let before = coordinator.snapshot().unwrap();
    let conn = rusqlite::Connection::open(root.join("facts.db")).unwrap();
    conn.execute_batch("CREATE TRIGGER reject_compaction_input BEFORE INSERT ON kernel_events WHEN NEW.event_type='context.compaction.prepared' BEGIN SELECT RAISE(ABORT,'injected input failure'); END;").unwrap();
    assert!(coordinator
        .prepare_context_if_needed("initial", 4096)
        .is_err());
    assert_eq!(coordinator.snapshot().unwrap(), before);
    conn.execute_batch("DROP TRIGGER reject_compaction_input;")
        .unwrap();
    assert!(coordinator
        .prepare_context_if_needed("initial", 4096)
        .unwrap());
    assert!(coordinator
        .snapshot()
        .unwrap()
        .compaction
        .pending
        .unwrap()
        .owner
        .is_none());
}

#[test]
fn kernel_compaction_after_settled_tool_batch_preserves_approval_and_result_facts() {
    let clock = TestClock::new(1000);
    let cancellation = CancellationRegistry::default();
    let (db, root, run_id) = compaction_fixture(&clock);
    let original = db.kernel_initial_input(&run_id).unwrap();
    let coordinator =
        KernelCoordinator::start_prepared(&db, &clock, &run_id, &cancellation).unwrap();
    coordinator.dispatch_initial("initial-owner",&Ask,|binding,frame,_|Ok(fox_engine_protocol::KernelInitialModelResponse {
        schema_version:1,run_id:binding.run_id.clone(),turn_id:frame.input.turn_id.clone(),checkpoint_seq:frame.checkpoint_seq,
        assistant_message:json!({"role":"assistant","stopReason":"toolUse","content":[
            {"type":"toolCall","id":"read-protected","name":"read","arguments":{"path":"proof.txt"}}]})
    })).unwrap();
    let snapshot = coordinator.snapshot().unwrap();
    let batch = snapshot.tool_calls[0].batch_id.clone();
    assert!(
        coordinator.prepare_context_if_needed(&batch, 4096).is_err(),
        "approval boundary cannot compact"
    );
    coordinator
        .resolve_approval("read-protected", kernel::ApprovalDecision::AllowOnce)
        .unwrap();
    coordinator.dispatch_tool("read-protected","read-owner",|_,_,_|Ok((true,json!({"content":[{"type":"text","text":std::fs::read_to_string(root.join("proof.txt")).unwrap()}]})))).unwrap();
    let before = coordinator.prepare_stored_batch_resume(&batch).unwrap();
    let facts = coordinator.snapshot().unwrap().tool_calls;
    assert!(coordinator.prepare_context_if_needed(&batch, 4096).unwrap());
    coordinator
        .dispatch_pending_compaction("summary-owner", |_, request, _, _| Ok(response(request)))
        .unwrap();
    let after = coordinator.prepare_stored_batch_resume(&batch).unwrap();
    assert_eq!(after.tools, before.tools);
    assert_eq!(after.assistant_message, before.assistant_message);
    assert_eq!(coordinator.snapshot().unwrap().tool_calls, facts);
    assert!(after.tools[0].result["content"]
        .as_array()
        .unwrap()
        .iter()
        .any(|block| block["text"]
            .as_str()
            .unwrap_or("")
            .contains("FOX_EXECUTION_RECEIPT_V1")));
    assert_eq!(db.kernel_initial_input(&run_id).unwrap(), original);
    coordinator.dispatch_batch(&batch,"final-owner",&Allow,|binding,frame,_|Ok(fox_engine_protocol::KernelModelResponse {
        schema_version:1,run_id:binding.run_id.clone(),turn_id:frame.turn_id.clone(),batch_id:frame.batch_id.clone(),checkpoint_seq:frame.checkpoint_seq,
        assistant_message:json!({"role":"assistant","stopReason":"stop","content":[{"type":"text","text":"done"}]})
    })).unwrap();
    assert_eq!(coordinator.snapshot().unwrap().state, "completed");
}

#[test]
fn kernel_compaction_has_a_persisted_pass_limit_instead_of_unbounded_summary_calls() {
    let clock = TestClock::new(1000);
    let cancellation = CancellationRegistry::default();
    let (db, root, run_id) = compaction_fixture_count(&clock, 90);
    let original = db.kernel_initial_input(&run_id).unwrap();
    let coordinator =
        KernelCoordinator::start_prepared(&db, &clock, &run_id, &cancellation).unwrap();
    for _ in 0..context::MAX_PASSES {
        assert!(coordinator
            .prepare_context_if_needed("initial", 4096)
            .unwrap());
        coordinator
            .dispatch_pending_compaction("owner", |_, request, _, _| Ok(response(request)))
            .unwrap();
    }
    drop(coordinator);
    drop(db);
    let db = Database::open(root.join("facts.db")).unwrap();
    let coordinator = KernelCoordinator::reopen(&db, &clock, &run_id, &cancellation).unwrap();
    assert_eq!(
        coordinator
            .prepare_context_if_needed("initial", 4096)
            .unwrap_err(),
        context::INSUFFICIENT
    );
    assert_eq!(
        coordinator.snapshot().unwrap().compaction.compactions,
        context::MAX_PASSES as u32
    );
    assert_eq!(db.kernel_initial_input(&run_id).unwrap(), original);
}
