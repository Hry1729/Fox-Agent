//! Pending-request settlement and the single authoritative terminal per run.
//! Plan section 6.4.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// How a pending request (Host request, reverse request, tool call) left the
/// waiting state. `OutcomeUnknown` is first-class: a request that may have run
/// but whose receipt was lost is never coerced into one of the other three.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum PendingOutcome {
    Succeeded { receipt: String },
    Failed { error: String },
    /// Confirmed not executed, or confirmed stopped.
    Cancelled,
    /// May have executed; to be reconciled. Keeps its correlation id.
    OutcomeUnknown { reason: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingProgress {
    /// Not yet handed to the executor.
    NotSent,
    /// Handed over (written, dispatched); result not yet known.
    InFlight,
}

/// Registry of pending requests for one run. Every entry leaves the waiting
/// state when the run is cancelled or the connection is lost (rule 1); late
/// responses are kept for reconciliation (rule 2).
#[derive(Debug, Default)]
pub struct PendingRegistry {
    pending: BTreeMap<String, (u64, PendingProgress)>,
    settled: BTreeMap<String, PendingOutcome>,
    late: Vec<LateResponse>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LateResponse {
    pub correlation_id: String,
    pub generation: u64,
    pub settled_as: Option<PendingOutcome>,
    pub response: String,
}

impl PendingRegistry {
    pub fn open(&mut self, correlation_id: &str, generation: u64) {
        self.pending.insert(correlation_id.to_owned(), (generation, PendingProgress::NotSent));
    }

    pub fn mark_in_flight(&mut self, correlation_id: &str) {
        if let Some(entry) = self.pending.get_mut(correlation_id) {
            entry.1 = PendingProgress::InFlight;
        }
    }

    pub fn complete(&mut self, correlation_id: &str, generation: u64, outcome: PendingOutcome, raw: &str) {
        match self.pending.get(correlation_id) {
            Some((opened_in, _)) if *opened_in == generation => {
                self.pending.remove(correlation_id);
                self.settled.insert(correlation_id.to_owned(), outcome);
            }
            _ => self.late.push(LateResponse {
                correlation_id: correlation_id.to_owned(),
                generation,
                settled_as: self.settled.get(correlation_id).cloned(),
                response: raw.to_owned(),
            }),
        }
    }

    /// Cancel or disconnect: nothing stays waiting. Not-yet-sent requests are
    /// cancelled; in-flight ones are unknown.
    pub fn settle_all(&mut self, reason: &str) -> Vec<(String, PendingOutcome)> {
        let drained = std::mem::take(&mut self.pending);
        let mut result = Vec::with_capacity(drained.len());
        for (id, (_, progress)) in drained {
            let outcome = match progress {
                PendingProgress::NotSent => PendingOutcome::Cancelled,
                PendingProgress::InFlight => PendingOutcome::OutcomeUnknown {
                    reason: reason.to_owned(),
                },
            };
            self.settled.insert(id.clone(), outcome.clone());
            result.push((id, outcome));
        }
        result
    }

    pub fn waiting(&self) -> usize {
        self.pending.len()
    }

    pub fn outcome(&self, correlation_id: &str) -> Option<&PendingOutcome> {
        self.settled.get(correlation_id)
    }

    pub fn late_responses(&self) -> &[LateResponse] {
        &self.late
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunTerminal {
    Completed,
    Failed,
    Cancelled,
}

/// UI states of plan 6.4 rule 5; `ConnectionLost` is not a terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunPhase {
    Running,
    WaitingApproval,
    Cancelling,
    ConnectionLost,
    Terminal(RunTerminal),
}

/// One authoritative terminal per run. Later terminal claims are retained as
/// audit and never flip the committed one (rules 2-4).
#[derive(Debug, Default)]
pub struct TerminalArbiter {
    committed: Option<Committed>,
    cancel_requested: bool,
    rejected: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Committed {
    pub terminal: RunTerminal,
    /// Cancel was requested but the run finished first.
    pub cancel_also_requested: bool,
    /// The process tree was force-killed; side effects may be unverified.
    pub possible_unverified_side_effects: bool,
}

impl TerminalArbiter {
    pub fn request_cancel(&mut self) {
        self.cancel_requested = true;
    }

    pub fn commit(&mut self, terminal: RunTerminal, force_killed: bool) -> &Committed {
        if let Some(existing) = &self.committed {
            self.rejected.push(format!(
                "terminal {terminal:?} ignored; already committed {:?}",
                existing.terminal
            ));
        } else {
            self.committed = Some(Committed {
                terminal,
                cancel_also_requested: self.cancel_requested && terminal != RunTerminal::Cancelled,
                possible_unverified_side_effects: force_killed,
            });
        }
        self.committed.as_ref().expect("committed above")
    }

    pub fn committed(&self) -> Option<&Committed> {
        self.committed.as_ref()
    }

    pub fn rejected(&self) -> &[String] {
        &self.rejected
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancel_settles_everything_and_keeps_unknown_distinct() {
        let mut registry = PendingRegistry::default();
        registry.open("a", 1);
        registry.open("b", 1);
        registry.mark_in_flight("b");
        let settled = registry.settle_all("cancelled by user");
        assert_eq!(registry.waiting(), 0);
        assert_eq!(settled[0], ("a".into(), PendingOutcome::Cancelled));
        assert!(matches!(settled[1].1, PendingOutcome::OutcomeUnknown { .. }));
    }

    #[test]
    fn late_response_is_kept_not_applied() {
        let mut registry = PendingRegistry::default();
        registry.open("a", 1);
        registry.mark_in_flight("a");
        registry.settle_all("disconnect");
        registry.complete("a", 1, PendingOutcome::Succeeded { receipt: "r".into() }, "{ok}");
        assert!(matches!(registry.outcome("a"), Some(PendingOutcome::OutcomeUnknown { .. })));
        assert_eq!(registry.late_responses().len(), 1);
        assert!(registry.late_responses()[0].settled_as.is_some());
    }

    #[test]
    fn response_from_old_generation_is_late() {
        let mut registry = PendingRegistry::default();
        registry.open("a", 2);
        registry.complete("a", 1, PendingOutcome::Failed { error: "x".into() }, "{}");
        assert_eq!(registry.waiting(), 1);
        assert_eq!(registry.late_responses().len(), 1);
    }

    #[test]
    fn first_terminal_wins_and_cancel_is_recorded_on_completion() {
        let mut arbiter = TerminalArbiter::default();
        arbiter.request_cancel();
        let committed = arbiter.commit(RunTerminal::Completed, false).clone();
        assert!(committed.cancel_also_requested);
        arbiter.commit(RunTerminal::Cancelled, false);
        assert_eq!(arbiter.committed().unwrap().terminal, RunTerminal::Completed);
        assert_eq!(arbiter.rejected().len(), 1);
    }

    #[test]
    fn force_kill_marks_unverified_side_effects() {
        let mut arbiter = TerminalArbiter::default();
        arbiter.request_cancel();
        assert!(arbiter.commit(RunTerminal::Cancelled, true).possible_unverified_side_effects);
    }
}
