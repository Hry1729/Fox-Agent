//! Mid-run steering ("运行中补充要求") persistence and dispatch-boundary
//! tests. The end-to-end live-loop scenario lives in real_eval_tests.rs as an
//! ignored real-runtime test; these cover the durable state machine without a
//! Node worker.

use super::*;
use serde_json::json;

pub(super) fn stop_response(
    frame: &fox_engine_protocol::KernelInitialModelFrame,
    binding: &RunControlBinding,
) -> fox_engine_protocol::KernelInitialModelResponse {
    fox_engine_protocol::KernelInitialModelResponse {
        schema_version: 1,
        run_id: binding.run_id.clone(),
        turn_id: frame.input.turn_id.clone(),
        checkpoint_seq: frame.checkpoint_seq,
        assistant_message: json!({"role":"assistant","stopReason":"stop",
            "content":[{"type":"text","text":"已完成"}]}),
    }
}

/// A prepared authoritative Run with a frozen model configuration and initial
/// input, so a dispatch boundary (initial/response) can actually be driven by
/// this test without a live Node worker.
pub(super) fn steering_model_fixture(clock: &TestClock) -> (Database, PathBuf, String) {
    let config = worker_configuration();
    let hash = config.hash().unwrap();
    let (db, root, run_id) =
        fixture_with_start_opt(clock, &hash, Some(&config), false, true);
    db.freeze_kernel_initial_input(&initial_input(&run_id, &hash)).unwrap();
    freeze_host_scope(&db, &run_id);
    (db, root, run_id)
}

#[test]
fn steering_enqueue_is_idempotent_ordered_and_bounded() {
    let clock = TestClock::new(1000);
    let (db, _root, run_id) = fixture(&clock);
    let now = crate::database::now_ms();
    let first = db
        .enqueue_run_steering(&run_id, "msg-1", "第一条补充", now)
        .unwrap();
    let second = db
        .enqueue_run_steering(&run_id, "msg-2", "第二条补充", now + 1)
        .unwrap();
    assert_eq!((first, second), (1, 2));
    // Same idempotency id returns the original row, never a copy.
    let duplicate = db
        .enqueue_run_steering(&run_id, "msg-1", "第一条补充-重发", now + 2)
        .unwrap();
    assert_eq!(duplicate, 1);
    let rows = db.run_steering_messages(&run_id).unwrap();
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().map(|row| row.seq).eq([1, 2]));
    assert_eq!(rows[0].content, "第一条补充");

    // Empty/whitespace text and overlong text are rejected, not sanitized.
    assert!(db
        .enqueue_run_steering(&run_id, "msg-empty", "   ", now)
        .is_err());
    let too_long = "中".repeat(8_001);
    assert!(db
        .enqueue_run_steering(&run_id, "msg-long", &too_long, now)
        .is_err());

    // Outstanding (received+delivered) bytes are bounded per Run at 64 KiB.
    // Each message must itself stay under the 8000-character ceiling, so the
    // queue is filled with several full blocks instead of one oversized message.
    let block = "x".repeat(8_000); // 8000 ASCII bytes, exactly at the per-message cap
    let mut accepted = 0;
    for index in 0..16 {
        if db
            .enqueue_run_steering(&run_id, &format!("bytes-{index}"), &block, now)
            .is_ok()
        {
            accepted += 1;
        } else {
            break;
        }
    }
    // The two Chinese rows already occupy 30 bytes; eight 8000-byte blocks
    // total 64 030 (<= 64 KiB) and a ninth would exceed the ceiling.
    assert_eq!(accepted, 8);

    // A foreign conversation cannot steer a run it does not own.
    let other_conversation = db
        .create_conversation(db.default_agent_id(), None, None, Some("read_only"))
        .unwrap();
    assert!(
        db.active_kernel_run_for_conversation(&other_conversation.id)
            .unwrap()
            .is_none()
    );
}

/// R3: the queue cap is measured in UTF-8 BYTES at the authoritative acceptance
/// boundary. SQLite's `length()` on TEXT counts characters, so a queue of
/// Chinese text used to be able to hold three times the declared byte cap.
#[test]
fn the_steering_queue_cap_counts_utf8_bytes_not_characters() {
    let clock = TestClock::new(1000);
    let (db, _root, run_id) = fixture(&clock);
    let now = crate::database::now_ms();
    // Each message is 8000 Chinese characters = 24 000 UTF-8 bytes, comfortably
    // under the 8000-character ceiling and far over one third of the 64 KiB cap.
    let block = "中".repeat(8_000);
    let first = db
        .enqueue_run_steering(&run_id, "bytes-cn-1", &block, now)
        .expect("the first 24 KiB block fits");
    assert_eq!(first, 1);
    let second = db
        .enqueue_run_steering(&run_id, "bytes-cn-2", &block, now + 1)
        .expect("the second 24 KiB block still fits");
    assert_eq!(second, 2);
    // 48 000 bytes are outstanding: a third block would reach 72 000 > 65 536, so
    // it must be refused. Under the old character-based accounting six such
    // messages were accepted, i.e. 144 000 bytes against a 64 KiB cap.
    let error = db
        .enqueue_run_steering(&run_id, "bytes-cn-3", &block, now + 2)
        .expect_err("the third block must exceed the byte cap");
    assert!(error.contains("full"), "unexpected error: {error}");
    let rows = db.run_steering_messages(&run_id).unwrap();
    assert_eq!(rows.len(), 2, "only the messages that fit are accepted");

    // Mixed text (emoji + ASCII + Chinese) is accounted the same way in a Run
    // that is actually prepared for dispatch.
    let (mixed_run, _mixed_root, mixed_id) = steering_model_fixture(&TestClock::new(2000));
    let mixed = "emoji😀中文abc";
    // 5 ASCII + one 4-byte emoji + 2 Chinese + 3 ASCII = 11 Unicode scalars and
    // 18 UTF-8 bytes: the queue cap counts the bytes, the per-message ceiling
    // counts the scalars.
    assert_eq!(mixed.chars().count(), 11);
    assert_eq!(mixed.len(), 18);
    mixed_run
        .enqueue_run_steering(&mixed_id, "mixed-1", mixed, now)
        .unwrap();
    let row = &mixed_run.run_steering_messages(&mixed_id).unwrap()[0];
    assert_eq!(row.content.chars().count(), 11);
    assert_eq!(row.content.len(), 18);
}

/// Durable steering-lane rounds recorded for the Run, written directly so a test
/// can place the Run at its additional-input budget without running four real
/// rounds.
fn record_steering_lane_rounds(db_path: &std::path::Path, run_id: &str, count: i64) {
    let conn = rusqlite::Connection::open(db_path).expect("seed connection");
    for index in 0..count {
        conn.execute(
            "INSERT INTO kernel_events(run_id, seq, event_type, payload_json, created_at)
             VALUES (?1, ?2, 'engine.continuation_requested', ?3, ?4)",
            rusqlite::params![
                run_id,
                10_000 + index,
                serde_json::json!({"lane": "steering", "effectKey": format!("continuation:{index}")})
                    .to_string(),
                crate::database::now_ms()
            ],
        )
        .expect("seed steering round");
    }
}

/// N3: the context gate must be asked about the request that will really be
/// sent. Measuring only the frozen history (plus the fixed reserve) let a Run
/// whose history fits but whose history-plus-queue does not detach on every
/// attempt without ever planning a compaction, so no attempt could progress.
#[test]
fn the_context_gate_accounts_for_the_input_it_will_actually_send() {
    let clock = TestClock::new(1000);
    let (db, _root, run_id) = steering_model_fixture(&clock);
    let cancellation = CancellationRegistry::default();
    let coordinator =
        KernelCoordinator::start_prepared(&db, &clock, &run_id, &cancellation).unwrap();

    // No outstanding input: nothing is added to the request.
    assert_eq!(coordinator.active_steering_bytes().unwrap(), 0);

    // Each queued row contributes its content plus the notice wrapper the model
    // sees — not just the raw text and not nothing.
    let content = "另外检查 Sheet2 的合计行";
    db.enqueue_run_steering(&run_id, "ctx-1", content, crate::database::now_ms())
        .unwrap();
    let one = coordinator.active_steering_bytes().unwrap();
    assert!(
        one > content.len(),
        "the notice wrapper must be part of the measured bytes: {one}"
    );
    let wrapper = super::steering::steering_notice_text(content);
    assert!(wrapper.contains(content));
    assert!(
        one >= wrapper.len(),
        "measured bytes must cover the full notice: {one} < {}",
        wrapper.len()
    );

    // A second row adds roughly its own bytes again, so the gate's answer moves
    // with the queue instead of staying pinned to the frozen history.
    db.enqueue_run_steering(&run_id, "ctx-2", content, crate::database::now_ms())
        .unwrap();
    let two = coordinator.active_steering_bytes().unwrap();
    assert!(two > one, "two rows must measure more than one: {two} <= {one}");

    // The gate itself: with a reserve that cannot absorb this queue it must stop
    // reporting "nothing to do". Either it plans a compaction, or it ends with
    // an explicit, classified capacity error — never a silent skip that leaves
    // the dispatch detaching forever. (#12 splits the old single INSUFFICIENT
    // into explained stop classes; all of them are explicit.)
    let huge_extra = 10 * 1024 * 1024;
    match coordinator.prepare_context_if_needed("initial", huge_extra) {
        Ok(false) => panic!("an impossible budget must not report 'nothing to do'"),
        Ok(true) => {} // a compaction plan was prepared: real progress
        Err(error) => assert!(
            [
                crate::kernel_compaction::INSUFFICIENT,
                crate::kernel_compaction::NO_CANDIDATES,
                crate::kernel_compaction::NO_REDUCTION,
                crate::kernel_compaction::SINGLE_TOO_LARGE,
            ]
            .contains(&error.as_str()),
            "capacity failure must be explicit and classified: {error}"
        ),
    }
}

/// N5/F1: a request accepted **inside the decision window** — after the last
/// queue read, before the write-set — must be answered by this round, not turn
/// the Run into a failure and not leave a leased model dispatch behind.
///
/// The barrier below is deterministic: it fires at the last moment before the
/// decision commits, which is exactly the window the terminal guard exists for.
#[test]
fn a_request_accepted_inside_the_decision_window_is_answered_not_failed() {
    let clock = TestClock::new(1000);
    let (db, _root, run_id) = steering_model_fixture(&clock);
    let cancellation = CancellationRegistry::default();
    let coordinator =
        KernelCoordinator::start_prepared(&db, &clock, &run_id, &cancellation).unwrap();

    let db_barrier = db.clone();
    let run_barrier = run_id.clone();
    super::live::test_barrier::install(
        &run_id,
        Box::new(move || {
            db_barrier
                .enqueue_run_steering(
                    &run_barrier,
                    "window-row",
                    "窗口内追加的要求",
                    crate::database::now_ms(),
                )
                .expect("the Host accepts a request while the Run is active");
        }),
    );

    let seq_before = coordinator.last_event_seq_for_test();
    // The round's model result is in hand; the commit must re-plan around the
    // arrival instead of failing.
    coordinator
        .dispatch_initial("owner", &Allow, |binding, frame, _| {
            Ok(stop_response(frame, binding))
        })
        .expect("a request accepted inside the decision window must not fail the Run");

    // The Run is alive, and the accepted request was bound to its own lane round.
    assert_ne!(coordinator.snapshot().unwrap().state, "completed");
    assert_eq!(coordinator.snapshot().unwrap().state, "running");
    assert_eq!(db.kernel_count_steering_followups(&run_id).unwrap(), 1);
    assert_eq!(db.kernel_count_continuations(&run_id).unwrap(), 0);
    let rows = db.run_steering_messages(&run_id).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].status, "received",
        "the non-live transport leaves the row for its own lane dispatch to collect \
         (single source: run_steering_messages); it must not be cancelled or lost: {rows:?}"
    );

    // The model result was preserved and the follow-up round carries the full
    // history: the reply exactly once, the prompt exactly once (F5).
    let staged_seq = coordinator.last_event_seq_for_test();
    assert!(staged_seq > seq_before);
    let staged = db
        .kernel_continuation_input(&run_id, &format!("continuation:{staged_seq}"))
        .expect("the steering round input is frozen with its decision");    let messages: Vec<serde_json::Value> = staged.messages.clone();
    let text_of = |message: &serde_json::Value| -> String {
        message["content"]
            .as_array()
            .map(|blocks| {
                blocks
                    .iter()
                    .filter_map(|block| block["text"].as_str())
                    .collect::<Vec<_>>()
                    .join("")
            })
            .unwrap_or_else(|| message["content"].as_str().unwrap_or_default().to_owned())
    };
    for (label, needle) in [
        ("the initial user message", "初始问题"),
        ("the round reply", "已完成"),
        ("the steering prompt", "Fox 补充要求处理"),
    ] {
        let count = messages
            .iter()
            .filter(|message| text_of(message).contains(needle))
            .count();
        assert_eq!(count, 1, "{label} must appear exactly once: {messages:?}");
    }
    assert!(
        !messages.iter().any(|message| text_of(message).contains("Fox 续答检查")),
        "the accepted input must not ride the stop-review prompt"
    );
    // Order: initial user turn, then the reply it stopped on, then the prompt.
    let reply_at = messages
        .iter()
        .position(|message| text_of(message).contains("已完成"))
        .unwrap();
    let prompt_at = messages
        .iter()
        .position(|message| text_of(message).contains("Fox 补充要求处理"))
        .unwrap();
    assert_eq!(reply_at + 1, prompt_at, "prompt must directly follow the reply");

    // Restart evidence: a fresh coordinator over the same database sees a healthy
    // in-progress Run, and the durable facts say the same thing — no uncertain
    // leased dispatch was left behind, and the additional-input round is still
    // dispatchable.
    let restart_clock = TestClock::new(1000);
    let reopened = KernelCoordinator::reopen(&db, &restart_clock, &run_id, &cancellation)
        .expect("a restart must recover the raced Run from durable facts");
    assert_eq!(reopened.snapshot().unwrap().state, "running");
    let uncertain_event = "kernel.uncertain_execution";
    let uncertain = db
        .query_count_raw(
            "SELECT COUNT(*) FROM kernel_events WHERE run_id=?1 AND event_type=?2",
            &[&run_id, &uncertain_event],
        )
        .unwrap();
    assert_eq!(uncertain, 0, "the recovered round left a leased dispatch behind");
    // The rehydrated Run still owns the additional-input round: the restart sees
    // the `request_continuation_model` effect it has to dispatch, so the accepted
    // request is answered after a restart instead of being stranded.
    let snapshot = reopened.snapshot().unwrap();
    assert!(
        snapshot
            .pending_effects
            .iter()
            .any(|effect| format!("{:?}", effect.kind).to_lowercase().contains("continuation")),
        "the additional-input round must stay dispatchable after a restart: {:?}",
        snapshot.pending_effects
    );
}

/// The same guarantee on the batch path: the follow-up keeps the settled tool
/// history and appends the round's reply exactly once.
#[test]
fn the_batch_steering_follow_up_appends_the_reply_exactly_once() {
    let clock = TestClock::new(1000);
    let (db, _root, run_id) = steering_model_fixture(&clock);
    let cancellation = CancellationRegistry::default();
    let coordinator =
        KernelCoordinator::start_prepared(&db, &clock, &run_id, &cancellation).unwrap();
    super::settle_worker_batch(&coordinator);

    let db_barrier = db.clone();
    let run_barrier = run_id.clone();
    super::live::test_barrier::install(
        &run_id,
        Box::new(move || {
            db_barrier
                .enqueue_run_steering(
                    &run_barrier,
                    "batch-window-row",
                    "批次窗口内追加",
                    crate::database::now_ms(),
                )
                .expect("accepted");
        }),
    );

    coordinator
        .dispatch_batch("worker-batch", "owner", &Allow, |binding, frame, _| {
            Ok(fox_engine_protocol::KernelModelResponse {
                schema_version: 1,
                run_id: binding.run_id.clone(),
                turn_id: frame.turn_id.clone(),
                batch_id: frame.batch_id.clone(),
                checkpoint_seq: frame.checkpoint_seq,
                assistant_message: json!({"role":"assistant","stopReason":"stop",
                    "content":[{"type":"text","text":"批后汇报。"}]}),
            })
        })
        .expect("the raced batch round must be re-planned");

    assert_eq!(db.kernel_count_steering_followups(&run_id).unwrap(), 1);
    let staged_seq = coordinator.last_event_seq_for_test();
    let staged = db
        .kernel_continuation_input(&run_id, &format!("continuation:{staged_seq}"))
        .expect("frozen follow-up input");
    let text = serde_json::to_string(&staged).unwrap();
    // Exactly one copy of this round's reply, and the settled tool round intact.
    assert_eq!(
        text.matches("批后汇报。").count(),
        1,
        "the round reply must appear exactly once: {text}"
    );
    assert_eq!(text.matches("Fox 补充要求处理").count(), 1);
    assert!(text.contains("worker-read") && text.contains("toolResult"));
    assert!(text.contains("durable coordinator"));
    // Reply before prompt, and the tool result before the reply.
    let tool_at = text.find("toolResult").unwrap();
    let reply_at = text.find("批后汇报。").unwrap();
    let prompt_at = text.find("Fox 补充要求处理").unwrap();
    assert!(tool_at < reply_at && reply_at < prompt_at, "{text}");
}

/// The additional-input budget is spent *before* the request is accepted, so
/// a user never gets "已接收" for text the Run can no longer answer.
#[test]
fn input_is_refused_once_the_additional_request_budget_is_spent() {
    let clock = TestClock::new(1000);
    let (db, root, run_id) = steering_model_fixture(&clock);
    let now = crate::database::now_ms();
    assert_eq!(db.kernel_count_steering_followups(&run_id).unwrap(), 0);
    db.enqueue_run_steering(&run_id, "within-budget", "第一条补充要求", now)
        .expect("a Run with rounds left accepts input");

    record_steering_lane_rounds(&root.join("facts.db"), &run_id, crate::database::MAX_STEERING_FOLLOWUPS);
    assert_eq!(
        db.kernel_count_steering_followups(&run_id).unwrap(),
        crate::database::MAX_STEERING_FOLLOWUPS
    );
    let error = db
        .enqueue_run_steering(&run_id, "over-budget", "预算用尽后的补充", now + 1)
        .expect_err("a spent budget must refuse new input at the receipt boundary");
    assert!(
        error.contains("additional-request rounds"),
        "the refusal must explain itself: {error}"
    );
    // Nothing was accepted, so no accepted-but-unanswerable row exists.
    let rows = db.run_steering_messages(&run_id).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].message_id, "within-budget");
}

/// N4: the round that answers mid-run input must see everything the previous
/// round saw — the settled tool calls and their results — not a rebuild from the
/// Run's initial input.
#[test]
fn a_steering_follow_up_keeps_the_settled_tool_history() {
    let clock = TestClock::new(1000);
    let (db, _root, run_id) = steering_model_fixture(&clock);
    let cancellation = CancellationRegistry::default();
    let coordinator =
        KernelCoordinator::start_prepared(&db, &clock, &run_id, &cancellation).unwrap();
    // A batch whose tool call really ran and settled.
    super::settle_worker_batch(&coordinator);

    let seq_before = coordinator.last_event_seq_for_test();
    // The user adds a request while the batch's model round is in flight.
    let db_for_deliver = db.clone();
    let run_for_deliver = run_id.clone();
    coordinator
        .dispatch_batch("worker-batch", "steering-owner", &Allow, move |binding, frame, _| {
            db_for_deliver
                .enqueue_run_steering(
                    &run_for_deliver,
                    "steer-after-tools",
                    "另外核对耗时分布",
                    crate::database::now_ms(),
                )
                .expect("the Host accepts a request while the Run is active");
            Ok(fox_engine_protocol::KernelModelResponse {
                schema_version: 1,
                run_id: binding.run_id.clone(),
                turn_id: frame.turn_id.clone(),
                batch_id: frame.batch_id.clone(),
                checkpoint_seq: frame.checkpoint_seq,
                assistant_message: json!({"role":"assistant","stopReason":"stop",
                    "content":[{"type":"text","text":"先汇报工具结果。"}]}),
            })
        })
        .unwrap();

    // The accepted request got its own lane round instead of completing the Run.
    assert_eq!(db.kernel_count_steering_followups(&run_id).unwrap(), 1);
    assert_eq!(db.kernel_count_continuations(&run_id).unwrap(), 0);
    assert_eq!(coordinator.snapshot().unwrap().state, "running");
    let rows = db.run_steering_messages(&run_id).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].status, "received");

    // The frozen input of that round is the model view the next request sends:
    // the batch's own history, its assistant tool call, the settled tool result
    // and the refused stop — then the steering prompt. Rebuilding from the Run's
    // initial input would drop all of the middle.
    let staged_seq = coordinator.last_event_seq_for_test();
    assert!(staged_seq > seq_before);
    let staged = db
        .kernel_continuation_input(&run_id, &format!("continuation:{staged_seq}"))
        .expect("the steering round input is frozen with its decision");
    let text = serde_json::to_string(&staged).unwrap();
    assert!(
        text.contains("read the file"),
        "the batch history must survive into the follow-up round: {text}"
    );
    assert!(
        text.contains("worker-read") && text.contains("toolUse"),
        "the settled assistant tool call must survive into the follow-up round"
    );
    assert!(
        text.contains("toolResult"),
        "the settled tool result must be visible to the follow-up round"
    );
    assert!(
        text.contains("durable coordinator"),
        "the real tool output must be in the follow-up round, not just a receipt"
    );
    assert!(
        text.contains("先汇报工具结果。"),
        "the refused stop stays in the history the follow-up answers"
    );
    assert!(text.contains("Fox 补充要求处理"));
    assert!(!text.contains("Fox 续答检查"));
    assert!(
        !text.contains("steer-after-tools"),
        "queued text is spliced from the durable queue at dispatch time, not duplicated here"
    );
}

/// N5: when the terminal decision loses a race with a freshly accepted request,
/// the transports must see a scheduling signal, not an engine failure.
#[test]
fn a_lost_terminal_race_is_reported_as_a_replan_signal() {
    let clock = TestClock::new(1000);
    let (db, _root, run_id) = steering_model_fixture(&clock);
    let cancellation = CancellationRegistry::default();
    let coordinator =
        KernelCoordinator::start_prepared(&db, &clock, &run_id, &cancellation).unwrap();
    let db_for_deliver = db.clone();
    let run_for_deliver = run_id.clone();
    // The request is accepted while the round is in flight. The delivery hook
    // runs between the transport's own steering snapshot and the terminal
    // write-set, which is the racing window this guard exists for.
    let error = coordinator
        .dispatch_initial("owner", &Allow, move |binding, frame, _| {
            db_for_deliver
                .enqueue_run_steering(
                    &run_for_deliver,
                    "race-row",
                    "刚好在这一轮追加",
                    crate::database::now_ms(),
                )
                .expect("accepted");
            Ok(stop_response(frame, binding))
        })
        .err();
    // Either the transport planned the additional-input round (the common case)
    // or it hit the guard; both leave the Run alive with the row open, and
    // neither is a generic failure.
    if let Some(error) = error {
        assert_eq!(
            error,
            super::STEERING_REPLAN,
            "a lost terminal race must be a re-plan signal"
        );
    }
    assert_ne!(coordinator.snapshot().unwrap().state, "completed");
    assert_eq!(
        db.run_steering_messages(&run_id).unwrap()[0].status,
        "received"
    );
    assert!(db
        .enqueue_run_steering(&run_id, "still-active", "还能继续", crate::database::now_ms())
        .is_ok());

    // The guard's own signal: the decision write-set reports the competition,
    // and the transports translate exactly that marker into a re-plan.
    let guard = coordinator
        .complete_bypassing_the_steering_lane_for_test()
        .expect_err("the terminal decision must refuse while a row is open");
    assert!(
        guard.starts_with(kernel::STEERING_COMPETITION),
        "unexpected guard error: {guard}"
    );
    assert_eq!(super::replan_or_error(guard), super::STEERING_REPLAN);
    assert_eq!(
        super::replan_or_error("kernel.some_real_failure".to_string()),
        "kernel.some_real_failure"
    );
}

#[test]
fn steering_is_collected_then_applied_by_the_initial_dispatch() {
    let clock = TestClock::new(1000);
    let (db, root, run_id) = steering_model_fixture(&clock);
    let now = crate::database::now_ms();
    db.enqueue_run_steering(&run_id, "steer-initial", "请额外核对编码兼容性", now)
        .unwrap();
    let cancellation = CancellationRegistry::default();
    let coordinator =
        KernelCoordinator::start_prepared(&db, &clock, &run_id, &cancellation).unwrap();
    coordinator
        .dispatch_initial("initial-owner", &Allow, |binding, frame, _token| {
            // The frozen request itself was not rewritten: the addition lands
            // at the dispatch boundary as an extra ordinary user message.
            let texts: Vec<String> = frame
                .input
                .messages
                .iter()
                .filter_map(|message| message["content"][0]["text"].as_str().map(str::to_owned))
                .collect();
            assert!(texts.iter().any(|text| text.contains("用户在运行过程中补充要求")));
            assert!(texts.iter().any(|text| text.contains("请额外核对编码兼容性")));
            // Steering is plain text: it never advertises tools or grants.
            assert!(frame.input.messages.iter().all(|message| !message["content"]
                .as_array()
                .is_some_and(|blocks| blocks.iter().any(|block| block["type"] == "toolCall"))));
            Ok(stop_response(frame, binding))
        })
        .unwrap();

    let rows = db.run_steering_messages(&run_id).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].status, "applied");
    assert!(rows[0].applied_at.is_some());
    assert!(rows[0].applied_event_seq.is_some());
    assert_eq!(rows[0].applied_dispatch_key.as_deref(), Some(kernel::INITIAL_MODEL_IDEMPOTENCY_KEY));
    assert_eq!(coordinator.snapshot().unwrap().state, "completed");

    // Reopen: the applied state is durable and never re-applied.
    drop(db);
    let reopened = Database::open(root.join("facts.db")).unwrap();
    let rows = reopened.run_steering_messages(&run_id).unwrap();
    assert_eq!(rows[0].status, "applied");
    assert!(reopened
        .enqueue_run_steering(&run_id, "msg-after-terminal", "迟来的补充", crate::database::now_ms())
        .is_err());
}

#[test]
fn steering_is_delivered_once_across_replacement_dispatches_and_cancelled_on_terminal() {
    let clock = TestClock::new(1000);
    let (db, _root, run_id) = steering_model_fixture(&clock);
    let now = crate::database::now_ms();
    db.enqueue_run_steering(&run_id, "steer-retry", "重试也只能看到一次", now)
        .unwrap();

    // First dispatch collection binds the row; a replacement collection (for a
    // worker that died before any response) returns the same rows and must not
    // rebind or duplicate them.
    let first = db
        .deliver_steering_for_dispatch(&run_id, "initial-model-delivery")
        .unwrap();
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].status, "delivered");
    let second = db
        .deliver_steering_for_dispatch(&run_id, "continuation-delivery:continuation:9")
        .unwrap();
    assert_eq!(second.len(), 1);
    assert_eq!(second[0].status, "delivered");
    assert_eq!(
        second[0].applied_dispatch_key.as_deref(),
        Some("initial-model-delivery")
    );
    let pending = db.pending_run_steering(&run_id).unwrap();
    assert!(pending.is_empty(), "flipped rows are no longer pending");
    let active = db.active_run_steering(&run_id).unwrap();
    assert_eq!(active.len(), 1, "delivered rows stay active until answered");

    // A run failure is terminal: everything still open is cancelled, and the
    // cancelled row can never reach a model afterwards.
    let cancellation = CancellationRegistry::default();
    let coordinator =
        KernelCoordinator::start_prepared(&db, &clock, &run_id, &cancellation).unwrap();
    coordinator
        .fail("kernel.test_failure", "simulated terminal failure")
        .unwrap();
    let rows = db.run_steering_messages(&run_id).unwrap();
    assert_eq!(rows[0].status, "cancelled");
    assert!(db.active_run_steering(&run_id).unwrap().is_empty());
    assert!(db
        .deliver_steering_for_dispatch(&run_id, "post-terminal")
        .unwrap()
        .is_empty());
}

#[test]
fn delivered_rows_apply_inside_the_response_write_set_only() {
    // Delivered rows become applied by a response commit even when no new
    // directive is armed (Final response); the two transitions are one
    // transaction, so no state can observe "answered but still delivered".
    let clock = TestClock::new(1000);
    let (db, _root, run_id) = steering_model_fixture(&clock);
    let now = crate::database::now_ms();
    db.enqueue_run_steering(&run_id, "steer-final", "最后一轮的补充", now)
        .unwrap();
    // Pre-arm the row exactly the way a replacement/reopen transport would:
    // it is already `delivered` before this dispatch begins.
    let bound = db
        .deliver_steering_for_dispatch(&run_id, "initial-model-delivery")
        .unwrap();
    assert_eq!(bound[0].status, "delivered");
    let cancellation = CancellationRegistry::default();
    let coordinator =
        KernelCoordinator::start_prepared(&db, &clock, &run_id, &cancellation).unwrap();
    // A Final (stop, no directive) response still applies every previously
    // delivered row inside its response write-set.
    coordinator
        .dispatch_initial("initial-owner", &Allow, |binding, frame, _token| {
            let spliced: Vec<String> = frame
                .input
                .messages
                .iter()
                .filter_map(|message| {
                    message["content"][0]["text"].as_str().map(str::to_owned)
                })
                .collect();
            assert!(spliced.iter().any(|text| text.contains("最后一轮的补充")));
            Ok(stop_response(frame, binding))
        })
        .unwrap();
    let rows = db.run_steering_messages(&run_id).unwrap();
    assert_eq!(rows[0].status, "applied");
    assert!(rows[0].applied_at.is_some());
    assert!(rows[0].applied_event_seq.is_some());
    assert_eq!(coordinator.snapshot().unwrap().state, "completed");
}
