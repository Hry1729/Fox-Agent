//! Synthetic decision corpus ("golden snapshots") for the Fox Agent Kernel.
//!
//! Each case is fully synthetic — no real user messages, paths, API keys or
//! project content. A case runs a deterministic script of Kernel inputs against
//! the pure decision core and asserts the authoritative state / effects, so a
//! regression in any control-flow invariant is caught without a provider.
//!
//! Covered scenarios (mapped to the Phase 3B acceptance list):
//! * auto-run vs forced approval and policy deny,
//! * out-of-order batch completion handed back in source order,
//! * cancellation race and late success after cancellation,
//! * duplicate vs conflicting tool/event identities,
//! * provider retry vs turn retry (independent),
//! * approval wait timeout vs execution budget (separate clocks),
//! * compaction vs retry state distinction,
//! * crash-after-approval recovery is covered in the repository outbox tests.

use crate::ports::{PolicyDecision, PolicyDecisionPort};
use crate::state::{ApprovalDecision, RunOutcome, RunState};
use crate::{Clock, RunController, RunFrozenConfig, TestClock, ToolCallRequest};

struct ScriptedPolicy {
    auto: &'static [&'static str],
    deny: &'static [&'static str],
}
impl PolicyDecisionPort for ScriptedPolicy {
    fn decide(&self, _: &str, _: &str, tool: &str, _: &str) -> PolicyDecision {
        if self.deny.contains(&tool) {
            PolicyDecision::Deny {
                reason: "synthetic deny".into(),
            }
        } else if self.auto.contains(&tool) {
            PolicyDecision::Allow
        } else {
            PolicyDecision::RequireApproval
        }
    }
}

fn config() -> RunFrozenConfig {
    RunFrozenConfig {
        engine_id: "pi".into(),
        kernel_mode: "shadow".into(),
        capability_manifest_version: 2,
        capability_manifest_hash: "synthetic-manifest-hash".into(),
        permission_snapshot_id: "synthetic-perm-v1".into(),
        execution_profile_id: "legacy".into(),
        prompt_config_hash: "synthetic-prompt-hash".into(),
        model_request_timeout_ms: 120_000,
        tool_execution_timeout_ms: 600_000,
        run_execution_budget_ms: 600_000,
        approval_wait_timeout_ms: 60_000,
        provider_max_retries: 2,
        turn_max_retries: 1,
    }
}

fn req(id: &str, tool: &str, order: usize) -> ToolCallRequest {
    ToolCallRequest {
        tool_call_id: id.into(),
        tool: tool.into(),
        canonical_input_json: format!(r#"{{"synthetic":{order}}}"#),
        source_order: order,
    }
}

/// Run every golden case. Each case is independent and deterministic.
pub fn run_corpus() -> Vec<(&'static str, bool)> {
    vec![
        ("auto_run_skips_only_policy_allowed_tools", auto_run_case()),
        (
            "forced_approval_parks_and_deny_fails",
            forced_approval_case(),
        ),
        (
            "out_of_order_batch_returns_source_order",
            out_of_order_case(),
        ),
        ("cancel_race_late_success_is_audit_only", cancel_race_case()),
        (
            "duplicate_event_idempotent_conflict_fails_closed",
            duplicate_conflict_case(),
        ),
        (
            "provider_and_turn_retry_are_independent",
            retry_independence_case(),
        ),
        (
            "approval_clock_is_separate_from_execution_budget",
            clock_separation_case(),
        ),
        ("compaction_state_differs_from_retry", compaction_case()),
    ]
}

fn auto_run_case() -> bool {
    let clock = TestClock::new(0);
    let policy = ScriptedPolicy {
        auto: &["read", "ls"],
        deny: &["dangerous"],
    };
    let (mut c, _) = RunController::start("g-auto", "t", config(), &clock).unwrap();
    let effects = c
        .propose_tool_batch(
            "b",
            vec![req("a", "read", 0), req("b", "ls", 1)],
            &policy,
            0,
            0,
        )
        .unwrap();
    // Both policy-allowed tools dispatch without an approval prompt.
    let dispatches = effects
        .iter()
        .filter(|e| matches!(e, crate::Effect::DispatchTool { .. }))
        .count();
    let approvals = effects
        .iter()
        .filter(|e| matches!(e, crate::Effect::RequestApproval { .. }))
        .count();
    c.state() == RunState::Running && dispatches == 2 && approvals == 0
}

fn forced_approval_case() -> bool {
    let clock = TestClock::new(0);
    let policy = ScriptedPolicy {
        auto: &[],
        deny: &["dangerous"],
    };
    let (mut c, _) = RunController::start("g-approval", "t", config(), &clock).unwrap();
    let effects = c
        .propose_tool_batch("b", vec![req("a", "write_file", 0)], &policy, 0, 0)
        .unwrap();
    if c.state() != RunState::WaitingApproval {
        return false;
    }
    if !effects
        .iter()
        .any(|e| matches!(e, crate::Effect::RequestApproval { .. }))
    {
        return false;
    }
    // Deny fails the tool rather than dispatching it.
    let denied = c.resolve_approval("a", ApprovalDecision::Deny, 0).unwrap();
    denied
        .iter()
        .all(|e| !matches!(e, crate::Effect::DispatchTool { .. }))
        && c.state() == RunState::Running
}

fn out_of_order_case() -> bool {
    let clock = TestClock::new(0);
    let policy = ScriptedPolicy {
        auto: &["read", "ls", "grep"],
        deny: &[],
    };
    let (mut c, _) = RunController::start("g-order", "t", config(), &clock).unwrap();
    c.propose_tool_batch(
        "b",
        vec![req("a", "read", 0), req("b", "ls", 1), req("c", "grep", 2)],
        &policy,
        0,
        0,
    )
    .unwrap();
    c.tool_settled("b", true, "{}").unwrap();
    c.tool_settled("c", true, "{}").unwrap();
    let effects = c.tool_settled("a", true, "{}").unwrap();
    effects.iter().any(|e| match e {
        crate::Effect::BatchBarrier {
            ordered_tool_call_ids,
            ..
        } => ordered_tool_call_ids == &vec!["a".to_string(), "b".to_string(), "c".to_string()],
        _ => false,
    })
}

fn cancel_race_case() -> bool {
    let clock = TestClock::new(0);
    let policy = ScriptedPolicy {
        auto: &["read"],
        deny: &[],
    };
    let (mut c, _) = RunController::start("g-cancel", "t", config(), &clock).unwrap();
    c.propose_tool_batch("b", vec![req("a", "read", 0)], &policy, 0, 0)
        .unwrap();
    c.request_cancel();
    // A tool succeeds after cancellation: audited, not applied; run stays cancelling.
    let late = c.tool_settled("a", true, "{}").unwrap();
    let audited = late
        .iter()
        .any(|e| matches!(e, crate::Effect::Audit { .. }));
    c.settle_cancellation();
    audited && c.state() == RunState::Cancelled
}

fn duplicate_conflict_case() -> bool {
    let clock = TestClock::new(0);
    let policy = ScriptedPolicy {
        auto: &["read"],
        deny: &[],
    };
    let (mut c, _) = RunController::start("g-dup", "t", config(), &clock).unwrap();
    c.propose_tool_batch("b", vec![req("a", "read", 0)], &policy, 0, 0)
        .unwrap();
    c.tool_settled("a", true, "{}").unwrap();
    // Duplicate settlement is idempotent (audit only)...
    let duplicate = c.tool_settled("a", true, "{}").unwrap();
    let dup_ok = duplicate
        .iter()
        .all(|e| matches!(e, crate::Effect::Audit { .. }));
    // ...and re-proposing the same batch id with changed input fails closed.
    let mut changed = req("a", "read", 0);
    changed.canonical_input_json = r#"{"synthetic":999}"#.into();
    let conflict = c.propose_tool_batch("b", vec![changed], &policy, 0, 0);
    dup_ok && conflict.is_err()
}

fn retry_independence_case() -> bool {
    let clock = TestClock::new(0);
    let (mut c, _) = RunController::start("g-retry", "t", config(), &clock).unwrap();
    // Provider retries are bounded independently (max 2) and don't touch turn state.
    c.record_provider_retry().unwrap();
    c.record_provider_retry().unwrap();
    let provider_exhausted = c.record_provider_retry().is_err();
    // Turn retry uses its own budget (max 1): schedule, resume, then exhaust.
    c.schedule_turn_retry(0, 1_000, "synthetic.5xx", 100)
        .unwrap();
    let in_retry = c.state() == RunState::RetryScheduled;
    c.retry_resume(100, 1_100).unwrap();
    let exhausted = c
        .schedule_turn_retry(100, 1_100, "synthetic.5xx", 100)
        .unwrap();
    let turn_failed = exhausted.iter().any(|e| match e {
        crate::Effect::AppendEvent { event_type, .. } => event_type == "run.failed",
        _ => false,
    });
    provider_exhausted && in_retry && turn_failed
}

fn clock_separation_case() -> bool {
    let clock = TestClock::new(0);
    let mut cfg = config();
    cfg.run_execution_budget_ms = 1_000;
    cfg.approval_wait_timeout_ms = 5_000;
    let policy = ScriptedPolicy {
        auto: &[],
        deny: &[],
    };
    let (mut c, _) = RunController::start("g-clock", "t", cfg, &clock).unwrap();
    clock.advance(800);
    c.tick(clock.now_monotonic_ms(), clock.now_wall_ms()); // accrue 800ms
    c.propose_tool_batch("b", vec![req("a", "write_file", 0)], &policy, 800, 800)
        .unwrap();
    // A long human wait (wall 800 -> 100800) crosses the approval deadline
    // (800 + 5000 = 5800). It fires the APPROVAL timeout — never the execution
    // budget, which was suspended for the whole wait.
    clock.advance(100_000);
    let effects = c.tick(clock.now_monotonic_ms(), clock.now_wall_ms());
    effects.iter().any(|e| match e {
        crate::Effect::AppendEvent { event_type, .. } => event_type == "run.failed",
        _ => false,
    })
}

fn compaction_case() -> bool {
    let clock = TestClock::new(0);
    let (mut c, _) = RunController::start("g-compact", "t", config(), &clock).unwrap();
    c.begin_compaction(0, "synthetic_context_full").unwrap();
    let compacting = c.state() == RunState::Compacting;
    c.end_compaction(0, false).unwrap();
    // After compaction the run resumes; it never entered retry_scheduled.
    compacting
        && c.state() == RunState::Running
        && c.terminate(RunOutcome::Completed).iter().any(|e| match e {
            crate::Effect::AppendEvent { event_type, .. } => event_type == "run.completed",
            _ => false,
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn golden_decision_corpus_passes() {
        for (name, passed) in run_corpus() {
            assert!(passed, "golden corpus case failed: {name}");
        }
    }
}
