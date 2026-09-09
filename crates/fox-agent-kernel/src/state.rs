//! Fox Agent Kernel — domain state model.
//!
//! This module is intentionally dependency-free: it depends only on `std` and
//! [`crate::ports`] traits. It must never reference `tauri::AppHandle`,
//! React/TypeScript types, Pi private types, or concrete provider types. Tauri
//! commands, the SQLite repository and the Node/JSONL IPC live in adapters that
//! implement the ports. Making the Kernel the sole production authority for
//! state transitions, approval, retry, budget, cancellation and terminal
//! classification is the migration target; the legacy path remains authoritative
//! until production wiring and Shadow validation are complete.

use std::fmt;

/// Engine identifier. Frozen at run creation; a run never silently switches engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EngineId {
    Pi,
    DeepSeekHarness,
    Codex,
}

impl EngineId {
    pub fn as_str(self) -> &'static str {
        match self {
            EngineId::Pi => "pi",
            EngineId::DeepSeekHarness => "deepseek_harness",
            EngineId::Codex => "codex",
        }
    }

    pub fn parse(value: &str) -> Option<EngineId> {
        match value {
            "pi" => Some(EngineId::Pi),
            "deepseek_harness" | "deepseek-harness" => Some(EngineId::DeepSeekHarness),
            "codex" => Some(EngineId::Codex),
            _ => None,
        }
    }
}

/// Kernel operating mode for a run. Frozen at run creation (see [`crate::ports`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KernelMode {
    /// Legacy path remains authoritative; Kernel records compatibility facts only.
    Legacy,
    /// Kernel computes decisions and records diffs, but the legacy chain executes.
    Shadow,
    /// Kernel decisions are authoritative.
    Authoritative,
}

impl KernelMode {
    pub fn as_str(self) -> &'static str {
        match self {
            KernelMode::Legacy => "legacy",
            KernelMode::Shadow => "shadow",
            KernelMode::Authoritative => "authoritative",
        }
    }

    pub fn parse(value: &str) -> Option<KernelMode> {
        match value {
            "legacy" => Some(KernelMode::Legacy),
            "shadow" => Some(KernelMode::Shadow),
            "authoritative" => Some(KernelMode::Authoritative),
            _ => None,
        }
    }
}

/// Lifecycle states of a Run. Terminal states are reached exactly once and never
/// transition to each other.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum RunState {
    Created,
    Running,
    /// Waiting on a human approval decision for one or more tool calls. The
    /// execution budget clock is suspended while in this state.
    WaitingApproval,
    /// A whole-turn retry has been scheduled (error recovery). Distinct from
    /// `Compacting`, which is context management.
    RetryScheduled,
    Compacting,
    /// Cancellation requested; in-flight model stream / hooks / host requests /
    /// tools are being torn down before the single `Cancelled` terminal is written.
    Cancelling,
    Completed,
    Failed,
    Cancelled,
    BudgetExhausted,
}

impl RunState {
    pub fn as_str(self) -> &'static str {
        match self {
            RunState::Created => "created",
            RunState::Running => "running",
            RunState::WaitingApproval => "waiting_approval",
            RunState::RetryScheduled => "retry_scheduled",
            RunState::Compacting => "compacting",
            RunState::Cancelling => "cancelling",
            RunState::Completed => "completed",
            RunState::Failed => "failed",
            RunState::Cancelled => "cancelled",
            RunState::BudgetExhausted => "budget_exhausted",
        }
    }

    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            RunState::Completed
                | RunState::Failed
                | RunState::Cancelled
                | RunState::BudgetExhausted
        )
    }

    pub fn parse(value: &str) -> Option<RunState> {
        Some(match value {
            "created" => RunState::Created,
            "running" => RunState::Running,
            "waiting_approval" => RunState::WaitingApproval,
            "retry_scheduled" => RunState::RetryScheduled,
            "compacting" => RunState::Compacting,
            "cancelling" => RunState::Cancelling,
            "completed" => RunState::Completed,
            "failed" => RunState::Failed,
            "cancelled" => RunState::Cancelled,
            "budget_exhausted" => RunState::BudgetExhausted,
            _ => return None,
        })
    }
}

/// Terminal outcome, written exactly once per run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunOutcome {
    Completed,
    Failed { code: String, message: String },
    Cancelled,
    BudgetExhausted { code: String, message: String },
}

impl RunOutcome {
    pub fn state(&self) -> RunState {
        match self {
            RunOutcome::Completed => RunState::Completed,
            RunOutcome::Failed { .. } => RunState::Failed,
            RunOutcome::Cancelled => RunState::Cancelled,
            RunOutcome::BudgetExhausted { .. } => RunState::BudgetExhausted,
        }
    }

    pub fn event_type(&self) -> &'static str {
        match self {
            RunOutcome::Completed => "run.completed",
            RunOutcome::Failed { .. } => "run.failed",
            RunOutcome::Cancelled => "run.cancelled",
            RunOutcome::BudgetExhausted { .. } => "run.budget_exhausted",
        }
    }
}

/// Tool call lifecycle states.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum ToolCallState {
    Pending,
    WaitingApproval,
    Running,
    Completed,
    Failed,
    Cancelled,
}

impl ToolCallState {
    pub fn as_str(self) -> &'static str {
        match self {
            ToolCallState::Pending => "pending",
            ToolCallState::WaitingApproval => "waiting_approval",
            ToolCallState::Running => "running",
            ToolCallState::Completed => "completed",
            ToolCallState::Failed => "failed",
            ToolCallState::Cancelled => "cancelled",
        }
    }

    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            ToolCallState::Completed | ToolCallState::Failed | ToolCallState::Cancelled
        )
    }

    pub fn parse(value: &str) -> Option<ToolCallState> {
        Some(match value {
            "pending" => ToolCallState::Pending,
            "waiting_approval" => ToolCallState::WaitingApproval,
            "running" => ToolCallState::Running,
            "completed" => ToolCallState::Completed,
            "failed" => ToolCallState::Failed,
            "cancelled" => ToolCallState::Cancelled,
            _ => return None,
        })
    }
}

/// A human decision on an approval request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ApprovalDecision {
    AllowOnce,
    AllowConversation,
    Deny,
}

impl ApprovalDecision {
    pub fn as_str(self) -> &'static str {
        match self {
            ApprovalDecision::AllowOnce => "allow_once",
            ApprovalDecision::AllowConversation => "allow_conversation",
            ApprovalDecision::Deny => "denied",
        }
    }
}

/// Error returned by Kernel state transitions. Unknown input always fails closed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KernelError {
    /// The run already reached a terminal state; the transition is rejected.
    Terminal { current: &'static str },
    /// The transition is not legal from the current state.
    IllegalTransition {
        from: &'static str,
        to: &'static str,
    },
    /// The referenced tool call is unknown or already settled.
    UnknownToolCall(String),
    /// The tool call is not waiting for an approval.
    NotWaitingApproval(String),
    /// Input failed closed (unknown mode / engine / policy / schema).
    FailClosed(String),
}

impl fmt::Display for KernelError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KernelError::Terminal { current } => {
                write!(formatter, "run is already terminal ({current})")
            }
            KernelError::IllegalTransition { from, to } => {
                write!(formatter, "illegal run transition {from} -> {to}")
            }
            KernelError::UnknownToolCall(id) => {
                write!(formatter, "unknown or settled tool call: {id}")
            }
            KernelError::NotWaitingApproval(id) => {
                write!(formatter, "tool call is not waiting for approval: {id}")
            }
            KernelError::FailClosed(reason) => write!(formatter, "kernel fail-closed: {reason}"),
        }
    }
}

impl std::error::Error for KernelError {}
