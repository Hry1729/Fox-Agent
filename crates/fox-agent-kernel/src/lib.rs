//! Fox Agent Kernel — unified control core.
//!
//! The Kernel is the single authority for run/turn/tool-batch state, permission
//! and approval decisions, retry/budget/cancellation policy, terminal
//! classification and completion judgment. Runtime Host, Resource Gateway,
//! Engine Adapters and React are adapters/read-models around it.
//!
//! This standalone crate is independent of Tauri, SQLite and every engine.
//! Runtime adapters decide whether to use it in observer or authoritative mode;
//! extracting the crate does not change production authority.

pub mod cancellation;
pub mod controller;
pub use cancellation::{CancellationRegistry, CancellationToken};
#[cfg(test)]
mod compaction_tests;
#[cfg(test)]
pub mod corpus;
pub mod ports;
pub mod recovery;
pub mod shadow;
pub mod state;

pub use shadow::{
    DiffCategory, LegacyDisposition, RetryTimeoutFacts, ShadowContext, ShadowDiff,
    ShadowDisposition, ShadowToolObservation, ToolDisposition, SHADOW_DIFF_SCHEMA_VERSION,
};

pub use recovery::{
    plan_recovery, RecoveredApproval, RecoveredUncertainEffect, RecoveryFacts, RecoveryPlan,
};

pub use controller::{
    approval_effect_key, batch_delivery_effect_key, batch_delivery_idempotency_key,
    dispatch_effect_key, dispatch_idempotency_key, persist_events, Effect, KernelPersistCommand,
    PersistApprovalResolution, PersistBatch, PersistEvent, PersistOutboxEffect, PersistTool,
    RehydratedBatch, RehydratedRun, RehydratedToolCall, RunController, INITIAL_MODEL_EFFECT_KEY,
    INITIAL_MODEL_IDEMPOTENCY_KEY, STEERING_COMPETITION,
};
pub use ports::{
    CancellationPort, Clock, ClockReading, CompactionState, EnginePort, EventStorePort,
    KernelSnapshot, OutboxEffect, OutboxEffectKind, OutboxStatus, PolicyDecision,
    PolicyDecisionPort, RetryState, RunFrozenConfig, RunSnapshot, SnapshotProjectionPort,
    SnapshotToolBatch, SnapshotToolCall, ToolCallRequest, ToolDispatchPort,
};
pub use state::{
    ApprovalDecision, EngineId, KernelError, KernelMode, RunOutcome, RunState, ToolCallState,
};

/// Deterministic fixed clock for tests.
#[cfg(any(test, feature = "test-support"))]
pub struct TestClock {
    now: std::sync::Mutex<i64>,
}

#[cfg(any(test, feature = "test-support"))]
impl TestClock {
    pub fn new(start: i64) -> Self {
        TestClock {
            now: std::sync::Mutex::new(start),
        }
    }
    pub fn advance(&self, ms: i64) {
        *self.now.lock().unwrap() += ms;
    }
    pub fn get(&self) -> i64 {
        *self.now.lock().unwrap()
    }
    /// Test convenience: one deterministic value for both time domains.
    pub fn now_ms(&self) -> i64 {
        self.get()
    }
}

#[cfg(any(test, feature = "test-support"))]
impl Clock for TestClock {
    fn now_monotonic_ms(&self) -> i64 {
        self.get()
    }
    fn now_wall_ms(&self) -> i64 {
        // Tests use one deterministic clock for both domains.
        self.get()
    }
}

/// Policy port used by tests: allow-list tools, deny-list tools, otherwise
/// require approval.
#[cfg(test)]
struct TestPolicy {
    allow: Vec<&'static str>,
    deny: Vec<&'static str>,
}

#[cfg(test)]
impl PolicyDecisionPort for TestPolicy {
    fn decide(
        &self,
        _run_id: &str,
        _permission_snapshot_id: &str,
        tool: &str,
        _canonical_input_json: &str,
    ) -> PolicyDecision {
        if self.deny.contains(&tool) {
            PolicyDecision::Deny {
                reason: "test deny".into(),
            }
        } else if self.allow.contains(&tool) {
            PolicyDecision::Allow
        } else {
            PolicyDecision::RequireApproval
        }
    }
}

#[cfg(test)]
fn test_config() -> RunFrozenConfig {
    RunFrozenConfig {
        engine_id: "pi".into(),
        kernel_mode: "authoritative".into(),
        capability_manifest_version: 2,
        capability_manifest_hash: "manifest-hash-v2".into(),
        permission_snapshot_id: "perm-v1".into(),
        execution_profile_id: "legacy".into(),
        prompt_config_hash: "prompt-hash".into(),
        model_request_timeout_ms: 120_000,
        model_first_response_ms: 60_000,
        model_idle_ms: 120_000,
        tool_execution_timeout_ms: 600_000,
        run_execution_budget_ms: 600_000,
        run_execution_limited: true,
        approval_wait_timeout_ms: 3_600_000,
        provider_max_retries: 2,
        turn_max_retries: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(id: &str, tool: &str, order: usize) -> ToolCallRequest {
        ToolCallRequest {
            tool_call_id: id.into(),
            tool: tool.into(),
            canonical_input_json: "{}".into(),
            source_order: order,
        }
    }

    fn has_event(effects: &[Effect], ty: &str) -> bool {
        effects.iter().any(|e| match e {
            Effect::AppendEvent { event_type, .. } => event_type == ty,
            _ => false,
        })
    }

    #[test]
    fn unknown_engine_or_mode_fails_closed() {
        let clock = TestClock::new(0);
        let mut bad = test_config();
        bad.engine_id = "mystery".into();
        assert!(RunController::start("r", "t", bad, &clock).is_err());
        let mut bad = test_config();
        bad.kernel_mode = "shadowy".into();
        assert!(RunController::start("r", "t", bad, &clock).is_err());
        let mut bad = test_config();
        bad.permission_snapshot_id = "  ".into();
        assert!(RunController::start("r", "t", bad, &clock).is_err());
    }

    #[test]
    fn canonical_engine_ids_match_the_frozen_control_contract() {
        for engine in [EngineId::Pi, EngineId::DeepSeekHarness, EngineId::Codex] {
            assert_eq!(EngineId::parse(engine.as_str()), Some(engine));
        }
        assert_eq!(EngineId::DeepSeekHarness.as_str(), "deepseek_harness");
        assert_eq!(EngineId::parse("deepseek-harness"), Some(EngineId::DeepSeekHarness));
    }

    #[test]
    fn allowed_tools_dispatch_and_batch_barriers_in_source_order() {
        let clock = TestClock::new(0);
        let (mut c, _) = RunController::start("r", "t", test_config(), &clock).unwrap();
        let policy = TestPolicy {
            allow: vec!["read", "ls", "grep"],
            deny: vec![],
        };
        let effects = c
            .propose_tool_batch(
                "b1",
                vec![
                    call("a", "read", 0),
                    call("b", "ls", 1),
                    call("c", "grep", 2),
                ],
                &policy,
                clock.now_ms(),
                clock.now_ms(),
            )
            .unwrap();
        // All three dispatch; no barrier until all settle.
        assert_eq!(
            effects
                .iter()
                .filter(|e| matches!(e, Effect::DispatchTool { .. }))
                .count(),
            3
        );
        assert!(!effects
            .iter()
            .any(|e| matches!(e, Effect::BatchBarrier { .. })));

        // Settle out of order: b, c, a.
        c.tool_settled("b", true, "{}").unwrap();
        c.tool_settled("c", true, "{}").unwrap();
        let effects = c.tool_settled("a", true, "{}").unwrap();
        let barrier = effects
            .iter()
            .find_map(|e| match e {
                Effect::BatchBarrier {
                    ordered_tool_call_ids,
                    ..
                } => Some(ordered_tool_call_ids),
                _ => None,
            })
            .expect("barrier emitted once batch fully settles");
        // Results handed to the model are in source order regardless of completion order.
        assert_eq!(
            barrier,
            &vec!["a".to_string(), "b".to_string(), "c".to_string()]
        );
    }

    #[test]
    fn duplicate_tool_settlement_is_idempotent() {
        let clock = TestClock::new(0);
        let (mut c, _) = RunController::start("r", "t", test_config(), &clock).unwrap();
        let policy = TestPolicy {
            allow: vec!["read"],
            deny: vec![],
        };
        assert!(c
            .propose_tool_batch(
                "invalid-order",
                vec![call("x", "read", 1)],
                &policy,
                clock.now_ms(),
                clock.now_ms(),
            )
            .is_err());
        c.propose_tool_batch(
            "b",
            vec![call("a", "read", 0)],
            &policy,
            clock.now_ms(),
            clock.now_ms(),
        )
        .unwrap();
        c.tool_settled("a", true, "{}").unwrap();
        let again = c.tool_settled("a", true, "{}").unwrap();
        assert!(again.iter().all(|e| matches!(e, Effect::Audit { .. })));
    }

    #[test]
    fn rehydrated_running_budgets_reanchor_on_the_first_monotonic_tick() {
        let mut config = test_config();
        config.tool_execution_timeout_ms = 10;
        config.run_execution_budget_ms = 20;
        let retry = RetryState {
            provider_max: config.provider_max_retries,
            turn_max: config.turn_max_retries,
            ..RetryState::default()
        };
        let mut controller = RunController::rehydrate(RehydratedRun {
            run_id: "recovered-run".into(),
            turn_id: "recovered-turn".into(),
            config,
            state: RunState::Running,
            seq: 0,
            running_elapsed_ms: 5,
            approval_deadline_wall_ms: None,
            terminal_written: false,
            batches: vec![RehydratedBatch {
                batch_id: "batch".into(),
                ordered: vec!["call".into()],
                barrier_emitted: false,
            }],
            tools: vec![RehydratedToolCall {
                tool_call_id: "call".into(),
                batch_id: "batch".into(),
                tool: "read".into(),
                input_json: "{}".into(),
                source_order: 0,
                state: ToolCallState::Running,
                result_json: None,
            }],
            retry,
            compaction: CompactionState::default(),
            model_request_since_wall_ms: None,
        })
        .unwrap();

        assert!(controller.tick(100, 1_000).is_empty());
        assert_eq!(controller.running_elapsed_ms(), 5);
        let tool_timeout = controller.tick(110, 1_010);
        assert!(has_event(&tool_timeout, "tool.failed"));
        assert!(tool_timeout
            .iter()
            .any(|effect| matches!(effect, Effect::ToolDispatchSettled { .. })));

        let budget_timeout = controller.tick(115, 1_015);
        assert!(has_event(&budget_timeout, "run.budget_exhausted"));
        assert_eq!(controller.state(), RunState::BudgetExhausted);
    }

    #[test]
    fn duplicate_batch_replay_is_idempotent_but_identity_changes_fail_closed() {
        let clock = TestClock::new(0);
        let (mut c, _) = RunController::start("r", "t", test_config(), &clock).unwrap();
        let policy = TestPolicy {
            allow: vec!["read"],
            deny: vec![],
        };
        c.propose_tool_batch(
            "b",
            vec![call("a", "read", 0)],
            &policy,
            clock.now_ms(),
            clock.now_ms(),
        )
        .unwrap();

        let replay = c
            .propose_tool_batch(
                "b",
                vec![call("a", "read", 0)],
                &policy,
                clock.now_ms(),
                clock.now_ms(),
            )
            .unwrap();
        assert!(replay
            .iter()
            .all(|effect| matches!(effect, Effect::Audit { .. })));

        let mut changed = call("a", "read", 0);
        changed.canonical_input_json = r#"{"path":"different"}"#.into();
        assert!(c
            .propose_tool_batch("b", vec![changed], &policy, clock.now_ms(), clock.now_ms())
            .is_err());
    }

    #[test]
    fn approval_parks_run_and_deny_fails_tool() {
        let clock = TestClock::new(0);
        let (mut c, _) = RunController::start("r", "t", test_config(), &clock).unwrap();
        let policy = TestPolicy {
            allow: vec![],
            deny: vec![],
        };
        let effects = c
            .propose_tool_batch(
                "b",
                vec![call("a", "write_file", 0)],
                &policy,
                clock.now_ms(),
                clock.now_ms(),
            )
            .unwrap();
        assert_eq!(c.state(), RunState::WaitingApproval);
        assert!(effects
            .iter()
            .any(|e| matches!(e, Effect::RequestApproval { .. })));

        // User denies.
        let effects = c
            .resolve_approval("a", ApprovalDecision::Deny, clock.now_ms())
            .unwrap();
        assert!(effects
            .iter()
            .any(|e| matches!(e, Effect::DispatchTool { .. }) == false));
        // The denied tool settles the batch (failed) -> barrier present.
        assert!(effects
            .iter()
            .any(|e| matches!(e, Effect::BatchBarrier { .. })));
        assert_eq!(c.state(), RunState::Running);
    }

    #[test]
    fn approval_after_cancel_fails_closed() {
        let clock = TestClock::new(0);
        let (mut c, _) = RunController::start("r", "t", test_config(), &clock).unwrap();
        let policy = TestPolicy {
            allow: vec![],
            deny: vec![],
        };
        c.propose_tool_batch(
            "b",
            vec![call("a", "write_file", 0)],
            &policy,
            clock.now_ms(),
            clock.now_ms(),
        )
        .unwrap();
        c.request_cancel();
        c.settle_cancellation();
        // A late human approval must not execute or resurrect the run.
        assert!(c
            .resolve_approval("a", ApprovalDecision::AllowOnce, clock.now_ms())
            .is_err());
        assert_eq!(c.state(), RunState::Cancelled);
    }

    #[test]
    fn tool_result_cannot_bypass_pending_approval() {
        let clock = TestClock::new(0);
        let (mut c, _) = RunController::start("r", "t", test_config(), &clock).unwrap();
        let policy = TestPolicy {
            allow: vec![],
            deny: vec![],
        };
        c.propose_tool_batch(
            "b",
            vec![call("a", "write_file", 0)],
            &policy,
            clock.now_ms(),
            clock.now_ms(),
        )
        .unwrap();

        assert!(c.tool_settled("a", true, r#"{"ok":true}"#).is_err());
        assert_eq!(c.state(), RunState::WaitingApproval);
    }

    #[test]
    fn cancellation_writes_single_terminal_and_late_success_is_audit_only() {
        let clock = TestClock::new(0);
        let (mut c, _) = RunController::start("r", "t", test_config(), &clock).unwrap();
        let policy = TestPolicy {
            allow: vec!["read"],
            deny: vec![],
        };
        c.propose_tool_batch(
            "b",
            vec![call("a", "read", 0), call("b2", "read", 1)],
            &policy,
            clock.now_ms(),
            clock.now_ms(),
        )
        .unwrap();
        let cancel_effects = c.request_cancel();
        assert_eq!(c.state(), RunState::Cancelling);
        // Running tools get a cancel notification.
        assert!(cancel_effects
            .iter()
            .any(|e| matches!(e, Effect::CancelToolCall { .. })));
        assert!(cancel_effects
            .iter()
            .any(|e| matches!(e, Effect::CancelEngineTurn { .. })));

        // A tool returns success after cancellation: audit only, run stays cancelling.
        let late = c.tool_settled("a", true, "{}").unwrap();
        assert!(late.iter().any(|e| matches!(e, Effect::Audit { .. })));
        assert_eq!(c.state(), RunState::Cancelling);

        let terminal = c.settle_cancellation();
        assert!(terminal.iter().any(
            |e| matches!(e, Effect::AppendEvent { event_type, .. } if event_type == "run.cancelled")
        ));
        assert_eq!(c.state(), RunState::Cancelled);

        // Terminal is written exactly once.
        let again = c.settle_cancellation();
        assert!(again.iter().all(|e| matches!(e, Effect::Audit { .. })));
        // A late completed/failed terminal cannot override cancelled.
        let rejected = c.terminate(RunOutcome::Failed {
            code: "x".into(),
            message: "y".into(),
        });
        assert!(rejected.iter().all(|e| matches!(e, Effect::Audit { .. })));
        assert_eq!(c.state(), RunState::Cancelled);
    }

    #[test]
    fn denied_tool_fails_and_does_not_dispatch() {
        let clock = TestClock::new(0);
        let (mut c, _) = RunController::start("r", "t", test_config(), &clock).unwrap();
        let policy = TestPolicy {
            allow: vec![],
            deny: vec!["rm_rf"],
        };
        let effects = c
            .propose_tool_batch(
                "b",
                vec![call("a", "rm_rf", 0)],
                &policy,
                clock.now_ms(),
                clock.now_ms(),
            )
            .unwrap();
        assert!(!effects
            .iter()
            .any(|e| matches!(e, Effect::DispatchTool { .. })));
        assert!(!effects
            .iter()
            .any(|e| matches!(e, Effect::RequestApproval { .. })));
        assert!(has_event(&effects, "tool.failed"));
    }

    #[test]
    fn continuous_runs_survive_elapsed_day_but_honor_cancel_and_model_stalls() {
        let clock = TestClock::new(0);
        let mut config = test_config();
        config.run_execution_limited = false;
        let (mut c, _) = RunController::start("r", "t", config.clone(), &clock).unwrap();
        c.settle_model_request();
        clock.advance(86_400_000);
        c.tick(clock.now_ms(), clock.now_ms());
        assert_eq!(c.state(), RunState::Running);
        c.request_cancel();
        c.settle_cancellation();
        assert_eq!(c.state(), RunState::Cancelled);
        let (mut stalled, _) = RunController::start("stall", "t", config, &clock).unwrap();
        clock.advance(60_001);
        let effects = stalled.tick(clock.now_ms(), clock.now_ms());
        assert_eq!(stalled.state(), RunState::Failed);
        assert!(effects.iter().any(|e| matches!(e, Effect::AppendEvent { payload_json, .. } if payload_json.contains("model.first_response_timeout"))));
    }

    #[test]
    fn continuous_run_retries_after_old_duration_window() {
        let clock = TestClock::new(0);
        let mut config = test_config();
        config.run_execution_limited = false;
        config.turn_max_retries = 1;
        let legacy = serde_json::to_value(test_config()).unwrap();
        assert!(legacy.get("run_execution_limited").is_none());
        assert!(serde_json::from_value::<RunFrozenConfig>(legacy).unwrap().run_execution_limited);
        let decoded: RunFrozenConfig = serde_json::from_str(&serde_json::to_string(&config).unwrap()).unwrap();
        assert!(!decoded.run_execution_limited);
        let (mut c, _) = RunController::start("r", "t", decoded, &clock).unwrap();
        c.settle_model_request();
        clock.advance(86_400_000);
        c.tick(clock.now_ms(), clock.now_ms());
        c.begin_model_request(clock.now_ms(), clock.now_ms());
        let evidence = r#"{"schemaVersion":1,"runId":"r","turnId":"t","checkpointSeq":7,"category":"model_timeout","httpStatus":null,"retryAfterMs":null}"#;
        let effects = c.schedule_model_retry(clock.now_ms(), clock.now_ms(), "initial", evidence, false, 1000).unwrap();
        assert!(has_event(&effects, "run.retrying"));
        assert_eq!(c.state(), RunState::RetryScheduled);
    }

    #[test]
    fn execution_budget_clock_suspends_during_approval() {
        let clock = TestClock::new(0);
        let mut config = test_config();
        config.run_execution_budget_ms = 1_000;
        let (mut c, _) = RunController::start("r", "t", config, &clock).unwrap();
        let policy = TestPolicy {
            allow: vec![],
            deny: vec![],
        };
        // Run for 600ms, then require approval.
        clock.advance(600);
        assert!(c.tick(clock.now_ms(), clock.now_ms()).is_empty());
        c.propose_tool_batch(
            "b",
            vec![call("a", "write_file", 0)],
            &policy,
            clock.now_ms(),
            clock.now_ms(),
        )
        .unwrap();
        assert_eq!(c.state(), RunState::WaitingApproval);
        // A long human wait (far past the budget) uses the approval clock, not the
        // execution budget.
        clock.advance(10_000);
        assert!(c.tick(clock.now_ms(), clock.now_ms()).is_empty());
        // Approve; resume running. Only the 600ms before approval accrues.
        c.resolve_approval("a", ApprovalDecision::AllowOnce, clock.now_ms())
            .unwrap();
        clock.advance(300);
        assert!(c.tick(clock.now_ms(), clock.now_ms()).is_empty()); // 900ms total, under 1000.
        clock.advance(200); // 1100ms -> budget exhausted.
        let effects = c.tick(clock.now_ms(), clock.now_ms());
        assert!(has_event(&effects, "run.budget_exhausted"));
    }

    #[test]
    fn approval_wait_has_its_own_expiry_and_cannot_wait_forever() {
        let clock = TestClock::new(0);
        let mut config = test_config();
        config.run_execution_budget_ms = 1_000;
        config.approval_wait_timeout_ms = 5_000;
        let (mut c, _) = RunController::start("r", "t", config, &clock).unwrap();
        let policy = TestPolicy {
            allow: vec![],
            deny: vec![],
        };
        c.propose_tool_batch(
            "b",
            vec![call("a", "write_file", 0)],
            &policy,
            clock.now_ms(),
            clock.now_ms(),
        )
        .unwrap();

        clock.advance(4_999);
        assert!(c.tick(clock.now_ms(), clock.now_ms()).is_empty());
        clock.advance(1);
        let effects = c.tick(clock.now_ms(), clock.now_ms());
        // The un-decided call was never dispatched, so it expires instead of
        // failing, and the run reports a continuable expiry rather than a plain
        // failure that would look unrecoverable to the user.
        assert!(has_event(&effects, "tool.expired"));
        assert!(has_event(&effects, "run.awaiting_approval_expired"));
        assert!(has_event(&effects, "run.approval_expired"));
        assert!(!has_event(&effects, "tool.failed"));
        assert!(!has_event(&effects, "run.failed"));
        assert_eq!(c.state(), RunState::ApprovalExpired);
        assert!(c.state().is_continuable());
        assert!(RunOutcome::ApprovalExpired {
            code: "approval.wait_timeout".into(),
            message: String::new(),
        }
        .is_continuable());
        // The expired approval is dead: resolving it now fails closed.
        assert!(c
            .resolve_approval("a", ApprovalDecision::AllowOnce, clock.now_ms())
            .is_err());
    }

    /// Expiry must not throw away work that already succeeded, and it must leave
    /// behind the facts a continuation needs: which approval died and what was
    /// already banked.
    #[test]
    fn approval_expiry_preserves_completed_results_and_records_the_dead_approval() {
        let clock = TestClock::new(0);
        let mut config = test_config();
        config.approval_wait_timeout_ms = 5_000;
        let (mut c, _) = RunController::start("r", "t", config, &clock).unwrap();
        let policy = TestPolicy {
            allow: vec!["read"],
            deny: vec![],
        };
        // First batch: a read that settles successfully.
        c.propose_tool_batch(
            "b1",
            vec![call("done", "read", 0)],
            &policy,
            clock.now_ms(),
            clock.now_ms(),
        )
        .unwrap();
        c.tool_settled("done", true, "{\"ok\":true}").unwrap();
        // Second batch: a write that requires a human and is never answered.
        c.propose_tool_batch(
            "b2",
            vec![call("pending", "write_file", 0)],
            &policy,
            clock.now_ms(),
            clock.now_ms(),
        )
        .unwrap();
        assert_eq!(c.state(), RunState::WaitingApproval);
        clock.advance(5_000);
        let effects = c.tick(clock.now_ms(), clock.now_ms());
        assert!(has_event(&effects, "run.approval_expired"));
        assert_eq!(c.state(), RunState::ApprovalExpired);
        // The completed result survives, and the resume evidence names both the
        // dead approval and the banked result.
        let evidence = effects
            .iter()
            .find_map(|effect| match effect {
                Effect::AppendEvent {
                    event_type,
                    payload_json,
                    ..
                } if event_type == "run.awaiting_approval_expired" => Some(payload_json.clone()),
                _ => None,
            })
            .expect("expiry evidence event");
        assert!(evidence.contains("\"pending\""), "{evidence}");
        assert!(evidence.contains("\"done\""), "{evidence}");
        assert!(evidence.contains("\"expiredApprovalsExecutable\":false"));
        assert!(evidence.contains("\"resumable\":true"));
        // The banked tool is still completed and its result is still present.
        assert_eq!(c.tool_call_state("done"), Some(ToolCallState::Completed));
        assert_eq!(c.tool_call_result_json("done"), Some("{\"ok\":true}"));
        assert_eq!(c.tool_call_state("pending"), Some(ToolCallState::Expired));
    }

    #[test]
    fn run_cannot_complete_while_a_tool_is_unresolved() {
        let clock = TestClock::new(0);
        let (mut c, _) = RunController::start("r", "t", test_config(), &clock).unwrap();
        let policy = TestPolicy {
            allow: vec!["read"],
            deny: vec![],
        };
        c.propose_tool_batch(
            "b",
            vec![call("a", "read", 0)],
            &policy,
            clock.now_ms(),
            clock.now_ms(),
        )
        .unwrap();

        let effects = c.terminate(RunOutcome::Completed);
        assert!(effects
            .iter()
            .all(|effect| matches!(effect, Effect::Audit { .. })));
        assert_eq!(c.state(), RunState::Running);
    }

    #[test]
    fn failure_terminalizes_and_cancels_every_unresolved_tool_first() {
        let clock = TestClock::new(0);
        let (mut c, _) = RunController::start("r", "t", test_config(), &clock).unwrap();
        let policy = TestPolicy {
            allow: vec!["read"],
            deny: vec![],
        };
        c.propose_tool_batch(
            "b",
            vec![call("a", "read", 0)],
            &policy,
            clock.now_ms(),
            clock.now_ms(),
        )
        .unwrap();

        let effects = c.terminate(RunOutcome::Failed {
            code: "provider.request_failed".into(),
            message: "provider failed".into(),
        });
        let tool_terminal = effects
            .iter()
            .position(|effect| {
                matches!(effect, Effect::AppendEvent { event_type, .. } if event_type == "tool.cancelled")
            })
            .unwrap();
        let run_terminal = effects
            .iter()
            .position(|effect| {
                matches!(effect, Effect::AppendEvent { event_type, .. } if event_type == "run.failed")
            })
            .unwrap();
        assert!(tool_terminal < run_terminal);
        assert!(effects.iter().any(
            |effect| matches!(effect, Effect::CancelToolCall { tool_call_id } if tool_call_id == "a")
        ));
        assert_eq!(c.state(), RunState::Failed);

        // The executor may still return after cancellation; the result is audit-only.
        let late = c.tool_settled("a", true, r#"{"ok":true}"#).unwrap();
        assert!(late
            .iter()
            .all(|effect| matches!(effect, Effect::Audit { .. })));
    }

    #[test]
    fn retry_and_compaction_use_distinct_states_and_resume() {
        let clock = TestClock::new(0);
        let mut config = test_config();
        config.turn_max_retries = 1;
        let (mut c, _) = RunController::start("r", "t", config, &clock).unwrap();

        // Turn retry schedules a distinct retry_scheduled state (attempt 1 <= max 1).
        let effects = c
            .schedule_turn_retry(0, 1_000, "provider.5xx", 500)
            .unwrap();
        assert_eq!(c.state(), RunState::RetryScheduled);
        assert!(has_event(&effects, "run.retrying"));
        // Resume running, then a second failure exhausts the turn budget -> failed.
        c.retry_resume(500, 1_500).unwrap();
        assert_eq!(c.state(), RunState::Running);
        let exhausted = c
            .schedule_turn_retry(500, 1_500, "provider.5xx", 500)
            .unwrap();
        assert!(exhausted.iter().any(
            |e| matches!(e, Effect::AppendEvent { event_type, .. } if event_type == "run.failed")
        ));

        // Fresh controller for compaction: distinct from retry.
        let (mut c2, _) = RunController::start("r2", "t2", test_config(), &clock).unwrap();
        let effects = c2.begin_compaction(0, "context_full").unwrap();
        assert_eq!(c2.state(), RunState::Compacting);
        assert!(has_event(&effects, "context.compaction.started"));
        let effects = c2.end_compaction(0, false).unwrap();
        assert_eq!(c2.state(), RunState::Running);
        assert!(has_event(&effects, "context.compaction.completed"));

        // Provider retry is independent from turn retry.
        let (mut c3, _) = RunController::start("r3", "t3", test_config(), &clock).unwrap();
        c3.record_provider_retry().unwrap(); // attempt 1 <= max 2
        c3.record_provider_retry().unwrap(); // attempt 2 <= max 2
        assert!(c3.record_provider_retry().is_err()); // 3 > 2 fail-closed
    }

    #[test]
    fn tool_execution_times_out_on_monotonic_clock() {
        let clock = TestClock::new(0);
        let mut config = test_config();
        config.tool_execution_timeout_ms = 1_000;
        let (mut c, _) = RunController::start("r", "t", config, &clock).unwrap();
        let policy = TestPolicy {
            allow: vec!["slow"],
            deny: vec![],
        };
        c.propose_tool_batch("b", vec![call("a", "slow", 0)], &policy, 0, 0)
            .unwrap();
        // Under the tool timeout: nothing.
        assert!(c.tick(999, 999).is_empty());
        // Past it: the tool fails with a timeout and the batch barrier fires.
        let effects = c.tick(1_000, 1_000);
        assert!(effects.iter().any(|e| matches!(
            e, Effect::AppendEvent { event_type, .. } if event_type == "tool.failed"
        )));
        assert!(effects.iter().any(
            |effect| matches!(effect, Effect::ToolDispatchSettled { tool_call_id } if tool_call_id == "a")
        ));
    }

    #[test]
    fn running_tool_timeout_is_enforced_while_sibling_waits_for_approval() {
        let clock = TestClock::new(0);
        let mut config = test_config();
        config.tool_execution_timeout_ms = 1_000;
        config.approval_wait_timeout_ms = 10_000;
        let (mut c, _) = RunController::start("r-mixed", "t", config, &clock).unwrap();
        let policy = TestPolicy {
            allow: vec!["read_file"],
            deny: vec![],
        };
        c.propose_tool_batch(
            "b-mixed",
            vec![
                call("running", "read_file", 0),
                call("waiting", "write_file", 1),
            ],
            &policy,
            0,
            1_000,
        )
        .unwrap();
        assert_eq!(c.state(), RunState::WaitingApproval);

        let effects = c.tick(1_000, 1_500);
        assert!(effects.iter().any(
            |effect| matches!(effect, Effect::CancelToolCall { tool_call_id } if tool_call_id == "running")
        ));
        assert!(effects.iter().any(
            |effect| matches!(effect, Effect::ToolDispatchSettled { tool_call_id } if tool_call_id == "running")
        ));
        assert_eq!(c.state(), RunState::WaitingApproval);
    }

    #[test]
    fn scheduled_retry_uses_durable_wall_due_time() {
        let clock = TestClock::new(0);
        let mut config = test_config();
        config.turn_max_retries = 1;
        let (mut c, _) = RunController::start("r-retry-due", "t", config, &clock).unwrap();
        c.schedule_turn_retry(0, 10_000, "provider.5xx", 500)
            .unwrap();
        assert_eq!(c.state(), RunState::RetryScheduled);
        assert!(c.tick(100, 10_499).is_empty());
        assert_eq!(c.state(), RunState::RetryScheduled);

        let effects = c.tick(200, 10_500);
        assert!(has_event(&effects, "run.retry.completed"));
        assert_eq!(c.state(), RunState::Running);
    }

    #[test]
    fn approval_wall_clock_rollback_fails_closed() {
        let clock = TestClock::new(1_000_000);
        let mut config = test_config();
        config.approval_wait_timeout_ms = 5_000;
        let (mut c, _) = RunController::start("r", "t", config, &clock).unwrap();
        let policy = TestPolicy {
            allow: vec![],
            deny: vec![],
        };
        c.propose_tool_batch(
            "b",
            vec![call("a", "write_file", 0)],
            &policy,
            1_000_000,
            1_000_000,
        )
        .unwrap();
        assert_eq!(c.state(), RunState::WaitingApproval);
        // Wall clock jumps BACKWARDS far beyond the wait start: fail closed.
        let effects = c.tick(1_000_000, 1_000_000 - 10_000);
        assert!(effects.iter().any(|e| matches!(
            e, Effect::AppendEvent { event_type, .. } if event_type == "run.failed"
        )));
    }

    #[test]
    fn model_request_times_out_with_a_single_terminal_when_no_tool_in_flight() {
        let clock = TestClock::new(0);
        let mut config = test_config();
        config.model_request_timeout_ms = 5_000;
        config.model_first_response_ms = 5_000;
        config.model_idle_ms = 5_000;
        let (mut c, _) = RunController::start("r", "t", config, &clock).unwrap();
        // Running with no tools in flight => waiting on the model.
        clock.advance(4_999);
        assert!(c.tick(clock.now_ms(), clock.now_ms()).is_empty());
        clock.advance(1);
        let effects = c.tick(clock.now_ms(), clock.now_ms());
        assert!(has_event(&effects, "run.failed"));
        assert_eq!(c.state(), RunState::Failed);
    }

    #[test]
    fn model_timeout_does_not_fire_during_approval_wait() {
        let clock = TestClock::new(0);
        let mut config = test_config();
        config.model_request_timeout_ms = 5_000;
        config.model_first_response_ms = 5_000;
        config.model_idle_ms = 5_000;
        config.approval_wait_timeout_ms = 60_000;
        let (mut c, _) = RunController::start("r", "t", config, &clock).unwrap();
        let policy = TestPolicy {
            allow: vec![],
            deny: vec![],
        };
        c.propose_tool_batch("b", vec![call("a", "write_file", 0)], &policy, 0, 0)
            .unwrap();
        assert_eq!(c.state(), RunState::WaitingApproval);
        // Far past the model timeout: the approval wall clock governs, not model.
        clock.advance(100_000);
        let effects = c.tick(clock.now_ms(), clock.now_ms());
        // Approval wait timeout fires (approval.wait_timeout), not model timeout.
        assert!(has_event(&effects, "run.approval_expired"));
        assert!(!has_event(&effects, "run.failed"));
        let code = effects.iter().any(|e| match e {
            Effect::AppendEvent { payload_json, .. } => {
                payload_json.contains("approval.wait_timeout")
            }
            _ => false,
        });
        assert!(
            code,
            "should be the approval-wait timeout, not model timeout"
        );
        assert!(!effects.iter().any(|e| match e {
            Effect::AppendEvent { payload_json, .. } => {
                payload_json.contains("model.first_response_timeout")
                    || payload_json.contains("model.idle_timeout")
                    || payload_json.contains("model.request_timeout")
            }
            _ => false,
        }));
    }

    #[test]
    fn cancel_wins_against_model_timeout_single_authoritative_terminal() {
        let clock = TestClock::new(0);
        let mut config = test_config();
        config.model_request_timeout_ms = 5_000;
        config.model_first_response_ms = 5_000;
        config.model_idle_ms = 5_000;
        let (mut c, _) = RunController::start("r", "t", config, &clock).unwrap();
        clock.advance(10_000);
        // Cancel first (user), then the stale model timeout tick arrives.
        c.request_cancel();
        c.settle_cancellation();
        let stale = c.tick(clock.now_ms(), clock.now_ms());
        // The tick must not override cancellation with a model-timeout failure.
        assert!(stale.iter().all(|e| matches!(e, Effect::Audit { .. })));
        assert_eq!(c.state(), RunState::Cancelled);
    }

    #[test]
    fn model_timeout_only_armed_on_explicit_request_and_disarmed_on_settle() {
        let clock = TestClock::new(0);
        let mut config = test_config();
        config.model_request_timeout_ms = 5_000;
        config.model_first_response_ms = 5_000;
        config.model_idle_ms = 5_000;
        let (mut c, _) = RunController::start("r", "t", config, &clock).unwrap();
        // The model responds (proposes a tool batch): the request settles.
        let policy = TestPolicy {
            allow: vec!["read"],
            deny: vec![],
        };
        c.propose_tool_batch("b", vec![call("a", "read", 0)], &policy, 0, 0)
            .unwrap();
        assert!(!c.model_request_in_flight());
        // Long after the timeout, with a tool still running, NO model timeout
        // fires (the tool-execution timeout governs, and there is no in-flight
        // model request).
        clock.advance(100_000);
        let effects = c.tick(clock.now_ms(), clock.now_ms());
        assert!(
            !effects.iter().any(|e| matches!(
                e, Effect::AppendEvent { payload_json, .. } if payload_json.contains("model.request_timeout")
            )),
            "model timeout must not fire after the request settled"
        );
        // A new explicit turn dispatch re-arms the request, which then times out
        // on the first-response bound (no output was ever observed).
        c.begin_model_request(clock.now_ms(), clock.now_ms());
        assert!(c.model_request_in_flight());
        clock.advance(6_000);
        let effects = c.tick(clock.now_ms(), clock.now_ms());
        assert!(effects.iter().any(|e| matches!(
            e, Effect::AppendEvent { payload_json, .. } if payload_json.contains("model.first_response_timeout")
        )));
    }

    #[test]
    fn model_timeout_window_survives_rehydrate_via_persisted_wall_anchor() {
        // Rehydrate a run whose model request was in flight at wall 1_000, with
        // a 5_000ms timeout. Even in a fresh monotonic domain the persisted wall
        // anchor counts, so the request must NOT get a fresh full window.
        let rehydrated = RehydratedRun {
            run_id: "r".into(),
            turn_id: "t".into(),
            config: {
                let mut cfg = test_config();
                cfg.model_request_timeout_ms = 5_000;
                cfg.model_first_response_ms = 5_000;
                cfg.model_idle_ms = 5_000;
                cfg
            },
            state: RunState::Running,
            seq: 1,
            running_elapsed_ms: 0,
            approval_deadline_wall_ms: None,
            terminal_written: false,
            batches: vec![],
            tools: vec![],
            retry: RetryState {
                provider_attempts: 0,
                provider_max: 2,
                turn_attempts: 0,
                turn_max: 0,
                ..RetryState::default()
            },
            compaction: CompactionState::default(),
            model_request_since_wall_ms: Some(1_000),
        };
        let mut c = RunController::rehydrate(rehydrated).unwrap();
        assert!(
            c.model_request_in_flight(),
            "in-flight request survives rehydrate"
        );
        // New process wall 7_000 => 6_000 elapsed against the persisted anchor
        // (>= 5_000): the backstop fires despite the fresh monotonic domain,
        // classified by the first-response bound (no output was observed).
        let effects = c.tick(0, 7_000);
        assert_eq!(c.state(), RunState::Failed);
        assert!(effects.iter().any(|e| matches!(
            e, Effect::AppendEvent { payload_json, .. } if payload_json.contains("model.first_response_timeout")
        )), "rehydrated model request must not reset to a fresh window");
    }

    #[test]
    fn terminal_run_holds_no_model_request_anchor() {
        let clock = TestClock::new(0);
        let mut config = test_config();
        config.model_request_timeout_ms = 5_000;
        config.model_first_response_ms = 5_000;
        config.model_idle_ms = 5_000;
        let (mut c, _) = RunController::start("r", "t", config, &clock).unwrap();
        assert!(
            c.model_request_in_flight(),
            "fresh run arms a model request"
        );
        // Move to a terminal state via model timeout.
        clock.advance(6_000);
        let _ = c.tick(clock.now_ms(), clock.now_ms());
        assert!(c.state().is_terminal(), "run is terminal");
        assert!(
            !c.model_request_in_flight(),
            "terminal run must not hold an in-flight model request"
        );
    }

    #[test]
    fn rehydrated_terminal_run_does_not_restore_model_request() {
        let rehydrated = RehydratedRun {
            run_id: "r".into(),
            turn_id: "t".into(),
            config: test_config(),
            state: RunState::Failed,
            seq: 5,
            running_elapsed_ms: 0,
            approval_deadline_wall_ms: None,
            terminal_written: true,
            batches: vec![],
            tools: vec![],
            retry: RetryState {
                provider_attempts: 0,
                provider_max: 2,
                turn_attempts: 0,
                turn_max: 0,
                ..RetryState::default()
            },
            compaction: CompactionState::default(),
            // Even if a wall anchor was persisted, a terminal rehydrate must
            // not restore the in-flight flag.
            model_request_since_wall_ms: Some(1_000),
        };
        let c = RunController::rehydrate(rehydrated).unwrap();
        assert!(c.state().is_terminal());
        assert!(
            !c.model_request_in_flight(),
            "terminal rehydrate must not resume a model request"
        );
    }

    #[test]
    fn model_first_response_idle_and_total_timeouts_are_distinct_and_progress_aware() {
        // The Host-side tick bound is a fail-closed backstop (no dispatch
        // lease owner to reset); retry admission is the worker-evidence path.
        // This test pins the three classified codes and the progress-aware
        // ordering: first-response before idle before whole-round.
        let clock = TestClock::new(0);
        let mut config = test_config();
        config.model_request_timeout_ms = 10_000;
        config.model_first_response_ms = 2_000;
        config.model_idle_ms = 3_000;
        config.turn_max_retries = 5;
        let (mut c, _) = RunController::start("r", "t", config, &clock).unwrap();
        // Silent before the first-response bound.
        clock.advance(1_999);
        assert!(c.tick(clock.now_ms(), clock.now_ms()).is_empty());
        // No output at all: the first-response bound fires first.
        clock.advance(1);
        let effects = c.tick(clock.now_ms(), clock.now_ms());
        assert!(effects.iter().any(|e| matches!(
            e, Effect::AppendEvent { payload_json, .. } if payload_json.contains("model.first_response_timeout")
        )));
        assert_eq!(c.state(), RunState::Failed);
        // Progress observed: idleness is measured from the last progress and
        // the idle bound fires before the whole-round bound.
        let mut config = test_config();
        config.model_request_timeout_ms = 10_000;
        config.model_first_response_ms = 2_000;
        config.model_idle_ms = 3_000;
        let (mut c, _) = RunController::start("r2", "t", config, &clock).unwrap();
        c.note_model_progress(clock.now_ms(), clock.now_ms());
        clock.advance(2_999);
        assert!(c.tick(clock.now_ms(), clock.now_ms()).is_empty());
        clock.advance(1);
        let effects = c.tick(clock.now_ms(), clock.now_ms());
        assert!(effects.iter().any(|e| matches!(
            e, Effect::AppendEvent { payload_json, .. } if payload_json.contains("model.idle_timeout")
        )));
        assert_eq!(c.state(), RunState::Failed);
        // Fresh progress every second keeps every sub-round bound quiet until
        // the whole-round backstop fires.
        let mut config = test_config();
        config.model_request_timeout_ms = 10_000;
        config.model_first_response_ms = 2_000;
        config.model_idle_ms = 3_000;
        let (mut c, _) = RunController::start("r3", "t", config, &clock).unwrap();
        for _ in 0..9 {
            c.note_model_progress(clock.now_ms(), clock.now_ms());
            clock.advance(1_000);
            assert!(c.tick(clock.now_ms(), clock.now_ms()).is_empty());
        }
        clock.advance(1_000);
        let effects = c.tick(clock.now_ms(), clock.now_ms());
        assert!(effects.iter().any(|e| matches!(
            e, Effect::AppendEvent { payload_json, .. } if payload_json.contains("model.request_timeout")
        )));
        assert_eq!(c.state(), RunState::Failed);
    }

    #[test]
    fn model_timeout_retries_only_when_no_new_tool_is_in_flight() {
        // The Host tick backstop fails closed even with an idle turn budget:
        // only the coordinator's settled worker evidence may retry, because
        // only it owns the dispatch lease needed to reset the delivery.
        let clock = TestClock::new(0);
        let mut config = test_config();
        config.model_request_timeout_ms = 5_000;
        config.model_first_response_ms = 5_000;
        config.model_idle_ms = 5_000;
        config.turn_max_retries = 1;
        let (mut c, _) = RunController::start("r", "t", config, &clock).unwrap();
        // A dispatched-but-unsettled tool makes the timeout fail closed: the
        // Host must reconcile the uncertain execution instead of retrying.
        let policy = TestPolicy {
            allow: vec!["read"],
            deny: vec![],
        };
        c.propose_tool_batch("b", vec![call("a", "read", 0)], &policy, 0, 0)
            .unwrap();
        // A follow-up model request is armed while the tool is still running.
        c.begin_model_request(clock.now_ms(), clock.now_ms());
        clock.advance(6_000);
        let effects = c.tick(clock.now_ms(), clock.now_ms());
        assert_eq!(c.state(), RunState::Failed);
        // No output was ever observed, so the first-response bound fires; the
        // point is that it fails closed instead of scheduling a retry.
        assert!(effects.iter().any(|e| matches!(
            e, Effect::AppendEvent { payload_json, .. } if payload_json.contains("model.first_response_timeout")
        )));
        // With nothing outstanding the same timeout still terminates (tick is
        // a backstop); the retry path is exercised through the coordinator.
        let clock = TestClock::new(0);
        let mut config = test_config();
        config.model_request_timeout_ms = 5_000;
        config.model_first_response_ms = 5_000;
        config.model_idle_ms = 5_000;
        config.turn_max_retries = 1;
        let (mut c, _) = RunController::start("r2", "t", config, &clock).unwrap();
        clock.advance(5_000);
        let _ = c.tick(clock.now_ms(), clock.now_ms());
        assert_eq!(c.state(), RunState::Failed);
        // The settled-failure retry path accepts model_timeout and spends the
        // turn budget, then terminates on exhaustion.
        let evidence = r#"{"schemaVersion":1,"runId":"r3","turnId":"t","checkpointSeq":7,"category":"model_timeout","httpStatus":null,"retryAfterMs":null,"telemetry":{"elapsedMs":120000,"idleElapsedMs":119000,"firstResponseMs":2000,"textBytes":449,"reasoningBytes":0,"toolParamBytes":82000}}"#;
        let mut config = test_config();
        config.model_request_timeout_ms = 120_000;
        config.model_first_response_ms = 60_000;
        config.model_idle_ms = 120_000;
        config.turn_max_retries = 1;
        let (mut c, _) = RunController::start("r3", "t", config, &clock).unwrap();
        let effects = c
            .schedule_model_retry(1_000, 1_000, "deliver-batch:b", evidence, false, 1_000)
            .unwrap();
        assert!(has_event(&effects, "run.retrying"));
        assert_eq!(c.state(), RunState::RetryScheduled);
        assert_eq!(c.shadow_checkpoint(0).retry.turn_attempts, 1);
        clock.advance(1_000);
        // The due retry resumes with model_dispatch_pending set: the Host
        // claims the delivery and arms the request before the engine runs.
        let _ = c.tick(clock.now_ms(), clock.now_ms()).len();
        c.begin_model_request(clock.now_ms(), clock.now_ms());
        let effects = c
            .schedule_model_retry(2_000, 2_000, "deliver-batch:b", evidence, false, 1_000)
            .unwrap();
        assert_eq!(c.state(), RunState::Failed);
        assert!(effects.iter().any(|e| matches!(
            e, Effect::AppendEvent { payload_json, .. } if payload_json.contains("kernel.model_retry_exhausted")
        )));
    }

    #[test]
    fn model_progress_notes_are_ignored_without_an_in_flight_request() {
        let clock = TestClock::new(0);
        let (mut c, _) = RunController::start("r", "t", test_config(), &clock).unwrap();
        c.settle_model_request();
        c.note_model_progress(clock.now_ms(), clock.now_ms());
        // Well under the run budget: with no request in flight nothing fires.
        clock.advance(100_000);
        assert!(c.tick(clock.now_ms(), clock.now_ms()).is_empty());
    }

    #[test]
    fn legacy_short_frozen_config_rehydrates_without_widening_budget_or_identity() {
        let mut stored = serde_json::to_value(test_config()).unwrap();
        stored["model_request_timeout_ms"] = serde_json::json!(30_000);
        stored.as_object_mut().unwrap().remove("model_first_response_ms");
        stored.as_object_mut().unwrap().remove("model_idle_ms");
        let config: RunFrozenConfig = serde_json::from_value(stored.clone()).unwrap();
        assert_eq!(config.model_request_timeout_ms,30_000);
        assert_eq!(config.model_first_response_ms,30_000);
        assert_eq!(config.model_idle_ms,30_000);
        assert_eq!(config.permission_snapshot_id,test_config().permission_snapshot_id);
        let clock = TestClock::new(0);
        let (controller, _) = RunController::start("r","t",config,&clock).unwrap();
        assert!(!controller.is_terminal());
        stored["model_idle_ms"] = serde_json::json!(30_001);
        let explicit: RunFrozenConfig = serde_json::from_value(stored.clone()).unwrap();
        assert!(RunController::start("r","t",explicit,&clock).is_err());
        stored["model_idle_ms"] = serde_json::Value::Null;
        assert!(serde_json::from_value::<RunFrozenConfig>(stored).is_err());
    }

    #[test]
    fn model_sub_round_budgets_above_the_whole_round_fail_closed() {
        let mut config = test_config();
        config.model_first_response_ms = config.model_request_timeout_ms + 1;
        assert!(RunController::start("r", "t", config.clone(), &TestClock::new(0)).is_err());
        config.model_first_response_ms = 60_000;
        config.model_idle_ms = config.model_request_timeout_ms + 1;
        assert!(RunController::start("r", "t", config, &TestClock::new(0)).is_err());
    }
}
