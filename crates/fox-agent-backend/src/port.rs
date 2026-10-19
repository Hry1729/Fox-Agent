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
    /// Sandboxed mode could not start; never degraded to full access.
    Sandbox(crate::sandbox::SandboxError),
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
            Self::Sandbox(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for PortError {}

impl From<crate::sandbox::SandboxError> for PortError {
    fn from(error: crate::sandbox::SandboxError) -> Self {
        Self::Sandbox(error)
    }
}

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
// Revocation stop (full access withdrawn while the process runs unsandboxed)
// ---------------------------------------------------------------------------

/// What the Host must stop when [`crate::sandbox::recheck_permission_mode`]
/// returns `RestartSandboxed`. Implemented by the live connection
/// ([`crate::acp::AcpConnection`] has an async equivalent, see
/// `AcpConnection::stop_for_revocation`) and by mocks in tests.
pub trait RevocationTarget {
    /// Stop the current turn (`session/cancel`) and answer every pending
    /// native permission request with `cancelled`. Returns the native ids that
    /// were answered.
    fn cancel_turn(&mut self) -> Vec<String>;
    /// Terminate the whole backend process tree. Returns `true` when the tree
    /// had to be force-killed (side effects of a running command unverified).
    fn kill_process_tree(&mut self) -> bool;
}

/// Where a session stands after a revocation stop. There is no state in
/// which it keeps running outside the sandbox.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum RevokedSessionState {
    /// Process gone; the next continue must rebuild under srt (preflight
    /// included, fail-closed).
    NeedsSandboxedRebuild,
    /// The sandboxed rebuild failed; the session stays stopped until a later
    /// attempt succeeds. Never restarted in full access.
    StoppedSandboxUnavailable { error: crate::sandbox::SandboxError },
}

/// Record of one revocation stop, for audit and for the rebuild.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevocationStop {
    pub reason: String,
    /// Native permission requests answered `cancelled`.
    pub cancelled_permissions: Vec<String>,
    /// Every pending request of the run, settled: not yet sent ->
    /// `cancelled`, in flight -> `outcome_unknown` (settlement rule 1).
    pub settled: Vec<(String, crate::settlement::PendingOutcome)>,
    /// The run's authoritative terminal (first one wins; a turn that already
    /// completed keeps `completed`).
    pub terminal: crate::settlement::Committed,
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

/// Second half, after the process tree is gone.
pub fn finish_revocation(
    arbiter: &mut crate::settlement::TerminalArbiter,
    reason: &str,
    cancelled_permissions: Vec<String>,
    settled: Vec<(String, crate::settlement::PendingOutcome)>,
    force_killed: bool,
) -> RevocationStop {
    let unknown = settled
        .iter()
        .any(|(_, outcome)| matches!(outcome, crate::settlement::PendingOutcome::OutcomeUnknown { .. }));
    let terminal = arbiter.commit(crate::settlement::RunTerminal::Cancelled, force_killed || unknown).clone();
    RevocationStop {
        reason: reason.to_owned(),
        cancelled_permissions,
        settled,
        terminal,
        state: RevokedSessionState::NeedsSandboxedRebuild,
        replay_last_operation: false,
    }
}

/// The "revocation stop" entry: cancel the turn, settle every pending request
/// (in flight -> `outcome_unknown`), kill the process tree, and mark the
/// session as needing a sandboxed rebuild without replay. Order matters:
/// cancel first so the backend gets a chance to stop cleanly and pending
/// permissions are answered, then settle, then kill.
pub fn stop_for_revocation(
    target: &mut dyn RevocationTarget,
    pending: &mut crate::settlement::PendingRegistry,
    arbiter: &mut crate::settlement::TerminalArbiter,
    reason: &str,
) -> RevocationStop {
    let cancelled = target.cancel_turn();
    let settled = settle_for_revocation(pending, arbiter, reason);
    let force_killed = target.kill_process_tree();
    finish_revocation(arbiter, reason, cancelled, settled, force_killed)
}

/// A session rebuilt after revocation. The Host spawns `launch` and opens a
/// new connection generation ([`EventIntake::bump_generation`]); it does not
/// resend the last prompt or re-run `review`.
#[derive(Debug)]
pub struct RebuiltSession {
    pub launch: crate::sandbox::PreparedLaunch,
    pub replay_last_operation: bool,
    /// Correlation ids settled `outcome_unknown` by the stop, for the user.
    pub review: Vec<String>,
}

/// Rebuild a revoked session. `prepare` is the normal sandboxed launch path
/// (`preflight` + `prepare_launch(PermissionMode::Sandboxed, ..)`); it is
/// given the mode so a caller cannot ask for anything else. On failure, or if
/// what came back is not sandboxed, the stop record moves to
/// `StoppedSandboxUnavailable` and the session stays stopped.
pub fn rebuild_after_revocation(
    stop: &mut RevocationStop,
    prepare: impl FnOnce(crate::sandbox::PermissionMode) -> Result<crate::sandbox::PreparedLaunch, crate::sandbox::SandboxError>,
) -> Result<RebuiltSession, crate::sandbox::SandboxError> {
    use crate::sandbox::{PermissionMode, SandboxError, SandboxErrorCode};
    let result = prepare(PermissionMode::Sandboxed).and_then(|launch| {
        if launch.mode == PermissionMode::Sandboxed && launch.sandbox.is_some() {
            Ok(launch)
        } else {
            Err(SandboxError::new(
                SandboxErrorCode::SandboxPolicyInvalid,
                "rebuild after revocation must run sandboxed; refusing an unsandboxed launch",
            ))
        }
    });
    match result {
        Ok(launch) => {
            stop.state = RevokedSessionState::NeedsSandboxedRebuild;
            Ok(RebuiltSession {
                launch,
                replay_last_operation: false,
                review: stop.needs_review().into_iter().map(str::to_owned).collect(),
            })
        }
        Err(error) => {
            stop.state = RevokedSessionState::StoppedSandboxUnavailable { error: error.clone() };
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::sandbox::{PermissionMode, PreparedLaunch, SandboxDeclaration, SandboxError, SandboxErrorCode, HostOs};
    use crate::settlement::{PendingOutcome, PendingRegistry, RunTerminal, TerminalArbiter};

    #[derive(Default)]
    struct MockConnection {
        calls: Vec<&'static str>,
        pending_permissions: Vec<String>,
        alive: bool,
    }

    impl RevocationTarget for MockConnection {
        fn cancel_turn(&mut self) -> Vec<String> {
            self.calls.push("cancel");
            std::mem::take(&mut self.pending_permissions)
        }
        fn kill_process_tree(&mut self) -> bool {
            self.calls.push("kill");
            self.alive = false;
            true
        }
    }

    fn launch(mode: PermissionMode, sandboxed: bool) -> PreparedLaunch {
        let dir = std::env::temp_dir().join(format!("fox-port-rebuild-{}-{sandboxed}", std::process::id()));
        PreparedLaunch {
            mode,
            spec: crate::launch::LaunchSpec {
                backend: AgentBackend::Pi,
                program: "/x".into(),
                args: vec![],
                version_pin: "x",
                credential_env: &[],
                experimental: false,
                session_env: vec![],
            },
            sandbox: sandboxed.then(|| crate::sandbox::SandboxSession::for_test(dir)),
            declaration: SandboxDeclaration::declare(mode, None, HostOs::Linux),
        }
    }

    fn revoked() -> (MockConnection, PendingRegistry, TerminalArbiter, RevocationStop) {
        let mut connection = MockConnection { pending_permissions: vec!["perm-0".into()], alive: true, ..Default::default() };
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
        assert_eq!(connection.calls, ["cancel", "kill"], "cancel before kill");
        assert!(!connection.alive);
        assert_eq!(stop.cancelled_permissions, ["perm-0"]);
        assert_eq!(pending.waiting(), 0, "nothing left waiting");
        assert!(matches!(pending.outcome("tool-sent"), Some(PendingOutcome::OutcomeUnknown { .. })), "in flight = unknown");
        assert_eq!(pending.outcome("tool-queued"), Some(&PendingOutcome::Cancelled), "never sent = cancelled");
        assert_eq!(stop.needs_review(), ["tool-sent"]);
        assert_eq!(stop.terminal.terminal, RunTerminal::Cancelled);
        assert!(stop.terminal.possible_unverified_side_effects);
        assert_eq!(arbiter.committed().unwrap().terminal, RunTerminal::Cancelled);
        assert_eq!(stop.state, RevokedSessionState::NeedsSandboxedRebuild);
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
        assert_eq!(connection.calls, ["cancel", "kill"], "process is stopped regardless");
        assert_eq!(arbiter.rejected().len(), 1);
    }

    #[test]
    fn rebuild_is_sandboxed_only_without_replay_and_stays_stopped_on_failure() {
        let (_, _, _, mut stop) = revoked();
        // Sandbox cannot start: session stays stopped, no fallback.
        let error = rebuild_after_revocation(&mut stop, |mode| {
            assert_eq!(mode, PermissionMode::Sandboxed, "rebuild asks for sandboxed only");
            Err(SandboxError::new(SandboxErrorCode::SandboxDependencyMissing, "srt missing"))
        })
        .unwrap_err();
        assert_eq!(error.code, SandboxErrorCode::SandboxDependencyMissing);
        assert!(matches!(stop.state, RevokedSessionState::StoppedSandboxUnavailable { .. }));

        // A launcher that hands back an unsandboxed launch is refused too.
        let error = rebuild_after_revocation(&mut stop, |_| Ok(launch(PermissionMode::FullAccess, false))).unwrap_err();
        assert_eq!(error.code, SandboxErrorCode::SandboxPolicyInvalid);
        assert!(matches!(stop.state, RevokedSessionState::StoppedSandboxUnavailable { .. }));

        // Success: sandboxed, no replay, unknown operations handed over for review.
        let rebuilt = rebuild_after_revocation(&mut stop, |_| Ok(launch(PermissionMode::Sandboxed, true))).unwrap();
        assert_eq!(rebuilt.launch.mode, PermissionMode::Sandboxed);
        assert!(!rebuilt.replay_last_operation);
        assert_eq!(rebuilt.review, ["tool-sent"]);
        assert_eq!(stop.state, RevokedSessionState::NeedsSandboxedRebuild);
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
