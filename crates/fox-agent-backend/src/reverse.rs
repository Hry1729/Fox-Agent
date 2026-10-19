//! Agent-initiated (reverse) requests: native approvals, MCP business tool
//! calls, extension UI. Plan section 6.3.

use crate::port::AgentBackend;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReverseKind {
    Approval,
    McpToolCall,
    ExtensionUi,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionState {
    Pending,
    UserDecided,
    /// The answer was written to the native transport. Without a native
    /// acknowledgement mechanism this is where the state stays.
    ResponseWritten,
    NativeAcknowledged,
    Expired,
    Cancelled,
    Superseded,
    /// Sent (or being sent) but delivery cannot be confirmed: disconnect during
    /// write, process exit, lost receipt. Never coerced into success/failure.
    OutcomeUnknown,
}

impl DecisionState {
    pub fn is_final(self) -> bool {
        matches!(
            self,
            Self::NativeAcknowledged
                | Self::Expired
                | Self::Cancelled
                | Self::Superseded
                | Self::OutcomeUnknown
        )
    }

    /// The card can still be answered.
    pub fn answerable(self) -> bool {
        self == Self::Pending
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReverseRequest {
    pub request_id: String,
    pub native_request_id: String,
    pub backend: AgentBackend,
    pub conversation_id: String,
    pub run_id: String,
    pub turn_id: String,
    pub generation: u64,
    pub kind: ReverseKind,
    pub action: String,
    pub resource: String,
    /// Only options the native side actually offers; Fox never invents
    /// "just this once" or "always" granularity.
    pub options: Vec<String>,
    pub expires_at_ms: Option<i64>,
    pub state: DecisionState,
    pub chosen_option: Option<String>,
    /// Late responses or acknowledgements that arrived after a final state;
    /// kept for reconciliation and audit, never applied.
    pub audit: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReverseError {
    /// The request already took its one answer; the caller gets this, not a
    /// second effect.
    AlreadyHandled(DecisionState),
    NotAnOffer(String),
    WrongGeneration { request: u64, current: u64 },
    IllegalTransition { from: DecisionState, to: DecisionState },
    /// The protocol has no acknowledgement; refusing to fabricate one.
    NoNativeAcknowledgement,
}

impl ReverseRequest {
    /// The user picks an option. Second and later answers are reported as
    /// already handled.
    pub fn decide(&mut self, option: &str, current_generation: u64) -> Result<(), ReverseError> {
        if !self.state.answerable() {
            return Err(ReverseError::AlreadyHandled(self.state));
        }
        if self.generation != current_generation {
            // A reconnect invalidates the card (6.3 rule 4).
            self.state = DecisionState::Superseded;
            return Err(ReverseError::WrongGeneration {
                request: self.generation,
                current: current_generation,
            });
        }
        if !self.options.iter().any(|candidate| candidate == option) {
            return Err(ReverseError::NotAnOffer(option.to_owned()));
        }
        self.state = DecisionState::UserDecided;
        self.chosen_option = Some(option.to_owned());
        Ok(())
    }

    /// Outcome of writing the answer to the native transport.
    pub fn record_write(&mut self, delivered: WriteOutcome) -> Result<(), ReverseError> {
        if self.state != DecisionState::UserDecided {
            return Err(ReverseError::IllegalTransition {
                from: self.state,
                to: DecisionState::ResponseWritten,
            });
        }
        self.state = match delivered {
            WriteOutcome::Written => DecisionState::ResponseWritten,
            // Disconnect mid-write or unconfirmable delivery (rule 3).
            WriteOutcome::Unconfirmed => DecisionState::OutcomeUnknown,
        };
        Ok(())
    }

    /// The native side confirmed. Refused when the backend has no
    /// acknowledgement mechanism, so the state machine can never be walked to
    /// "acknowledged" just to look complete.
    pub fn record_native_ack(&mut self, protocol_acknowledges: bool) -> Result<(), ReverseError> {
        if !protocol_acknowledges {
            return Err(ReverseError::NoNativeAcknowledgement);
        }
        match self.state {
            DecisionState::ResponseWritten => {
                self.state = DecisionState::NativeAcknowledged;
                Ok(())
            }
            other if other.is_final() => {
                self.audit.push(format!("late native acknowledgement after {other:?}"));
                Ok(())
            }
            other => Err(ReverseError::IllegalTransition {
                from: other,
                to: DecisionState::NativeAcknowledged,
            }),
        }
    }

    /// Connection lost, process exited, or turn cancelled while this request
    /// was open. Pending becomes cancelled (the user never answered, so
    /// nothing was sent); a written or in-flight answer becomes unknown.
    pub fn settle_on_interruption(&mut self, reason: Interruption) {
        self.state = match (self.state, reason) {
            (DecisionState::Pending, Interruption::Timeout) => DecisionState::Expired,
            (DecisionState::Pending, _) => DecisionState::Cancelled,
            (DecisionState::UserDecided, _) | (DecisionState::ResponseWritten, _) => {
                DecisionState::OutcomeUnknown
            }
            (final_state, _) => final_state,
        };
    }

    /// A response for this request arriving after it is final.
    pub fn record_late(&mut self, description: impl Into<String>) {
        self.audit.push(description.into());
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteOutcome {
    Written,
    Unconfirmed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Interruption {
    Cancel,
    Disconnect,
    ProcessExit,
    Timeout,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> ReverseRequest {
        ReverseRequest {
            request_id: "r1".into(),
            native_request_id: "n1".into(),
            backend: AgentBackend::Codex,
            conversation_id: "c".into(),
            run_id: "run".into(),
            turn_id: "t".into(),
            generation: 3,
            kind: ReverseKind::Approval,
            action: "shell".into(),
            resource: "npm test".into(),
            options: vec!["allow_once".into(), "reject_once".into()],
            expires_at_ms: None,
            state: DecisionState::Pending,
            chosen_option: None,
            audit: vec![],
        }
    }

    #[test]
    fn only_one_answer_and_only_offered_options() {
        let mut request = request();
        assert_eq!(request.decide("allow_always", 3), Err(ReverseError::NotAnOffer("allow_always".into())));
        request.decide("allow_once", 3).unwrap();
        assert_eq!(request.decide("reject_once", 3), Err(ReverseError::AlreadyHandled(DecisionState::UserDecided)));
    }

    #[test]
    fn reconnect_supersedes_the_card() {
        let mut request = request();
        assert!(matches!(request.decide("allow_once", 4), Err(ReverseError::WrongGeneration { .. })));
        assert_eq!(request.state, DecisionState::Superseded);
    }

    #[test]
    fn no_fabricated_native_acknowledgement() {
        let mut request = request();
        request.decide("allow_once", 3).unwrap();
        request.record_write(WriteOutcome::Written).unwrap();
        assert_eq!(request.record_native_ack(false), Err(ReverseError::NoNativeAcknowledgement));
        assert_eq!(request.state, DecisionState::ResponseWritten);
    }

    #[test]
    fn disconnect_during_write_is_outcome_unknown() {
        let mut request = request();
        request.decide("allow_once", 3).unwrap();
        request.record_write(WriteOutcome::Unconfirmed).unwrap();
        assert_eq!(request.state, DecisionState::OutcomeUnknown);
    }

    #[test]
    fn interruption_settles_by_how_far_the_answer_got() {
        let mut pending = request();
        pending.settle_on_interruption(Interruption::Cancel);
        assert_eq!(pending.state, DecisionState::Cancelled);

        let mut written = request();
        written.decide("allow_once", 3).unwrap();
        written.record_write(WriteOutcome::Written).unwrap();
        written.settle_on_interruption(Interruption::Disconnect);
        assert_eq!(written.state, DecisionState::OutcomeUnknown);

        let mut timed_out = request();
        timed_out.settle_on_interruption(Interruption::Timeout);
        assert_eq!(timed_out.state, DecisionState::Expired);
    }

    #[test]
    fn late_ack_is_audit_only() {
        let mut request = request();
        request.settle_on_interruption(Interruption::Cancel);
        request.record_native_ack(true).unwrap();
        assert_eq!(request.state, DecisionState::Cancelled);
        assert_eq!(request.audit.len(), 1);
    }
}
