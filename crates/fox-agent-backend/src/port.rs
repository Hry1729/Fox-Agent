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

#[cfg(test)]
mod tests {
    use super::*;

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
