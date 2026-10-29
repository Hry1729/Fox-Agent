//! Frozen payloads for the Pi-native tool batch.
//!
//! Pi proposes a whole batch in one assistant message; the Host owns ordering,
//! admission, approval re-verification and the durable ledger. These
//! requests are the contract between the sidecar bridge and
//! `runtime_host::native_tool_batch`, and they ride the existing
//! `RuntimeRequest` envelope as its `payload`.
//!
//! Two rules shape every field here:
//!  * the bridge waits for *registration* only, never for settlement — a hook
//!    that waited for a result would deadlock the SDK's batch preparation;
//!  * identity the Host must not trust the model for (batch id, generation,
//!    ownership) is either Host-assigned or cross-checked against the ledger.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Envelope `type` for registering one whole batch before any call executes.
pub const TOOL_BATCH_REGISTER: &str = "tool_batch.register";
/// Envelope `type` for claiming and running one registered call.
pub const TOOL_CALL_EXECUTE: &str = "tool_call.execute";
/// Envelope `type` for closing a position that never produced a result.
pub const TOOL_CALL_CLOSE: &str = "tool_call.close";
/// Envelope `type` for abandoning a call whose Run was aborted.
pub const TOOL_CALL_CANCEL: &str = "tool_call.cancel";
/// Envelope `type` for reconciling a native session's active branch.
pub const TOOL_CALL_FACTS: &str = "tool_call.facts";

/// One position in the batch, in the order the model emitted it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NativeToolCallRegistration {
    pub call_id: String,
    pub tool: String,
    /// Position in the assistant message — not Host processing order. Both the
    /// barrier and the order results are appended in derive from this.
    pub source_order: i64,
    /// Digest of the arguments exactly as the model emitted them, before SDK
    /// conversion. It exists to detect a same-key/different-arguments replay;
    /// approval binds to the *converted* input carried by `tool_call.execute`.
    pub input_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToolBatchRegisterRequest {
    pub run_id: String,
    /// Generation of the Pi session that produced this batch. A batch arriving
    /// from a retired generation is refused rather than merged, so a reset can
    /// never inherit the previous context's tool positions.
    pub context_generation: i64,
    pub calls: Vec<NativeToolCallRegistration>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToolBatchRegisterResponse {
    /// Host-assigned. The bridge never invents batch identity, and a repeated
    /// registration of the same batch must return the same id rather than
    /// opening a second ledger entry.
    pub batch_id: String,
    /// How many positions the Host now holds for this batch. Equal to the
    /// request's call count on a fresh registration; on an idempotent replay it
    /// confirms the stored batch instead of the submitted one.
    pub registered_calls: u32,
}

/// Why a position is being closed without a result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ToolCallCloseReason {
    /// The model named a tool the Host does not expose.
    UnknownTool,
    /// The arguments did not survive validation. The SDK converts before it
    /// validates, so the Host cannot predict this and must be told.
    ValidationFailed,
    /// A hook or the Host refused the call before it ran.
    Intercepted,
    /// The Run was aborted and the call returned early without executing.
    Aborted,
    /// Preparation stopped before this position and no event ever arrived for
    /// it; closed at `turn_end` so the barrier does not wait forever.
    NotStarted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToolCallExecuteRequest {
    pub run_id: String,
    pub batch_id: String,
    pub call_id: String,
    pub tool: String,
    /// The arguments after SDK conversion — the values the tool will actually
    /// receive. Approval is re-verified against these, not against the model's
    /// original text, so a conversion can never widen a granted scope.
    pub input: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToolCallCloseRequest {
    pub run_id: String,
    pub batch_id: String,
    pub call_id: String,
    pub reason: ToolCallCloseReason,
    /// Closing is never a success. This records whether the model should see it
    /// as an error result, which every reason currently does.
    pub is_error: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToolCallCloseResponse {
    pub closed: bool,
    /// The ledger state the position ended in, so the bridge can tell a close
    /// that landed from one that found the position already settled.
    pub state: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToolCallCancelRequest {
    pub run_id: String,
    pub batch_id: String,
    pub call_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToolCallCancelResponse {
    pub cancelled: bool,
    /// Whether the Host kept a partial result as evidence. A cancelled call
    /// always fails towards the model; this only says the side effects it
    /// already caused are still on the record, and that an unknown-outcome
    /// write must never be retried under a new call id.
    pub result_recorded: bool,
    /// Set when the Host could not establish whether the operation took
    /// effect. Such a call is not automatically redone.
    pub outcome_unknown: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NativeToolCallFactQuery {
    pub call_id: String,
    pub tool: String,
    pub input_digest: String,
    /// The active Pi branch already contains a result for this call. A result
    /// without a committed Host receipt is a consistency error.
    pub has_result: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToolCallFactsRequest {
    /// The trusted querying Run, which can be a finished Run while opening.
    /// Facts may belong to earlier Runs in the same native session generation.
    pub run_id: String,
    pub context_generation: i64,
    pub calls: Vec<NativeToolCallFactQuery>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NativeToolCallFactState {
    Committed,
    NotExecuted,
    OutcomeUnknown,
    WaitingApproval,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NativeToolCallFact {
    pub call_id: String,
    pub state: NativeToolCallFactState,
    /// The exact execute response payload committed by the Host, or its
    /// durable nonexecution receipt. Blocked facts carry no appendable result.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToolCallFactsResponse {
    pub context_generation: i64,
    pub blocked: bool,
    /// Same order as the query. Every call must be checked before Pi appends
    /// any receipt, so an inconsistent branch cannot be partially repaired.
    pub facts: Vec<NativeToolCallFact>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registration() -> ToolBatchRegisterRequest {
        ToolBatchRegisterRequest {
            run_id: "run-1".into(),
            context_generation: 3,
            calls: vec![
                NativeToolCallRegistration {
                    call_id: "call_a".into(), tool: "read".into(),
                    source_order: 0, input_digest: "sha256:aa".into(),
                },
                NativeToolCallRegistration {
                    call_id: "call_b".into(), tool: "write_file".into(),
                    source_order: 1, input_digest: "sha256:bb".into(),
                },
            ],
        }
    }

    #[test]
    fn the_request_types_are_the_frozen_envelope_names() {
        assert_eq!(TOOL_BATCH_REGISTER, "tool_batch.register");
        assert_eq!(TOOL_CALL_EXECUTE, "tool_call.execute");
        assert_eq!(TOOL_CALL_CLOSE, "tool_call.close");
        assert_eq!(TOOL_CALL_CANCEL, "tool_call.cancel");
        assert_eq!(TOOL_CALL_FACTS, "tool_call.facts");
    }

    #[test]
    fn registration_carries_generation_and_source_order_on_the_wire() {
        let json = serde_json::to_value(registration()).unwrap();
        assert_eq!(json["contextGeneration"], 3);
        assert_eq!(json["calls"][1]["sourceOrder"], 1);
        assert_eq!(json["calls"][1]["inputDigest"], "sha256:bb");
        // The batch id is Host-assigned, so the request must not carry one.
        assert!(json.as_object().unwrap().get("batchId").is_none());
        let decoded: ToolBatchRegisterRequest = serde_json::from_value(json).unwrap();
        assert_eq!(decoded, registration());
    }

    #[test]
    fn close_reasons_serialize_as_stable_snake_case_codes() {
        for (reason, code) in [
            (ToolCallCloseReason::UnknownTool, "unknown_tool"),
            (ToolCallCloseReason::ValidationFailed, "validation_failed"),
            (ToolCallCloseReason::Intercepted, "intercepted"),
            (ToolCallCloseReason::Aborted, "aborted"),
            (ToolCallCloseReason::NotStarted, "not_started"),
        ] {
            assert_eq!(serde_json::to_value(reason).unwrap(), code);
            assert_eq!(serde_json::from_value::<ToolCallCloseReason>(serde_json::json!(code)).unwrap(), reason);
        }
        assert!(serde_json::from_value::<ToolCallCloseReason>(serde_json::json!("succeeded")).is_err());
    }

    #[test]
    fn unknown_fields_and_tampered_identity_are_refused() {
        let mut execute = serde_json::json!({
            "runId": "run-1", "batchId": "batch-1", "callId": "call_a",
            "tool": "read", "input": { "path": "A.txt" },
        });
        assert!(serde_json::from_value::<ToolCallExecuteRequest>(execute.clone()).is_ok());
        execute["approved"] = serde_json::json!(true);
        assert!(serde_json::from_value::<ToolCallExecuteRequest>(execute).is_err(),
            "a bridge may not smuggle its own approval flag past the Host");

        let mut cancel = serde_json::json!({ "runId": "run-1", "batchId": "batch-1", "callId": "call_a" });
        assert!(serde_json::from_value::<ToolCallCancelRequest>(cancel.clone()).is_ok());
        cancel["callId"] = serde_json::json!(42);
        assert!(serde_json::from_value::<ToolCallCancelRequest>(cancel).is_err());
    }

    #[test]
    fn facts_cover_existing_pi_results_and_do_not_make_blocked_calls_appendable() {
        let request = serde_json::json!({"runId":"querying-run","contextGeneration":3,
            "calls":[{"callId":"old-call","tool":"read","inputDigest":"sha256:aa","hasResult":true}]});
        let decoded: ToolCallFactsRequest = serde_json::from_value(request.clone()).unwrap();
        assert!(decoded.calls[0].has_result);
        let mut extra = request;
        extra["calls"][0]["approved"] = serde_json::json!(true);
        assert!(serde_json::from_value::<ToolCallFactsRequest>(extra).is_err());
        for state in [NativeToolCallFactState::OutcomeUnknown, NativeToolCallFactState::WaitingApproval] {
            let fact = serde_json::to_value(NativeToolCallFact { call_id:"old-call".into(), state, response:None }).unwrap();
            assert!(!fact.as_object().unwrap().contains_key("response"));
        }
    }
}
