use std::collections::{HashMap, VecDeque};

pub(super) const ACTIVE_RUN_LIMIT: usize = 2;

#[derive(Clone, Debug)]
pub(super) struct Session {
    pub conversation_id: String,
    pub runtime_session_id: String,
    pub generation: i64,
    pub run_id: Option<String>,
    pub waiting: bool,
}

#[derive(Default)]
pub(super) struct Sessions {
    by_conversation: HashMap<String, Session>,
    resuming: VecDeque<String>,
}

impl Sessions {
    pub fn remember(&mut self, conversation: &str, runtime: &str, generation: i64) -> Result<(), String> {
        if conversation.is_empty() || runtime.is_empty() || generation < 1 {
            return Err("sidecar session identity is incomplete".into());
        }
        if let Some(old) = self.by_conversation.get(conversation) {
            if old.run_id.is_some() && (old.runtime_session_id != runtime || old.generation != generation) {
                return Err("cannot replace a sidecar session owned by an active Run".into());
            }
            if old.runtime_session_id == runtime && old.generation == generation { return Ok(()); }
        }
        self.by_conversation.insert(conversation.to_owned(), Session {
            conversation_id: conversation.to_owned(), runtime_session_id: runtime.to_owned(),
            generation, run_id: None, waiting: false,
        });
        Ok(())
    }

    pub fn start(&mut self, conversation: &str, runtime: &str, generation: i64, run: &str) -> Result<(), String> {
        if run.is_empty() || self.by_run(run).is_some() { return Err("sidecar Run is already active or invalid".into()); }
        if !self.can_admit(conversation) { return Err("sidecar conversation or execution capacity is occupied".into()); }
        self.remember(conversation, runtime, generation)?;
        let session = self.by_conversation.get_mut(conversation).ok_or("sidecar session disappeared")?;
        session.run_id = Some(run.to_owned());
        session.waiting = false;
        Ok(())
    }

    pub fn reserve_waiting(&mut self, conversation: &str, runtime: &str, generation: i64, run: &str) -> Result<(), String> {
        if run.is_empty() || self.by_run(run).is_some()
            || self.by_conversation.get(conversation).is_some_and(|session| session.run_id.is_some()) {
            return Err("sidecar conversation already has an active Run".into());
        }
        self.remember(conversation, runtime, generation)?;
        let session = self.by_conversation.get_mut(conversation).ok_or("sidecar session disappeared")?;
        session.run_id = Some(run.to_owned());
        session.waiting = true;
        Ok(())
    }

    pub fn by_conversation(&self, conversation: &str) -> Option<&Session> { self.by_conversation.get(conversation) }

    pub fn by_run(&self, run: &str) -> Option<&Session> {
        self.by_conversation.values().find(|session| session.run_id.as_deref() == Some(run))
    }

    pub fn occupied(&self) -> usize {
        self.by_conversation.values().filter(|session| session.run_id.is_some() && !session.waiting).count()
    }

    pub fn active(&self) -> Vec<Session> {
        self.by_conversation.values().filter(|session| session.run_id.is_some()).cloned().collect()
    }

    pub fn is_empty(&self) -> bool { !self.by_conversation.values().any(|session| session.run_id.is_some()) }

    pub fn can_admit(&self, conversation: &str) -> bool {
        self.resuming.is_empty() && self.occupied() < ACTIVE_RUN_LIMIT
            && self.by_conversation.get(conversation).is_none_or(|session| session.run_id.is_none())
    }

    pub fn set_waiting(&mut self, run: &str, waiting: bool) -> Result<bool, String> {
        let occupied = self.occupied();
        let Some(session) = self.by_conversation.values_mut().find(|session| session.run_id.as_deref() == Some(run)) else {
            return Ok(false);
        };
        if session.waiting && !waiting {
            if !self.resuming.iter().any(|pending| pending == run) { self.resuming.push_back(run.to_owned()); }
            if occupied >= ACTIVE_RUN_LIMIT || self.resuming.front().map(String::as_str) != Some(run) {
                return Err("sidecar execution capacity is occupied".into());
            }
            self.resuming.pop_front();
        }
        let changed = session.waiting != waiting;
        session.waiting = waiting;
        Ok(changed)
    }

    pub fn finish(&mut self, run: &str) -> bool {
        self.resuming.retain(|pending| pending != run);
        let Some(session) = self.by_conversation.values_mut().find(|session| session.run_id.as_deref() == Some(run)) else {
            return false;
        };
        session.run_id = None;
        session.waiting = false;
        true
    }

    pub fn remove(&mut self, conversation: &str) -> Result<Option<Session>, String> {
        if self.by_conversation.get(conversation).is_some_and(|session| session.run_id.is_some()) {
            return Err("cannot close a session with an active Run".into());
        }
        Ok(self.by_conversation.remove(conversation))
    }

    pub fn single_active(&self) -> Option<&Session> {
        let mut active = self.by_conversation.values().filter(|session| session.run_id.is_some());
        let first = active.next()?;
        if active.next().is_some() { None } else { Some(first) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversations_are_independent_but_a_native_file_has_one_writer() {
        let mut sessions = Sessions::default();
        sessions.start("a", "sa", 1, "ra").unwrap();
        sessions.start("b", "sb", 4, "rb").unwrap();
        assert!(sessions.start("a", "sa", 1, "ra2").is_err());
        assert!(sessions.remember("a", "sa2", 2).is_err());
        assert!(sessions.start("c", "sc", 1, "rc").is_err());
        assert!(sessions.single_active().is_none());
        assert!(sessions.finish("ra"));
        assert_eq!(sessions.by_run("rb").unwrap().generation, 4);
        sessions.start("c", "sc", 1, "rc").unwrap();
        assert!(!sessions.finish("ra"));
    }

    #[test]
    fn waiting_releases_capacity_without_releasing_session_ownership() {
        let mut sessions = Sessions::default();
        sessions.start("a", "sa", 1, "ra").unwrap();
        sessions.start("b", "sb", 1, "rb").unwrap();
        assert!(sessions.set_waiting("ra", true).unwrap());
        assert!(!sessions.can_admit("a"));
        sessions.start("c", "sc", 1, "rc").unwrap();
        assert!(sessions.set_waiting("ra", false).is_err());
        sessions.finish("rb");
        assert!(sessions.set_waiting("ra", false).unwrap());
        assert_eq!(sessions.occupied(), 2);
    }

    #[test]
    fn idle_session_remains_available_until_explicit_close() {
        let mut sessions = Sessions::default();
        sessions.start("a", "sa", 1, "ra").unwrap();
        assert!(sessions.remove("a").is_err());
        sessions.finish("ra");
        assert_eq!(sessions.by_conversation("a").unwrap().runtime_session_id, "sa");
        sessions.remember("a", "sa2", 2).unwrap();
        assert_eq!(sessions.remove("a").unwrap().unwrap().generation, 2);
        assert!(sessions.by_conversation("a").is_none());
    }
    #[test]
    fn resumed_run_gets_capacity_before_a_new_queued_conversation() {
        let mut sessions = Sessions::default();
        sessions.start("a", "sa", 1, "ra").unwrap();
        sessions.start("b", "sb", 1, "rb").unwrap();
        sessions.set_waiting("ra", true).unwrap();
        sessions.start("c", "sc", 1, "rc").unwrap();
        assert!(sessions.set_waiting("ra", false).is_err());
        sessions.finish("rb");
        assert!(!sessions.can_admit("d"));
        assert!(sessions.set_waiting("ra", false).unwrap());
        assert_eq!(sessions.occupied(), 2);
        sessions.finish("rc");
        assert!(sessions.can_admit("d"));
    }

    #[test]
    fn recovered_approval_reserves_identity_before_an_execution_slot() {
        let mut sessions = Sessions::default();
        sessions.start("a", "sa", 1, "ra").unwrap();
        sessions.start("b", "sb", 1, "rb").unwrap();
        sessions.reserve_waiting("c", "sc", 5, "rc").unwrap();
        assert_eq!(sessions.by_run("rc").unwrap().generation, 5);
        assert_eq!(sessions.occupied(), 2);
        assert!(sessions.set_waiting("rc", false).is_err());
        assert!(sessions.reserve_waiting("c", "sc", 5, "duplicate").is_err());
        sessions.finish("rc");
        sessions.finish("rb");
        assert!(sessions.can_admit("d"), "cancelled recovery must not leave a resume ticket");
    }

}
