use std::collections::{HashMap, VecDeque};

pub(super) const ACTIVE_RUN_LIMIT: usize = 2;

#[derive(Clone, Debug)]
pub(super) struct Session {
    pub conversation_id: String,
    pub runtime_session_id: String,
    pub generation: i64,
    pub run_id: Option<String>,
    pub waiting: bool,
    /// K2: the contact whose serial execution range this Run belongs to.
    pub companion_id: Option<String>,
    /// A background task releases the contact position while waiting for an
    /// answer; the private chat keeps it because the user is right there.
    pub companion_background: bool,
    preserve_waiting_companion: bool,
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
            generation, run_id: None, waiting: false, companion_id: None, companion_background: false,
            preserve_waiting_companion: false,
        });
        Ok(())
    }

    pub fn start(
        &mut self,
        conversation: &str,
        runtime: &str,
        generation: i64,
        run: &str,
        companion: Option<(&str, bool)>,
    ) -> Result<(), String> {
        if run.is_empty() || self.by_run(run).is_some() { return Err("sidecar Run is already active or invalid".into()); }
        if !self.can_admit(conversation, companion.map(|(id, _)| id)) {
            return Err("sidecar conversation or execution capacity is occupied".into());
        }
        self.remember(conversation, runtime, generation)?;
        let session = self.by_conversation.get_mut(conversation).ok_or("sidecar session disappeared")?;
        session.run_id = Some(run.to_owned());
        session.waiting = false;
        session.companion_id = companion.map(|(id, _)| id.to_owned());
        session.companion_background = companion.is_some_and(|(_, background)| background);
        Ok(())
    }

    pub fn reserve_waiting(
        &mut self,
        conversation: &str,
        runtime: &str,
        generation: i64,
        run: &str,
        companion: Option<(&str, bool)>,
    ) -> Result<(), String> {
        if run.is_empty() || self.by_run(run).is_some()
            || self.by_conversation.get(conversation).is_some_and(|session| session.run_id.is_some()) {
            return Err("sidecar conversation already has an active Run".into());
        }
        self.remember(conversation, runtime, generation)?;
        let session = self.by_conversation.get_mut(conversation).ok_or("sidecar session disappeared")?;
        session.run_id = Some(run.to_owned());
        session.waiting = true;
        session.companion_id = companion.map(|(id, _)| id.to_owned());
        session.companion_background = companion.is_some_and(|(_, background)| background);
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

    pub fn can_admit(&self, conversation: &str, companion: Option<&str>) -> bool {
        self.resuming.is_empty() && self.occupied() < ACTIVE_RUN_LIMIT
            && self.by_conversation.get(conversation).is_none_or(|session| session.run_id.is_none())
            && companion.is_none_or(|id| !self.holds_companion(id))
    }

    /// K2: one Run per contact at a time, across the private chat and every
    /// background execution conversation. A background Run can release the
    /// position while waiting if it has no child still using the workspace.
    pub fn holds_companion(&self, companion: &str) -> bool {
        self.by_conversation.values().any(|session| {
            session.run_id.is_some()
                && session.companion_id.as_deref() == Some(companion)
                && (!session.waiting || !session.companion_background || session.preserve_waiting_companion)
        })
    }

    pub fn set_waiting(&mut self, run: &str, waiting: bool) -> Result<bool, String> {
        self.set_waiting_with_dispatch(run, waiting, &[])
    }

    pub fn preserve_waiting_companion(&mut self, run: &str) {
        if let Some(session) = self.by_conversation.values_mut()
            .find(|session| session.run_id.as_deref() == Some(run))
        {
            session.preserve_waiting_companion = true;
        }
    }

    /// `dispatching` lists the Runs that already won a dispatch grant but have
    /// not started their session yet (`dispatching_runs`), as `(run_id,
    /// companion_id)`. They reserve the same global capacity and contact
    /// position as a started Run, so an answered approval evaluated under the
    /// same state lock cannot preempt the granted start into a failure.
    pub fn set_waiting_with_dispatch(
        &mut self,
        run: &str,
        waiting: bool,
        dispatching: &[(&str, Option<&str>)],
    ) -> Result<bool, String> {
        let occupied = self.occupied();
        let Some((was_waiting, reacquire_companion, companion)) = self
            .by_conversation
            .values()
            .find(|session| session.run_id.as_deref() == Some(run))
            .map(|session| (
                session.waiting,
                session.companion_background && !session.preserve_waiting_companion,
                session.companion_id.clone(),
            ))
        else {
            return Ok(false);
        };
        if was_waiting && !waiting {
            let starting = dispatching
                .iter()
                .filter(|(id, _)| *id != run && self.by_run(id).is_none())
                .count();
            if reacquire_companion {
                // Re-enter the contact queue rather than the global resume
                // FIFO: other contacts keep their concurrency while this Run
                // waits for the contact's position, and an answered approval
                // never overtakes a Run that is already executing or dispatched.
                if occupied + starting >= ACTIVE_RUN_LIMIT
                    || companion.as_deref().is_some_and(|id| {
                        self.holds_companion(id)
                            || dispatching.iter().any(|(other, reserved)| {
                                *other != run && *reserved == Some(id)
                            })
                    })
                {
                    return Err("sidecar execution capacity is occupied".into());
                }
            } else {
                // A dispatch grant already counts against capacity. Yield
                // without a resume ticket so the granted start stays
                // admissible; the caller retries once the reservation lands.
                if starting > 0 && occupied + starting >= ACTIVE_RUN_LIMIT {
                    return Err("sidecar execution capacity is occupied".into());
                }
                if !self.resuming.iter().any(|pending| pending == run) { self.resuming.push_back(run.to_owned()); }
                if occupied >= ACTIVE_RUN_LIMIT || self.resuming.front().map(String::as_str) != Some(run) {
                    return Err("sidecar execution capacity is occupied".into());
                }
                self.resuming.pop_front();
            }
        }
        let session = self
            .by_conversation
            .values_mut()
            .find(|session| session.run_id.as_deref() == Some(run))
            .ok_or("sidecar session disappeared")?;
        let changed = session.waiting != waiting;
        session.waiting = waiting;
        if !waiting {
            session.preserve_waiting_companion = false;
        }
        Ok(changed)
    }

    pub fn finish(&mut self, run: &str) -> bool {
        self.resuming.retain(|pending| pending != run);
        let Some(session) = self.by_conversation.values_mut().find(|session| session.run_id.as_deref() == Some(run)) else {
            return false;
        };
        session.run_id = None;
        session.waiting = false;
        session.companion_id = None;
        session.companion_background = false;
        session.preserve_waiting_companion = false;
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
    fn background_wait_with_children_keeps_the_companion_slot() {
        let mut sessions = Sessions::default();
        sessions.start("task", "task-session", 1, "parent", Some(("fox", true))).unwrap();
        sessions.start("other", "other-session", 1, "sibling", None).unwrap();

        sessions.preserve_waiting_companion("parent");
        sessions.set_waiting("parent", true).unwrap();
        assert!(!sessions.can_admit("private", Some("fox")));
        sessions.start("child", "child-session", 1, "child", None).unwrap();
        assert!(sessions.finish("child"));
        assert!(sessions.set_waiting("parent", false).unwrap());

        sessions.set_waiting("parent", true).unwrap();
        assert!(sessions.can_admit("private", Some("fox")));
    }

    #[test]
    fn conversations_are_independent_but_a_native_file_has_one_writer() {
        let mut sessions = Sessions::default();
        sessions.start("a", "sa", 1, "ra", None).unwrap();
        sessions.start("b", "sb", 4, "rb", None).unwrap();
        assert!(sessions.start("a", "sa", 1, "ra2", None).is_err());
        assert!(sessions.remember("a", "sa2", 2).is_err());
        assert!(sessions.start("c", "sc", 1, "rc", None).is_err());
        assert!(sessions.single_active().is_none());
        assert!(sessions.finish("ra"));
        assert_eq!(sessions.by_run("rb").unwrap().generation, 4);
        sessions.start("c", "sc", 1, "rc", None).unwrap();
        assert!(!sessions.finish("ra"));
    }

    #[test]
    fn waiting_releases_capacity_without_releasing_session_ownership() {
        let mut sessions = Sessions::default();
        sessions.start("a", "sa", 1, "ra", None).unwrap();
        sessions.start("b", "sb", 1, "rb", None).unwrap();
        assert!(sessions.set_waiting("ra", true).unwrap());
        assert!(!sessions.can_admit("a", None));
        sessions.start("c", "sc", 1, "rc", None).unwrap();
        assert!(sessions.set_waiting("ra", false).is_err());
        sessions.finish("rb");
        assert!(sessions.set_waiting("ra", false).unwrap());
        assert_eq!(sessions.occupied(), 2);
    }

    #[test]
    fn idle_session_remains_available_until_explicit_close() {
        let mut sessions = Sessions::default();
        sessions.start("a", "sa", 1, "ra", None).unwrap();
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
        sessions.start("a", "sa", 1, "ra", None).unwrap();
        sessions.start("b", "sb", 1, "rb", None).unwrap();
        sessions.set_waiting("ra", true).unwrap();
        sessions.start("c", "sc", 1, "rc", None).unwrap();
        assert!(sessions.set_waiting("ra", false).is_err());
        sessions.finish("rb");
        assert!(!sessions.can_admit("d", None));
        assert!(sessions.set_waiting("ra", false).unwrap());
        assert_eq!(sessions.occupied(), 2);
        sessions.finish("rc");
        assert!(sessions.can_admit("d", None));
    }

    #[test]
    fn recovered_approval_reserves_identity_before_an_execution_slot() {
        let mut sessions = Sessions::default();
        sessions.start("a", "sa", 1, "ra", None).unwrap();
        sessions.start("b", "sb", 1, "rb", None).unwrap();
        sessions.reserve_waiting("c", "sc", 5, "rc", None).unwrap();
        assert_eq!(sessions.by_run("rc").unwrap().generation, 5);
        assert_eq!(sessions.occupied(), 2);
        assert!(sessions.set_waiting("rc", false).is_err());
        assert!(sessions.reserve_waiting("c", "sc", 5, "duplicate", None).is_err());
        sessions.finish("rc");
        sessions.finish("rb");
        assert!(sessions.can_admit("d", None), "cancelled recovery must not leave a resume ticket");
    }

    /// K2: the private chat and every background execution conversation of one
    /// contact share a single execution position; other contacts do not.
    #[test]
    fn one_contact_runs_at_a_time_across_chat_and_background_conversations() {
        let mut sessions = Sessions::default();
        sessions.start("chat", "s1", 1, "foreground", Some(("cmp_a", false))).unwrap();
        assert!(sessions.holds_companion("cmp_a"));
        assert!(sessions
            .start("task-1", "s2", 1, "background", Some(("cmp_a", true)))
            .is_err(), "a background task of the same contact must queue");
        assert!(sessions
            .start("task-2", "s3", 1, "other-contact", Some(("cmp_b", true)))
            .is_ok(), "another contact keeps the global concurrency");
        assert!(!sessions.can_admit("task-1", Some("cmp_a")));
        assert!(sessions.finish("foreground"));
        assert!(sessions.start("task-1", "s2", 1, "background", Some(("cmp_a", true))).is_ok());
    }

    /// K2: an answered background approval — approved or denied, the Run
    /// continues either way — re-enters the contact queue behind the chat Run.
    #[test]
    fn background_approval_yields_the_contact_position_and_requeues_after_an_answer() {
        let mut sessions = Sessions::default();
        sessions.start("task-1", "s1", 1, "background", Some(("cmp_a", true))).unwrap();
        assert!(sessions.set_waiting("background", true).unwrap());
        assert!(!sessions.holds_companion("cmp_a"), "waiting background yields the position");
        assert_eq!(sessions.occupied(), 0);
        sessions.start("chat", "s2", 1, "foreground", Some(("cmp_a", false))).unwrap();
        assert!(sessions.set_waiting("background", false).is_err(), "the answer must queue behind the chat");
        assert!(sessions.can_admit("task-2", Some("cmp_b")), "other contacts are not blocked by the wait");
        assert!(sessions.finish("foreground"));
        assert!(sessions.set_waiting("background", false).unwrap());
        assert!(sessions.holds_companion("cmp_a"));
        assert_eq!(sessions.occupied(), 1);
    }

    /// K2: the private chat keeps the position while waiting, so the user can
    /// answer in the conversation and no background task slips in beside it.
    #[test]
    fn foreground_approval_keeps_the_contact_position_while_waiting() {
        let mut sessions = Sessions::default();
        sessions.start("chat", "s1", 1, "foreground", Some(("cmp_a", false))).unwrap();
        assert!(sessions.set_waiting("foreground", true).unwrap());
        assert!(sessions.holds_companion("cmp_a"));
        assert!(!sessions.can_admit("task-1", Some("cmp_a")));
        assert!(sessions
            .start("task-1", "s2", 1, "background", Some(("cmp_a", true)))
            .is_err());
        assert!(sessions.start("task-2", "s3", 1, "other", Some(("cmp_b", true))).is_ok());
        assert!(sessions.set_waiting("foreground", false).unwrap());
    }

    /// K2: a restart-recovered background approval reserves its identity first
    /// and still has to win the contact position before it executes.
    #[test]
    fn recovered_background_approval_queues_for_the_contact_position() {
        let mut sessions = Sessions::default();
        sessions.start("chat", "s1", 1, "foreground", Some(("cmp_a", false))).unwrap();
        sessions
            .reserve_waiting("task-1", "s2", 3, "recovered", Some(("cmp_a", true)))
            .unwrap();
        assert!(sessions.by_run("recovered").unwrap().waiting);
        assert_eq!(sessions.occupied(), 1, "the reservation holds no execution position yet");
        assert!(sessions.set_waiting("recovered", false).is_err());
        assert!(sessions.finish("foreground"));
        assert!(sessions.set_waiting("recovered", false).unwrap());
        assert_eq!(sessions.by_run("recovered").unwrap().generation, 3);
    }

    /// K2/P2: a Run that already won dispatch but has not started yet reserves
    /// the contact position. An answered background approval must queue behind
    /// it, and the granted start stays admissible (no start failure).
    #[test]
    fn answered_background_approval_cannot_preempt_a_dispatch_reservation() {
        let mut sessions = Sessions::default();
        sessions.start("task-1", "s1", 1, "background", Some(("cmp_a", true))).unwrap();
        assert!(sessions.set_waiting("background", true).unwrap());
        let dispatching = [("granted", Some("cmp_a"))];
        assert!(sessions.set_waiting_with_dispatch("background", false, &dispatching).is_err(),
            "the answer must wait for the dispatch reservation");
        assert!(sessions.by_run("background").unwrap().waiting);
        sessions.start("task-2", "s2", 1, "granted", Some(("cmp_a", true)))
            .expect("the granted start must not fail because of the answered approval");
        // Once started, the reservation counts through the session itself.
        assert!(sessions.set_waiting_with_dispatch("background", false, &dispatching).is_err());
        assert!(sessions.set_waiting_with_dispatch("background", false, &[]).is_err());
        assert!(sessions.finish("granted"));
        assert!(sessions.set_waiting_with_dispatch("background", false, &[]).unwrap());
        assert!(sessions.holds_companion("cmp_a"));
    }

    /// K2/P2: dispatch reservations also count against the global capacity,
    /// even when they belong to another contact.
    #[test]
    fn answered_background_approval_respects_reserved_global_capacity() {
        let mut sessions = Sessions::default();
        sessions.start("task-1", "s1", 1, "background", Some(("cmp_a", true))).unwrap();
        sessions.start("task-2", "s2", 1, "other", Some(("cmp_b", true))).unwrap();
        assert!(sessions.set_waiting("background", true).unwrap());
        let dispatching = [("granted", Some("cmp_c"))];
        assert!(sessions.set_waiting_with_dispatch("background", false, &dispatching).is_err(),
            "one executing Run plus one granted start already fill the capacity");
        sessions.start("task-3", "s3", 1, "granted", Some(("cmp_c", true)))
            .expect("the granted start keeps its reserved slot");
        assert!(sessions.set_waiting_with_dispatch("background", false, &dispatching).is_err(),
            "the started reservation now counts through its session");
        assert!(sessions.finish("other"));
        // The reservation still names a started Run: it must not be counted
        // twice, so the freed slot admits the answered approval.
        assert!(sessions.set_waiting_with_dispatch("background", false, &dispatching).unwrap());
        assert!(sessions.holds_companion("cmp_a"));
    }

    /// K2/P2: a foreground resume blocked by a dispatch grant yields without a
    /// resume ticket, so the granted start is not refused by `can_admit`.
    #[test]
    fn foreground_resume_yields_to_a_dispatch_grant_without_a_ticket() {
        let mut sessions = Sessions::default();
        sessions.start("chat", "s1", 1, "foreground", Some(("cmp_a", false))).unwrap();
        sessions.start("task-2", "s2", 1, "other", Some(("cmp_b", true))).unwrap();
        assert!(sessions.set_waiting("foreground", true).unwrap());
        let dispatching = [("granted", Some("cmp_c"))];
        assert!(sessions.set_waiting_with_dispatch("foreground", false, &dispatching).is_err());
        assert!(sessions.can_admit("task-3", Some("cmp_c")),
            "no resume ticket may block the granted start");
        sessions.start("task-3", "s3", 1, "granted", Some(("cmp_c", true)))
            .expect("the granted start keeps its reserved slot");
        assert!(sessions.set_waiting_with_dispatch("foreground", false, &[]).is_err(),
            "capacity is full once the granted Run started");
        assert!(sessions.finish("other"));
        assert!(sessions.set_waiting("foreground", false).unwrap());
    }
}
