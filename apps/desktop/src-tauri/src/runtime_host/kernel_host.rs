//! One owning loop per durable Run. Model workers and resource executors finish
//! (including cancellation cleanup) before the loop can publish a terminal.
use super::{
    kernel_coordinator::KernelCoordinator, kernel_run_lock::KernelRunLock, RuntimeCommand,
};
use crate::{
    database::Database,
    kernel::{
        self, CancellationPort, CancellationRegistry, Clock, OutboxEffectKind, OutboxStatus,
        PolicyDecisionPort,
    },
};
use fox_engine_protocol::RunControlBinding;
use serde_json::Value;
use std::{path::Path, time::Duration};

fn terminal(state: &str) -> bool {
    matches!(
        state,
        "completed" | "failed" | "cancelled" | "budget_exhausted" | "approval_expired"
    )
}
/// Outcome of the B1a admission at the real execution boundary.
enum DispatchAdmission {
    /// The claim winner may run the existing executor chain.
    Proceed(ClaimedExecution),
    /// A repeat delivery, terminal refusal or unknown outcome: answer from
    /// durable facts without executing anything.
    Repeat(Value),
}

/// Linear Host capability: constructed only after winning the durable claim.
pub(super) struct ClaimedExecution {
    credential: fox_engine_protocol::ExecutionCredential,
}
impl ClaimedExecution {
    pub(super) fn credential(&self) -> &fox_engine_protocol::ExecutionCredential {
        &self.credential
    }
}

/// What the executor reports about the target operation it just performed.
/// This is the Host-internal trusted evidence channel: it comes from the code
/// that actually performed (or refused) the operation and is NEVER inferred
/// from the shape of a tool result (`Ok`/`isError`).
#[cfg(test)]
type ExecutionOutcome = (Result<Value, String>, fox_engine_protocol::ExecutionEvidence);

/// Executors without a trusted effect channel cannot infer a start from JSON.
/// Read/Manage complete without starting an external operation; effectful calls
/// stay unknown until their concrete executor supplies evidence.
fn conservative_evidence(
    class: fox_engine_protocol::ActionClass,
    result: &Result<Value, String>,
) -> fox_engine_protocol::ExecutionEvidence {
    use fox_engine_protocol::{ActionClass, ExecutionEvidence};
    let _ = result;
    match class {
        ActionClass::Read | ActionClass::Manage => ExecutionEvidence::NotStarted,
        _ => ExecutionEvidence::Unknown,
    }
}

/// The real Host order for one dispatch execution, extracted so it is
/// testable without a Tauri app handle: admit (verify → claim) → execute the
/// verified durable input → settle from trusted execution evidence. The
/// executor only ever receives the durable canonical input.
///
/// The executor returns a pair `(result, evidence)`. A Rust `Ok` is not
/// evidence: Fox tools return `Ok(json)` with `isError=true` for business
/// failures, a read/query completing is not the start of an external process,
/// and a failure after a real start must stay uncertain. The Host therefore
/// settles ONLY from the executor's evidence value.
pub(super) fn execute_claimed_dispatch(
    database: &Database,
    binding: &RunControlBinding,
    effect: &kernel::OutboxEffect,
    tool: &str,
    payload: &Value,
    execute: impl FnOnce(&str, ClaimedExecution) -> (Result<Value, String>, fox_engine_protocol::ExecutionEvidence, fox_engine_protocol::CallOutcome),
) -> Result<Value, String> {
    let host = HostAdmission { database, binding };
    match host.admit(effect, tool, payload)? {
        DispatchAdmission::Repeat(receipt) => Ok(receipt),
        DispatchAdmission::Proceed(claim) => {
            let tool_call_id = effect
                .tool_call_id
                .as_deref()
                .ok_or("missing dispatch tool identity")?;
            let durable_input = database
                .tool_call_identity(&binding.run_id, tool_call_id)?
                .ok_or("dispatch has no durable tool call identity")?
                .1;
            let (result, evidence, outcome) = execute(&durable_input, claim);
            let dispatch_id =
                fox_engine_protocol::encode_dispatch_id(&binding.run_id, tool_call_id)?;
            settle_execution_outcome(database, &binding.run_id, &dispatch_id, &result, evidence, &outcome)?;
            result.and_then(|value| with_execution_receipt(database, &binding.run_id, &dispatch_id, value))
        }
    }
}

pub(super) fn with_execution_receipt(database: &Database, run_id: &str, dispatch_id: &str, mut value: Value) -> Result<Value, String> {
    if let Some(receipt) = database.execution_receipt(run_id, dispatch_id)? {
        if !value["details"].is_object() { value["details"] = serde_json::json!({}); }
        value["details"]["executionReceipt"] = serde_json::to_value(receipt).map_err(|e| e.to_string())?;
    }
    Ok(value)
}

pub(super) fn settle_execution_outcome(
    database: &Database, run_id: &str, dispatch_id: &str, result: &Result<Value, String>,
    evidence: fox_engine_protocol::ExecutionEvidence, outcome: &fox_engine_protocol::CallOutcome,
) -> Result<(), String> {
    use fox_engine_protocol::{CallOutcome, ExecutionEvidence};
    if evidence == ExecutionEvidence::Started {
        database.record_attempt_started(run_id, dispatch_id)?;
    }
    match (&evidence, &outcome) {
        (ExecutionEvidence::Unknown, _) => database.record_attempt_failure(
            run_id, dispatch_id, &refusal_code_from_result(result))?,
        (_, CallOutcome::Completed) => database.record_attempt_completed(
            run_id, dispatch_id, evidence == ExecutionEvidence::Started)?,
        (ExecutionEvidence::Started, CallOutcome::Failed { code } | CallOutcome::Refused { code }) =>
            database.record_attempt_failure(run_id, dispatch_id, code)?,
        (_, CallOutcome::Failed { code } | CallOutcome::Refused { code }) =>
            database.record_attempt_refusal(run_id, dispatch_id, code)?,
    }
    Ok(())
}

pub(super) fn call_outcome(result: &Result<Value, String>) -> fox_engine_protocol::CallOutcome {
    if matches!(result, Ok(value) if value.get("isError").and_then(Value::as_bool) != Some(true)) {
        fox_engine_protocol::CallOutcome::Completed
    } else {
        fox_engine_protocol::CallOutcome::Failed { code: refusal_code_from_result(result) }
    }
}

#[cfg(test)]
fn admit_and_execute_dispatch(
    database: &Database, binding: &RunControlBinding, effect: &kernel::OutboxEffect,
    tool: &str, payload: &Value, execute: impl FnOnce(&str) -> ExecutionOutcome,
) -> Result<Value, String> {
    execute_claimed_dispatch(database, binding, effect, tool, payload, |input, _claim| {
        let (result, evidence) = execute(input);
        let outcome = call_outcome(&result);
        (result, evidence, outcome)
    })
}

/// The refusal code the executor itself reported. Prefers the tool's own
/// structured code; never invents one.
fn refusal_code_from_result(result: &Result<Value, String>) -> String {
    match result {
        Err(error) => return execution_failure_code(error),
        Ok(value) => {
            if let Some(code) = value
                .get("details")
                .and_then(|details| details.get("errorCode"))
                .or_else(|| value.get("details").and_then(|details| details.get("error")).and_then(|error| error.get("code")))
                .and_then(serde_json::Value::as_str)
            {
                return code.to_owned();
            }
            if value.get("isError").and_then(serde_json::Value::as_bool) == Some(true) {
                return "tool.error".to_owned();
            }
        }
    }
    "kernel.not_started".to_owned()
}

/// Classify a Host error string into a stable code without inventing facts.
fn execution_failure_code(error: &str) -> String {
    if error.contains("cancelled") {
        return "tool.cancelled".to_owned();
    }
    if error.contains("permission") || error.contains("denied") {
        return "tool.permission_denied".to_owned();
    }
    "kernel.execution_failed".to_owned()
}

/// Admission half of the Host order (verify → claim). Kept separate so the
/// executor chain can stay in `RuntimeHost` where the gateway state lives.
struct HostAdmission<'a> {
    database: &'a Database,
    binding: &'a RunControlBinding,
}

impl HostAdmission<'_> {
    fn admit(
        &self,
        effect: &kernel::OutboxEffect,
        tool: &str,
        payload: &Value,
    ) -> Result<DispatchAdmission, String> {
        let tool_call_id = effect
            .tool_call_id
            .as_deref()
            .ok_or("missing dispatch tool identity")?;
        // The presented credential is built from durable facts only: the
        // dispatch payload can never contribute an identity field.
        let (durable_tool, durable_input) = self
            .database
            .tool_call_identity(&self.binding.run_id, tool_call_id)?
            .ok_or("dispatch has no durable tool call identity")?;
        if durable_tool != tool {
            return Err("dispatch payload tool differs from the durable identity".into());
        }
        // I4: the payload that will be executed must BE the verified durable
        // input. A strict canonical comparison happens before any claim, so a
        // substituted path/content never reaches an executor.
        let payload_input = payload
            .get("input")
            .map(|input| input.to_string())
            .unwrap_or_else(|| "null".to_owned());
        if payload_input != durable_input {
            return Err(
                "credential_mismatch: dispatch payload input differs from the durable identity"
                    .into(),
            );
        }
        let dispatch_id =
            fox_engine_protocol::encode_dispatch_id(&self.binding.run_id, tool_call_id)?;
        let presented = self.database.read_execution_credential(&self.binding.run_id, &dispatch_id)?
            .ok_or("credential_mismatch: no issued execution credential")?;
        if presented.intent_digest != crate::database::kernel_execution_admission::launch_params_hash(&durable_tool, &durable_input)
            || presented.conversation_id != self.binding.conversation_id
            || presented.resolved_profile != self.binding.execution_profile_id
            || presented.policy_snapshot_id != self.binding.permission_snapshot_id {
            return Err("credential_mismatch: durable run identity differs".into());
        }
        // Credential verification happens BEFORE the claim (read-only).
        self.database
            .verify_execution_credential(&self.binding.run_id, &presented)?;
        // Persistent single claim across connections. The claim transaction
        // also evaluates the real-time requirements: an unsatisfied one (no
        // verified backend, no Host observation, no live policy version, no
        // readable parent revocation) is a persistent terminal refusal.
        let outcome = self.database.claim_execution_attempt(
            &self.binding.run_id,
            &self.binding.conversation_id,
            &presented,
            &presented.intent_digest,
            &format!("kernel-host:{tool_call_id}"),
        )?;
        match outcome {
            fox_engine_protocol::AttemptOutcome::Claimed => Ok(DispatchAdmission::Proceed(ClaimedExecution { credential: presented })),
            fox_engine_protocol::AttemptOutcome::AlreadyRefused { code } => Ok(
                DispatchAdmission::Repeat(execution_receipt_result(
                    self.database,
                    &self.binding.run_id,
                    &dispatch_id,
                    tool,
                    Some(code),
                )?),
            ),
            fox_engine_protocol::AttemptOutcome::AlreadyLaunched | fox_engine_protocol::AttemptOutcome::AlreadyCompleted => Ok(
                DispatchAdmission::Repeat(execution_receipt_result(
                    self.database,
                    &self.binding.run_id,
                    &dispatch_id,
                    tool,
                    None,
                )?),
            ),
            fox_engine_protocol::AttemptOutcome::Unknown => Ok(
                DispatchAdmission::Repeat(execution_receipt_result(
                    self.database,
                    &self.binding.run_id,
                    &dispatch_id,
                    tool,
                    None,
                )?),
            ),
        }
    }
}

/// A tool result carrying the ACTUAL durable receipt for a repeat delivery,
/// terminal refusal or unknown outcome. Nothing is hard-coded: the stage, kind,
/// start evidence and original refusal code come from the durable attempt row.
pub(super) fn execution_receipt_result(
    database: &crate::database::Database,
    run_id: &str,
    dispatch_id: &str,
    tool: &str,
    fallback_code: Option<String>,
) -> Result<Value, String> {
    let receipt = database.execution_receipt(run_id, dispatch_id)?;
    let Some(receipt) = receipt else {
        // No durable attempt fact at all: nothing is claimed about execution.
        return Ok(execution_refusal_result(
            tool,
            &fallback_code.unwrap_or_else(|| "kernel.uncertain_execution".to_owned()),
        ));
    };
    let existing = database.read_execution_attempt(run_id, dispatch_id)?
        .is_some_and(|row| matches!(row.state, fox_engine_protocol::AttemptState::Completed | fox_engine_protocol::AttemptState::Launched));
    let code = receipt
        .reason_code
        .clone()
        .or(receipt.code_alias.clone())
        .unwrap_or_else(|| if existing { "kernel.existing_execution" } else { "kernel.uncertain_execution" }.to_owned());
    let details = serde_json::json!({
        "source": "fox_kernel_host",
        "error": { "code": code },
        "executionStarted": match receipt.execution_started {
            fox_engine_protocol::TriState::True => serde_json::json!(true),
            fox_engine_protocol::TriState::False => serde_json::json!(false),
            fox_engine_protocol::TriState::Unknown => serde_json::json!("unknown"),
        },
        "sideEffectState": match receipt.external_effect {
            fox_engine_protocol::SideEffectState::None => "none",
            fox_engine_protocol::SideEffectState::Prepared => "prepared",
            fox_engine_protocol::SideEffectState::Applying => "applying",
            fox_engine_protocol::SideEffectState::Committed => "committed",
            fox_engine_protocol::SideEffectState::NotApplied => "not_applied",
            fox_engine_protocol::SideEffectState::Uncertain => "uncertain",
            fox_engine_protocol::SideEffectState::Unknown => "unknown",
        },
        "stage": receipt.stage,
        "kind": receipt.kind,
        "controlPlane": receipt.control_plane,
        "dispatchId": receipt.dispatch_id,
        "codeAlias": receipt.code_alias,
        "allowReplay": receipt.allow_replay,
        "recovery": "This dispatch attempt is finished; a new logical attempt needs a new tool call and full re-admission."
    });
    with_execution_receipt(database, run_id, dispatch_id, serde_json::json!({
        "isError": !existing,
        "content": [{"type": "text", "text": format!("[tool.{code}] {tool} was not re-executed")}],
        "details": details,
    }))
}

/// Missing durable evidence must never be projected as a confirmed refusal.
fn execution_refusal_result(tool: &str, code: &str) -> Value {
    let details = serde_json::json!({
        "source": "fox_kernel_host",
        "error": { "code": code },
        "executionStarted": "unknown",
        "sideEffectState": "unknown",
        "controlPlane": "unknown",
        "allowReplay": false,
        "recovery": "This dispatch attempt is finished; a new logical attempt needs a new tool call and full re-admission."
    });
    serde_json::json!({
        "isError": true,
        "content": [{"type": "text", "text": format!("[tool.{code}] {tool} has no durable execution receipt; execution is unknown and must not be replayed")}],
        "details": details,
    })
}

/// Record the scope a conversation-level approval actually granted (#5).
///
/// `allow_once` is deliberately excluded: a one-shot decision must never become
/// a reusable permission. The scope is recomputed from the call's own input with
/// the same helper the policy uses, and it is only registered when the current
/// frozen policy would still have *asked* for these exact parameters - so the
/// approval can never widen anything (frozen read-only, an out-of-catalog tool,
/// a lifecycle hook and a denied scope all refuse).
fn register_approval_grant(
    database: &Database,
    run_id: &str,
    tool_call_id: &str,
    decision: kernel::ApprovalDecision,
) -> Result<(), String> {
    if decision != kernel::ApprovalDecision::AllowConversation {
        // A one-shot approval is auditable as a decision but must never be
        // registered as a durable permission.
        return database
            .kernel_register_authorization_grant(
                run_id,
                tool_call_id,
                decision.as_str(),
                "",
                None,
            )
            .map(|_| ());
    }
    // The approval row itself is the durable record of what was asked. Nothing
    // is registered unless a decision for exactly this call exists.
    let Some((tool, input_json)) = database.kernel_decided_approval_call(run_id, tool_call_id)? else {
        return Ok(());
    };
    let binding = database.run_control_binding(run_id)?;
    let Some(binding) = binding else {
        return Ok(());
    };
    // Only a call the frozen policy still asks about can be covered by a grant.
    // Everything else (frozen read-only, an out-of-catalog tool, a lifecycle
    // hook, an already-granted scope) refuses without registering anything.
    // The same live re-check the decision path uses: a reusable approval the
    // user has since withdrawn must not make the frozen policy look "already
    // granted", because that is what would let a new grant be registered
    // without a fresh human decision (REV-04).
    let live_grants: Vec<serde_json::Value> = binding
        .permission
        .grants
        .iter()
        .filter(|grant| match grant.kind {
            fox_engine_protocol::GrantKind::Resource => true,
            fox_engine_protocol::GrantKind::ApprovalReuse => database
                .conversation_tool_permission_granted(
                    &binding.conversation_id,
                    &grant.tool,
                    &grant.scope,
                )
                .unwrap_or(false),
        })
        .map(|grant| serde_json::json!([grant.tool, grant.scope]))
        .collect();
    if super::shadow_reconcile::frozen_kernel_tool_policy(
        &serde_json::json!({
            "mode": binding.permission.mode.as_str(),
            "projectRoot": binding.permission.project_root,
            "grants": live_grants,
        }),
        &tool,
        &input_json,
    ) != kernel::PolicyDecision::RequireApproval
    {
        return Ok(());
    }
    let Ok(input) = serde_json::from_str::<Value>(&input_json) else {
        return Ok(());
    };
    let scope = super::shadow_reconcile::tool_operation_scope(
        &tool,
        &input,
        binding.permission.project_root.as_deref(),
    );
    database
        .kernel_register_authorization_grant(
            run_id,
            tool_call_id,
            decision.as_str(),
            &tool,
            scope.as_deref(),
        )
        .map(|_| ())
}

/// Consume policy changes and queued approval decisions at either Host model
/// boundary. A cancel remains owned by the caller, but is returned before any
/// expired intent can be reopened or any tool dispatched. The caller decides
/// whether to settle cancellation (outer loop) or detach its live worker.
pub(super) fn consume_approval_step(
    coordinator: &KernelCoordinator<'_>,
    database: &Database,
    run_id: &str,
    policy: &dyn PolicyDecisionPort,
) -> Result<Option<i64>, String> {
    let commands = database.pending_kernel_host_commands(run_id)?;
    let snapshot = coordinator.snapshot()?;
    if terminal(&snapshot.state) {
        for command in commands {
            database.complete_kernel_host_command(run_id, command.seq)?;
        }
        return Ok(None);
    }
    if snapshot.state == "cancelling" {
        return Ok(None);
    }
    if let Some(command) = commands.iter().find(|command| command.kind == "cancel") {
        return Ok(Some(command.seq));
    }
    for call in &snapshot.tool_calls {
            if call.approval_state.as_deref() == Some("expired")
                && matches!(call.state.as_str(), "pending" | "waiting_approval")
            {
                match coordinator.reevaluate_tool(&call.tool_call_id, policy) {
                    Ok(()) => {},
                    Err(error) if error.contains("policy_version") || error.contains("stale_approval") => continue,
                    Err(error) => return Err(error),
                }
            }
    }
    for command in commands {
        coordinator.tick()?;
        let current = coordinator.snapshot()?;
        if !terminal(&current.state) && current.state != "cancelling" {
            let tool_id = command
                .tool_call_id
                .as_deref()
                .ok_or("missing queued approval tool")?;
            let decision = match command.decision.as_deref() {
                Some("allow_once") => kernel::ApprovalDecision::AllowOnce,
                Some("allow_conversation") => kernel::ApprovalDecision::AllowConversation,
                Some("denied") => kernel::ApprovalDecision::Deny,
                _ => return Err("invalid durable approval decision".into()),
            };
            let version = command.policy_version.ok_or("approval has no frozen policy version")?;
            let pending = current.tool_calls.iter().any(|tool| {
                tool.tool_call_id == tool_id
                    && tool.approval_state.as_deref() == Some("pending")
            });
            if pending {
                if let Err(error) = coordinator.resolve_approval_at_version(tool_id, decision, version) {
                    if error.contains("stale_approval") || error.contains("policy_version") {
                        database.complete_kernel_host_command(run_id, command.seq)?;
                        continue;
                    }
                    return Err(error);
                }
            }
            // A process can stop after resolving the approval but before grant
            // registration or command ack. Retry only the matching current
            // decision; the database registration checks its version atomically.
            let matching_decision = pending || current.tool_calls.iter().any(|tool| {
                tool.tool_call_id == tool_id
                    && tool.approval_state.as_deref() == Some(decision.as_str())
            });
            let conversation_id = database.run_control_binding(run_id)?
                .ok_or("missing frozen run binding")?
                .conversation_id;
            if matching_decision
                && database.execution_policy(&conversation_id)?.version == version
            {
                register_approval_grant(database, run_id, tool_id, decision)?;
            }
        }
        database.complete_kernel_host_command(run_id, command.seq)?;
    }
    Ok(None)
}

pub(crate) fn resource_failure_result(tool: &str, error: &str) -> Value {
    // Reader errors describe only the authorized path operation (missing path,
    // nonexistent file, OS access failure, scope escape). Keep them actionable.
    //
    // A02 (EX-v1): the three local executors are in the same category. Their
    // text is produced by this Host from the user's own project and child
    // process, never fetched from a remote body, so a command that failed with
    // a compiler error, a failing assertion or a denied path must reach the
    // model as a reason it can act on — a generic sentence makes the failure
    // unfixable and invites the model to repeat the identical call. It is still
    // bounded and credential-redacted before it travels.
    //
    // Other executors may return remote bodies or credentials; do not forward.
    let (label, limit) = if crate::resource_gateway::is_reader(tool) || tool == "attachment_compute"
    {
        ("Project file operation failed: ", 600usize)
    } else if matches!(tool, "run_command" | "write_file" | "edit_file") {
        // "ran and failed", "was stopped" and "never started" are different facts
        // for the model: the first has output to read, the last has a
        // precondition to fix. An unclassified text still says the operation did
        // not succeed — which is known — without claiming why.
        let label = match crate::tool_host::ToolErrorCode::classify(error) {
            Some(code) if code.is_completed_failure() => "Local operation failed: ",
            Some(crate::tool_host::ToolErrorCode::Cancelled) => "Local operation was cancelled: ",
            Some(crate::tool_host::ToolErrorCode::StartFailed)
            | Some(crate::tool_host::ToolErrorCode::InvalidInput)
            | Some(crate::tool_host::ToolErrorCode::Conflict)
            | Some(crate::tool_host::ToolErrorCode::PermissionDenied) => {
                "Local operation did not run: "
            }
            _ => "Local operation failed: ",
        };
        (label, 4_000usize)
    } else {
        return serde_json::json!({"content":[{"type":"text","text":"The resource request failed. No successful result is available."}],
            "details":{"code":"kernel.resource_failed","tool":tool}});
    };
    let message = format!(
        "{label}{}",
        super::redact_execution_diagnostic(error, limit)
    );
    let mut details = serde_json::json!({"code":"kernel.resource_failed","tool":tool});
    if let Some(code) = crate::tool_host::ToolErrorCode::classify(error) {
        details["errorCode"] = serde_json::Value::String(code.as_str().to_owned());
    }
    serde_json::json!({"content":[{"type":"text","text":message}],"details":details})
}

#[cfg(test)]
mod failure_tests {
    #[test]
    fn kernel_reader_failure_preserves_reason_without_claiming_policy_denial() {
        let result = super::resource_failure_result("ls", "tool path cannot be resolved: file not found");
        assert!(result["content"][0]["text"].as_str().unwrap().contains("file not found"));
        assert!(!result.to_string().contains("frozen policy"));
        let remote = super::resource_failure_result("http_request", "response includes private token");
        assert!(!remote.to_string().contains("private token"));
    }

    // A02 (EX-v1): a failed local execution must reach the model as a reason it
    // can act on, while remote executors keep their body to themselves.
    #[test]
    fn a_failed_command_keeps_its_compiler_reason_and_classification() {
        let result = super::resource_failure_result(
            "run_command",
            "[tool.nonzero_exit] src\\main.rs:12:5: error[E0308]: mismatched types",
        );
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("error[E0308]: mismatched types"), "{text}");
        assert!(!text.contains("No successful result is available"), "{text}");
        assert_eq!(result["details"]["errorCode"], "tool.nonzero_exit");
        assert_eq!(result["details"]["code"], "kernel.resource_failed");
    }

    #[test]
    fn a_refused_call_says_it_never_ran() {
        // A refusal produces no output at all. Saying only "failed" would send
        // the model looking for output to read instead of fixing the precondition.
        let result = super::resource_failure_result(
            "write_file",
            "[tool.permission_denied] 'notes/../secrets.env' escapes the authorized project root",
        );
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.starts_with("Local operation did not run: "), "{text}");
        assert!(text.contains("escapes the authorized project root"), "{text}");
        assert_eq!(result["details"]["errorCode"], "tool.permission_denied");
    }

    #[test]
    fn a_cancelled_edit_reports_the_denial_reason_rather_than_a_generic_failure() {
        let result = super::resource_failure_result(
            "edit_file",
            "[tool.cancelled] tool.cancelled: the Run was cancelled",
        );
        assert!(result["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("tool.cancelled"));
        assert_eq!(result["details"]["errorCode"], "tool.cancelled");
    }

    #[test]
    fn an_unclassified_failure_stays_generic_for_remote_tools_and_loses_nothing_locally() {
        // MCP/knowledge/web bodies are not this Host's to forward.
        let mcp = super::resource_failure_result("call_mcp_tool", "[tool.unknown] secret=abcd");
        assert!(mcp["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("No successful result is available"));
        assert!(!mcp.to_string().contains("secret=abcd"));
        // No tag, no invented classification.
        let local = super::resource_failure_result("run_command", "the child died oddly");
        assert!(local["details"].get("errorCode").is_none());
        assert!(local["content"][0]["text"].as_str().unwrap().contains("the child died oddly"));
    }

    #[test]
    fn diagnostics_are_bounded_and_credentials_never_travel() {
        let noisy = format!(
            "Authorization: Bearer super-secret-value\n{}",
            (0..12)
                .map(|_| "x".repeat(900))
                .collect::<Vec<_>>()
                .join("\n")
        );
        let result = super::resource_failure_result("run_command", &noisy);
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(!text.contains("super-secret-value"), "credentials must not travel");
        assert!(text.contains("[REDACTED]"), "redaction must be visible: {text}");
        assert!(text.chars().count() < 4_600, "{} chars is over the bound", text.chars().count());
        assert!(
            text.contains("截断"),
            "a bounded diagnostic has to say it was bounded, not look complete: {text}"
        );
    }
}

/// The lock is acquired before preparation by startup, and before recovery by
/// restart. Passing it here keeps ownership until every executor has returned.
#[cfg(test)]
pub(super) fn drive(
    ownership: KernelRunLock,
    database: &Database,
    clock: &dyn Clock,
    cancellation: &CancellationRegistry,
    run_id: &str,
    runtime: &RuntimeCommand,
    api_key: &str,
    policy: &dyn PolicyDecisionPort,
    // Sync lets the live batch loop run Host-classified independent read-only
    // calls concurrently; classification and leases stay Host-side.
    execute: impl Fn(
            &RunControlBinding,
            &kernel::OutboxEffect,
            &kernel::CancellationToken,
        ) -> Result<(bool, Value), String>
        + Sync,
) -> Result<(), String> {
    drive_with_actions(
        &ownership,
        database,
        clock,
        cancellation,
        run_id,
        runtime,
        api_key,
        policy,
        execute,
        |_| Ok(()),
        |_| Ok(()),
        &|_| {},
    )
}

pub(super) fn drive_with_actions(
    _ownership: &KernelRunLock,
    database: &Database,
    clock: &dyn Clock,
    cancellation: &CancellationRegistry,
    run_id: &str,
    runtime: &RuntimeCommand,
    api_key: &str,
    policy: &dyn PolicyDecisionPort,
    // Sync: the live loop may fan out Host-classified independent read-only
    // tool calls onto bounded worker threads.
    execute: impl Fn(
            &RunControlBinding,
            &kernel::OutboxEffect,
            &kernel::CancellationToken,
        ) -> Result<(bool, Value), String>
        + Sync,
    after_commit: impl Fn(&str) -> Result<(), String>,
    settle_children: impl Fn(bool) -> Result<(), String>,
    preview: &super::kernel_model_worker::PreviewSink,
) -> Result<(), String> {
    drive_with_actions_transport(_ownership, database, clock, cancellation, run_id, runtime,
        api_key, policy, execute, after_commit, settle_children, preview, false).map(|_| ())
}

/// Test-only transport selection exercises the owning Host's complete
/// per-round drive with the real Pi Node worker. Production always selects
/// the normal live-first behavior through `drive_with_actions` above.
#[cfg(test)]
pub(super) fn drive_with_actions_per_round(
    ownership: &KernelRunLock,
    database: &Database,
    clock: &dyn Clock,
    cancellation: &CancellationRegistry,
    run_id: &str,
    runtime: &RuntimeCommand,
    api_key: &str,
    policy: &dyn PolicyDecisionPort,
    execute: impl Fn(&RunControlBinding, &kernel::OutboxEffect, &kernel::CancellationToken) -> Result<(bool, Value), String> + Sync,
    after_commit: impl Fn(&str) -> Result<(), String>,
    settle_children: impl Fn(bool) -> Result<(), String>,
    preview: &super::kernel_model_worker::PreviewSink,
) -> Result<(), String> {
    drive_with_actions_transport(ownership, database, clock, cancellation, run_id, runtime,
        api_key, policy, execute, after_commit, settle_children, preview, true).map(|_| ())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum KernelDriveOutcome {
    Terminal,
    Parked,
}

/// A transient Host preparation error must not cancel a still-waiting child
/// Job. The durable Run state, not the stack's return value, decides when its
/// already-issued cancellation tokens may be retired.
pub(super) fn kernel_scope_should_retire(database: &Database, run_id: &str) -> bool {
    // Only a durably parked Run owns a still-live child Job token after the
    // Host stack has returned. Created/running preparation failures retain the
    // original cleanup behavior; a missing/corrupt aggregate fails closed.
    database.kernel_host_run_state(run_id).ok().flatten().as_deref()
        != Some("waiting_jobs")
        || database.pending_kernel_host_commands(run_id).ok()
            .is_some_and(|commands| commands.iter().any(|command| command.kind == "cancel"))
}

#[allow(clippy::too_many_arguments)]
pub(super) fn drive_with_actions_transport(
    _ownership: &KernelRunLock,
    database: &Database,
    clock: &dyn Clock,
    cancellation: &CancellationRegistry,
    run_id: &str,
    runtime: &RuntimeCommand,
    api_key: &str,
    policy: &dyn PolicyDecisionPort,
    execute: impl Fn(&RunControlBinding, &kernel::OutboxEffect, &kernel::CancellationToken) -> Result<(bool, Value), String> + Sync,
    after_commit: impl Fn(&str) -> Result<(), String>,
    settle_children: impl Fn(bool) -> Result<(), String>,
    preview: &super::kernel_model_worker::PreviewSink,
    force_per_round: bool,
) -> Result<KernelDriveOutcome, String> {
    // Validate all frozen inputs before any recovery dispatch. Never reconstruct
    // a missing input/scope/model from current settings.
    database.kernel_host_scope(run_id)?;
    database.kernel_initial_input(run_id)?;
    let coordinator = if database.kernel_host_run_state(run_id)?.as_deref() == Some("created") {
        KernelCoordinator::start_prepared(database, clock, run_id, cancellation)?
    } else {
        KernelCoordinator::reopen(database, clock, run_id, cancellation)?
    }
    .with_preview(preview);
    let owner = format!("kernel-host:{}", uuid::Uuid::new_v4());

    loop {
        // Drain durable post-result actions and all child ownership before any
        // operation below can publish a parent terminal (including a timeout).
        // The action CAS independently refuses dispatch after queued cancel.
        for tool_id in database.pending_kernel_host_action_ids(run_id)? {
            after_commit(&tool_id)?;
        }
        settle_children(false)?;
        if let Some(cancel_seq) = consume_approval_step(&coordinator, database, run_id, policy)? {
            if coordinator.snapshot()?.state == "waiting_jobs" {
                // Cancellation wins over a simultaneous wait-budget breach.
                coordinator.cancel_waiting_jobs_now()?;
            } else {
                coordinator.cancel()?;
            }
            // This loop executes resources synchronously; no resource is still
            // in flight once control has returned to this point.
            coordinator.settle_cancellation()?;
            database.complete_kernel_host_command(run_id, cancel_seq)?;
            // The Run is terminal now. Acknowledge any approval queued in the
            // same batch under the terminal gate, without applying its decision
            // or calling child cleanup again after parent cancellation.
            consume_approval_step(&coordinator, database, run_id, policy)?;
            return Ok(KernelDriveOutcome::Terminal);
        }
        if coordinator.snapshot()?.state == "waiting_jobs" {
            // Ordinary tick does not debit the waiting wall interval; its DB
            // guard correctly rejects that unaccounted projection. Account the
            // original fixed deadline first, then leave the Pi/RPC stack.
            coordinator.account_waiting_jobs_now()?;
            return Ok(if terminal(&coordinator.snapshot()?.state) {
                KernelDriveOutcome::Terminal
            } else {
                KernelDriveOutcome::Parked
            });
        }
        coordinator.tick()?;
        let snapshot = coordinator.snapshot()?;
        if terminal(&snapshot.state) {
            return Ok(KernelDriveOutcome::Terminal);
        }
        if snapshot.state == "cancelling" {
            coordinator.settle_cancellation()?;
            continue;
        }
        // Known pre-execution rejection, not uncertain executor work. Read its
        // durable typed result (also after restart), never arbitrary error text.
        if super::kernel_delegation::correction_limit_reached(&snapshot) {
            settle_children(true)?;
            coordinator.tick()?;
            coordinator.fail(
                "kernel.child_arguments_exhausted",
                "子任务参数连续校验失败，已提供 3 次纠错机会并停止继续派发。请检查工具参数后重试；此错误不是 API 额度不足。",
            )?;
            continue;
        }
        if snapshot.state == "retry_scheduled" {
            std::thread::sleep(Duration::from_millis(100));
            continue;
        }
        // Exclusive OS ownership establishes that no previous process can still
        // complete its lease. An uncertain action is never replayed automatically.
        if snapshot.pending_effects.iter().any(|effect| {
            effect.status == OutboxStatus::Leased
                && matches!(
                    effect.kind,
                    OutboxEffectKind::DispatchTool
                        | OutboxEffectKind::InitialModel
                        | OutboxEffectKind::ContinuationModel
                        | OutboxEffectKind::DeliverToolBatch
                )
        }) {
            coordinator.fail(
                "kernel.uncertain_execution",
                "Execution was interrupted after dispatch; it was not replayed.",
            )?;
            continue;
        }
        let next = snapshot.pending_effects.iter().find(|effect| {
            effect.status == OutboxStatus::Pending
                // An approved write must wait for other outstanding approvals,
                // rather than reaching a strict resource gate and failing forever.
                && !super::kernel_coordinator::live::dispatch_waits_for_approval(&snapshot.state, effect)
                && matches!(
                    effect.kind,
                    OutboxEffectKind::InitialModel
                        | OutboxEffectKind::ContinuationModel
                        | OutboxEffectKind::DispatchTool
                        | OutboxEffectKind::DeliverToolBatch
                )
        });
        let result = if snapshot.state == "compacting" {
            coordinator.resume_context_compaction(&owner, runtime, api_key)
        } else if let Some(effect) = next {
            match effect.kind {
                OutboxEffectKind::InitialModel => {
                    let binding = database
                        .run_control_binding(run_id)?
                        .ok_or("authoritative Run has no frozen control binding")?;
                    if binding.engine_id == "pi" && !force_per_round {
                        // Loop-capable engine session: the worker drives the
                        // engine's own tool loop and the Host services each
                        // round output durably. Old runtimes fall back to the
                        // per-round transport.
                        match coordinator.dispatch_initial_live(
                            &owner,
                            policy,
                            runtime,
                            api_key,
                            &execute,
                            &after_commit,
                            &settle_children,
                        ) {
                            Err(error)
                                if error
                                    .contains("lacks the durable round-loop capability") =>
                            {
                                coordinator
                                    .dispatch_initial_with_worker(&owner, policy, runtime, api_key)
                            }
                            other => other,
                        }
                    } else {
                        coordinator
                            .dispatch_initial_with_worker(&owner, policy, runtime, api_key)
                    }
                }
                OutboxEffectKind::ContinuationModel => {
                    let binding = database.run_control_binding(run_id)?
                        .ok_or("authoritative Run has no frozen control binding")?;
                    if binding.engine_id == "pi" && !force_per_round {
                        coordinator.dispatch_continuation_live(&effect.effect_key, &owner, policy,
                            runtime, api_key, &execute, &after_commit, &settle_children)
                    } else {
                        coordinator.dispatch_continuation_with_worker(&effect.effect_key,
                            &owner,policy,runtime,api_key)
                    }
                },
                OutboxEffectKind::DeliverToolBatch => coordinator
                    .dispatch_stored_batch_with_worker(
                        effect
                            .batch_id
                            .as_deref()
                            .ok_or("missing model batch identity")?,
                        &owner,
                        policy,
                        runtime,
                        api_key,
                    ),
                OutboxEffectKind::DispatchTool => coordinator
                    .dispatch_tool(
                        effect
                            .tool_call_id
                            .as_deref()
                            .ok_or("missing dispatch tool identity")?,
                        &owner,
                        &execute,
                    )
                    .map(|_| ()),
                _ => unreachable!(),
            }
        } else if snapshot.state == "waiting_approval" || snapshot.state == "retry_scheduled" {
            std::thread::sleep(Duration::from_millis(100));
            continue;
        } else if database.kernel_last_event_type(run_id)?.as_deref() == Some("engine.continuation_requested") {
            coordinator.fail(
                "kernel.continuation_interrupted",
                "停止前续答检查已发出，但引擎在下一轮输出前中断。原始历史与已完成的工具结果保持不变；为避免重复执行或重复请求，任务已停止，请重新发起新任务继续。",
            )?;
            continue;
        } else {
            coordinator.fail(
                "kernel.no_progress",
                "No dispatchable work exists for this active Run.",
            )?;
            continue;
        };
        if let Err(error) = result {
            #[cfg(test)]
            if std::env::var_os("FOX_TEST_REAL_HOST_JOB_CHAIN_CHILD").is_some() {
                // The isolated test uses only a local HTTP fixture. Keep raw
                // adapter diagnostics out of the persistent Run/UI path.
                eprintln!("local Host Job chain dispatch error: {error}");
            }
            if error == super::kernel_coordinator::live::LIVE_DETACHED
                || error == super::kernel_coordinator::STEERING_REPLAN
            {
                // The transport detached after committing durable state, or a
                // terminal decision lost a race with a freshly accepted
                // additional request. Either way the next iteration re-reads
                // durable facts (pending deliveries, retries, terminal states)
                // and plans the safe next step; nothing here may replay engine
                // work, and neither case is an execution failure.
                continue;
            }
            settle_children(true)?;
            // Prefer a durable UI cancellation over classifying the interrupted
            // worker as an engine error. The executor has already cleaned up.
            if database
                .pending_kernel_host_commands(run_id)?
                .iter()
                .any(|command| command.kind == "cancel")
            {
                continue;
            }
            coordinator.tick()?;
            if !terminal(&coordinator.snapshot()?.state) {
                // Do not persist arbitrary adapter errors: they may contain
                // request bodies or credentials. Detailed diagnostics stay local.
                let (code, message) = match error.as_str() {
                    "kernel.jobs_pending" => ("kernel.jobs_pending", "后台计算尚未完成；任务已保留进度，请检查作业状态后继续。"),
                    crate::kernel_compaction::UNCERTAIN => (crate::kernel_compaction::UNCERTAIN,
                        "上下文压缩请求已发出，但结果未确认。原始历史保留，未自动重复请求；请检查后重新发起任务。"),
                    crate::kernel_compaction::INSUFFICIENT => (crate::kernel_compaction::INSUFFICIENT,
                        "保留工具结果、执行凭据和最近消息后，上下文仍超出安全容量。原始历史未删改，请缩小任务或新建对话。"),
                    crate::kernel_compaction::NO_CANDIDATES => (crate::kernel_compaction::NO_CANDIDATES, "没有可安全折叠的旧历史；已保留进度，可调整输入后续做。"),
                    crate::kernel_compaction::NO_REDUCTION => (crate::kernel_compaction::NO_REDUCTION, "摘要没有减少上下文；已保留进度，可调整输入后续做。"),
                    crate::kernel_compaction::SINGLE_TOO_LARGE => (crate::kernel_compaction::SINGLE_TOO_LARGE, "单项输入过大；请分页或引用该结果，再续做。"),
                    crate::kernel_compaction::FAILED => (crate::kernel_compaction::FAILED,
                        "上下文压缩未取得有效结果，任务已停止。原始历史保留，未自动重复请求。"),
                    _ => ("kernel.execution_failed", "The owned model or resource executor failed; uncertain work was not replayed."),
                };
                let reason=match code {
                    crate::kernel_compaction::NO_CANDIDATES => Some("no_candidates"),
                    crate::kernel_compaction::NO_REDUCTION => Some("no_reduction"),
                    crate::kernel_compaction::SINGLE_TOO_LARGE => Some("single_item_too_large"),
                    crate::kernel_compaction::INSUFFICIENT => Some("insufficient"),
                    crate::kernel_compaction::FAILED => Some("summary_failed"),
                    _ => None,
                };
                if let Some(reason)=reason {let _=database.kernel_context_budget_stopped(run_id,reason);}
                coordinator.fail(code, message)?;
            }
        }
    }
}

pub(super) fn acquire(sessions_dir: &Path, run_id: &str) -> Result<KernelRunLock, String> {
    KernelRunLock::acquire(sessions_dir, run_id)
}

pub(super) fn initial_input(
    binding: &RunControlBinding,
    prompt: &Value,
    prompt_hash: &str,
) -> Result<fox_engine_protocol::KernelInitialModelInput, String> {
    let mut history = prompt["messages"]
        .as_array()
        .ok_or("missing initial history")?
        .clone();
    // The database history contains the just-created user message. Replace that
    // final entry with the original submitted text/images, not a truncated copy.
    if history
        .last()
        .is_some_and(|message| message["role"] == "user")
    {
        history.pop();
    }
    for message in &mut history {
        if message["role"] == "assistant" {
            let text = message["content"]
                .as_str()
                .ok_or("invalid desktop assistant history")?;
            *message = serde_json::json!({"role":"assistant","content":[{"type":"text","text":text}],"stopReason":"stop"});
        }
    }
    let mut content = vec![
        serde_json::json!({"type":"text","text":prompt["text"].as_str().ok_or("missing submitted text")?}),
    ];
    content.extend(
        prompt["images"]
            .as_array()
            .ok_or("missing initial images")?
            .iter()
            .cloned(),
    );
    history.push(serde_json::json!({"role":"user","content":content}));
    let input = fox_engine_protocol::KernelInitialModelInput {
        schema_version: 1,
        run_id: binding.run_id.clone(),
        turn_id: format!("kernel-turn:{}", binding.run_id),
        prompt_config_hash: prompt_hash.into(),
        messages: history,
    };
    input.validate()?;
    Ok(input)
}

/// The Host-verified managed write behind one frozen dispatch.
///
/// A managed write is identified from Host facts only: the real dispatch tool,
/// the frozen connector identity and scope, and the target the Host itself
/// admits for this dispatch — never from a tool result. The classification
/// itself lives in [`super::managed_files`], where it is unit-tested directly.
impl super::RuntimeHost {
    pub(super) fn stop_kernel_runs(&self) -> Result<(), String> {
        let active = {
            let mut state = self
                .state
                .lock()
                .map_err(|_| "runtime state lock poisoned")?;
            // The registration and dispatch checks use the same lock. A cleanup
            // callback setting the display state to ready cannot reopen admission.
            state.shutting_down = true;
            state.kernel_active_runs.iter().cloned().collect::<Vec<_>>()
        };
        for run_id in active {
            if let Err(error) = self.database.queue_kernel_host_command(&run_id, None) {
                if !self
                    .database
                    .kernel_host_run_state(&run_id)?
                    .as_deref()
                    .is_some_and(terminal)
                {
                    return Err(error);
                }
            }
            self.state
                .lock()
                .map_err(|_| "runtime state lock poisoned")?
                .cancellation
                .request_run_cancel(&run_id);
        }
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !self
            .state
            .lock()
            .map_err(|_| "runtime state lock poisoned")?
            .kernel_active_runs
            .is_empty()
        {
            if std::time::Instant::now() >= deadline {
                return Err("Kernel shutdown is still waiting for owned resource cleanup".into());
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        Ok(())
    }

    pub(crate) fn recover_kernel_runs_detached(&self) -> Result<(), String> {
        let runs = self.database.kernel_host_recoverable_runs()?;
        for run_id in runs {
            // A normal start already holds this OS lock before it registers its
            // active marker. Claiming the lock first closes that small window:
            // a duplicate recovery never registers, drives, or clears its owner.
            let ownership = match acquire(&self.sessions_dir, &run_id) {
                Ok(ownership) => ownership,
                Err(error) if error == super::kernel_run_lock::KERNEL_RUN_ALREADY_OWNED => continue,
                Err(error) => return Err(error),
            };
            {
                let mut state = self.state.lock().map_err(|_| "runtime state lock poisoned")?;
                if state.shutting_down {
                    return Err("Runtime Host is shutting down".into());
                }
                if state.kernel_active_runs.contains(&run_id) {
                    continue;
                }
                state.cancellation.register_run(&run_id)?;
                state.kernel_active_runs.insert(run_id.clone());
                state.state = "recovering".into();
            }
            let host = self.clone();
            std::mem::drop(tauri::async_runtime::spawn_blocking(move || {
                let result = (|| {
                    let binding = host
                        .database
                        .run_control_binding(&run_id)?
                        .ok_or("missing Kernel recovery binding")?;
                    let cancellation = host
                        .state
                        .lock()
                        .map_err(|_| "runtime state lock poisoned")?
                        .cancellation
                        .clone();
                    let runtime = if host.database.kernel_host_run_state(&run_id)?.as_deref()
                        == Some("waiting_jobs") {
                        RuntimeCommand { program: std::path::PathBuf::new(), script: None }
                    } else {
                        host.runtime_command()?
                    };
                    host.drive_kernel_run(ownership, &binding, &runtime, &cancellation, false)
                })();
                if result.is_err() {
                    // Failure recording reacquires ownership; it cannot terminate
                    // a Run currently held by another process.
                    let _ = host.record_kernel_start_failure(&run_id);
                }
                let retire = kernel_scope_should_retire(&host.database, &run_id);
                if let Ok(mut state) = host.state.lock() {
                    state.kernel_active_runs.remove(&run_id);
                    if retire {
                        state.cancellation.retire_run(&run_id);
                    }
                    if state.kernel_active_runs.is_empty() {
                        state.state = "ready".into();
                    }
                    if result.is_err() {
                        state.last_error = Some(
                            "A Kernel Run could not be recovered; no fallback execution was used."
                                .into(),
                        );
                    }
                }
                host.dispatch_next_queued_run();
            }));
        }
        Ok(())
    }

    /// A controlled wake owns the same OS Run lock as normal start/recovery.
    /// The background Job has already outlived its Pi session; only durable
    /// terminal notices may create the one continuation request. The automatic
    /// scanner is intentionally a later batch.
    pub(crate) fn wake_kernel_waiting_run(&self, run_id: &str) -> Result<bool, String> {
        self.wake_kernel_waiting_run_with_transport(run_id, false)
    }

    #[cfg(test)]
    pub(crate) fn wake_kernel_waiting_run_forced_round_for_test(
        &self, run_id: &str,
    ) -> Result<bool, String> {
        self.wake_kernel_waiting_run_with_transport(run_id, true)
    }

    fn wake_kernel_waiting_run_with_transport(
        &self, run_id: &str, force_per_round: bool,
    ) -> Result<bool, String> {
        let ownership = acquire(&self.sessions_dir, run_id)?;
        if self.database.kernel_host_run_state(run_id)?.as_deref() != Some("waiting_jobs") {
            return Ok(false);
        }
        let binding = self.database.run_control_binding(run_id)?
            .ok_or("waiting Kernel Run has no frozen control binding")?;
        let cancellation = self.state.lock()
            .map_err(|_| "runtime state lock poisoned")?.cancellation.clone();
        let coordinator = KernelCoordinator::reopen(
            &self.database, &super::shadow_reconcile::ReconcilerClock,
            run_id, &cancellation,
        )?;
        // A persisted user cancellation wins even if the last Job completed at
        // the same instant. The ordinary drive consumes that command without
        // opening a model transport.
        if self.database.pending_kernel_host_commands(run_id)?
            .iter().any(|command| command.kind == "cancel") {
            drop(coordinator);
            self.start_kernel_run_with_transport(
                ownership, &binding, Value::Null, Value::Null, force_per_round,
            )?;
            return Ok(false);
        }
        coordinator.account_waiting_jobs_now()?;
        if coordinator.snapshot()?.state != "waiting_jobs" {
            return Ok(false);
        }
        if !coordinator.wake_waiting_jobs_now()? {
            return Ok(false);
        }
        drop(coordinator);
        // The wake intent is durable before Node starts. A preparation failure
        // settles it through the normal Kernel failure path, never by replaying
        // the settled model response or the completed Job.
        let result = self.start_kernel_run_with_transport(
            ownership, &binding, Value::Null, Value::Null, force_per_round,
        );
        if result.is_err() {
            let _ = self.record_kernel_start_failure(run_id);
        }
        result.map(|_| true)
    }

    pub(super) fn record_kernel_start_failure(&self, run_id: &str) -> Result<(), String> {
        let _ownership = acquire(&self.sessions_dir, run_id)?;
        if self.database.kernel_host_run_state(run_id)?.as_deref() == Some("waiting_jobs")
            && !self.database.pending_kernel_host_commands(run_id)?
                .iter().any(|command| command.kind == "cancel") {
            // The first model response is already settled. A failure to reopen
            // the Host cannot turn a parked Run into a false execution failure.
            return Ok(());
        }
        if self.database.kernel_fail_before_aggregate(run_id)? {
            return Ok(());
        }
        if self.database.kernel_host_run_state(run_id)?.as_deref() == Some("created") {
            let (config, turn) = self.database.kernel_prepared_failure_config(run_id)?;
            let clock = super::shadow_reconcile::ReconcilerClock;
            let (mut controller, mut effects) =
                kernel::RunController::start(run_id, &turn, config, &clock)
                    .map_err(|error| error.to_string())?;
            if self
                .database
                .pending_kernel_host_commands(run_id)?
                .iter()
                .any(|command| command.kind == "cancel")
            {
                effects.extend(controller.request_cancel());
                effects.extend(controller.settle_cancellation());
            } else {
                effects.extend(controller.terminate(kernel::RunOutcome::Failed { code: "kernel.preparation_failed".into(),
                    message: "The frozen preparation was incomplete; no model or resource execution was started.".into() }));
            }
            return self.database.kernel_commit_decision(
                run_id,
                clock.now_wall_ms(),
                &controller.persist_command(&effects),
            );
        }
        let cancellation = self
            .state
            .lock()
            .map_err(|_| "runtime state lock poisoned")?
            .cancellation
            .clone();
        let coordinator = KernelCoordinator::reopen(
            &self.database,
            &super::shadow_reconcile::ReconcilerClock,
            run_id,
            &cancellation,
        )?;
        if self
            .database
            .pending_kernel_host_commands(run_id)?
            .iter()
            .any(|command| command.kind == "cancel")
        {
            if coordinator.snapshot()?.state == "waiting_jobs" {
                coordinator.cancel_waiting_jobs_now()?;
            } else {
                coordinator.cancel()?;
            }
            coordinator.settle_cancellation()?;
        } else {
            coordinator.fail("kernel.preparation_failed", "The frozen Kernel preparation or recovery could not be validated; no fallback was used.")?;
        }
        cancellation.retire_run(run_id);
        Ok(())
    }

    pub(super) fn start_kernel_run(
        &self,
        ownership: KernelRunLock,
        binding: &RunControlBinding,
        prompt: Value,
        service: Value,
    ) -> Result<(), String> {
        self.start_kernel_run_with_transport(ownership, binding, prompt, service, false)
    }

    #[cfg(test)]
    pub(crate) fn start_kernel_run_forced_round_for_test(
        &self, ownership: KernelRunLock, binding: &RunControlBinding,
        prompt: Value, service: Value,
    ) -> Result<(), String> {
        self.start_kernel_run_with_transport(ownership, binding, prompt, service, true)
    }

    fn start_kernel_run_with_transport(
        &self, ownership: KernelRunLock, binding: &RunControlBinding,
        prompt: Value, service: Value, force_per_round: bool,
    ) -> Result<(), String> {
        let cancellation = {
            let _transition = self
                .run_transition
                .lock()
                .map_err(|_| "runtime transition lock poisoned")?;
            let mut state = self
                .state
                .lock()
                .map_err(|_| "runtime state lock poisoned")?;
            if state.shutting_down {
                return Err("Runtime Host is shutting down".into());
            }
            if state.cancelled_dispatches.remove(&binding.run_id) {
                return Err(super::CANCELLED_BEFORE_SUBMISSION.into());
            }
            state.cancellation.register_run(&binding.run_id)?;
            state.kernel_active_runs.insert(binding.run_id.clone());
            state.state = "busy".into();
            state.execution_profile_id = binding.execution_profile_id.clone();
            state.cancellation.clone()
        };
        let result = (|| {
            let existing = self
                .database
                .kernel_host_run_state(&binding.run_id)?;
            // Waiting has no model dispatch. It must be able to release the
            // Host even when Node runtime startup is unavailable.
            let runtime = if existing.as_deref() == Some("waiting_jobs") {
                RuntimeCommand { program: std::path::PathBuf::new(), script: None }
            } else {
                self.runtime_command()?
            };
            if existing.is_some() {
                // A pre-existing aggregate must use all of its persisted inputs.
                return self.drive_kernel_run(ownership, binding, &runtime, &cancellation, force_per_round);
            }
            let token = cancellation.run_token(&binding.run_id)?;
            let mut supported = super::kernel_gateway::supported_tools();
            if let Some(child) = self.database.child_run(&binding.run_id)? {
                if self
                    .database
                    .run_control_binding(&child.parent_run_id)?
                    .is_some_and(|parent| {
                        parent.authority == fox_engine_protocol::ExecutionAuthority::Authoritative
                    })
                {
                    let parent_scope = self.database.kernel_host_scope(&child.parent_run_id)?;
                    supported.retain(|tool| parent_scope.tool_names.contains(*tool));
                }
            }
            let config = super::kernel_model_worker::describe(
                &runtime,
                binding,
                service,
                prompt.clone(),
                supported,
                &token,
            )?;
            let hash = config.hash()?;
            let input = initial_input(binding, &prompt, &hash)?;
            let scope =
                super::kernel_gateway::freeze_scope(&self.database, binding, &prompt, &config)?;
            let frozen = kernel::RunFrozenConfig {
                engine_id: binding.engine_id.clone(),
                kernel_mode: "authoritative".into(),
                capability_manifest_version: 2,
                capability_manifest_hash: super::runtime_shadow_hash(
                    &serde_json::to_string(&scope).map_err(|_| "invalid resource scope")?,
                ),
                permission_snapshot_id: binding.permission_snapshot_id.clone(),
                execution_profile_id: binding.execution_profile_id.clone(),
                prompt_config_hash: hash.clone(),
                model_request_timeout_ms: binding.budgets.model_request_ms,
                model_first_response_ms: binding.budgets.model_first_response_ms,
                model_idle_ms: binding.budgets.model_idle_ms,
                tool_execution_timeout_ms: binding.budgets.tool_execution_ms,
                run_execution_budget_ms: binding.budgets.run_execution_ms,
                run_execution_limited: binding.budgets.run_execution_limited,
                approval_wait_timeout_ms: binding.budgets.approval_wait_ms,
                provider_max_retries: 2,
                turn_max_retries: 1,
                experimental_compute_job_notice: std::env::var("FOX_EXPERIMENTAL_COMPUTE_JOB_NOTICE")
                    .is_ok_and(|value| value == "1"),
            };
            self.database.kernel_create_run(
                &binding.run_id,
                &binding.engine_id,
                "authoritative",
                2,
                &binding.permission_snapshot_id,
                &binding.execution_profile_id,
                &hash,
                &serde_json::to_string(&frozen).map_err(|_| "invalid frozen Kernel Run")?,
            )?;
            self.database
                .freeze_kernel_model_config(&binding.run_id, &config)?;
            self.database.freeze_kernel_initial_input(&input)?;
            self.database
                .freeze_kernel_host_scope(&binding.run_id, &scope)?;
            self.drive_kernel_run(ownership, binding, &runtime, &cancellation, force_per_round)
        })();
        let retire = kernel_scope_should_retire(&self.database, &binding.run_id);
        if let Ok(mut state) = self.state.lock() {
            state.kernel_active_runs.remove(&binding.run_id);
            if retire {
                state.cancellation.retire_run(&binding.run_id);
            }
            if state.kernel_active_runs.is_empty() {
                state.state = "ready".into();
            }
        }
        result.map(|_| ())
    }

    fn drive_kernel_run(
        &self,
        ownership: KernelRunLock,
        binding: &RunControlBinding,
        runtime: &RuntimeCommand,
        cancellation: &CancellationRegistry,
        force_per_round: bool,
    ) -> Result<KernelDriveOutcome, String> {
        let config = self.database.kernel_model_config(&binding.run_id)?;
        let base_url = config.model_service["baseUrl"]
            .as_str()
            .ok_or("missing frozen model URL")?;
        let api_key = if self.database.kernel_host_run_state(&binding.run_id)?.as_deref()
            == Some("waiting_jobs") {
            // A parked Run has no model request. Avoid even consulting a key
            // store while releasing an already-settled Host session.
            String::new()
        } else {
            crate::model_service::get_api_key(base_url).unwrap_or_default()
        };
        let policy = super::kernel_gateway::GatewayPolicy {
            binding: binding.clone(),
            scope: self.database.kernel_host_scope(&binding.run_id)?,
            // #5: production may also read the run's additional authorization
            // grants, so a conversation approval is reusable without rewriting
            // the frozen binding.
            database: Some(self.database.clone()),
            // Office artifact references (office_import_data.artifactId) are
            // resolved against this conversation's own compute store.
            sessions_dir: Some(self.sessions_dir.clone()),
            // Host-private placement for rendered previews, working copies and
            // commit staging. Derived from the Host's own state, so the model
            // cannot redirect these areas.
            artifacts_dir: self
                .sessions_dir
                .parent()
                .map(std::path::Path::to_path_buf),
        };
        let proposal_policy = super::kernel_gateway::GatewayProposalPolicy {
            gateway: &policy,
            database: &self.database,
        };
        let app = self.app.clone();
        let display_database = self.database.clone();
        let preview = move |notice: &fox_engine_protocol::KernelModelPreview| {
            use tauri::Emitter;
            if display_database.save_kernel_model_display(notice).unwrap_or(false) {
                // Accepted previews (including tool-parameter-only progress)
                // feed the controller's idle bound via the volatile registry.
                // Stale/foreign frames are ignored by the save above and never
                // become progress.
                display_database.note_kernel_model_progress(
                    &notice.run_id,
                    crate::database::now_ms(),
                );
                let _ = app.emit("fox://kernel-model-preview", notice);
            }
        };
        let result = drive_with_actions_transport(
            &ownership,
            &self.database,
            &super::shadow_reconcile::ReconcilerClock,
            cancellation,
            &binding.run_id,
            runtime,
            &api_key,
            &proposal_policy,
            |_, effect, token| {
                let payload: Value = serde_json::from_str(&effect.payload_json)
                    .map_err(|_| "invalid frozen tool dispatch")?;
                let tool = payload["tool"].as_str().ok_or("missing tool name")?;
                // B1a execution boundary: the persistent single claim. The
                // credential is verified against the authoritative snapshot
                // before the claim, the payload must equal the durable identity,
                // and only the claim winner starts the first execution attempt.
                // Repeat deliveries, terminal refusals and unknown outcomes are
                // answered from durable facts without executing anything.
                let result = execute_claimed_dispatch(
                    &self.database,
                    binding,
                    effect,
                    tool,
                    &payload,
                    |durable_input, claim| {
                        self.execute_claimed_resource(binding, &policy, effect, tool, durable_input, token, claim)
                    },
                );
                match result {
                    Ok(result) => Ok((
                        result.get("isError").and_then(Value::as_bool) != Some(true),
                        result,
                    )),
                    // A work transaction may already have created a child or
                    // durable intent. Do not turn a staging failure into a
                    // retryable ordinary tool result and advance the model.
                    Err(error)
                        if super::work_tools::is_work_tool(tool)
                            || super::kernel_delegation::TOOLS.contains(&tool) =>
                    {
                        Err(error)
                    }
                    Err(error) if token.check().is_ok() => {
                        let dispatch_id = fox_engine_protocol::encode_dispatch_id(&binding.run_id, effect.tool_call_id.as_deref().ok_or("missing dispatch identity")?)?;
                        Ok((false, with_execution_receipt(&self.database, &binding.run_id, &dispatch_id,
                            resource_failure_result(tool, &error))?))
                    },
                    Err(error) => Err(error),
                }
            },
            |tool_id| self.dispatch_kernel_host_action(binding, tool_id),
            |force_cancel| self.wait_kernel_action_children(binding, force_cancel),
            &preview,
            force_per_round,
        );
        if result.is_err() {
            self.wait_kernel_action_children(binding, true)?;
        }
        result
    }

    /// B1a admission at the real execution boundary (frozen contract v1.6).
    ///
    /// Order: read the durable (tool, canonical input) identity, prove the
    /// dispatch payload is byte-identical to it, present a credential built
    /// from durable facts, verify it against the authoritative snapshot
    /// (read-only), then take the persistent single claim. Only the claim
    /// winner starts the first execution attempt, and it executes the verified
    /// durable input — never the payload.
    /// B1a admission at the real execution boundary. The production Host
    /// delegates to the shared `HostAdmission` order so the drive loop and the
    /// Host-order tests exercise exactly the same sequence.
    fn admit_dispatch_execution(
        &self,
        binding: &RunControlBinding,
        effect: &kernel::OutboxEffect,
        tool: &str,
        payload: &Value,
    ) -> Result<DispatchAdmission, String> {
        HostAdmission {
            database: &self.database,
            binding,
        }
        .admit(effect, tool, payload)
    }

    fn execute_claimed_resource(
        &self, binding: &RunControlBinding, policy: &super::kernel_gateway::GatewayPolicy,
        effect: &kernel::OutboxEffect, tool: &str, input_json: &str,
        token: &kernel::CancellationToken, claim: ClaimedExecution,
    ) -> (Result<Value, String>, fox_engine_protocol::ExecutionEvidence, fox_engine_protocol::CallOutcome) {
        use fox_engine_protocol::{CallOutcome, ExecutionEvidence};
        let input: Value = match serde_json::from_str(input_json) {
            Ok(input) => input,
            Err(_) => return (Err("invalid durable input".into()), ExecutionEvidence::NotStarted,
                CallOutcome::Refused { code: "tool.invalid_input".into() }),
        };
        let validate = || -> Result<(), String> {
            token.check()?;
            self.database.revalidate_execution_credential(claim.credential())?;
            self.database.kernel_validate_resource_acquisition_for(&binding.run_id, Some(tool))?;
            policy.validate(tool, &input)
        };
        if let Err(error) = validate() {
            return (Err(error.clone()), ExecutionEvidence::NotStarted, CallOutcome::Refused { code: error });
        }
        if matches!(tool, "write_file" | "edit_file") {
            let (result, evidence) = super::managed_files::execute_admitted_file(
                super::managed_files::ManagedExecutionContext {
                    database: &self.database, backups_dir: &self.managed_files_dir,
                    conversation_id: &binding.conversation_id, run_id: &binding.run_id,
                    project_root: binding.permission.project_root.as_deref(),
                    permission_mode: binding.permission.mode.as_str(), scope: &policy.scope,
                    sessions_dir: Some(&self.sessions_dir),
                }, tool, &input, effect.tool_call_id.as_deref().unwrap_or_default(),
                claim.credential(), Some(token), &mut || validate());
            let outcome = call_outcome(&result);
            return (result, evidence, outcome);
        }
        if tool == "run_command" {
            let root = Path::new(binding.permission.project_root.as_deref().unwrap_or(""));
            let budget = match policy.remaining_budget(&self.database) {
                Ok(budget) => budget,
                Err(error) => return (Err(error.clone()), ExecutionEvidence::NotStarted, CallOutcome::Refused { code: error }),
            };
            return super::command_jobs::execute_authorized(&self.database, &binding.run_id, &input,
                root, token, budget, claim);
        }
        let result = if crate::resource_gateway::is_reader(tool) {
            policy.execute_reader(&self.database, tool, &input,
                effect.tool_call_id.as_deref().unwrap_or_default(), token)
        } else {
            self.execute_admitted_dispatch(binding, policy, effect, tool, input_json, token)
        };
        let evidence = conservative_evidence(claim.credential().action_class, &result);
        let outcome = call_outcome(&result);
        (result, evidence, outcome)
    }

    /// The existing executor chain, reached only by the claim winner, and it
    /// runs the VERIFIED durable canonical input (never the dispatch payload).
    fn execute_admitted_dispatch(
        &self,
        binding: &RunControlBinding,
        policy: &super::kernel_gateway::GatewayPolicy,
        effect: &kernel::OutboxEffect,
        tool: &str,
        durable_input_json: &str,
        token: &kernel::CancellationToken,
    ) -> Result<Value, String> {
        let input: Value = serde_json::from_str(durable_input_json)
            .map_err(|_| "durable canonical input is not valid JSON")?;
        if super::work_tools::is_work_tool(tool) {
            policy.execute_work(
                &self.database,
                effect
                    .tool_call_id
                    .as_deref()
                    .ok_or("missing work tool identity")?,
                tool,
                &input,
                token,
            )
        } else if super::kernel_delegation::TOOLS.contains(&tool) {
            policy.execute_delegation(
                &self.database,
                effect
                    .tool_call_id
                    .as_deref()
                    .ok_or("missing delegation identity")?,
                tool,
                &input,
                token,
            )
        } else if super::kernel_gateway::is_context_resource(tool) {
            policy.execute_context_resource(
                &self.database,
                &self.attachments_dir,
                &self.sessions_dir,
                &self.skills_dir,
                tool,
                &input,
                token,
            )
        } else if super::kernel_gateway::is_knowledge(tool) {
            use tauri::Manager;
            let app_state = self.app.state::<crate::app_state::AppState>();
            policy.execute_knowledge(
                &self.database,
                &self.yuxi_client,
                &app_state.local_knowledge,
                tool,
                &input,
                token,
            )
        } else {
            // A managed write is identified from Host facts only: the real
            // dispatch tool (including the frozen Office wrapper) plus the
            // target the Host itself admitted for this frozen dispatch. The
            // shared seam protects the current bytes before the write and
            // registers the resulting content version, so the desktop Host and
            // the real-task evaluation cannot drift apart on what "a managed
            // write" means.
            let outcome = super::managed_files::execute_with_managed_versions(
                super::managed_files::ManagedExecutionContext {
                    database: &self.database,
                    backups_dir: &self.managed_files_dir,
                    conversation_id: &binding.conversation_id,
                    run_id: &binding.run_id,
                    project_root: binding.permission.project_root.as_deref(),
                    permission_mode: binding.permission.mode.as_str(),
                    scope: &policy.scope,
                    sessions_dir: Some(&self.sessions_dir),
                },
                tool,
                &input,
                effect.tool_call_id.as_deref(),
                || policy.execute(&self.database, tool, &input, token),
            );
            outcome
        }
    }

    fn dispatch_kernel_host_action(
        &self,
        binding: &RunControlBinding,
        tool_id: &str,
    ) -> Result<(), String> {
        use super::work_tools::WorkToolPostFinalizeDirective;
        use tauri::Emitter;
        let Some(body) = self
            .database
            .claim_kernel_host_action(&binding.run_id, tool_id)?
        else {
            return Ok(());
        };
        #[derive(serde::Deserialize)]
        #[serde(untagged)]
        enum Action {
            Work(WorkToolPostFinalizeDirective),
            Delegation(super::kernel_delegation::CancelAction),
        }
        let action: Action =
            serde_json::from_str(&body).map_err(|_| "invalid frozen Kernel Host action")?;
        let directive = match action {
            Action::Work(directive) => directive,
            Action::Delegation(action) => {
                match action {
                    super::kernel_delegation::CancelAction::Child { child_run_id } => {
                        let child = self
                            .database
                            .child_run(&child_run_id)?
                            .ok_or("missing child")?;
                        if child.parent_run_id != binding.run_id {
                            return Err("Kernel child cancel identity mismatch".into());
                        }
                        super::ensure_child_cancel_not_graph_bound(&self.database, &child_run_id)?;
                        self.cancel_child_runtime(&child_run_id)?;
                    }
                    super::kernel_delegation::CancelAction::Team {
                        team_run_id,
                        reason,
                    } => {
                        let team = self
                            .database
                            .get_expert_team(&team_run_id)?
                            .ok_or("missing team")?;
                        if team.run.parent_run_id != binding.run_id {
                            return Err("Kernel team cancel identity mismatch".into());
                        }
                        for child in team.members {
                            if !super::run_status_is_terminal(&child.status) {
                                self.cancel_child_runtime(&child.child_run_id)?;
                            }
                        }
                        self.wait_kernel_action_children(binding, false)?;
                        self.database.cancel_expert_team(&team_run_id, &reason)?;
                    }
                }
                self.wait_kernel_action_children(binding, false)?;
                return self
                    .database
                    .complete_kernel_host_action(&binding.run_id, tool_id);
            }
        };
        match directive {
            WorkToolPostFinalizeDirective::StartChild(dispatch) => {
                if dispatch.child_run.parent_run_id != binding.run_id
                    || dispatch.started.run.id != dispatch.child_run.child_run_id
                {
                    return Err("Kernel child action identity mismatch".into());
                }
                self.start_child_runtime(
                    dispatch.started,
                    dispatch.child_run,
                    binding.conversation_id.clone(),
                )?;
            }
            WorkToolPostFinalizeDirective::CancelGraphChild(directive) => {
                let activation = self
                    .database
                    .activate_graph_node_cancel_intent(&directive.intent_id)
                    .map_err(|error| error.to_string())?;
                if activation.child_run_id != directive.child_run_id {
                    return Err("Kernel graph cancel identity mismatch".into());
                }
                if activation.activated {
                    self.signal_graph_child_cancel(&activation.child_run_id, false)?;
                }
            }
            WorkToolPostFinalizeDirective::StartGraphReviewer(directive) => {
                let activation = self
                    .database
                    .activate_graph_node_review_request(&directive.request_id)
                    .map_err(|error| error.to_string())?;
                if activation.activated {
                    let agent_id = self
                        .database
                        .conversation_agent_id(&binding.conversation_id)?;
                    super::dispatch_graph_reviewer_child(self, &directive.request_id, &agent_id)?;
                }
            }
            WorkToolPostFinalizeDirective::AcceptReadOnlyGraph(directive) => {
                let acceptance = self
                    .database
                    .activate_read_only_graph_acceptance(&directive.acceptance_id)
                    .map_err(|error| error.to_string())?;
                if acceptance.accepted {
                    let _ = self.app.emit("fox://graph-acceptance", acceptance);
                }
            }
        }
        // Keep the owning parent loop inside this barrier until child executors
        // and their cleanup have returned. No parent model/terminal is published
        // while this action still owns running child resources.
        self.wait_kernel_action_children(binding, false)?;
        self.database
            .complete_kernel_host_action(&binding.run_id, tool_id)
    }

    fn wait_kernel_action_children(
        &self,
        binding: &RunControlBinding,
        force_cancel: bool,
    ) -> Result<(), String> {
        self.wait_kernel_selected_children(binding, force_cancel, None)
    }

    pub(super) fn wait_kernel_selected_children(
        &self,
        binding: &RunControlBinding,
        force_cancel: bool,
        selected: Option<&std::collections::BTreeSet<String>>,
    ) -> Result<(), String> {
        let elapsed = self
            .database
            .kernel_build_full_snapshot(&binding.run_id)?
            .running_elapsed_ms;
        let deadline = binding.budgets.remaining_run_ms(elapsed)
            .map(|remaining| std::time::Instant::now() + Duration::from_millis(remaining.max(0) as u64));
        loop {
            let active = self
                .database
                .active_child_run_ids(&binding.run_id)?
                .into_iter()
                .filter(|id| selected.is_none_or(|selected| selected.contains(id)))
                .collect::<Vec<_>>();
            let owned = self
                .child_hosts
                .lock()
                .map_err(|_| "child runtime map is poisoned")?
                .keys()
                .cloned()
                .collect::<Vec<_>>();
            let mut owned_children = Vec::new();
            for id in owned {
                if selected.is_none_or(|selected| selected.contains(&id))
                    && self
                        .database
                        .child_run(&id)?
                        .is_some_and(|child| child.parent_run_id == binding.run_id)
                {
                    owned_children.push(id);
                }
            }
            if active.is_empty() && owned_children.is_empty() {
                return Ok(());
            }
            let cancelling = force_cancel
                || deadline.is_some_and(|deadline| std::time::Instant::now() >= deadline)
                || self
                    .database
                    .pending_kernel_host_commands(&binding.run_id)?
                    .iter()
                    .any(|command| command.kind == "cancel");
            for id in active {
                if !owned_children.contains(&id)
                    && self.database.run_control_binding(&id)?.is_none()
                {
                    let _ownership = match acquire(&self.sessions_dir, &id) {
                        Ok(ownership) => ownership,
                        Err(error)
                            if error
                                .starts_with("Kernel Run already owned or cannot be locked:") =>
                        {
                            continue
                        }
                        Err(error) => return Err(error),
                    };
                    let child = self
                        .database
                        .child_run(&id)?
                        .ok_or("missing Kernel child record")?;
                    let settled = if cancelling {
                        self.database
                            .kernel_cancel_unstarted_child(&child.parent_run_id, &id)?
                    } else {
                        self.database
                            .kernel_fail_unstarted_child(&child.parent_run_id, &id)?
                    };
                    if settled {
                        continue;
                    }
                    return Err(
                        "Unowned child has no frozen Kernel authority; execution was not resumed"
                            .into(),
                    );
                }
                if cancelling {
                    if let Err(error) = self.cancel_child_runtime(&id) {
                        if !self
                            .database
                            .child_run_status(&id)?
                            .as_deref()
                            .is_some_and(super::run_status_is_terminal)
                        {
                            return Err(error);
                        }
                    }
                }
                if !owned_children.contains(&id) {
                    // Failed preparation can leave only a frozen binding. Under
                    // the child's exclusive lock, settle it without model I/O.
                    if cancelling
                        || self
                            .database
                            .kernel_host_run_state(&id)?
                            .as_deref()
                            .is_none_or(|state| state == "created")
                    {
                        if let Err(error) = self.record_kernel_start_failure(&id) {
                            if !error.starts_with("Kernel Run already owned or cannot be locked:") {
                                return Err(error);
                            }
                        }
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

/// Host-order regression tests for the B1a-R1 seams: a claim is not a start
/// confirmation, only execution evidence confirms it, the executor only ever
/// sees the verified durable input, and the safe management surface stays
/// reachable while external launches stay refused. They drive the SAME
/// `admit_and_execute_dispatch` the production closure uses, with real SQLite
/// durable facts.
#[cfg(test)]
mod host_order_tests {
    #[test]
    fn missing_receipt_does_not_invent_a_confirmed_non_start() {
        let (db, _root, run_id, _, _) = fixture();
        let result = super::execution_receipt_result(&db, &run_id, "missing", "run_command", None).unwrap();
        assert_eq!(result["details"]["executionStarted"], "unknown");
        assert_eq!(result["details"]["sideEffectState"], "unknown");
        assert_eq!(result["details"]["allowReplay"], false);
    }
    use super::*;
    use crate::database::Database;
    use fox_engine_protocol::{encode_dispatch_id, ExecutionStage, TriState};

    fn fixture() -> (Database, std::path::PathBuf, String, String, RunControlBinding) {
        let path = std::env::temp_dir().join(format!("fox-host-order-{}.db", uuid::Uuid::new_v4()));
        let db = Database::open(path.clone()).unwrap();
        let conversation = db
            .create_conversation("fox-general", Some("host order"), None, None)
            .unwrap();
        let run = db.create_run(&conversation.id, "host order run", None).unwrap();
        let run_id = run.run.id.clone();
        let mut budgets = fox_engine_protocol::TimeBudgets::default();
        budgets.tool_execution_ms = 30_000;
        let binding = db
            .freeze_kernel_run_control(&run_id, "durable", budgets)
            .unwrap();
        db.with_connection(|c| {
            c.execute(
                "INSERT INTO kernel_runs(run_id, engine_id, kernel_mode, capability_manifest_version,
                     permission_snapshot_id, execution_profile_id, prompt_config_hash, frozen_config_json,
                     state, last_event_seq, created_at, updated_at, terminal_at)
                 VALUES (?1,'kernel','authoritative',1,?2,'durable','h','{}','running',0,1,1,NULL)",
                rusqlite::params![run_id, binding.permission_snapshot_id],
            )?;
            c.execute(
                "INSERT INTO kernel_tool_calls(run_id, tool_call_id, batch_id, tool, source_order,
                     canonical_input_json, state, result_json, created_at, settled_at)
                 VALUES (?1,'call-1','b1','read',0,'{\"path\":\"a.txt\"}','pending',NULL,1,NULL)",
                rusqlite::params![run_id],
            )?;
            Ok(())
        })
        .unwrap();
        (db, path, run_id, conversation.id, binding)
    }

    fn effect_for(tool_call_id: &str, input: serde_json::Value) -> kernel::OutboxEffect {
        kernel::OutboxEffect {
            effect_key: format!("dispatch:{tool_call_id}"),
            kind: kernel::OutboxEffectKind::DispatchTool,
            payload_json: serde_json::json!({"tool": "read", "input": input}).to_string(),
            tool_call_id: Some(tool_call_id.to_owned()),
            batch_id: Some("b1".to_owned()),
            idempotency_key: format!("tool-dispatch:{tool_call_id}"),
            status: kernel::OutboxStatus::Pending,
            attempts: 0,
        }
    }

    /// Issue a credential the way the admission transaction does: every field
    /// derived from the run's durable facts (the same values the Host presents
    /// back), never from the model.
    fn issue_credential(
        db: &Database,
        binding: &RunControlBinding,
        conversation_id: &str,
        tool_call_id: &str,
        tool: &str,
        input: &str,
        class: fox_engine_protocol::ActionClass,
    ) -> String {
        let dispatch_id = encode_dispatch_id(&binding.run_id, tool_call_id).unwrap();
        let credential = fox_engine_protocol::ExecutionCredential::new(
            dispatch_id.clone(),
            binding.run_id.clone(),
            conversation_id.to_owned(),
            crate::database::kernel_execution_admission::launch_params_hash(tool, input),
            class,
            None,
            binding.execution_profile_id.clone(),
            binding.permission_snapshot_id.clone(),
            None,
            binding.budgets.tool_execution_ms,
            None,
            crate::database::kernel_execution_admission::unverified_backend_requirement(),
        )
        .unwrap();
        db.issue_execution_credential(&credential).unwrap();
        dispatch_id
    }

    #[test]
    fn claim_is_not_start_and_only_execution_evidence_confirms_it() {
        let (db, _path, run_id, conversation_id, binding) = fixture();
        let dispatch_id = issue_credential(
            &db,
            &binding,
            &conversation_id,
            "call-1",
            "read",
            "{\"path\":\"a.txt\"}",
            fox_engine_protocol::ActionClass::Read,
        );
        let effect = effect_for("call-1", serde_json::json!({"path": "a.txt"}));

        let mut executed_with: Option<String> = None;
        let mut start_inside_executor = None;
        let result = admit_and_execute_dispatch(
            &db,
            &binding,
            &effect,
            "read",
            &serde_json::json!({"input": {"path": "a.txt"}}),
            |durable| {
                executed_with = Some(durable.to_owned());
                start_inside_executor = db
                    .read_execution_attempt(&run_id, &dispatch_id)
                    .unwrap()
                    .map(|row| row.start_confirmed);
                (
                    Ok(serde_json::json!({"content":[{"type":"text","text":"ok"}]})),
                    fox_engine_protocol::ExecutionEvidence::Started,
                )
            },
        )
        .unwrap();
        assert_eq!(result["isError"].as_bool(), None, "result: {result}");
        // I4: the executor received exactly the durable canonical input.
        assert_eq!(executed_with.as_deref(), Some("{\"path\":\"a.txt\"}"));
        // I1: inside the executor the start fact was still unknown.
        assert_eq!(start_inside_executor, Some(None));
        let row = db.read_execution_attempt(&run_id, &dispatch_id).unwrap().unwrap();
        assert_eq!(row.state, fox_engine_protocol::AttemptState::Completed);
        assert_eq!(row.start_confirmed, Some(true));
        assert_eq!(row.terminal_state.as_deref(), Some("completed"));
        let receipt = db.execution_receipt(&run_id, &dispatch_id).unwrap().unwrap();
        assert_eq!(receipt.execution_started, TriState::True);
        assert_eq!(receipt.stage, ExecutionStage::LaunchConfirmed);
    }

    #[test]
    fn an_ok_structured_refusal_is_not_a_start_confirmation() {
        // R1-01: a tool result can be Ok(json) with isError=true. That is a
        // pre-execution refusal: nothing started, and the attempt is terminal.
        let (db, _path, run_id, conversation_id, binding) = fixture();
        let dispatch_id = issue_credential(
            &db,
            &binding,
            &conversation_id,
            "call-1",
            "read",
            "{\"path\":\"a.txt\"}",
            fox_engine_protocol::ActionClass::Read,
        );
        let effect = effect_for("call-1", serde_json::json!({"path": "a.txt"}));
        let result = execute_claimed_dispatch(
            &db,
            &binding,
            &effect,
            "read",
            &serde_json::json!({"input": {"path": "a.txt"}}),
            |_, _claim| {
                (
                    Ok(serde_json::json!({
                        "isError": true,
                        "content":[{"type":"text","text":"Local operation did not run: path escapes the authorized project root"}],
                        "details":{"errorCode":"tool.permission_denied"}
                    })),
                    fox_engine_protocol::ExecutionEvidence::NotStarted,
                    fox_engine_protocol::CallOutcome::Refused {
                        code: "tool.permission_denied".to_owned(),
                    },
                )
            },
        )
        .unwrap();
        assert_eq!(result["isError"].as_bool(), Some(true));
        let row = db.read_execution_attempt(&run_id, &dispatch_id).unwrap().unwrap();
        assert_eq!(
            row.state,
            fox_engine_protocol::AttemptState::Refused {
                code: "tool.permission_denied".to_owned()
            },
            "a structured pre-execution refusal must be terminal with its own code"
        );
        assert_eq!(row.start_confirmed, None);
        let receipt = db.execution_receipt(&run_id, &dispatch_id).unwrap().unwrap();
        assert_eq!(receipt.execution_started, TriState::False);
        assert_eq!(receipt.reason_code.as_deref(), Some("tool.permission_denied"));
    }

    #[test]
    fn the_same_is_error_shape_with_unknown_evidence_is_not_projected_to_false() {
        // Result shape and call outcome do not decide the execution fact. The
        // same isError payload stays Unknown when trusted evidence is Unknown.
        let (db, _path, run_id, conversation_id, binding) = fixture();
        let dispatch_id = issue_credential(
            &db,
            &binding,
            &conversation_id,
            "call-1",
            "read",
            "{\"path\":\"a.txt\"}",
            fox_engine_protocol::ActionClass::Read,
        );
        let effect = effect_for("call-1", serde_json::json!({"path": "a.txt"}));
        let result = execute_claimed_dispatch(
            &db,
            &binding,
            &effect,
            "read",
            &serde_json::json!({"input": {"path": "a.txt"}}),
            |_, _claim| (
                Ok(serde_json::json!({
                    "isError": true,
                    "content":[{"type":"text","text":"Local operation did not run: path escapes the authorized project root"}],
                    "details":{"errorCode":"tool.permission_denied"}
                })),
                fox_engine_protocol::ExecutionEvidence::Unknown,
                fox_engine_protocol::CallOutcome::Failed {
                    code: "tool.permission_denied".to_owned(),
                },
            ),
        )
        .unwrap();
        assert_eq!(result["isError"].as_bool(), Some(true));
        let row = db.read_execution_attempt(&run_id, &dispatch_id).unwrap().unwrap();
        assert_eq!(row.state, fox_engine_protocol::AttemptState::Unknown);
        assert_eq!(row.start_confirmed, None, "Unknown must not collapse to false");
        assert_eq!(row.terminal_state.as_deref(), Some("failed"));
        assert_eq!(row.error_code.as_deref(), Some("tool.permission_denied"));
        let receipt = db.execution_receipt(&run_id, &dispatch_id).unwrap().unwrap();
        assert_eq!(receipt.execution_started, TriState::Unknown);
        assert_ne!(receipt.execution_started, TriState::False);
    }

    #[test]
    fn a_read_completion_never_claims_an_external_process_start() {
        // R1-01: a successful read/query is a completed call, not a process start.
        let (db, _path, run_id, conversation_id, binding) = fixture();
        let dispatch_id = issue_credential(
            &db,
            &binding,
            &conversation_id,
            "call-1",
            "read",
            "{\"path\":\"a.txt\"}",
            fox_engine_protocol::ActionClass::Read,
        );
        let effect = effect_for("call-1", serde_json::json!({"path": "a.txt"}));
        let result = admit_and_execute_dispatch(
            &db,
            &binding,
            &effect,
            "read",
            &serde_json::json!({"input": {"path": "a.txt"}}),
            |_| (
                Ok(serde_json::json!({"content":[{"type":"text","text":"file body"}]})),
                fox_engine_protocol::ExecutionEvidence::NotStarted,
            ),
        )
        .unwrap();
        assert_eq!(result["isError"].as_bool(), None, "result: {result}");
        let row = db.read_execution_attempt(&run_id, &dispatch_id).unwrap().unwrap();
        assert_eq!(row.state, fox_engine_protocol::AttemptState::Completed);
        assert_eq!(row.start_confirmed, Some(false));
        assert_eq!(row.terminal_state.as_deref(), Some("completed"));
        assert_eq!(row.error_code, None, "a successful read has no refusal code");
        let receipt = db.execution_receipt(&run_id, &dispatch_id).unwrap().unwrap();
        assert_eq!(receipt.execution_started, TriState::False);
        assert_eq!(receipt.reason_code, None);
    }

    #[test]
    fn a_manage_query_completion_is_not_a_start() {
        // R1-01: status/output/cancel complete a management query; no external
        // process is started by them.
        let (db, _path, run_id, conversation_id, binding) = fixture();
        let dispatch_id = issue_credential(
            &db,
            &binding,
            &conversation_id,
            "call-1",
            "run_command",
            "{\"action\":\"status\",\"jobId\":\"j\"}",
            fox_engine_protocol::ActionClass::Manage,
        );
        db.with_connection(|c| {
            c.execute(
                "UPDATE kernel_tool_calls SET tool='run_command',
                     canonical_input_json='{\"action\":\"status\",\"jobId\":\"j\"}'
                  WHERE run_id=?1 AND tool_call_id='call-1'",
                rusqlite::params![run_id],
            )?;
            Ok(())
        })
        .unwrap();
        let effect = effect_for("call-1", serde_json::json!({"action": "status", "jobId": "j"}));
        let result = admit_and_execute_dispatch(
            &db,
            &binding,
            &effect,
            "run_command",
            &serde_json::json!({"input": {"action": "status", "jobId": "j"}}),
            |_| (
                Ok(serde_json::json!({"content":[{"type":"text","text":"running"}]})),
                fox_engine_protocol::ExecutionEvidence::NotStarted,
            ),
        )
        .unwrap();
        assert_eq!(result["isError"].as_bool(), None, "result: {result}");
        let row = db.read_execution_attempt(&run_id, &dispatch_id).unwrap().unwrap();
        assert_eq!(row.state, fox_engine_protocol::AttemptState::Completed);
        assert_eq!(row.start_confirmed, Some(false));
        assert_eq!(row.terminal_state.as_deref(), Some("completed"));
        assert_eq!(row.error_code, None, "a successful query has no refusal code");
        let receipt = db.execution_receipt(&run_id, &dispatch_id).unwrap().unwrap();
        assert_eq!(receipt.execution_started, TriState::False);
        assert_eq!(receipt.reason_code, None);
    }

    #[test]
    fn an_error_after_a_real_start_stays_unknown_and_never_replays() {
        // R1-01: an executor error may have happened after a real start. The
        // Host must not claim "not started" and must not replay.
        let (db, _path, run_id, conversation_id, binding) = fixture();
        let dispatch_id = issue_credential(
            &db,
            &binding,
            &conversation_id,
            "call-1",
            "read",
            "{\"path\":\"a.txt\"}",
            fox_engine_protocol::ActionClass::Read,
        );
        let effect = effect_for("call-1", serde_json::json!({"path": "a.txt"}));
        let result = admit_and_execute_dispatch(
            &db,
            &binding,
            &effect,
            "read",
            &serde_json::json!({"input": {"path": "a.txt"}}),
            |_| (
                Err("the child died oddly".to_owned()),
                fox_engine_protocol::ExecutionEvidence::Unknown,
            ),
        )
        .unwrap_err();
        assert!(result.contains("child died"), "{result}");
        let row = db.read_execution_attempt(&run_id, &dispatch_id).unwrap().unwrap();
        assert_eq!(row.state, fox_engine_protocol::AttemptState::Unknown);
        assert_eq!(row.start_confirmed, None, "no start may be invented");
        // A repeat delivery is read-only and reports the uncertain fact.
        let repeat = admit_and_execute_dispatch(
            &db,
            &binding,
            &effect,
            "read",
            &serde_json::json!({"input": {"path": "a.txt"}}),
            |_| (
                Ok(serde_json::json!({"content":[]})),
                fox_engine_protocol::ExecutionEvidence::Started,
            ),
        )
        .unwrap();
        assert_eq!(repeat["details"]["stage"], "interrupted");
        assert_eq!(repeat["details"]["executionStarted"], "unknown");
        assert_eq!(repeat["details"]["codeAlias"], "job.interrupted_unknown");
        assert!(!repeat["details"]["allowReplay"].as_bool().unwrap());
    }

    #[test]
    fn a_confirmed_start_is_never_regressed_by_an_unknown_outcome() {
        // R1-01: once evidence confirms a start, an unknown outcome keeps True.
        let (db, _path, run_id, conversation_id, binding) = fixture();
        let dispatch_id = issue_credential(
            &db,
            &binding,
            &conversation_id,
            "call-1",
            "read",
            "{\"path\":\"a.txt\"}",
            fox_engine_protocol::ActionClass::Read,
        );
        let effect = effect_for("call-1", serde_json::json!({"path": "a.txt"}));
        let error = execute_claimed_dispatch(
            &db,
            &binding,
            &effect,
            "read",
            &serde_json::json!({"input": {"path": "a.txt"}}),
            |_, _claim| (
                Err("the confirmed child later failed".to_owned()),
                fox_engine_protocol::ExecutionEvidence::Started,
                fox_engine_protocol::CallOutcome::Failed {
                    code: "tool.child_failed".to_owned(),
                },
            ),
        )
        .unwrap_err();
        assert!(error.contains("confirmed child later failed"), "{error}");
        // The real executor confirmed the start before reporting a failed call.
        // Settlement makes the outcome unknown without erasing that start fact.
        let row = db.read_execution_attempt(&run_id, &dispatch_id).unwrap().unwrap();
        assert_eq!(row.state, fox_engine_protocol::AttemptState::Unknown);
        assert_eq!(row.start_confirmed, Some(true));
        assert_eq!(row.terminal_state.as_deref(), Some("failed"));
        assert_eq!(row.error_code.as_deref(), Some("tool.child_failed"));
        let receipt = db.execution_receipt(&run_id, &dispatch_id).unwrap().unwrap();
        assert_eq!(receipt.execution_started, TriState::True);
        assert!(receipt.clone().with_execution_started(TriState::False).is_err());
    }

    #[test]
    fn same_tool_different_path_never_reaches_an_executor() {
        let (db, _path, run_id, conversation_id, binding) = fixture();
        let dispatch_id = issue_credential(
            &db,
            &binding,
            &conversation_id,
            "call-1",
            "read",
            "{\"path\":\"a.txt\"}",
            fox_engine_protocol::ActionClass::Read,
        );
        // Same tool, substituted path in the payload.
        let effect = effect_for("call-1", serde_json::json!({"path": "secrets.env"}));
        let mut calls = 0usize;
        let error = admit_and_execute_dispatch(
            &db,
            &binding,
            &effect,
            "read",
            &serde_json::json!({"input": {"path": "secrets.env"}}),
            |_| {
                calls += 1;
                (
                    Ok(serde_json::json!({"content":[]})),
                    fox_engine_protocol::ExecutionEvidence::Started,
                )
            },
        )
        .unwrap_err();
        assert!(
            error.contains("payload input differs from the durable identity"),
            "{error}"
        );
        assert_eq!(calls, 0, "no executor may run for a substituted payload");
        // The mismatch is refused BEFORE the claim, so no attempt fact exists.
        assert!(db.read_execution_attempt(&run_id, &dispatch_id).unwrap().is_none());
    }

    #[test]
    fn a_repeat_delivery_returns_the_real_receipt_and_never_replays() {
        let (db, _path, run_id, conversation_id, binding) = fixture();
        let dispatch_id = issue_credential(
            &db,
            &binding,
            &conversation_id,
            "call-1",
            "read",
            "{\"path\":\"a.txt\"}",
            fox_engine_protocol::ActionClass::Read,
        );
        let effect = effect_for("call-1", serde_json::json!({"path": "a.txt"}));
        let first = admit_and_execute_dispatch(
            &db,
            &binding,
            &effect,
            "read",
            &serde_json::json!({"input": {"path": "a.txt"}}),
            |_| (
                Ok(serde_json::json!({"content":[{"type":"text","text":"ok"}]})),
                fox_engine_protocol::ExecutionEvidence::Started,
            ),
        )
        .unwrap();
        assert_eq!(first["isError"].as_bool(), None, "first: {first}");
        let mut calls = 0usize;
        let repeat = admit_and_execute_dispatch(
            &db,
            &binding,
            &effect,
            "read",
            &serde_json::json!({"input": {"path": "a.txt"}}),
            |_| {
                calls += 1;
                (
                    Ok(serde_json::json!({"content":[]})),
                    fox_engine_protocol::ExecutionEvidence::Started,
                )
            },
        )
        .unwrap();
        assert_eq!(calls, 0);
        // The repeat reports the ACTUAL durable receipt, not a hard-coded one.
        assert_eq!(repeat["details"]["stage"], "launch_confirmed");
        assert_eq!(repeat["details"]["executionStarted"], true);
        assert_eq!(repeat["details"]["dispatchId"], dispatch_id);
    }

    #[test]
    fn safe_queries_stay_reachable_while_external_launches_stay_refused() {
        let (db, path, run_id, conversation_id, binding) = fixture();
        // A status query of an already-admitted job is safe management: it is
        // admitted and executed through the real Host order.
        let status_id = issue_credential(
            &db,
            &binding,
            &conversation_id,
            "call-1",
            "run_command",
            "{\"action\":\"status\",\"jobId\":\"j\"}",
            fox_engine_protocol::ActionClass::Manage,
        );
        db.with_connection(|c| {
            c.execute(
                "UPDATE kernel_tool_calls SET tool='run_command',
                     canonical_input_json='{\"action\":\"status\",\"jobId\":\"j\"}'
                  WHERE run_id=?1 AND tool_call_id='call-1'",
                rusqlite::params![run_id],
            )?;
            Ok(())
        })
        .unwrap();
        let status_effect = effect_for(
            "call-1",
            serde_json::json!({"action": "status", "jobId": "j"}),
        );
        let result = admit_and_execute_dispatch(
            &db,
            &binding,
            &status_effect,
            "run_command",
            &serde_json::json!({"input": {"action": "status", "jobId": "j"}}),
            |_| (
                Ok(serde_json::json!({"content":[{"type":"text","text":"status ok"}]})),
                fox_engine_protocol::ExecutionEvidence::NotStarted,
            ),
        )
        .unwrap();
        assert_eq!(
            result["isError"].as_bool(),
            None,
            "a safe query must not be refused by the backend gate: {result}"
        );

        // The same environment with an external start: refused, persisted, and
        // no job row is fabricated to carry it.
        let start_id = issue_credential(
            &db,
            &binding,
            &conversation_id,
            "call-2",
            "run_command",
            "{\"action\":\"start\",\"command\":\"echo hi\"}",
            fox_engine_protocol::ActionClass::Execute,
        );
        db.with_connection(|c| {
            c.execute(
                "INSERT INTO kernel_tool_calls(run_id, tool_call_id, batch_id, tool, source_order,
                     canonical_input_json, state, result_json, created_at, settled_at)
                 VALUES (?1,'call-2','b1','run_command',1,'{\"action\":\"start\",\"command\":\"echo hi\"}','pending',NULL,1,NULL)",
                rusqlite::params![run_id],
            )?;
            Ok(())
        })
        .unwrap();
        let start_effect = effect_for(
            "call-2",
            serde_json::json!({"action": "start", "command": "echo hi"}),
        );
        let mut calls = 0usize;
        let refused = admit_and_execute_dispatch(
            &db,
            &binding,
            &start_effect,
            "run_command",
            &serde_json::json!({"input": {"action": "start", "command": "echo hi"}}),
            |_| {
                calls += 1;
                (
                    Ok(serde_json::json!({"content":[]})),
                    fox_engine_protocol::ExecutionEvidence::Started,
                )
            },
        )
        .unwrap();
        assert_eq!(calls, 0);
        assert_eq!(refused["isError"].as_bool(), Some(true), "refused: {refused}");
        // An external launch is refused fail-closed. The requirement order puts
        // the missing live policy version first; the missing backend is the
        // next one and is reported by the same terminal refusal.
        let code = refused["details"]["error"]["code"].as_str().unwrap().to_owned();
        assert!(
            code == "policy_version_unavailable" || code == "sandbox_unavailable",
            "unexpected refusal code {code}"
        );
        assert_eq!(refused["details"]["executionStarted"], false);
        let jobs: i64 = db
            .with_connection(|c| {
                c.query_row(
                    "SELECT COUNT(*) FROM kernel_jobs WHERE run_id=?1",
                    rusqlite::params![run_id],
                    |r| r.get(0),
                )
            })
            .unwrap();
        assert_eq!(jobs, 0, "no job row may carry a refusal");
        drop(db);
        let reopened = Database::open(path).unwrap();
        let row = reopened
            .read_execution_attempt(&run_id, &start_id)
            .unwrap()
            .unwrap();
        // The refusal is durable and terminal (the launch never happened).
        assert_eq!(
            row.state,
            fox_engine_protocol::AttemptState::Refused {
                code: "policy_version_unavailable".to_owned()
            }
        );
        assert_eq!(row.start_confirmed, None);
        // The safe query from earlier still resolved to a real durable success:
        // it completed without fabricating an external process start.
        let row = reopened
            .read_execution_attempt(&run_id, &status_id)
            .unwrap()
            .unwrap();
        assert_eq!(row.start_confirmed, Some(false));
        assert_eq!(row.state, fox_engine_protocol::AttemptState::Completed);
        assert_eq!(row.terminal_state.as_deref(), Some("completed"));
        assert_eq!(row.error_code, None);
    }
}
