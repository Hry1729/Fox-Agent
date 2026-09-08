//! Recovery orchestration for the Fox Agent Kernel.
//!
//! This is pure logic over the durable read-model produced by the repository
//! adapter: it decides WHAT to recover and HOW to treat uncertain effects, and
//! returns a [`RecoveryPlan`] the production Host executes after startup. It does
//! not call an executor itself and it never asks the model to regenerate a tool
//! call. It lives behind the (currently off) Kernel mode gate; the legacy path
//! remains the production authority.
//!
//! Crash semantics for external effects:
//! * `pending` — committed but never leased; safe to dispatch now.
//! * `leased`  — a previous process may have produced the side effect before it
//!   crashed. It is NEVER blindly re-run. If the executor can reconcile by
//!   idempotency key it returns the existing result; otherwise the effect is
//!   marked `failed/requires_reconcile` and surfaced for explicit handling.

use crate::ports::{OutboxEffect, OutboxEffectKind};

/// An approval prompt that is still pending after restart; the Host re-publishes
/// it to the UI (no second approval row is created).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveredApproval {
    pub run_id: String,
    pub tool_call_id: String,
    pub tool: String,
    pub input_json: String,
}

/// An external effect whose post-crash state is uncertain and must not be
/// silently re-executed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveredUncertainEffect {
    pub run_id: String,
    pub effect: OutboxEffect,
}

/// The plan produced from durable facts at startup.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RecoveryPlan {
    /// Approvals still waiting on a human; re-publish the prompt.
    pub republish_approvals: Vec<RecoveredApproval>,
    /// Safe pending effects committed but never leased; execute with the same key.
    /// Includes tool/batch delivery and cancellation, not only tool dispatch.
    pub redispatch_pending: Vec<(String, OutboxEffect)>,
    /// Leased effects whose side effect is uncertain; reconcile, never rerun blind.
    pub uncertain_leased: Vec<RecoveredUncertainEffect>,
    /// Runs already in a terminal state. Pending dispatch/delivery/approval work
    /// is suppressed, but durable cancellation effects are still allowed to drain.
    pub terminal_run_ids: Vec<String>,
}

/// Durable inputs read by the recovery logic. The repository adapter supplies
/// these from one consistent snapshot per run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RecoveryFacts {
    pub run_id: String,
    pub state: String,
    /// Approvals still pending (tool waiting for a human), with tool/input.
    pub pending_approvals: Vec<RecoveredApproval>,
    /// Outbox effects not yet completed.
    pub open_outbox: Vec<OutboxEffect>,
}

/// Decide recovery for a set of runs from durable facts.
pub fn plan_recovery(facts: Vec<RecoveryFacts>) -> RecoveryPlan {
    let mut plan = RecoveryPlan::default();
    for fact in facts {
        let terminal = matches!(
            fact.state.as_str(),
            "completed" | "failed" | "cancelled" | "budget_exhausted"
        );
        if terminal {
            plan.terminal_run_ids.push(fact.run_id.clone());
        }
        let resume_work = !terminal && fact.state != "cancelling";
        if resume_work {
            for approval in fact.pending_approvals {
                plan.republish_approvals.push(approval);
            }
        }
        for effect in fact.open_outbox {
            match effect.status {
                crate::ports::OutboxStatus::Pending => {
                    // Committed, never executed: safe to dispatch under the same
                    // stable idempotency key.
                    let safe_pending = matches!(
                        effect.kind,
                        OutboxEffectKind::CancelEngineTurn | OutboxEffectKind::CancelToolCall
                    ) || resume_work
                        && matches!(
                            effect.kind,
                            OutboxEffectKind::DispatchTool
                                | OutboxEffectKind::DeliverToolBatch
                                | OutboxEffectKind::PublishSnapshot
                        );
                    if safe_pending {
                        plan.redispatch_pending.push((fact.run_id.clone(), effect));
                    }
                }
                crate::ports::OutboxStatus::Leased => {
                    plan.uncertain_leased.push(RecoveredUncertainEffect {
                        run_id: fact.run_id.clone(),
                        effect,
                    });
                }
                crate::ports::OutboxStatus::Completed
                | crate::ports::OutboxStatus::Failed => {}
            }
        }
    }
    plan
}
