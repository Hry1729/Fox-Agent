//! Concurrency admission (plan 6.2). Every limit hit is a visible outcome
//! (queued or refused with a reason), never a silent drop or unbounded wait.

use crate::port::AgentBackend;
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Limits {
    pub queued_inputs_per_session: usize,
    pub global_native_processes: usize,
    pub per_backend_processes: BTreeMap<AgentBackend, usize>,
    pub pending_reverse_per_session: usize,
}

impl Default for Limits {
    /// Placeholders until phase 0 measures memory per process (plan 6.2 says
    /// "待定"); configurable, never hard-coded at call sites.
    fn default() -> Self {
        Self {
            queued_inputs_per_session: 4,
            global_native_processes: 4,
            per_backend_processes: AgentBackend::ALL.into_iter().map(|backend| (backend, 2)).collect(),
            pending_reverse_per_session: 8,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Admission {
    Start,
    Queued { position: usize, reason: String },
    Refused(String),
}

#[derive(Debug, Default)]
pub struct Admissions {
    limits: Limits,
    active_turn: BTreeMap<String, bool>,
    queued: BTreeMap<String, usize>,
    processes: BTreeMap<String, AgentBackend>,
    process_queue: Vec<(String, AgentBackend)>,
}

impl Admissions {
    pub fn new(limits: Limits) -> Self {
        Self { limits, ..Self::default() }
    }

    /// Submit a user turn. One active turn per session; extra input queues up
    /// to the limit, then is refused. Never reinterpreted as steering.
    pub fn submit_turn(&mut self, session: &str) -> Admission {
        if !self.active_turn.get(session).copied().unwrap_or(false) {
            self.active_turn.insert(session.to_owned(), true);
            return Admission::Start;
        }
        let queued = self.queued.entry(session.to_owned()).or_default();
        if *queued >= self.limits.queued_inputs_per_session {
            return Admission::Refused(format!(
                "this conversation already has {} queued inputs",
                self.limits.queued_inputs_per_session
            ));
        }
        *queued += 1;
        Admission::Queued {
            position: *queued,
            reason: "a turn is already running in this conversation".into(),
        }
    }

    /// The active turn ended; returns whether a queued input starts next.
    pub fn finish_turn(&mut self, session: &str) -> bool {
        match self.queued.get_mut(session) {
            Some(count) if *count > 0 => {
                *count -= 1;
                true
            }
            _ => {
                self.active_turn.insert(session.to_owned(), false);
                false
            }
        }
    }

    /// Ask for a native process for a session.
    pub fn acquire_process(&mut self, session: &str, backend: AgentBackend) -> Admission {
        if self.processes.contains_key(session) {
            return Admission::Start;
        }
        let per_backend = self.processes.values().filter(|candidate| **candidate == backend).count();
        let backend_limit = self.limits.per_backend_processes.get(&backend).copied().unwrap_or(1);
        if self.processes.len() < self.limits.global_native_processes && per_backend < backend_limit {
            self.processes.insert(session.to_owned(), backend);
            return Admission::Start;
        }
        if !self.process_queue.iter().any(|(queued, _)| queued == session) {
            self.process_queue.push((session.to_owned(), backend));
        }
        let position = self.process_queue.iter().position(|(queued, _)| queued == session).unwrap_or(0) + 1;
        Admission::Queued {
            position,
            reason: if per_backend >= backend_limit {
                format!("{} already has {backend_limit} active sessions", backend.as_str())
            } else {
                format!("{} agent processes are already running", self.limits.global_native_processes)
            },
        }
    }

    /// Release a process; returns the queued session that may start now.
    pub fn release_process(&mut self, session: &str) -> Option<String> {
        self.processes.remove(session);
        let index = self.process_queue.iter().position(|(_, backend)| {
            let per_backend = self.processes.values().filter(|candidate| *candidate == backend).count();
            per_backend < self.limits.per_backend_processes.get(backend).copied().unwrap_or(1)
                && self.processes.len() < self.limits.global_native_processes
        })?;
        let (next, backend) = self.process_queue.remove(index);
        self.processes.insert(next.clone(), backend);
        Some(next)
    }

    /// Too many open reverse requests is abnormal: the turn is stopped.
    pub fn reverse_request_allowed(&self, open_in_session: usize) -> Result<(), String> {
        if open_in_session >= self.limits.pending_reverse_per_session {
            return Err(format!(
                "more than {} unanswered agent requests; stopping the turn",
                self.limits.pending_reverse_per_session
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_active_turn_then_visible_queue_then_refusal() {
        let mut admissions = Admissions::new(Limits { queued_inputs_per_session: 1, ..Limits::default() });
        assert_eq!(admissions.submit_turn("s"), Admission::Start);
        assert!(matches!(admissions.submit_turn("s"), Admission::Queued { position: 1, .. }));
        assert!(matches!(admissions.submit_turn("s"), Admission::Refused(_)));
        assert!(admissions.finish_turn("s"));
        assert!(!admissions.finish_turn("s"));
        assert_eq!(admissions.submit_turn("s"), Admission::Start);
    }

    #[test]
    fn process_limits_queue_and_hand_over() {
        let mut limits = Limits { global_native_processes: 2, ..Limits::default() };
        limits.per_backend_processes.insert(AgentBackend::Codex, 1);
        let mut admissions = Admissions::new(limits);
        assert_eq!(admissions.acquire_process("a", AgentBackend::Codex), Admission::Start);
        assert!(matches!(admissions.acquire_process("b", AgentBackend::Codex), Admission::Queued { .. }));
        assert_eq!(admissions.acquire_process("c", AgentBackend::Pi), Admission::Start);
        assert_eq!(admissions.release_process("a"), Some("b".into()));
    }
}
