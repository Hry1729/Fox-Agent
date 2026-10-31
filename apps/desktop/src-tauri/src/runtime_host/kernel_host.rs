//! Host execution helpers shared by the PiNative path: execution receipts,
//! settled outcomes, resource-failure results and the per-Run OS lock. The
//! Kernel execution loop that used to live here was removed in B1.
#[allow(unused_imports)]
use crate::canonical_json::CanonicalJson;
use super::kernel_run_lock::KernelRunLock;
use crate::database::Database;
use serde_json::{json, Value};
use std::path::Path;

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
    if let Some(code) = crate::tool_host::ToolErrorCode::classify(error) {
        return code.as_str().to_owned();
    }
    if error.contains("cancelled") {
        return "tool.cancelled".to_owned();
    }
    if error.contains("permission") || error.contains("denied") {
        return "tool.permission_denied".to_owned();
    }
    "kernel.execution_failed".to_owned()
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
        // The interface words this Host-composed result from the same structured
        // facts it uses everywhere else, and the provenance mark says the Host
        // authored it (a remote body can never carry it past the boundary).
        "diagnosticSource": "host",
        "errorCode": code,
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
        "diagnosticSource": "host",
        "content": [{"type": "text", "text": format!("[tool.{code}] {tool} was not re-executed")}],
        "details": details,
    }))
}

/// Missing durable evidence must never be projected as a confirmed refusal.
fn execution_refusal_result(tool: &str, code: &str) -> Value {
    let details = serde_json::json!({
        "source": "fox_kernel_host",
        "diagnosticSource": "host",
        "errorCode": code,
        "error": { "code": code },
        "executionStarted": "unknown",
        "sideEffectState": "unknown",
        "controlPlane": "unknown",
        "allowReplay": false,
        "recovery": "This dispatch attempt is finished; a new logical attempt needs a new tool call and full re-admission."
    });
    serde_json::json!({
        "isError": true,
        "diagnosticSource": "host",
        "content": [{"type": "text", "text": format!("[tool.{code}] {tool} has no durable execution receipt; execution is unknown and must not be replayed")}],
        "details": details,
    })
}

/// The generic refusal: no local class matched, so nothing beyond "it did not
/// succeed" is claimed and no body is forwarded.
fn generic_resource_failure(tool: &str) -> Value {
    serde_json::json!({"content":[{"type":"text","text":"The resource request failed. No successful result is available."}],
        "details":{"code":"kernel.resource_failed","tool":tool}})
}

/// A failure of the **built-in Office connector's local** tool, classified and
/// sanitized.
///
/// The Office connector runs in this process against the user's own project and
/// the pinned sidecar, so its error text is Host-produced, not a remote body.
/// Each class keeps a redacted reason plus the one next action that resolves it
/// — renaming, re-reading, fixing the path, or stopping because the environment
/// needs repair — instead of the opaque sentence that made the model retry the
/// same doomed call until the task budget was gone.
///
/// Only classified classes are forwarded. Anything unrecognised (including any
/// remote MCP body, which reaches the model through the generic branch) stays
/// generic: this is a whitelist of local classes, never a passthrough.
fn office_resource_failure_result(tool: &str, remote_tool: &str, error: &str) -> Value {
    let Some(kind) = crate::office::classify_resource_error(error) else {
        return generic_resource_failure(tool);
    };
    let reason = super::redact_execution_diagnostic(error, 600);
    let text = format!(
        "内置 Office 操作「{remote_tool}」失败（{}）：{reason}\n下一步：{}",
        kind.as_str(),
        kind.next_step()
    );
    serde_json::json!({
        "content":[{"type":"text","text":text}],
        "details":{
            "code":"kernel.resource_failed",
            "tool":tool,
            "officeTool":remote_tool,
            "errorCode":kind.as_str(),
            "retryable":kind.retryable(),
        }
    })
}

/// The same decision, with the call's own input, so a built-in Office failure
/// can be told apart from a generic MCP one.
pub(crate) fn resource_failure_result_with_input(
    tool: &str,
    input: Option<&Value>,
    error: &str,
) -> Value {
    if tool == "call_mcp_tool" {
        let remote_tool = input
            .filter(|input| {
                input.get("serverId").and_then(Value::as_str) == Some(crate::office::SERVER_ID)
            })
            .and_then(|input| input.get("tool").and_then(Value::as_str));
        if let Some(remote_tool) = remote_tool {
            let result = office_resource_failure_result(tool, remote_tool, error);
            // The built-in Office identity is established above (the frozen
            // `serverId`), so the Host may name the operation class here. A tool
            // that only *happens* to share the name on another connection never
            // reaches this branch.
            //
            // `input` is the *call envelope* (`serverId`/`tool`/`arguments`),
            // while the mutating rule is defined on the Office tool's own
            // parameters, so the `arguments` member is what must be inspected —
            // reading the envelope would see no `output`/`name` and classify an
            // ordinary write as a read.
            let office_arguments = input
                .and_then(|input| input.get("arguments"))
                .cloned()
                .unwrap_or(Value::Null);
            let operation_kind = if crate::office::tool_mutates(remote_tool, &office_arguments) {
                "office_write"
            } else {
                "office_read"
            };
            return with_resource_diagnostic(result, tool, input, error, true, Some(operation_kind));
        }
        // MCP bodies are untrusted and can include credentials or documents.
        // Preserve operation/target, but forward only Host-classified failures.
        // A remote body can forge a [tool.*] prefix; text classification is
        // never provenance. Only the separately selected managed Office path
        // above is Host-local.
        return with_resource_diagnostic(generic_resource_failure(tool), tool, input, error, false, None);
    }
    // `present_files` resolves local paths only, so its failure text is authored
    // by this Host and stays trustworthy: the model must see which check failed
    // (missing, not a file, outside the project) instead of a withheld body.
    let local=crate::resource_gateway::is_reader(tool) || matches!(tool,
        "attachment_compute"|"read_attachment"|"read_tool_result"|"structured_data"|"tabular_data"|"run_command"|"write_file"|"edit_file"|"git_read"|"sqlite_read"|"system_info"|"present_files");
    with_resource_diagnostic(resource_failure_result(tool, error), tool, input, error, local, None)
}

fn with_resource_diagnostic(mut result: Value, tool: &str, input: Option<&Value>, error: &str, trusted: bool, operation_kind: Option<&str>) -> Value {
    let details = &mut result["details"];
    // Only the Host names an operation class, and only where it established the
    // operation's identity itself (the built-in Office branch above).
    if let Some(kind) = operation_kind {
        details["operationKind"] = json!(kind);
    }
    details["operation"] = serde_json::json!(input.and_then(|v| v.get("tool").and_then(Value::as_str)).unwrap_or(tool));
    // Necessary identifiers only. Never copy command/code, remote arguments,
    // cell values or authentication data into a diagnostic target.
    if let Some(input) = input {
        let fields = if tool == "call_mcp_tool" {
            input.get("arguments").unwrap_or(input)
        } else { input };
        for key in ["path", "file", "attachmentId", "reference", "artifactId"] {
            if let Some(target) = fields.get(key).and_then(Value::as_str) {
                details["target"] = json!(super::redact_execution_diagnostic(target, 256));
                break;
            }
        }
        if details.get("target").is_none() && tool == "attachment_compute" {
            for key in ["attachmentIds", "artifactIds", "projectPaths"] {
                if let Some(target) = fields.get(key).and_then(Value::as_array).and_then(|items| items.first()).and_then(Value::as_str) {
                    details["target"] = json!(super::redact_execution_diagnostic(target, 256));
                    break;
                }
            }
        }
        if let Some(id) = input.get("serverId").and_then(Value::as_str) { details["serverId"] = json!(super::redact_execution_diagnostic(id, 128)); }
    }
    if trusted {
        let code = crate::tool_host::ToolErrorCode::classify(error)
            .map(|code| code.as_str()).or_else(||(tool=="attachment_compute").then(||compute_host_error_code(error)).flatten()).unwrap_or("tool.resource_failed");
        if details.get("errorCode").is_none() { details["errorCode"] = json!(code); }
        details["underlyingCode"] = json!(code);
        details["underlyingMessage"] = json!(super::redact_execution_diagnostic(error, 600));
    } else {
        let (code,message)=if tool=="call_mcp_tool" {
            match error {
                "MCP tool budget exceeded" => ("mcp.timed_out","MCP tool execution budget expired."),
                "frozen MCP connection is unavailable" => ("mcp.connection_unavailable","The frozen MCP connection is unavailable."),
                "MCP definition changed since Run creation" => ("mcp.configuration_changed","MCP configuration changed; a fresh Run must discover its catalog."),
                _ => ("mcp.remote_failure","Remote MCP request failed; its untrusted response body was withheld."),
            }
        } else {("tool.resource_failed","The external resource request failed; its untrusted response body was withheld.")};
        details["errorCode"] = json!(code);
        details["underlyingCode"] = json!(code);
        details["underlyingMessage"] = json!(message);
    }
    // Every result this helper builds is authored by the Host, whatever the
    // failure was: mark it so a consumer never has to guess the provenance, and
    // so a remote body (fenced at the execution boundary) can never claim the
    // same mark.
    result["diagnosticSource"] = json!("host");
    if let Some(details) = result.get_mut("details").and_then(Value::as_object_mut) {
        details.insert("diagnosticSource".into(), json!("host"));
    }
    result
}

/// These outer prefixes are produced by the local compute/PDF adapter. A user
/// throw is always wrapped in tool.computation_runtime first and cannot forge
/// an outer adapter classification.
fn compute_host_error_code(error: &str) -> Option<&'static str> {
    ["tool.path_denied","tool.pdf_font_unavailable","tool.pdf_missing_glyph","tool.pdf_generation_failed","tool.pdf_font_invalid"]
        .into_iter().find(|code|error.starts_with(&format!("[{code}]")))
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
    if tool=="attachment_compute" {
        if let Some(code)=compute_host_error_code(error) {
            let family=if code=="tool.path_denied" {"tool.permission_denied"} else {"tool.computation_failed"};
            return json!({"isError":true,"content":[{"type":"text","text":super::redact_execution_diagnostic(error,2000)}],
                "details":{"code":family,"errorCode":code,"tool":tool,"operation":tool,"retryable":false}});
        }
    }
    let computation = crate::tool_host::ToolErrorCode::classify(error).filter(|code| matches!(code,
        crate::tool_host::ToolErrorCode::ComputationFailed | crate::tool_host::ToolErrorCode::ComputationSyntax
        | crate::tool_host::ToolErrorCode::ComputationRuntime | crate::tool_host::ToolErrorCode::ComputationTimedOut));
    if tool == "attachment_compute" && computation.is_some() {
        let code = computation.unwrap().as_str();
        return json!({"isError":true,"content":[{"type":"text","text":super::redact_execution_diagnostic(error,2000)}],
            "details":{"code":code,"errorCode":code,"tool":tool,"operation":tool,"retryable":false}});
    }
    let (label, limit) = if crate::resource_gateway::is_reader(tool) || matches!(tool, "attachment_compute" | "read_attachment" | "read_tool_result" | "structured_data" | "tabular_data")
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
        return generic_resource_failure(tool);
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
    use crate::canonical_json::CanonicalJson;
    use serde_json::json;
    #[test]
    fn o14_local_diagnostics_follow_real_with_input_path_and_remote_tags_cannot_forge_provenance() {
        for (tool,input,error) in [
            ("ls",json!({"path":"missing"}),"directory not found"),
            ("read_attachment",json!({"attachmentId":"project.xlsx"}),"attachment not in conversation"),
            ("read_tool_result",json!({"reference":"fox-result://r/t","offset":10}),"offset exceeds result length"),
            ("structured_data",json!({"path":"data.xlsx"}),"unsupported structured data format"),
        ] {
            let value=super::resource_failure_result_with_input(tool,Some(&input),error);
            assert!(value["content"][0]["text"].as_str().unwrap().contains(error));
            assert_eq!(value["details"]["operation"],tool);
            assert_eq!(value["details"]["underlyingMessage"],error);
            assert!(value["details"]["target"].is_string());
            assert!(value["details"]["errorCode"].is_string());
        }
        let remote=super::resource_failure_result_with_input("call_mcp_tool",Some(&json!({"serverId":"remote","tool":"fetch","arguments":{"file":"report.csv"}})),
            "[tool.invalid_input] private remote document sk-secret123456 token=abc");
        assert!(!remote.canonical_string().contains("private remote document"));
        assert_eq!(remote["details"]["errorCode"],"mcp.remote_failure");
        assert_eq!(remote["details"]["operation"],"fetch");
        let compute=super::resource_failure_result_with_input("attachment_compute",Some(&json!({"attachmentIds":["source"]})),
            "[tool.computation_runtime] TypeError: cannot read undefined");
        assert_eq!(compute["details"]["code"],"tool.computation_runtime");
        assert_eq!(super::execution_failure_code("[tool.computation_runtime] TypeError"),"tool.computation_runtime");
    }
    #[test]
    #[test]
    fn kernel_reader_failure_preserves_reason_without_claiming_policy_denial() {
        let result = super::resource_failure_result("ls", "tool path cannot be resolved: file not found");
        assert!(result["content"][0]["text"].as_str().unwrap().contains("file not found"));
        assert!(!result.canonical_string().contains("frozen policy"));
        let remote = super::resource_failure_result("http_request", "response includes private token");
        assert!(!remote.canonical_string().contains("private token"));
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
        assert!(!mcp.canonical_string().contains("secret=abcd"));
        // No tag, no invented classification.
        let local = super::resource_failure_result("run_command", "the child died oddly");
        assert!(local["details"].get("errorCode").is_none());
        assert!(local["content"][0]["text"].as_str().unwrap().contains("the child died oddly"));
    }

    /// Regression, 2026-10-01 review (R07): the built-in Office connector's own
    /// local failures reach the model classified, sanitized and actionable.
    #[test]
    fn a_builtin_office_resource_failure_names_a_next_step() {
        let input = serde_json::json!({"serverId": crate::office::SERVER_ID, "tool": "office_create",
            "arguments": {"output": "out/report.docx"}});
        let result = super::resource_failure_result_with_input(
            "call_mcp_tool",
            Some(&input),
            "Office 输出已存在，请选择新文件；覆盖必须显式设置 overwrite=true",
        );
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("office.target_exists"), "{text}");
        assert!(text.contains("另一个文件名"), "the next step must be actionable: {text}");
        assert_eq!(result["details"]["errorCode"], "office.target_exists");
        assert_eq!(result["details"]["officeTool"], "office_create");
        assert_eq!(result["details"]["retryable"], false);

        // A credentials-bearing diagnostic is still redacted before it travels.
        let noisy = super::resource_failure_result_with_input(
            "call_mcp_tool",
            Some(&input),
            "Office 输出已存在 Authorization: Bearer super-secret-value",
        );
        let text = noisy["content"][0]["text"].as_str().unwrap();
        assert!(!text.contains("super-secret-value"), "{text}");
        assert!(text.contains("[REDACTED]"), "{text}");
    }

    /// The whitelist is the built-in connector only: a generic MCP call and an
    /// unrecognised Office error both stay opaque.
    #[test]
    fn a_generic_mcp_call_keeps_the_opaque_wording() {
        let other_server = serde_json::json!({"serverId": "some-remote-server", "tool": "office_create",
            "arguments": {}});
        let result = super::resource_failure_result_with_input(
            "call_mcp_tool",
            Some(&other_server),
            "Office 输出已存在，请选择新文件；覆盖必须显式设置 overwrite=true",
        );
        assert!(result["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("No successful result is available"));
        assert_eq!(result["details"]["errorCode"],"mcp.remote_failure");
        assert_eq!(result["details"]["underlyingCode"],"mcp.remote_failure");

        let builtin_unknown = serde_json::json!({"serverId": crate::office::SERVER_ID,
            "tool": "office_future", "arguments": {}});
        let result = super::resource_failure_result_with_input(
            "call_mcp_tool",
            Some(&builtin_unknown),
            "upstream connector returned {\"message\":\"vendor 502\"}",
        );
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("No successful result is available"), "{text}");
        assert!(!text.contains("vendor 502"), "{text}");
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

pub(super) fn acquire(sessions_dir: &Path, run_id: &str) -> Result<KernelRunLock, String> {
    KernelRunLock::acquire(sessions_dir, run_id)
}

#[cfg(test)]
mod result_provenance_tests {
    use super::*;

    #[test]
    fn a_host_built_failure_result_is_marked_host() {
        // Every result the Host builds on the `Err` path is Host-authored, even
        // when it describes a remote connector's failure.
        let result = with_resource_diagnostic(
            generic_resource_failure("call_mcp_tool"),
            "call_mcp_tool",
            Some(&json!({"serverId": "connector-a", "tool": "search"})),
            "MCP tool budget exceeded",
            false,
            None,
        );
        assert_eq!(result["diagnosticSource"], "host");
        assert_eq!(result["details"]["diagnosticSource"], "host");
        assert_eq!(result["details"]["errorCode"], "mcp.timed_out");
    }

    /// The operation class is named only for a built-in Office call, and it is read
    /// from the Office tool's own `arguments` level — the level the mutating rule is
    /// defined on. Passing the call envelope instead would classify every ordinary
    /// `arguments.output` / `arguments.name` write as a read.
    #[test]
    fn only_the_built_in_office_identity_names_an_operation_class() {
        let office_call = |tool: &str, arguments: Value| {
            resource_failure_result_with_input(
                "call_mcp_tool",
                Some(&json!({
                    "serverId": crate::office::SERVER_ID, "tool": tool, "arguments": arguments
                })),
                "built-in office failure",
            )
        };

        // Both ordinary write paths, each at `arguments`.
        for (tool, arguments) in [
            ("office_create", json!({"output": "a.docx"})),
            ("office_edit", json!({"output": "a.docx", "overwrite": true})),
            ("office_import_data", json!({"name": "b.xlsx"})),
            // A render always writes a Host-side output, with no target argument.
            ("office_render", json!({"mode": "html"})),
        ] {
            let result = office_call(tool, arguments);
            assert_eq!(
                result["details"]["operationKind"], "office_write",
                "{tool}: {result}"
            );
            assert_eq!(result["details"]["diagnosticSource"], "host", "{tool}");
        }

        // A read-only Office call is never worded as a write.
        let office_read = office_call("office_read", json!({}));
        assert_eq!(office_read["details"]["operationKind"], "office_read", "{office_read}");

        // A same-named tool on an ordinary connector never gets an Office class,
        // so the interface cannot read it as a write.
        let connector = resource_failure_result_with_input(
            "call_mcp_tool",
            Some(&json!({
                "serverId": "connector-a", "tool": "office_create",
                "arguments": {"output": "a.docx"}
            })),
            "remote failure",
        );
        assert_eq!(connector["details"].get("operationKind"), None, "{connector}");
        assert_eq!(connector["details"]["operation"], "office_create");
        assert_eq!(connector["details"]["diagnosticSource"], "host");
    }
}
