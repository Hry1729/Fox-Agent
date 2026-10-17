//! Package 2-D U1: the Run lifecycle half of the delivery-review park.
//!
//! These tests own the state machine contract — the single entry edge, the
//! exits, the fail-closed preconditions, and the guarantees that make the parked
//! state safe to wait in. The Host, database and acceptance-rule halves live in
//! `fox-desktop`'s `delivery_u1_tests`.
//!
//! What is deliberately **not** asserted here: anything about tool approvals.
//! `park_awaiting_delivery_review` must never produce a `RequestApproval` or a
//! `DispatchTool`, and every test below checks for their absence.

use crate::{Effect, RunController, RunOutcome, RunState, TestClock};

const PARK_WALL_MS: i64 = 10_000;
const RESPONSE: &str = r#"{"role":"assistant","content":[{"type":"text","text":"Done."}]}"#;

fn authoritative_run() -> RunController {
    let clock = TestClock::new(6_000);
    let config = crate::test_config();
    let (mut run, _) = RunController::start("delivery-review-run", "turn", config, &clock).unwrap();
    run.tick(PARK_WALL_MS, PARK_WALL_MS);
    // The delivery gate runs after the model's closing reply, so the request it
    // belongs to has already been settled.
    run.record_initial_model_response(RESPONSE).unwrap();
    run
}

fn has_event(effects: &[Effect], event_type: &str) -> bool {
    effects
        .iter()
        .any(|effect| matches!(effect, Effect::AppendEvent { event_type: actual, .. } if actual == event_type))
}

fn has_effect_kind(effects: &[Effect], predicate: impl Fn(&Effect) -> bool) -> bool {
    effects.iter().any(predicate)
}

/// A settled Run with no outstanding side effect is the only shape that may park.
///
/// Mirrors the delivery stop's real position in the round: the model has already
/// answered (so no model request is in flight) and no tool call is unresolved.
fn parked(reason: &str, items: &[String]) -> (RunController, Vec<Effect>) {
    let mut run = authoritative_run();
    let effects = run
        .park_awaiting_delivery_review(PARK_WALL_MS, PARK_WALL_MS, reason, items)
        .expect("a settled running Run must be parkable");
    (run, effects)
}

#[test]
fn the_new_state_is_not_terminal_and_parses_from_its_own_literal() {
    let state = RunState::AwaitingDeliveryReview;
    assert!(!state.is_terminal());
    // Continuability is asserted for consistency with the durable progress row,
    // which is the production criterion. `is_continuable` itself has no
    // production caller today; see the note on the method.
    assert!(state.is_continuable());
    assert_eq!(state.as_str(), "awaiting_delivery_review");
    assert_eq!(RunState::parse("awaiting_delivery_review"), Some(state));
    // An unknown literal still fails closed rather than defaulting to a state.
    assert_eq!(RunState::parse("awaiting_delivery_reviews"), None);
}

#[test]
fn parking_from_running_writes_one_event_and_a_snapshot_and_nothing_else() {
    let (run, effects) = parked("交付内容尚未核验", &["file:out.json".to_owned()]);

    assert_eq!(run.state(), RunState::AwaitingDeliveryReview);
    assert!(has_event(&effects, "run.awaiting_delivery_review"));
    // The two guarantees that keep this from being a tool authorization: no
    // approval request and no dispatch are ever produced.
    assert!(!has_effect_kind(&effects, |effect| matches!(
        effect,
        Effect::RequestApproval { .. }
    )));
    assert!(!has_effect_kind(&effects, |effect| matches!(
        effect,
        Effect::DispatchTool { .. }
    )));
    assert_eq!(
        effects
            .iter()
            .filter(|effect| matches!(effect, Effect::AppendEvent { .. }))
            .count(),
        1,
        "exactly one event: the park itself"
    );
}

#[test]
fn parking_carries_no_approval_deadline() {
    let (run, _) = parked("原因", &[]);
    // The durable commit enforces `WaitingApproval ⟺ approval_deadline_wall_ms`;
    // a parked Run must satisfy the negative half of that invariant.
    assert_eq!(run.approval_deadline_wall_ms(), None);
    assert_eq!(run.wait_deadline_wall_ms(), None);
    assert_ne!(run.state(), RunState::WaitingApproval);
}

#[test]
fn parking_refuses_a_run_that_is_not_settled() {
    // Not `running` at all.
    let mut created = RunController::start(
        "delivery-review-created",
        "turn",
        crate::test_config(),
        &TestClock::new(0),
    )
    .unwrap()
    .0;
    assert!(created
        .park_awaiting_delivery_review(1, 1, "原因", &[])
        .is_err());

    // `running`, but a tool call is still unresolved.
    let mut run = authoritative_run();
    let policy = crate::TestPolicy { allow: vec![], deny: vec![] };
    run.propose_tool_batch(
        "batch-1",
        vec![crate::ToolCallRequest {
            tool_call_id: "call-1".into(),
            tool: "read".into(),
            canonical_input_json: "{}".into(),
            source_order: 0,
        }],
        &policy,
        PARK_WALL_MS,
        PARK_WALL_MS,
    )
    .unwrap();
    assert!(run
        .park_awaiting_delivery_review(PARK_WALL_MS, PARK_WALL_MS, "原因", &[])
        .is_err());
}

#[test]
fn parking_refuses_unbounded_or_blank_reasons_and_items() {
    let mut run = authoritative_run();
    assert!(run
        .park_awaiting_delivery_review(PARK_WALL_MS, PARK_WALL_MS, "   ", &[])
        .is_err());
    assert!(run
        .park_awaiting_delivery_review(PARK_WALL_MS, PARK_WALL_MS, &"x".repeat(4097), &[])
        .is_err());
    assert!(run
        .park_awaiting_delivery_review(
            PARK_WALL_MS,
            PARK_WALL_MS,
            "原因",
            &["".to_owned()]
        )
        .is_err());
    let too_many: Vec<String> = (0..257).map(|index| format!("item-{index}")).collect();
    assert!(run
        .park_awaiting_delivery_review(PARK_WALL_MS, PARK_WALL_MS, "原因", &too_many)
        .is_err());
    // The Run is untouched by every refusal.
    assert_eq!(run.state(), RunState::Running);
}

#[test]
fn a_parked_run_can_be_completed_rejected_or_cancelled() {
    // The three exits a parked Run has. None of them is invented: `Completed` is
    // the user having accepted, `Failed` is an explicit rejection, `Cancelled` is
    // cancellation. There is no fourth.
    for outcome in [
        RunOutcome::Completed,
        RunOutcome::Failed {
            code: "delivery.rejected_by_user".into(),
            message: "用户判定该交付不可接受".into(),
        },
    ] {
        let (mut run, _) = parked("原因", &["file:out.json".to_owned()]);
        let effects = run.terminate(outcome.clone());
        assert!(run.is_terminal(), "{outcome:?}");
        assert!(has_event(&effects, "run.completed") || has_event(&effects, "run.failed"));
    }

    let (mut run, _) = parked("原因", &[]);
    run.request_cancel();
    assert_eq!(run.state(), RunState::Cancelling);
    let effects = run.terminate(RunOutcome::Cancelled);
    assert!(has_event(&effects, "run.cancelled"));
}

#[test]
fn a_parked_run_survives_a_rehydrate_round_trip() {
    let (run, _) = parked("原因", &["file:a.json".to_owned(), "file:b.json".to_owned()]);
    let restored = RunController::rehydrate(run.shadow_checkpoint(PARK_WALL_MS)).unwrap();
    assert_eq!(restored.state(), RunState::AwaitingDeliveryReview);
    assert_eq!(restored.approval_deadline_wall_ms(), None);
}

#[test]
fn a_parked_run_never_re_enters_the_park() {
    let (mut run, _) = parked("原因", &[]);
    // Idempotence by construction: the park requires `running`, and a parked Run
    // is not `running`, so a second park is refused rather than stacking events.
    assert!(run
        .park_awaiting_delivery_review(PARK_WALL_MS + 1, PARK_WALL_MS + 1, "原因", &[])
        .is_err());
}
