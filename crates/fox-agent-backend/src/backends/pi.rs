//! pi backend specifics (plan 6.7, 7.3). Owned by the pi integration branch.
//!
//! PiBackend reaches pi through `pi-acp` (MIT, MVP adapter by svkozak), which
//! spawns `pi --mode rpc --no-themes` and bridges pi RPC to ACP v1. Everything
//! here is measured against pi-acp 0.0.34 + `@earendil-works/pi-coding-agent`
//! 1.4.0 (the pi Fox locks in services/agent-runtime). Evidence and the
//! measurement steps are in `docs/pi.md`; the real-binary checks are in
//! `tests/pi_backend.rs` (gated by `FOX_TEST_REAL_ACP=1`).
//!
//! Two launch shapes are described:
//!
//! * **Stock** (`launch_spec(AgentBackend::Pi)` as it is today): pi-acp runs
//!   whatever `pi` is first on PATH, with pi's own extension discovery. Tools
//!   run with **no** Host approval: pi-acp maps pi tool execution only to
//!   `tool_call` / `tool_call_update` notifications, never to
//!   `session/request_permission`. See [`capabilities`].
//! * **Gated** (prototype, `fixtures/pi/`): `PI_ACP_PI_COMMAND` points at
//!   `pi-gated.sh`, which runs the pinned pi with `--no-extensions -e
//!   fox-approval-gate.ts`. The extension holds every tool call in pi's
//!   `tool_call` hook and asks through `ctx.ui.confirm`, which pi-acp turns into
//!   `session/request_permission`. See [`capabilities_with_approval_gate`].
//!   Needs the Host to inject `PI_ACP_PI_COMMAND` / `FOX_PI_BIN` (not in the
//!   launch.rs allowlist yet: a change request to the shared layer, plan 7.1).

use crate::port::{
    ApprovalGate, CancelReach, CapabilityDeclaration, Enforcement, SessionCapabilities, ToolCategory, Visibility,
};
use serde_json::Value;

/// Adapter package and version Fox validates (same pin as `launch::PI_ACP_VERSION`).
pub const PI_ACP_PACKAGE: &str = "pi-acp";
pub const PI_ACP_VERSION: &str = crate::launch::PI_ACP_VERSION;
/// The pi binary the measurements were taken with (Fox's existing lock).
pub const PI_VERSION: &str = "1.4.0";
/// pi-acp README: "`pi` v0.81.0+ installed and available on your PATH".
pub const PI_MIN_VERSION_FOR_PI_ACP: &str = "0.81.0";

/// Native permission options pi-acp offers for an extension `confirm` dialog
/// (pi-acp `CONFIRM_PERMISSION_OPTIONS`). Only these exist: no "always", no
/// session-wide grant (plan 6.3 rule 2).
pub const OPTION_ALLOW_ONCE: &str = "yes";
pub const OPTION_REJECT_ONCE: &str = "no";

/// Title prefix and message marker written by `fixtures/pi/fox-approval-gate.ts`.
pub const APPROVAL_TITLE_PREFIX: &str = "Fox approval: ";
pub const APPROVAL_MARKER: &str = "tool-approval";

fn session_capabilities(initialize_response: &Value) -> SessionCapabilities {
    let caps = initialize_response.get("agentCapabilities").unwrap_or(&Value::Null);
    let session_caps = caps.get("sessionCapabilities").unwrap_or(&Value::Null);
    let present = |value: &Value, key: &str| match value.get(key) {
        Some(Value::Bool(flag)) => *flag,
        Some(Value::Null) | None => false,
        Some(_) => true,
    };
    SessionCapabilities {
        // pi-acp 0.0.34 declares `loadSession: true`; loadSession() restarts pi
        // with `--session <file>` and replays user/assistant messages from
        // pi's `get_messages`. Measured: declared in initialize. Replay itself
        // is source-read only (not exercised with a real model), see docs.
        load: present(caps, "loadSession"),
        // Not declared by pi-acp 0.0.34 (no session/resume handler).
        resume: present(session_caps, "resume"),
        // Not declared (pi-acp declares `delete`, which the port does not
        // model, but no `close`). Fox closes by killing the process.
        close: present(session_caps, "close"),
        // Declared as `list: {}` (pi-acp comment: UNSTABLE capability).
        list: present(session_caps, "list"),
        // ACP has no steer method and pi-acp exposes none. A prompt sent while
        // a turn runs is queued inside pi-acp as a NEW turn ("Queued message
        // (position n)"), not delivered as pi `steer`. `/steering` only sets pi's
        // steering mode. So steer is unsupported over ACP; the Host must queue
        // or cancel-and-resend and say so (plan 6.7).
        steer: false,
    }
}

/// Capabilities of the **stock** pi-acp launch, from its `initialize` response
/// plus what source reading and measurement established. Nothing is claimed
/// without evidence; unmeasured items stay `Unverified`.
pub fn capabilities(initialize_response: &Value) -> CapabilityDeclaration {
    CapabilityDeclaration {
        approval_coverage: vec![
            // pi-acp 0.0.34 handlePiEvent: `tool_execution_start/update/end`
            // become ACP tool_call / tool_call_update only. The only
            // `conn.requestPermission` call sites are extension UI `confirm` /
            // `select` (requestExtensionPermission). pi itself has no approval
            // step in RPC mode. => built-in read / write / edit / bash run
            // without Host approval.
            (ToolCategory::FileRead, ApprovalGate::None),
            (ToolCategory::FileWrite, ApprovalGate::None),
            (ToolCategory::Shell, ApprovalGate::None),
            // pi has no network tool; network happens inside bash (or an
            // extension tool) and is never separately gated.
            (ToolCategory::Network, ApprovalGate::None),
            // MCP servers passed in session/new are stored but not wired to pi
            // (pi-acp README "Limitations"; `mcpCapabilities: {http:false,
            // sse:false}`). Host MCP business tools are unreachable, so there is
            // nothing to gate yet: unverified, never shown as gated.
            (ToolCategory::McpBusinessTool, ApprovalGate::Unverified),
        ],
        // pi docs/security.md: no built-in sandbox; "runs with the permissions
        // of the user account"; project trust "is not a sandbox". pi-acp has no
        // fs/* or terminal/* delegation. Nothing enforces a boundary.
        sandbox_filesystem: Enforcement::Absent,
        sandbox_network: Enforcement::Absent,
        // session/cancel -> pi-acp `abort` RPC; session/prompt then resolves
        // with stopReason `cancelled` only after pi emits `agent_settled`.
        // Measured end to end with the gated launch (tests/pi_backend.rs
        // `real_gated_*`): (a) cancel while a tool waits for approval -> the
        // Host answers the pending permission `cancelled` -> stopReason
        // `cancelled`, tool not executed; (b) cancel while an approved bash
        // command runs -> stopReason `cancelled` and the command's process is
        // gone (pi abort kills the bash process tree). Not measured with a real
        // remote model mid-stream.
        cancel_turn: CancelReach::TurnConfirmed,
        // pi's bash tool spawns the shell with `detached: true`
        // (dist/core/tools/bash.js), i.e. in its OWN process group, so a group
        // kill alone does not reach it. Fox's stop (acp.rs `terminate`) now
        // also snapshots every descendant before the stop, SIGKILLs the ones
        // that left the group (pid + start time checked) and reports
        // `StopConfirmation::Confirmed` only when the group and every tracked
        // descendant are gone (crate::proctree). Measured on Linux 2026-10-19
        // with the gated launch (tests/pi_backend.rs): (a) normal close ->
        // pi's SIGTERM handler ends the detached bash itself, confirmed;
        // (b) pi SIGSTOPped before the close (cannot clean up) -> the group
        // kill misses bash's subtree, descendant tracking kills it, confirmed.
        // macOS (ps-based snapshot) and Windows (Job Object; the bash of Git
        // Bash stays in the job unless it breaks away) are not yet measured.
        cancel_process_tree: true,
        // ACP v1 has no acknowledgement for a permission response, and pi's
        // extension_ui_response has none either.
        approval_answer_acknowledged: false,
        // Tool start / update / end are mapped to tool_call and
        // tool_call_update (measured). Caveat (measured): pi emits
        // tool_execution_start BEFORE the tool_call hook, so pi-acp reports the
        // call `in_progress` while it is still waiting for approval; the Host
        // must show "awaiting approval" for a call with a pending gate request.
        // What a bash command does inside its own subprocess is not observable
        // (only its stdout/stderr text).
        observability: Visibility::ToolLifecycle,
        session: session_capabilities(initialize_response),
    }
}

/// Capabilities of the **gated** prototype launch (`fixtures/pi/pi-gated.sh`
/// loading only `fox-approval-gate.ts`). Differs from [`capabilities`] only in
/// approval coverage, and only where measured with real pi + pi-acp.
pub fn capabilities_with_approval_gate(initialize_response: &Value) -> CapabilityDeclaration {
    let mut declared = capabilities(initialize_response);
    declared.approval_coverage = vec![
        // The gate runs in pi's `tool_call` hook for every tool (pi
        // docs/extensions.md: fired after tool_execution_start, before execute,
        // can block). Measured with real pi-acp 0.0.34 + pi 0.84.2
        // (tests/pi_backend.rs real_gated_*): one request_permission per call;
        // `read` rejected -> tool_call_update failed, no file content returned;
        // `write` -> file absent until allowed. `edit`, `grep`, `find`, `ls`
        // pass the same hook but were not exercised one by one.
        (ToolCategory::FileRead, ApprovalGate::PerCall),
        (ToolCategory::FileWrite, ApprovalGate::PerCall),
        // Measured for bash: allow -> runs; reject -> blocked, no side effect;
        // session/cancel while pending -> blocked, turn ends `cancelled`.
        // Gate covers starting the command only; what runs inside it is not
        // gated or observed.
        (ToolCategory::Shell, ApprovalGate::PerCall),
        // Network inside an approved bash command is not gated.
        (ToolCategory::Network, ApprovalGate::None),
        (ToolCategory::McpBusinessTool, ApprovalGate::Unverified),
    ];
    declared
}

/// Refuse to run against an adapter other than the pinned one (plan 7.3
/// "validate at start, refuse when incompatible").
pub fn check_initialize(initialize_response: &Value) -> Result<(), String> {
    let info = initialize_response.get("agentInfo").unwrap_or(&Value::Null);
    let name = info.get("name").and_then(Value::as_str).unwrap_or("");
    let version = info.get("version").and_then(Value::as_str).unwrap_or("");
    if name != PI_ACP_PACKAGE {
        return Err(format!("expected agent {PI_ACP_PACKAGE}, got {name:?}"));
    }
    if version != PI_ACP_VERSION {
        return Err(format!("pi-acp {version} is not the validated version {PI_ACP_VERSION}"));
    }
    match initialize_response.get("protocolVersion").and_then(Value::as_u64) {
        Some(1) => Ok(()),
        other => Err(format!("unsupported ACP protocol version {other:?}")),
    }
}

/// A tool approval raised by `fox-approval-gate.ts`, decoded from the ACP
/// `session/request_permission` params.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolApproval {
    /// pi's own tool call id; matches the `toolCallId` of the `tool_call`
    /// update pi-acp emitted for the same call.
    pub tool_call_id: String,
    pub tool_name: String,
    pub input: Value,
    /// pi-acp's id for the dialog (`pi-ui-<uuid>`), not the tool call id.
    pub dialog_tool_call_id: String,
    /// The working directory pi reported for the call (`ctx.cwd`).
    pub cwd: Option<String>,
}

/// Decode a gate approval. Returns `None` for any other permission request
/// (another extension's `select` / `confirm` dialog): the Host must not show
/// those as tool approvals.
pub fn parse_tool_approval(request: &Value) -> Option<ToolApproval> {
    let tool_call = request.get("toolCall")?;
    let raw = tool_call.get("rawInput")?;
    if raw.get("method").and_then(Value::as_str) != Some("confirm") {
        return None;
    }
    let title = tool_call.get("title").and_then(Value::as_str)?;
    if !title.starts_with(APPROVAL_TITLE_PREFIX) {
        return None;
    }
    let message: Value = serde_json::from_str(raw.get("message").and_then(Value::as_str)?).ok()?;
    if message.get("fox").and_then(Value::as_str) != Some(APPROVAL_MARKER) {
        return None;
    }
    let options: Vec<&str> = request
        .get("options")
        .and_then(Value::as_array)
        .map(|options| options.iter().filter_map(|option| option.get("optionId").and_then(Value::as_str)).collect())
        .unwrap_or_default();
    if options != [OPTION_ALLOW_ONCE, OPTION_REJECT_ONCE] {
        return None;
    }
    Some(ToolApproval {
        tool_call_id: message.get("toolCallId").and_then(Value::as_str).unwrap_or("").to_owned(),
        tool_name: message.get("toolName").and_then(Value::as_str).unwrap_or("").to_owned(),
        input: message.get("input").cloned().unwrap_or(Value::Null),
        dialog_tool_call_id: tool_call.get("toolCallId").and_then(Value::as_str).unwrap_or("").to_owned(),
        cwd: message.get("cwd").and_then(Value::as_str).filter(|cwd| !cwd.is_empty()).map(str::to_owned),
    })
}

/// Classify a failed `session/new` / `session/prompt` from pi-acp so the Host
/// shows the real cause instead of a generic failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PiFailure {
    /// JSON-RPC -32000 "Authentication required": pi has no usable model
    /// credentials (measured on session/new with an empty HOME).
    AuthRequired,
    /// pi executable not found (pi-acp spawn ENOENT; measured with no pi on PATH).
    PiNotFound,
    Other,
}

pub fn classify_failure(message: &str) -> PiFailure {
    let lower = message.to_ascii_lowercase();
    if lower.contains("authentication required") || lower.contains("-32000") {
        PiFailure::AuthRequired
    } else if lower.contains("executable not found") || lower.contains("enoent") {
        PiFailure::PiNotFound
    } else {
        PiFailure::Other
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_initialize_declares_no_session_features() {
        let declared = capabilities(&serde_json::json!({"protocolVersion": 1}));
        assert_eq!(declared.session, SessionCapabilities::default());
        assert!(!declared.ui_claims_per_call_approval(ToolCategory::Shell));
    }
}
