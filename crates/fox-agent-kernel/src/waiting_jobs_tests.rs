use crate::{Effect, RunController, RunOutcome, RunState, TestClock, WaitingJobFact};

const PARK_WALL_MS: i64 = 10_000;
const RESPONSE: &str = r#"{"role":"assistant","content":[{"type":"text","text":"The calculation is running."}]}"#;
const HISTORY: &str = r#"[{"role":"user","content":[{"type":"text","text":"Calculate."}]}]"#;

fn parked_run(run_limited: bool, job_deadline_wall_ms: i64) -> (RunController, u64) {
    let clock = TestClock::new(6_000);
    let mut config = crate::test_config();
    config.experimental_compute_job_notice = true;
    config.run_execution_limited = run_limited;
    config.run_execution_budget_ms = 5_000;
    let (mut run, _) = RunController::start("waiting-job-run", "turn", config, &clock).unwrap();
    run.tick(PARK_WALL_MS, PARK_WALL_MS);
    assert_eq!(run.running_elapsed_ms(), 4_000);
    run.record_initial_model_response(RESPONSE).unwrap();
    let settled_response_seq = run.last_event_seq();
    run.park_waiting_jobs(
        PARK_WALL_MS,
        PARK_WALL_MS,
        "data-root",
        settled_response_seq,
        RESPONSE,
        HISTORY,
        &[WaitingJobFact {
            job_id: "job-1".into(),
            attempt: 1,
            deadline_wall_ms: job_deadline_wall_ms,
        }],
    )
    .unwrap();
    assert_eq!(run.state(), RunState::WaitingJobs);
    (run, settled_response_seq + 1)
}

fn has_event(effects: &[Effect], event_type: &str) -> bool {
    effects.iter().any(|effect| {
        matches!(effect, Effect::AppendEvent { event_type: actual, .. } if actual == event_type)
    })
}

#[test]
fn waiting_jobs_rejects_generic_completed_terminal_until_jobs_are_resolved() {
    let (mut run, _) = parked_run(true, 20_000);
    let deadline = run.wait_deadline_wall_ms();
    let accounted = run.wait_accounted_until_wall_ms();

    let effects = run.terminate(RunOutcome::Completed);

    assert_eq!(run.state(), RunState::WaitingJobs);
    assert!(!run.is_terminal());
    assert!(!has_event(&effects, "run.completed"));
    assert_eq!(run.wait_deadline_wall_ms(), deadline);
    assert_eq!(run.wait_accounted_until_wall_ms(), accounted);
}

#[test]
fn waiting_jobs_charges_remaining_run_budget_once_across_rehydrate() {
    let (mut run, park_seq) = parked_run(true, 20_000);
    assert_eq!(run.wait_deadline_wall_ms(), Some(11_000));
    assert_eq!(run.wait_accounted_until_wall_ms(), Some(PARK_WALL_MS));

    let first = run.account_waiting_jobs(park_seq, 10_500).unwrap();
    assert!(has_event(&first, "run.jobs_wait_accounted"));
    assert_eq!(run.running_elapsed_ms(), 4_500);
    assert!(run.account_waiting_jobs(park_seq, 10_500).unwrap().is_empty());
    assert!(run.tick(10_500, 10_500).is_empty());
    assert_eq!(run.running_elapsed_ms(), 4_500);

    let mut restored = RunController::rehydrate(run.shadow_checkpoint(10_500)).unwrap();
    assert!(restored.account_waiting_jobs(park_seq, 10_500).unwrap().is_empty());
    assert!(restored.account_waiting_jobs(park_seq, 10_499).is_err());
    assert_eq!(restored.running_elapsed_ms(), 4_500);

    let final_effects = restored.account_waiting_jobs(park_seq, 11_100).unwrap();
    assert_eq!(restored.running_elapsed_ms(), 5_000);
    assert_eq!(restored.state(), RunState::BudgetExhausted);
    assert!(has_event(&final_effects, "run.budget_exhausted"));
    assert_eq!(restored.wait_deadline_wall_ms(), None);
    assert_eq!(restored.wait_accounted_until_wall_ms(), None);
}
