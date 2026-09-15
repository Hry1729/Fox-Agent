use crate::*;

fn prepared() -> RunController {
    let clock = TestClock::new(1000);
    let mut config = crate::test_config();
    config.kernel_mode = "authoritative".into();
    // Keep the legacy single-bound timing for this compaction-deadline test:
    // sub-round bounds equal to the whole-round bound preserve the old fire
    // instant (validation requires first/idle <= total).
    config.model_first_response_ms = config.model_request_timeout_ms;
    config.model_idle_ms = config.model_request_timeout_ms;
    let (mut controller, _) =
        RunController::start_with_initial_input("r", "t", config, "hash", &clock).unwrap();
    controller
        .prepare_context_compaction("job", "{}", 1000)
        .unwrap();
    controller
}

#[test]
fn compaction_deadline_and_owner_survive_restart_without_resetting() {
    let mut controller = prepared();
    controller
        .dispatch_context_compaction("job", "owner", 1100, 5000)
        .unwrap();
    let data = controller.shadow_checkpoint(1200);
    let timeout = data.config.model_request_timeout_ms;
    assert_eq!(data.model_request_since_wall_ms, Some(5000));
    let mut restored = RunController::rehydrate(data).unwrap();
    assert!(restored
        .dispatch_context_compaction("job", "new-owner", 0, 5001)
        .is_err());
    assert!(!restored.tick(0, 5000 + timeout - 1).iter().any(
        |effect| matches!(effect,Effect::AppendEvent{event_type,..} if event_type=="run.failed")
    ));
    restored.tick(1, 5000 + timeout);
    assert_eq!(restored.state(), RunState::Failed);
}

#[test]
fn compaction_consumes_run_budget_and_completion_does_not_arm_normal_model() {
    let mut controller = prepared();
    controller
        .dispatch_context_compaction("job", "owner", 1000, 5000)
        .unwrap();
    controller.tick(1200, 5200);
    assert_eq!(controller.running_elapsed_ms(), 200);
    assert!(controller
        .complete_context_compaction("job", "foreign", "{}", true, 1200)
        .is_err());
    controller
        .complete_context_compaction("job", "owner", "{}", true, 1200)
        .unwrap();
    assert_eq!(controller.state(), RunState::Running);
    assert!(!controller.model_request_in_flight());
    assert_eq!(controller.shadow_checkpoint(1200).compaction.compactions, 1);
    assert!(controller.begin_initial_model_request(1200, 5200).is_ok());
}

#[test]
fn compaction_cancellation_and_clock_regression_reject_late_results() {
    let mut controller = prepared();
    controller
        .dispatch_context_compaction("job", "owner", 1000, 5000)
        .unwrap();
    controller.tick(1100, 4999);
    assert_eq!(controller.state(), RunState::Failed);
    assert!(controller
        .complete_context_compaction("job", "owner", "{}", true, 1100)
        .is_err());
    let mut controller = prepared();
    controller
        .dispatch_context_compaction("job", "owner", 1000, 5000)
        .unwrap();
    controller.request_cancel();
    assert!(controller
        .complete_context_compaction("job", "owner", "{}", true, 1100)
        .is_err());
}
