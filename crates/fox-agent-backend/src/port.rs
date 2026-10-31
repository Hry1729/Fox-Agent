//! Backend identity, capability declaration, session binding and event envelope.

use serde::{Deserialize, Serialize};

/// The outer Agent that drives a conversation. Distinct from the historical
/// `engine_id` (`pi | codex | deepseek_harness`), which keeps its meaning as the
/// model adapter inside FoxBackend and is never renamed (plan 6.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentBackend {
    Fox,
    Pi,
    Codex,
    Deepseek,
}

impl AgentBackend {
    pub const ALL: [AgentBackend; 4] = [Self::Fox, Self::Pi, Self::Codex, Self::Deepseek];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fox => "fox",
            Self::Pi => "pi",
            Self::Codex => "codex",
            Self::Deepseek => "deepseek",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|backend| backend.as_str() == value)
    }

    /// Experimental backends may skip full function/stability gates but never
    /// the safety floor (plan section 8).
    pub fn experimental(self) -> bool {
        matches!(self, Self::Deepseek)
    }

    /// Backends that are implemented but hidden from every user-facing
    /// choice (settings, new-conversation pickers, permission defaults), and
    /// that a NEW conversation cannot be frozen to. Existing conversations of
    /// a hidden backend keep their binding and run as before.
    ///
    /// Codex was hidden on 2026-10-24 because Fox's outer sandbox layer could
    /// not run it reliably. That layer is no longer applied to external
    /// backends (2026-10-26), so Codex is selectable again. Keep the list and
    /// `HIDDEN_AGENT_BACKENDS` in
    /// `apps/desktop/src/features/conversations/model/agent-backend.ts` in
    /// sync. Independent of `FOX_EXTERNAL_BACKENDS`, which can still only
    /// force a backend off.
    pub const HIDDEN: [AgentBackend; 0] = [];

    /// Whether this backend is hidden from user-facing choices ([`Self::HIDDEN`]).
    pub fn hidden(self) -> bool {
        Self::HIDDEN.contains(&self)
    }
}

/// How a tool category is gated before it runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalGate {
    /// Each call waits for the Host's answer.
    PerCall,
    /// Approval exists but is coarser than one call (session/always options).
    Coarse,
    /// Runs without Host approval.
    None,
    /// Not yet measured; never shown as gated.
    Unverified,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolCategory {
    FileRead,
    FileWrite,
    Shell,
    Network,
    McpBusinessTool,
}

/// Who enforces a boundary, if anyone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Enforcement {
    /// OS or native sandbox enforces it.
    Enforced,
    /// Only requested (prompt, config) - not enforced.
    Advisory,
    Absent,
    Unverified,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CancelReach {
    /// Native protocol confirms the turn stopped.
    TurnConfirmed,
    /// Cancel is sent but completion is not confirmable.
    TurnRequested,
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Visibility {
    /// Start and end of each tool call are reported.
    ToolLifecycle,
    /// Only the final message is visible.
    FinalOnly,
    Unverified,
}

/// The four separate dimensions of plan section 2. They are deliberately not
/// collapsed into one security level: an approved `npm test` is still free to do
/// whatever the sandbox allows inside the test script.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityDeclaration {
    pub approval_coverage: Vec<(ToolCategory, ApprovalGate)>,
    pub sandbox_filesystem: Enforcement,
    pub sandbox_network: Enforcement,
    pub cancel_turn: CancelReach,
    /// Fox (not the backend) can terminate the whole process tree.
    pub cancel_process_tree: bool,
    /// The native protocol acknowledges an approval answer. When false, an
    /// answer stays `response_written` and is never shown as acknowledged.
    pub approval_answer_acknowledged: bool,
    pub observability: Visibility,
    pub session: SessionCapabilities,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SessionCapabilities {
    /// Reopen with history replay (ACP `session/load`).
    pub load: bool,
    /// Reopen without replay (ACP `session/resume`).
    pub resume: bool,
    pub close: bool,
    pub list: bool,
    /// Mid-turn steering is available (ACP has no standard method; codex-acp
    /// exposes `_session/steering`).
    pub steer: bool,
}

impl CapabilityDeclaration {
    /// The conservative starting point for any backend before measurement:
    /// nothing gated, nothing enforced, cancel requested only.
    pub fn unverified() -> Self {
        Self {
            approval_coverage: [
                ToolCategory::FileRead,
                ToolCategory::FileWrite,
                ToolCategory::Shell,
                ToolCategory::Network,
                ToolCategory::McpBusinessTool,
            ]
            .into_iter()
            .map(|category| (category, ApprovalGate::Unverified))
            .collect(),
            sandbox_filesystem: Enforcement::Unverified,
            sandbox_network: Enforcement::Unverified,
            cancel_turn: CancelReach::TurnRequested,
            cancel_process_tree: true,
            approval_answer_acknowledged: false,
            observability: Visibility::Unverified,
            session: SessionCapabilities::default(),
        }
    }

    pub fn gate(&self, category: ToolCategory) -> ApprovalGate {
        self.approval_coverage
            .iter()
            .find(|(candidate, _)| *candidate == category)
            .map(|(_, gate)| *gate)
            .unwrap_or(ApprovalGate::Unverified)
    }

    /// What the UI may claim for a category. Only a measured per-call gate is
    /// shown as "each call needs approval".
    pub fn ui_claims_per_call_approval(&self, category: ToolCategory) -> bool {
        self.gate(category) == ApprovalGate::PerCall
    }
}

/// Created once when a conversation is bound; immutable afterwards (plan 5.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionBinding {
    pub conversation_id: String,
    pub agent_backend: AgentBackend,
    /// Backend's own session id; absent until the backend created it.
    pub native_session_ref: Option<String>,
    pub config_snapshot_ref: String,
    pub backend_version: String,
}

impl SessionBinding {
    /// A binding never changes backend. Returns an error instead of silently
    /// rebinding when a later request names another backend.
    pub fn route(&self, requested: AgentBackend) -> Result<AgentBackend, PortError> {
        if requested != self.agent_backend {
            return Err(PortError::BackendMismatch {
                bound: self.agent_backend,
                requested,
            });
        }
        Ok(self.agent_backend)
    }
}

/// One normalized event. `generation` changes on every reconnect/restart;
/// `native_seq` and `adapter_seq` are recorded separately so a replayed old
/// event is not mistaken for new execution (plan 6.1 rule 3, 6.4 rule 2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventEnvelope {
    pub event_id: String,
    pub backend: AgentBackend,
    pub conversation_id: String,
    pub run_id: String,
    pub turn_id: String,
    pub generation: u64,
    pub native_seq: Option<u64>,
    pub adapter_seq: u64,
    pub kind: String,
    pub payload: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PortError {
    BackendMismatch {
        bound: AgentBackend,
        requested: AgentBackend,
    },
    TurnBusy,
    Unsupported(&'static str),
    StaleGeneration {
        current: u64,
        received: u64,
    },
    Refused(String),
}

impl std::fmt::Display for PortError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BackendMismatch { bound, requested } => write!(
                f,
                "conversation is bound to {} and cannot be routed to {}",
                bound.as_str(),
                requested.as_str()
            ),
            Self::TurnBusy => write!(f, "a turn is already active in this conversation"),
            Self::Unsupported(what) => write!(f, "not supported by this backend: {what}"),
            Self::StaleGeneration { current, received } => write!(
                f,
                "event from connection generation {received} ignored (current {current})"
            ),
            Self::Refused(reason) => write!(f, "{reason}"),
        }
    }
}

impl std::error::Error for PortError {}

/// Dedupe and generation filter for incoming events. Events from an older
/// generation, or already seen ids, are returned as audit-only.
#[derive(Debug, Default)]
pub struct EventIntake {
    generation: u64,
    seen: std::collections::HashSet<String>,
    next_adapter_seq: u64,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Intake {
    Accept(u64),
    Duplicate,
    AuditOnly(PortError),
}

impl EventIntake {
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// A reconnect or restart. Everything from earlier generations becomes
    /// audit-only from here on.
    pub fn bump_generation(&mut self) -> u64 {
        self.generation += 1;
        self.generation
    }

    pub fn admit(&mut self, event_id: &str, generation: u64) -> Intake {
        if generation != self.generation {
            return Intake::AuditOnly(PortError::StaleGeneration {
                current: self.generation,
                received: generation,
            });
        }
        if !self.seen.insert(event_id.to_owned()) {
            return Intake::Duplicate;
        }
        self.next_adapter_seq += 1;
        Intake::Accept(self.next_adapter_seq)
    }
}

// ---------------------------------------------------------------------------
// Revocation stop (full access withdrawn while the process runs with it)
// ---------------------------------------------------------------------------

/// Whether the old process tree is known to have ended. The rule is "the
/// whole tree is gone", never "the parent exited" (see [`crate::proctree`]):
///
/// * Linux / macOS: the backend's process group is gone (`kill(-pgid, 0)` ->
///   `ESRCH`, or only zombies left) and every tracked descendant that left
///   the group (same pid + start time) is gone, within a short poll.
/// * Windows: Fox's Job Object reports `ActiveProcesses == 0`. Without a job
///   (the `taskkill /T /F` fallback) the result is always unconfirmed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum StopConfirmation {
    Confirmed,
    /// The kill failed, could not be checked, or something was still running
    /// when the poll ran out. `reasons` name what (pids, errors).
    FailedOrUnconfirmed { reasons: Vec<String> },
}

impl StopConfirmation {
    pub fn is_confirmed(&self) -> bool {
        matches!(self, Self::Confirmed)
    }

    pub fn reasons(&self) -> &[String] {
        match self {
            Self::Confirmed => &[],
            Self::FailedOrUnconfirmed { reasons } => reasons,
        }
    }
}

/// Result of terminating a backend process tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessTreeStop {
    /// The backend did not exit within the grace period and was killed (side
    /// effects of a running command unverified).
    pub force_killed: bool,
    pub confirmation: StopConfirmation,
    /// What "confirmed" covers when it covers less than the whole tree.
    pub confirmation_scope_note: Option<String>,
    /// Unix: the backend's process group, for [`crate::proctree::confirm_tree_gone`]
    /// when the Host re-checks an unconfirmed stop later.
    pub process_group: Option<u32>,
    /// Unix: every descendant seen before the kill (pid + start time).
    pub tracked: Vec<crate::proctree::ProcIdentity>,
}

impl ProcessTreeStop {
    pub fn confirmed(force_killed: bool) -> Self {
        Self { force_killed, confirmation: StopConfirmation::Confirmed, confirmation_scope_note: None, process_group: None, tracked: Vec::new() }
    }

    pub fn unconfirmed(force_killed: bool, reasons: Vec<String>) -> Self {
        Self { confirmation: StopConfirmation::FailedOrUnconfirmed { reasons }, ..Self::confirmed(force_killed) }
    }
}

/// What the Host must stop when [`crate::external_home::recheck_permission_mode`]
/// returns `RestartWithoutFullAccess`. Implemented by the live connection
/// ([`crate::acp::AcpConnection`] has an async equivalent, see
/// `AcpConnection::stop_for_revocation`) and by mocks in tests.
pub trait RevocationTarget {
    /// Enter the stopping state: from now on new tasks are refused and any
    /// permission request that arrives is answered `cancelled` at once,
    /// without reaching the user. Returns the native ids of requests that
    /// were pending and got answered `cancelled`.
    fn begin_stopping(&mut self) -> Vec<String>;
    /// Stop the current turn (`session/cancel`) and answer every pending
    /// native permission request with `cancelled`. Returns the native ids that
    /// were answered.
    fn cancel_turn(&mut self) -> Vec<String>;
    /// Terminate the whole backend process tree and say whether it is
    /// confirmed gone ([`StopConfirmation`]).
    fn kill_process_tree(&mut self) -> ProcessTreeStop;
}

/// User-facing remedy when the old process tree could not be confirmed gone.
pub const STOP_UNCONFIRMED_REMEDY: &str = "Fox 无法确认原“完全访问”进程已全部结束，为安全起见不会重建这个会话。请在任务管理器（Windows）或活动监视器/终端（macOS、Linux）中结束列出的残留进程，或退出并重新打开 Fox，然后再继续。";

/// Where a session stands after a revocation stop. The stopped process is
/// never kept running.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum RevokedSessionState {
    /// The connection is refusing new work and auto-cancelling late
    /// permission requests while its process tree is being stopped. Only
    /// observed while a stop is in progress.
    Stopping,
    /// Process tree confirmed gone; the next continue starts a new
    /// connection in the recorded mode, without replaying the last operation.
    NeedsRebuild,
    /// The old process tree could not be confirmed gone. Blocked: no rebuild
    /// in any mode until a later check confirms it
    /// ([`RevocationStop::record_confirmation`]).
    StoppedUnconfirmed { reasons: Vec<String>, remedy: String },
}

/// Record of one revocation stop, for audit and for the rebuild.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevocationStop {
    pub reason: String,
    /// Native permission requests answered `cancelled` (pending at the start
    /// of the stop).
    pub cancelled_permissions: Vec<String>,
    /// Every pending request of the run, settled: not yet sent ->
    /// `cancelled`, in flight -> `outcome_unknown` (settlement rule 1).
    pub settled: Vec<(String, crate::settlement::PendingOutcome)>,
    /// The run's authoritative terminal (first one wins; a turn that already
    /// completed keeps `completed`).
    pub terminal: crate::settlement::Committed,
    /// How the process tree ended and whether that is confirmed.
    pub process_tree: ProcessTreeStop,
    pub state: RevokedSessionState,
    /// Always `false`: the rebuilt session never replays the last operation on
    /// its own, because it may already have had side effects.
    pub replay_last_operation: bool,
}

impl RevocationStop {
    /// Operations whose effect is unknown. The rebuilt session shows them to
    /// the user for reconciliation instead of running them again.
    pub fn needs_review(&self) -> Vec<&str> {
        self.settled
            .iter()
            .filter(|(_, outcome)| matches!(outcome, crate::settlement::PendingOutcome::OutcomeUnknown { .. }))
            .map(|(id, _)| id.as_str())
            .collect()
    }

    /// A later check of an unconfirmed stop (on Unix
    /// [`crate::proctree::confirm_tree_gone`] with
    /// `process_tree.process_group` / `tracked`). `Confirmed` unblocks the
    /// rebuild; anything else keeps (and refreshes) `StoppedUnconfirmed`.
    /// No effect on a stop that was already confirmed.
    pub fn record_confirmation(&mut self, confirmation: StopConfirmation) {
        if self.process_tree.confirmation.is_confirmed() {
            return;
        }
        self.state = state_after_stop(&confirmation);
        self.process_tree.confirmation = confirmation;
    }
}

fn state_after_stop(confirmation: &StopConfirmation) -> RevokedSessionState {
    match confirmation {
        StopConfirmation::Confirmed => RevokedSessionState::NeedsRebuild,
        StopConfirmation::FailedOrUnconfirmed { reasons } => RevokedSessionState::StoppedUnconfirmed {
            reasons: reasons.clone(),
            remedy: STOP_UNCONFIRMED_REMEDY.to_owned(),
        },
    }
}

/// First half of a revocation stop: cancel was requested, permissions were
/// answered, pending requests leave the waiting state.
pub fn settle_for_revocation(
    pending: &mut crate::settlement::PendingRegistry,
    arbiter: &mut crate::settlement::TerminalArbiter,
    reason: &str,
) -> Vec<(String, crate::settlement::PendingOutcome)> {
    arbiter.request_cancel();
    pending.settle_all(&format!("stopped: {reason}"))
}

/// Second half, after the process tree was terminated. An unconfirmed stop
/// marks possible unverified side effects and blocks the rebuild.
pub fn finish_revocation(
    arbiter: &mut crate::settlement::TerminalArbiter,
    reason: &str,
    cancelled_permissions: Vec<String>,
    settled: Vec<(String, crate::settlement::PendingOutcome)>,
    process_tree: ProcessTreeStop,
) -> RevocationStop {
    let unknown = settled
        .iter()
        .any(|(_, outcome)| matches!(outcome, crate::settlement::PendingOutcome::OutcomeUnknown { .. }));
    let unverified = process_tree.force_killed || unknown || !process_tree.confirmation.is_confirmed();
    let terminal = arbiter.commit(crate::settlement::RunTerminal::Cancelled, unverified).clone();
    RevocationStop {
        reason: reason.to_owned(),
        cancelled_permissions,
        settled,
        terminal,
        state: state_after_stop(&process_tree.confirmation),
        process_tree,
        replay_last_operation: false,
    }
}

/// The "revocation stop" entry: enter `Stopping` (new tasks refused, late
/// permission requests auto-cancelled), cancel the turn, settle every pending
/// request (in flight -> `outcome_unknown`), kill the process tree and confirm
/// it is gone. Confirmed -> `NeedsRebuild` without replay;
/// unconfirmed -> `StoppedUnconfirmed` (blocked).
pub fn stop_for_revocation(
    target: &mut dyn RevocationTarget,
    pending: &mut crate::settlement::PendingRegistry,
    arbiter: &mut crate::settlement::TerminalArbiter,
    reason: &str,
) -> RevocationStop {
    let mut cancelled = target.begin_stopping();
    for id in target.cancel_turn() {
        if !cancelled.contains(&id) {
            cancelled.push(id);
        }
    }
    let settled = settle_for_revocation(pending, arbiter, reason);
    let process_tree = target.kill_process_tree();
    finish_revocation(arbiter, reason, cancelled, settled, process_tree)
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::settlement::{PendingOutcome, PendingRegistry, RunTerminal, TerminalArbiter};

    #[derive(Default)]
    struct MockConnection {
        calls: Vec<&'static str>,
        pending_permissions: Vec<String>,
        alive: bool,
        stopping: bool,
        /// What the kill reports (default: confirmed).
        kill_result: Option<ProcessTreeStop>,
    }

    impl RevocationTarget for MockConnection {
        fn begin_stopping(&mut self) -> Vec<String> {
            self.calls.push("stopping");
            self.stopping = true;
            std::mem::take(&mut self.pending_permissions)
        }
        fn cancel_turn(&mut self) -> Vec<String> {
            self.calls.push("cancel");
            std::mem::take(&mut self.pending_permissions)
        }
        fn kill_process_tree(&mut self) -> ProcessTreeStop {
            self.calls.push("kill");
            let result = self.kill_result.clone().unwrap_or_else(|| ProcessTreeStop::confirmed(true));
            self.alive = !result.confirmation.is_confirmed();
            result
        }
    }

    fn revoked() -> (MockConnection, PendingRegistry, TerminalArbiter, RevocationStop) {
        revoked_with(None)
    }

    fn revoked_with(kill_result: Option<ProcessTreeStop>) -> (MockConnection, PendingRegistry, TerminalArbiter, RevocationStop) {
        let mut connection =
            MockConnection { pending_permissions: vec!["perm-0".into()], alive: true, kill_result, ..Default::default() };
        let mut pending = PendingRegistry::default();
        pending.open("tool-sent", 1);
        pending.mark_in_flight("tool-sent");
        pending.open("tool-queued", 1);
        let mut arbiter = TerminalArbiter::default();
        let stop = stop_for_revocation(&mut connection, &mut pending, &mut arbiter, "full access withdrawn");
        (connection, pending, arbiter, stop)
    }

    #[test]
    fn revocation_stop_cancels_settles_kills_and_never_replays() {
        let (connection, pending, arbiter, stop) = revoked();
        assert_eq!(connection.calls, ["stopping", "cancel", "kill"], "stopping, then cancel, then kill");
        assert!(connection.stopping);
        assert!(!connection.alive);
        assert!(stop.process_tree.confirmation.is_confirmed());
        assert_eq!(stop.cancelled_permissions, ["perm-0"]);
        assert_eq!(pending.waiting(), 0, "nothing left waiting");
        assert!(matches!(pending.outcome("tool-sent"), Some(PendingOutcome::OutcomeUnknown { .. })), "in flight = unknown");
        assert_eq!(pending.outcome("tool-queued"), Some(&PendingOutcome::Cancelled), "never sent = cancelled");
        assert_eq!(stop.needs_review(), ["tool-sent"]);
        assert_eq!(stop.terminal.terminal, RunTerminal::Cancelled);
        assert!(stop.terminal.possible_unverified_side_effects);
        assert_eq!(arbiter.committed().unwrap().terminal, RunTerminal::Cancelled);
        assert_eq!(stop.state, RevokedSessionState::NeedsRebuild);
        assert!(!stop.replay_last_operation);
    }

    #[test]
    fn revocation_after_a_completed_turn_keeps_completed() {
        let mut connection = MockConnection { alive: true, ..Default::default() };
        let mut pending = PendingRegistry::default();
        let mut arbiter = TerminalArbiter::default();
        arbiter.commit(RunTerminal::Completed, false);
        let stop = stop_for_revocation(&mut connection, &mut pending, &mut arbiter, "revoked");
        assert_eq!(stop.terminal.terminal, RunTerminal::Completed, "first terminal wins");
        assert_eq!(connection.calls, ["stopping", "cancel", "kill"], "process is stopped regardless");
        assert_eq!(arbiter.rejected().len(), 1);
    }

    /// Kill failed or the tree could not be confirmed gone: blocked until a
    /// later check confirms it, and the state carries reasons + remedy.
    #[test]
    fn unconfirmed_stop_stays_blocked_until_confirmed() {
        for reasons in [
            vec!["kill(-4100, 9) failed: Operation not permitted".to_owned()],
            // Parent exited, a child of the same group is still running.
            vec!["process group 4100 still has running members [4102]".to_owned()],
        ] {
            let (connection, _, _, mut stop) = revoked_with(Some(ProcessTreeStop::unconfirmed(true, reasons.clone())));
            assert!(connection.alive, "mock still has the child");
            assert!(stop.terminal.possible_unverified_side_effects);
            assert_eq!(
                stop.state,
                RevokedSessionState::StoppedUnconfirmed { reasons: reasons.clone(), remedy: STOP_UNCONFIRMED_REMEDY.to_owned() }
            );
            stop.record_confirmation(StopConfirmation::FailedOrUnconfirmed { reasons: vec!["still there".into()] });
            assert!(matches!(&stop.state, RevokedSessionState::StoppedUnconfirmed { reasons, .. } if reasons == &["still there"]));
            stop.record_confirmation(StopConfirmation::Confirmed);
            assert_eq!(stop.state, RevokedSessionState::NeedsRebuild);
            assert!(!stop.replay_last_operation);
        }
    }

    #[test]
    fn stop_records_serialize_for_the_host() {
        let unconfirmed = StopConfirmation::FailedOrUnconfirmed { reasons: vec!["x".into()] };
        assert_eq!(serde_json::to_value(&unconfirmed).unwrap(), serde_json::json!({"status": "failed_or_unconfirmed", "reasons": ["x"]}));
        assert_eq!(serde_json::to_value(StopConfirmation::Confirmed).unwrap(), serde_json::json!({"status": "confirmed"}));
        let state = RevokedSessionState::StoppedUnconfirmed { reasons: vec!["x".into()], remedy: "r".into() };
        assert_eq!(serde_json::to_value(&state).unwrap()["state"], "stopped_unconfirmed");
        assert_eq!(serde_json::to_value(RevokedSessionState::Stopping).unwrap()["state"], "stopping");
    }

    #[test]
    fn backend_ids_round_trip_and_only_deepseek_is_experimental() {
        for backend in AgentBackend::ALL {
            assert_eq!(AgentBackend::parse(backend.as_str()), Some(backend));
        }
        assert_eq!(AgentBackend::parse("deepseek_harness"), None, "engine_id values are not backends");
        assert!(AgentBackend::Deepseek.experimental());
        assert!(!AgentBackend::Codex.experimental());
    }

    #[test]
    fn binding_refuses_cross_backend_routing() {
        let binding = SessionBinding {
            conversation_id: "c1".into(),
            agent_backend: AgentBackend::Codex,
            native_session_ref: None,
            config_snapshot_ref: "cfg".into(),
            backend_version: "codex-acp 2.2.2".into(),
        };
        assert_eq!(binding.route(AgentBackend::Codex), Ok(AgentBackend::Codex));
        assert!(matches!(binding.route(AgentBackend::Pi), Err(PortError::BackendMismatch { .. })));
    }

    #[test]
    fn unverified_capabilities_never_claim_per_call_approval() {
        let declared = CapabilityDeclaration::unverified();
        for category in [ToolCategory::FileWrite, ToolCategory::Shell, ToolCategory::Network] {
            assert!(!declared.ui_claims_per_call_approval(category));
        }
        assert!(!declared.approval_answer_acknowledged);
    }

    #[test]
    fn intake_drops_duplicates_and_stale_generations() {
        let mut intake = EventIntake::default();
        assert_eq!(intake.admit("e1", 0), Intake::Accept(1));
        assert_eq!(intake.admit("e1", 0), Intake::Duplicate);
        let generation = intake.bump_generation();
        assert!(matches!(intake.admit("e2", 0), Intake::AuditOnly(PortError::StaleGeneration { .. })));
        assert_eq!(intake.admit("e2", generation), Intake::Accept(2));
    }
}
