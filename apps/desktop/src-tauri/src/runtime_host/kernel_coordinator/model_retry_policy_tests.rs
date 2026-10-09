//! Classified model-retry policy: the delay/count table, its durable
//! accounting and its interaction with cancellation, restart and tool state.
//!
//! Every timing assertion uses the coordinator's controllable clock
//! (`TestClock`); nothing here depends on wall-clock time.
use super::*;

/// One settled failure built from the wire shape the worker actually sends, so
/// the classification is exercised against the real protocol vocabulary.
fn failure(value: Value) -> fox_engine_protocol::KernelModelFailure {
    let failure: fox_engine_protocol::KernelModelFailure =
        serde_json::from_value(value).expect("the fixture must match the wire shape");
    failure.validate().expect("the fixture must be a valid settled failure");
    failure
}

fn provider_failure(
    run_id: &str,
    status: u16,
    retry_after_ms: Option<u64>,
) -> fox_engine_protocol::KernelModelFailure {
    failure(json!({
        "schemaVersion": 1, "runId": run_id, "turnId": "turn-1", "checkpointSeq": 7,
        "category": "provider_unavailable", "httpStatus": status, "retryAfterMs": retry_after_ms,
    }))
}

fn incomplete_failure(run_id: &str, diagnostic: Value) -> fox_engine_protocol::KernelModelFailure {
    failure(json!({
        "schemaVersion": 1, "runId": run_id, "turnId": "turn-1", "checkpointSeq": 7,
        "category": "incomplete_response", "httpStatus": null, "retryAfterMs": null,
        "diagnostic": diagnostic,
    }))
}

fn timeout_failure_of(run_id: &str) -> fox_engine_protocol::KernelModelFailure {
    failure(json!({
        "schemaVersion": 1, "runId": run_id, "turnId": "turn-1", "checkpointSeq": 7,
        "category": "model_timeout", "httpStatus": null, "retryAfterMs": null,
    }))
}

fn retry_state(spent: u32) -> kernel::RetryState {
    kernel::RetryState {
        provider_max: 5,
        turn_max: 5,
        model_attempts: spent,
        ..kernel::RetryState::default()
    }
}

/// Deterministic jitter means one exact expected value per (run, key, attempt).
fn expected_ladder_delay(run_id: &str, effect_key: &str, attempt: u32) -> i64 {
    let rung = model_retry_ladder_ms(attempt);
    let jitter = model_retry_jitter_permille(run_id, effect_key, attempt);
    (rung * (1_000 + jitter) / 1_000).max(1)
}

// ---------------------------------------------------------------------------
// The delay / count table itself.
// ---------------------------------------------------------------------------

#[test]
fn classified_ladder_is_one_two_four_eight_ten_with_bounded_jitter() {
    let run = "ladder-run";
    let key = "deliver-batch:ladder";
    let mut previous = 0;
    for attempt in 0..5u32 {
        let plan =
            plan_model_retry(&provider_failure(run, 503, None), &retry_state(attempt), run, key);
        assert_eq!(plan.class, "transient_server_error", "attempt {attempt}");
        let delay = plan.delay_ms.expect("a transient server error is retried");
        assert_eq!(delay, expected_ladder_delay(run, key, attempt), "attempt {attempt}");
        let rung = model_retry_ladder_ms(attempt);
        assert!(
            delay >= rung - rung / 10 && delay <= rung + rung / 10 + 1,
            "attempt {attempt}: {delay} must stay within ±10% of {rung}"
        );
        assert!(delay > previous, "the ladder must grow: {previous} then {delay}");
        previous = delay;
    }
    // The rung values themselves are the stated table, independent of jitter.
    assert_eq!(MODEL_RETRY_LADDER_MS, [1_000, 2_000, 4_000, 8_000, 10_000]);
    // Beyond the last rung the ladder stays at 10s: the cumulative budget, not the
    // rung index, is what stops the attempts.
    assert_eq!(model_retry_ladder_ms(5), 10_000);
    assert_eq!(model_retry_ladder_ms(99), 10_000);
    // Jitter is bounded to ±10% and is reproducible from durable identity.
    for attempt in 0..6u32 {
        for (run, key) in [("a", "b"), ("ladder-run", "deliver-batch:ladder"), ("x", "y")] {
            let jitter = model_retry_jitter_permille(run, key, attempt);
            assert!((-100..=100).contains(&jitter), "{jitter}");
            assert_eq!(jitter, model_retry_jitter_permille(run, key, attempt));
        }
    }
}

#[test]
fn server_directed_wait_wins_and_jitter_never_shortens_it() {
    for (index, key) in ["deliver-batch:a", "continuation:7", "continuation:148", "deliver-batch:z"]
        .iter()
        .enumerate()
    {
        for attempt in 0..5u32 {
            let run = format!("server-directed-{index}");
            let plan = plan_model_retry(
                &provider_failure(&run, 429, Some(7_000)),
                &retry_state(attempt),
                &run,
                key,
            );
            assert_eq!(plan.class, "server_directed_wait");
            let delay = plan.delay_ms.expect("a server-directed wait is still a retry");
            assert!(delay >= 7_000, "attempt {attempt}: {delay} is earlier than Retry-After");
            // The wait is max(server wait, current ladder rung) plus jitter that is
            // only ever added, never subtracted.
            let ceiling = model_retry_ladder_ms(attempt).max(7_000) * 11 / 10 + 1;
            assert!(
                delay <= ceiling,
                "attempt {attempt}: {delay} must not overshoot beyond its upward jitter ({ceiling})"
            );
        }
    }
    // A server wait longer than the current ladder rung still wins.
    let plan = plan_model_retry(
        &provider_failure("long", 503, Some(60_000)),
        &retry_state(0),
        "long",
        "deliver-batch:x",
    );
    assert!(plan.delay_ms.unwrap() >= 60_000);
}

#[test]
fn timeouts_and_missing_evidence_never_get_a_one_second_retry() {
    let run = "flat-run";
    let key = "deliver-batch:flat";
    let absent = failure(json!({
        "schemaVersion": 1, "runId": run, "turnId": "turn-1", "checkpointSeq": 7,
        "category": "incomplete_response", "httpStatus": null, "retryAfterMs": null,
    }));
    for (label, settled, class) in [
        ("model_timeout", timeout_failure_of(run), "model_timeout"),
        (
            "missing_final_marker",
            incomplete_failure(run, json!({"reason": "missing_final_marker", "stopReason": "stop",
                "textChars": 120, "markerSeen": false, "completionRequired": true})),
            "missing_final_marker",
        ),
        (
            "incomplete_tool_proposal",
            incomplete_failure(run, json!({"reason": "incomplete_tool_proposal", "toolCallCount": 1})),
            "incomplete_tool_proposal",
        ),
        ("transport_no_output", incomplete_failure(run, json!({"reason": "transport_no_output"})),
            "unknown_evidence"),
        ("no diagnostic at all", absent, "unknown_evidence"),
    ] {
        let plan = plan_model_retry(&settled, &retry_state(0), run, key);
        assert_eq!(plan.class, class, "{label}");
        assert_eq!(
            plan.delay_ms,
            Some(MODEL_RETRY_INTERVAL_MS),
            "{label} must wait the conservative interval, not 1s"
        );
    }
    // A rate/quota limit without a server-directed wait is not dense either.
    let plan = plan_model_retry(&provider_failure(run, 429, None), &retry_state(0), run, key);
    assert_eq!(plan.class, "rate_limited");
    assert_eq!(plan.delay_ms, Some(MODEL_RETRY_INTERVAL_MS));
    // An unrecognised category is refused rather than guessed into a retry. The
    // frozen protocol cannot carry one, so it is built directly.
    let mut unknown = timeout_failure_of(run);
    unknown.category = "not_a_frozen_category".into();
    let class = classify_model_failure(&unknown);
    assert_eq!(class.token, "unknown_category");
    assert_eq!(class.wait, RetryWait::Refused);
    assert_eq!(plan_model_retry(&unknown, &retry_state(0), run, key).delay_ms, None);
}

#[test]
fn transport_losses_the_host_observed_are_dense_but_evidence_poor_ones_are_not() {
    let run = "transport-run";
    let key = "deliver-batch:transport";
    // The Host itself reaped the worker / its own model window expired: no
    // provider verdict ever arrived, and that is a transient connection failure.
    let reaped = failure(json!({
        "schemaVersion": 1, "runId": run, "turnId": "turn-1", "checkpointSeq": 7,
        "category": "model_transport_failure", "httpStatus": null, "retryAfterMs": null,
    }));
    let plan = plan_model_retry(&reaped, &retry_state(0), run, key);
    assert_eq!(plan.class, "transient_connection_failure");
    assert!(plan.delay_ms.unwrap() <= 1_100, "the first ladder rung is 1s ±10%");

    // A worker-reported transport failure with an unobserved cause: an auth,
    // permission or invalid-argument rejection looks exactly like this on today's
    // wire, so it must not be dense-retried.
    let unobserved = failure(json!({
        "schemaVersion": 1, "runId": run, "turnId": "turn-1", "checkpointSeq": 7,
        "category": "model_transport_failure", "httpStatus": null, "retryAfterMs": null,
        "diagnostic": {"reason": "unknown", "stopReason": "error", "textChars": 0,
            "thinkingChars": 0, "toolCallCount": 0},
    }));
    let plan = plan_model_retry(&unobserved, &retry_state(0), run, key);
    assert_eq!(plan.class, "unknown_evidence");
    assert_eq!(plan.delay_ms, Some(MODEL_RETRY_INTERVAL_MS));

    // A transport failure whose own branch named the cause keeps that branch.
    let interrupted = failure(json!({
        "schemaVersion": 1, "runId": run, "turnId": "turn-1", "checkpointSeq": 7,
        "category": "model_transport_failure", "httpStatus": null, "retryAfterMs": null,
        "diagnostic": {"reason": "stream_interrupted"},
    }));
    let plan = plan_model_retry(&interrupted, &retry_state(0), run, key);
    assert_eq!(plan.class, "stream_interrupted");
    assert!(plan.delay_ms.is_some());
}

#[test]
fn length_truncated_rounds_leave_the_network_ladder_for_one_recovery() {
    let run = "truncated-run";
    // The 2026-10-07 incident shape: no public text, the whole output budget spent
    // on thinking, provider stop reason `length`. It must NOT be re-sent on the
    // network ladder: it is classified as a length truncation and planned as the
    // single controlled recovery instead.
    let incident = incomplete_failure(run, json!({
        "reason": "empty_final_answer", "stopReason": "length", "textChars": 0,
        "thinkingChars": 27_000, "toolCallCount": 0, "markerSeen": false, "completionRequired": true,
    }));
    let plan = plan_model_retry(&incident, &retry_state(0), run, "deliver-batch:incident");
    assert_eq!(plan.class, "length_truncated_no_body");
    assert_eq!(plan.wait, RetryWait::ControlledRecovery);
    assert!(plan.needs_recovery_record());
    assert_eq!(plan.delay_ms, Some(MODEL_LENGTH_RECOVERY_INTERVAL_MS));
    assert_ne!(plan.delay_ms, Some(MODEL_RETRY_INTERVAL_MS), "not the flat interval");
    for rung in MODEL_RETRY_LADDER_MS {
        assert_ne!(plan.delay_ms, Some(rung), "never a network-ladder rung");
    }

    let clipped =
        incomplete_failure(run, json!({"reason": "output_length_limit", "stopReason": "length"}));
    let plan = plan_model_retry(&clipped, &retry_state(0), run, "deliver-batch:clipped");
    assert_eq!(plan.class, "length_truncated_partial_body");
    assert_eq!(plan.wait, RetryWait::ControlledRecovery);

    // An empty answer the provider did NOT truncate is a transient empty body: it
    // stays on the ladder, with its own token.
    let transient_empty =
        incomplete_failure(run, json!({"reason": "empty_final_answer", "stopReason": "stop"}));
    let plan = plan_model_retry(&transient_empty, &retry_state(0), run, "deliver-batch:empty");
    assert_eq!(plan.class, "empty_answer");
    assert_eq!(plan.wait, RetryWait::Ladder);

    // A stream that broke mid-flight is transient too, and keeps its own token.
    let interrupted = incomplete_failure(run, json!({"reason": "stream_interrupted"}));
    let plan = plan_model_retry(&interrupted, &retry_state(0), run, "deliver-batch:broken");
    assert_eq!(plan.class, "stream_interrupted");
    assert_eq!(plan.wait, RetryWait::Ladder);
}

#[test]
fn the_three_length_truncation_shapes_stay_distinguishable() {
    let run = "shapes-run";
    let cases = [
        (
            "thinking-only, zero body",
            json!({"reason": "empty_final_answer", "stopReason": "length", "textChars": 0,
                "thinkingChars": 25_000, "toolCallCount": 0}),
            "length_truncated_no_body",
        ),
        (
            "partial body cut off",
            json!({"reason": "output_length_limit", "stopReason": "length", "textChars": 4_000,
                "thinkingChars": 900, "toolCallCount": 0}),
            "length_truncated_partial_body",
        ),
        (
            "tool proposal cut off",
            json!({"reason": "incomplete_tool_proposal", "stopReason": "length",
                "textChars": 0, "toolCallCount": 1}),
            "length_truncated_tool_proposal",
        ),
        (
            "worker named the empty round, stop reason length",
            json!({"reason": "empty_final_answer", "stopReason": "length"}),
            "length_truncated_no_body",
        ),
        (
            "length with no body counter at all",
            json!({"stopReason": "length"}),
            "length_truncated_unknown_body",
        ),
    ];
    let mut seen: Vec<&str> = Vec::new();
    for (label, diagnostic, expected) in cases {
        let failure = incomplete_failure(run, diagnostic);
        let class = classify_model_failure(&failure);
        assert_eq!(class.token, expected, "{label}");
        assert_eq!(class.wait, RetryWait::ControlledRecovery, "{label}");
        seen.push(class.token);
    }
    // The three observable shapes are genuinely distinct tokens.
    assert!(seen.contains(&"length_truncated_no_body"));
    assert!(seen.contains(&"length_truncated_partial_body"));
    assert!(seen.contains(&"length_truncated_tool_proposal"));
}

#[test]
fn cumulative_budget_refuses_after_five_whatever_the_category_or_effect_key() {
    let run = "budget-run";
    for key in ["deliver-batch:b", "continuation:148", "initial-model"] {
        let plan = plan_model_retry(&provider_failure(run, 503, None), &retry_state(5), run, key);
        assert_eq!(plan.class, "retry_budget_spent");
        assert_eq!(plan.delay_ms, None);
        // A category switch cannot buy an attempt either.
        let plan = plan_model_retry(&timeout_failure_of(run), &retry_state(5), run, key);
        assert_eq!(plan.class, "retry_budget_spent");
        let plan = plan_model_retry(
            &incomplete_failure(run, json!({"reason": "missing_final_marker"})),
            &retry_state(5),
            run,
            key,
        );
        assert_eq!(plan.class, "retry_budget_spent");
        // Four spent still admits the fifth and last retry.
        let plan = plan_model_retry(&provider_failure(run, 503, None), &retry_state(4), run, key);
        assert!(plan.delay_ms.is_some(), "the fifth retry is still admitted");
    }
    // A strict frozen policy keeps its own stricter budget (never widened).
    let strict = kernel::RetryState {
        provider_max: 0,
        turn_max: 1,
        model_attempts: 1,
        ..kernel::RetryState::default()
    };
    assert_eq!(
        plan_model_retry(&provider_failure(run, 503, None), &strict, run, "deliver-batch:b").class,
        "retry_budget_spent"
    );
    // The same shape admits its configured single retry.
    let strict_fresh = kernel::RetryState {
        provider_max: 0,
        turn_max: 1,
        model_attempts: 0,
        ..kernel::RetryState::default()
    };
    assert!(plan_model_retry(&timeout_failure_of(run), &strict_fresh, run, "deliver-batch:b")
        .delay_ms
        .is_some());
    // The hard cap is 5 no matter how the frozen config was written.
    assert_eq!(MODEL_RETRY_MAX_ATTEMPTS, 5);
    let wide = kernel::RetryState {
        provider_max: 5,
        turn_max: 5,
        model_attempts: 5,
        ..kernel::RetryState::default()
    };
    assert!(plan_model_retry(&provider_failure(run, 503, None), &wide, run, "deliver-batch:b")
        .delay_ms
        .is_none());
}

// ---------------------------------------------------------------------------
// Integration: the classified policy through the real durable lanes.
// ---------------------------------------------------------------------------

fn failure_json(
    turn_id: &str,
    run_id: &str,
    checkpoint_seq: u64,
    category: &str,
    extra: Value,
) -> String {
    let mut value = json!({
        "schemaVersion": 1, "runId": run_id, "turnId": turn_id,
        "checkpointSeq": checkpoint_seq, "category": category,
        "httpStatus": null, "retryAfterMs": null,
    });
    for (key, item) in extra.as_object().unwrap() {
        value[key] = item.clone();
    }
    format!("kernel.settled_model_failure:{}", value)
}

/// Read the durable scheduling events through a separate read-only connection.
fn retry_events(root: &std::path::Path, run_id: &str) -> Vec<Value> {
    let connection = rusqlite::Connection::open(root.join("facts.db")).unwrap();
    let mut query = connection
        .prepare("SELECT payload_json FROM kernel_events WHERE run_id=?1 AND event_type='run.retrying' ORDER BY seq")
        .unwrap();
    let rows = query.query_map([run_id], |row| row.get::<_, String>(0)).unwrap();
    rows.map(|row| serde_json::from_str::<Value>(&row.unwrap()).unwrap()).collect()
}

fn terminal_failure(root: &std::path::Path, run_id: &str) -> (String, String) {
    let connection = rusqlite::Connection::open(root.join("facts.db")).unwrap();
    connection
        .query_row(
            "SELECT json_extract(payload_json,'$.code'), json_extract(payload_json,'$.message')
             FROM kernel_events WHERE run_id=?1 AND event_type='run.failed'",
            [run_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap()
}

#[test]
fn classified_retry_lane_persists_its_class_delay_and_due_time() {
    let clock = TestClock::new(1_000);
    let cancellation = CancellationRegistry::default();
    let (db, root, run_id) = fixture_with_retry_opt(&clock, "policy-prompt", None, false, false, (5, 5));
    let coordinator = KernelCoordinator::reopen(&db, &clock, &run_id, &cancellation).unwrap();
    settle_worker_batch(&coordinator);
    let tools = coordinator.snapshot().unwrap().tool_calls;
    let key = "deliver-batch:worker-batch";

    for attempt in 0..3u32 {
        coordinator
            .dispatch_batch("worker-batch", &format!("attempt-{attempt}"), &Allow, |binding, frame, _| {
                Err(failure_json(&frame.turn_id, &binding.run_id, frame.checkpoint_seq,
                    "provider_unavailable", json!({"httpStatus": 503})))
            })
            .unwrap();
        let payload = retry_events(&root, &run_id).pop().unwrap();
        assert_eq!(payload["class"], "transient_server_error");
        assert_eq!(payload["modelAttempt"], attempt + 1);
        assert_eq!(payload["modelMaxAttempts"], 5);
        assert_eq!(payload["kind"], "provider");
        let expected = expected_ladder_delay(&run_id, key, attempt);
        assert_eq!(payload["delayMs"].as_i64().unwrap(), expected, "attempt {attempt}");
        let scheduled = payload["scheduledAtWallMs"].as_i64().unwrap();
        assert_eq!(payload["dueWallMs"].as_i64().unwrap(), scheduled + expected);
        assert_eq!(coordinator.snapshot().unwrap().retry.due_wall_ms, Some(scheduled + expected));
        assert_eq!(coordinator.snapshot().unwrap().state, "retry_scheduled");
        // The waiting state really is a wait: an early claim is refused.
        assert!(coordinator
            .dispatch_batch("worker-batch", "early", &Allow, |_, _, _| panic!("not due yet"))
            .is_err());
        // Resuming happens only at the durable due time, and clears the schedule.
        clock.advance(payload["dueWallMs"].as_i64().unwrap() - clock.get());
        coordinator.tick().unwrap();
        assert_eq!(coordinator.snapshot().unwrap().state, "running");
        assert_eq!(coordinator.snapshot().unwrap().retry.due_wall_ms, None);
    }
    assert_eq!(coordinator.snapshot().unwrap().tool_calls, tools, "no tool may be replayed");
}

#[test]
fn category_switch_never_resets_the_cumulative_limit_and_tools_are_not_replayed() {
    let clock = TestClock::new(1_000);
    let cancellation = CancellationRegistry::default();
    let (db, root, run_id) = fixture_with_retry_opt(&clock, "policy-mix", None, false, false, (5, 5));
    let coordinator = KernelCoordinator::reopen(&db, &clock, &run_id, &cancellation).unwrap();
    settle_worker_batch(&coordinator);
    let tools = coordinator.snapshot().unwrap().tool_calls;

    // Five retries, switching category every round: 3 provider + 2 timeout. No
    // single category reaches its own maximum, so only the Run-scoped cumulative
    // budget can stop the sixth failure.
    let categories = [
        "provider_unavailable", "model_timeout", "provider_unavailable",
        "model_timeout", "provider_unavailable",
    ];
    for (index, category) in categories.iter().enumerate() {
        coordinator
            .dispatch_batch("worker-batch", &format!("mix-{index}"), &Allow, |binding, frame, _| {
                let extra = if *category == "provider_unavailable" {
                    json!({"httpStatus": 503})
                } else {
                    json!({})
                };
                Err(failure_json(&frame.turn_id, &binding.run_id, frame.checkpoint_seq, category, extra))
            })
            .unwrap_or_else(|error| panic!("round {index} ({category}) must be admitted: {error}"));
        assert_eq!(coordinator.snapshot().unwrap().state, "retry_scheduled", "round {index}");
        let due = coordinator.snapshot().unwrap().retry.due_wall_ms.unwrap();
        clock.advance(due - clock.get());
        coordinator.tick().unwrap();
    }
    let spent = coordinator.snapshot().unwrap().retry.clone();
    assert_eq!(spent.model_attempts, 5, "five retries were admitted in total");
    assert_eq!(spent.provider_attempts, 3, "3 provider attempts, none at their own max");
    assert_eq!(spent.turn_attempts, 2, "2 timeout attempts, none at their own max");

    // The sixth failure is refused: the cumulative limit is spent even though both
    // per-category limits still have room.
    coordinator
        .dispatch_batch("worker-batch", "mix-6", &Allow, |binding, frame, _| {
            Err(failure_json(&frame.turn_id, &binding.run_id, frame.checkpoint_seq,
                "provider_unavailable", json!({"httpStatus": 503})))
        })
        .expect("the refusal is a terminal decision, not a dispatch error");
    let snapshot = coordinator.snapshot().unwrap();
    assert_eq!(snapshot.state, "failed");
    assert_eq!(snapshot.tool_calls, tools, "a settled tool is never executed twice");
    assert_eq!(retry_events(&root, &run_id).len(), 5, "exactly five retries, never 25");
    let terminal = db.kernel_rehydrate(&run_id).unwrap().unwrap().retry;
    assert_eq!(terminal.model_attempts, 5);
    assert_eq!(terminal.model_retry_class.as_deref(), Some("retry_budget_spent"));
    let (code, message) = terminal_failure(&root, &run_id);
    assert_eq!(code, "kernel.model_retry_exhausted");
    assert!(message.contains("最多 5 次"), "the refusal reason must reach the user: {message}");
    let connection = rusqlite::Connection::open(root.join("facts.db")).unwrap();
    let rejected: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM kernel_events WHERE run_id=?1 AND event_type='engine.model_rejected'",
            [&run_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(rejected, 6);
    let replayed: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM kernel_effect_outbox WHERE run_id=?1 AND effect_type='dispatch_tool' AND attempts<>1",
            [&run_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(replayed, 0, "no tool side effect may be replayed");
    assert!(db
        .kernel_reconciliation_view(&coordinator.binding.conversation_id, &run_id)
        .unwrap()
        .items
        .is_empty());
}

#[test]
fn a_length_truncated_round_never_returns_to_the_plain_ladder() {
    // The 2026-10-07 incident shape, driven through the durable lane: the round has
    // no room left for its answer, so the Run must leave the network ladder and take
    // exactly one controlled recovery (detailed coverage lives in
    // `length_recovery_tests`).
    let clock = TestClock::new(1_000);
    let cancellation = CancellationRegistry::default();
    // A frozen model configuration is mandatory for a real dispatch, and the
    // recovery records the parameters it negotiated from it.
    let config = worker_configuration();
    let (db, root, run_id) =
        fixture_with_retry_opt(&clock, &config.hash().unwrap(), Some(&config), false, false, (5, 5));
    let coordinator = KernelCoordinator::reopen(&db, &clock, &run_id, &cancellation).unwrap();
    settle_worker_batch(&coordinator);
    let tools = coordinator.snapshot().unwrap().tool_calls;

    coordinator
        .dispatch_batch("worker-batch", "truncated", &Allow, |binding, frame, _| {
            Err(failure_json(&frame.turn_id, &binding.run_id, frame.checkpoint_seq,
                "incomplete_response", json!({"diagnostic": {
                    "reason": "empty_final_answer", "stopReason": "length", "textChars": 0,
                    "thinkingChars": 27_000, "toolCallCount": 0, "markerSeen": false,
                    "completionRequired": true,
                }})))
        })
        .unwrap();
    let snapshot = coordinator.snapshot().unwrap();
    assert_eq!(snapshot.state, "retry_scheduled");
    assert_eq!(snapshot.tool_calls, tools, "durable progress is kept");
    assert_eq!(snapshot.retry.length_recovery_attempts, 1, "the one recovery is spent");
    let events = retry_events(&root, &run_id);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["class"], "length_truncated_no_body");
    assert_eq!(events[0]["delayMs"].as_i64(), Some(MODEL_LENGTH_RECOVERY_INTERVAL_MS));
    for rung in MODEL_RETRY_LADDER_MS {
        assert_ne!(events[0]["delayMs"].as_i64(), Some(rung), "never a ladder rung");
    }
    assert_eq!(events[0]["lengthRecoveryAttempt"], 1);
    assert_eq!(events[0]["lengthRecoveryMax"], 1);
    assert!(events[0]["recoveryId"].as_str().is_some());
}

#[test]
fn a_server_directed_wait_survives_a_restart_and_is_never_shortened() {
    let clock = TestClock::new(1_000);
    let cancellation = CancellationRegistry::default();
    let (db, root, run_id) = fixture_with_retry_opt(&clock, "policy-server", None, false, false, (5, 5));
    let coordinator = KernelCoordinator::reopen(&db, &clock, &run_id, &cancellation).unwrap();
    settle_worker_batch(&coordinator);
    coordinator
        .dispatch_batch("worker-batch", "server-wait", &Allow, |binding, frame, _| {
            Err(failure_json(&frame.turn_id, &binding.run_id, frame.checkpoint_seq,
                "provider_unavailable", json!({"httpStatus": 429, "retryAfterMs": 7_000})))
        })
        .unwrap();
    let payload = retry_events(&root, &run_id).pop().unwrap();
    assert_eq!(payload["class"], "server_directed_wait");
    let scheduled = payload["scheduledAtWallMs"].as_i64().unwrap();
    let due = payload["dueWallMs"].as_i64().unwrap();
    assert!(due >= scheduled + 7_000, "Retry-After must not be shortened: {due} vs {scheduled}");

    // Restart: the schedule and the cumulative accounting survive.
    drop(coordinator);
    drop(db);
    let db = Database::open(root.join("facts.db")).unwrap();
    let coordinator = KernelCoordinator::reopen(&db, &clock, &run_id, &cancellation).unwrap();
    let reopened = coordinator.snapshot().unwrap().retry.clone();
    assert_eq!(reopened.due_wall_ms, Some(due));
    assert_eq!(reopened.model_attempts, 1);
    assert_eq!(reopened.model_retry_class.as_deref(), Some("server_directed_wait"));
    clock.advance(due - 1 - clock.get());
    coordinator.tick().unwrap();
    assert_eq!(coordinator.snapshot().unwrap().state, "retry_scheduled", "not due yet");
    clock.advance(1);
    coordinator.tick().unwrap();
    assert_eq!(coordinator.snapshot().unwrap().state, "running");
    coordinator
        .dispatch_batch("worker-batch", "server-wait-done", &Allow, |binding, frame, _| {
            Ok(fox_engine_protocol::KernelModelResponse {
                schema_version: 1, run_id: binding.run_id.clone(), turn_id: frame.turn_id.clone(),
                batch_id: frame.batch_id.clone(), checkpoint_seq: frame.checkpoint_seq,
                assistant_message: json!({"role": "assistant", "stopReason": "stop",
                    "content": [{"type": "text", "text": "已恢复并完成。"}]}),
            })
        })
        .unwrap();
    let snapshot = coordinator.snapshot().unwrap();
    assert_eq!(snapshot.state, "completed");
    // An accepted response clears the stale retry state for the next request.
    assert_eq!(snapshot.retry.model_attempts, 0);
    assert_eq!(snapshot.retry.model_retry_class, None);
    assert_eq!(snapshot.retry.due_wall_ms, None);
    assert_eq!(retry_events(&root, &run_id).len(), 1, "exactly one retry was scheduled");
}

#[test]
fn cancellation_refuses_a_classified_retry_before_any_delay_is_scheduled() {
    let clock = TestClock::new(1_000);
    let cancellation = CancellationRegistry::default();
    let (db, root, run_id) = fixture_with_retry_opt(&clock, "policy-cancel", None, false, false, (5, 5));
    let coordinator = KernelCoordinator::reopen(&db, &clock, &run_id, &cancellation).unwrap();
    settle_worker_batch(&coordinator);
    // The user cancels while the round is still in flight, and the worker reports
    // an otherwise perfectly retryable transient server error at the same moment.
    let refused = coordinator
        .dispatch_batch("worker-batch", "cancel-race", &Allow, |binding, frame, _| {
            coordinator.cancel().unwrap();
            Err(failure_json(&frame.turn_id, &binding.run_id, frame.checkpoint_seq,
                "provider_unavailable", json!({"httpStatus": 503})))
        });
    assert!(refused.is_err(), "cancellation must refuse the retry");
    assert!(retry_events(&root, &run_id).is_empty(), "no delay may be scheduled");
    assert_ne!(coordinator.snapshot().unwrap().state, "retry_scheduled");
}
