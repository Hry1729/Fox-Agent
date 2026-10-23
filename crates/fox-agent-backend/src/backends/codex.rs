//! codex backend specifics. Owned by the codex integration branch.
//!
//! CodexBackend = `@agentclientprotocol/codex-acp` (launch spec in
//! [`crate::launch`]) spoken to over ACP v1 by the shared client. This file only
//! holds what is specific to codex-acp: the measured capability declaration,
//! the mapping of the Host policy onto codex-acp's session modes, the
//! initialize check and the `_session/steering` extension. No agent loop here.
//!
//! Evidence (details and raw recordings in `docs/codex.md`):
//! * [M1] real `initialize` / `session/new` against codex-acp 2.2.2 (bundles
//!   `@openai/codex` 0.160.1) on Linux aarch64, 2026-10-19.
//! * [M2] the bundled `codex sandbox` run directly on the same host: writes
//!   outside the writable roots and outbound TCP fail, reads anywhere succeed.
//! * [S]  codex-acp 2.2.2 `dist/index.js` (AgentMode, CodexApprovalHandler,
//!   cancel/interruptSessionTurn, SESSION_STEERING_METHOD, createSessionConfig).
//! No model turn has been run (no OpenAI credentials), so nothing that needs a
//! live turn (approval round trip, cancel confirmation, sandbox through a turn)
//! is claimed as measured.

use crate::launch::CODEX_ACP_VERSION;
use crate::port::{
    ApprovalGate, CancelReach, CapabilityDeclaration, Enforcement, PortError, SessionCapabilities,
    ToolCategory, Visibility,
};
use serde_json::{json, Value};
use std::path::PathBuf;

/// `agentInfo.name` codex-acp reports in `initialize` [M1].
pub const AGENT_NAME: &str = "@agentclientprotocol/codex-acp";
/// The Codex CLI version the pinned adapter resolved to when measured [M1].
/// codex-acp depends on `@openai/codex@^0.160.1`, so npx may resolve a newer
/// CLI later; the Host should pin it (CODEX_PATH) or re-measure.
pub const MEASURED_CODEX_CLI_VERSION: &str = "0.160.1";
/// ACP extension method for mid-turn steering [S].
pub const STEERING_METHOD: &str = "_session/steering";
/// Name the Host gives its business-tool MCP server. codex-acp silently drops a
/// client MCP server whose name already exists in the user's Codex config
/// (`skippedMcpServers`) [S], so the name must be one users are unlikely to use
/// and the Host must check `skippedMcpServers` is empty.
pub const HOST_MCP_SERVER_NAME: &str = "fox_host_business_tools";

/// codex-acp session modes (`AgentMode` in [S]; listed in `session/new` [M1]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodexMode {
    /// approvalPolicy `on-request`, reviewer `user`, sandbox readOnly, network off.
    ReadOnly,
    /// approvalPolicy `on-request`, reviewer `user`, sandbox workspaceWrite
    /// (cwd + additional directories writable), network off.
    WorkspaceWrite,
    /// codex-acp default. Same sandbox as WorkspaceWrite but approvals are
    /// answered by Codex's automatic reviewer, not the Host. Never used by Fox.
    AutoReview,
    /// approvalPolicy `never`, sandbox dangerFullAccess. Never used by Fox.
    FullAccess,
}

impl CodexMode {
    pub fn id(self) -> &'static str {
        match self {
            Self::ReadOnly => "read-only",
            Self::WorkspaceWrite => "workspace-write",
            Self::AutoReview => "agent",
            Self::FullAccess => "agent-full-access",
        }
    }

    pub fn parse(id: &str) -> Option<Self> {
        [Self::ReadOnly, Self::WorkspaceWrite, Self::AutoReview, Self::FullAccess]
            .into_iter()
            .find(|mode| mode.id() == id)
    }

    /// Modes in which every approval Codex raises reaches the Host as
    /// `session/request_permission`.
    pub fn host_answers_approvals(self) -> bool {
        matches!(self, Self::ReadOnly | Self::WorkspaceWrite)
    }
}

/// Host platform; sandbox evidence differs per OS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Linux,
    MacOs,
    Windows,
    Other,
}

impl Platform {
    pub fn current() -> Self {
        if cfg!(target_os = "linux") {
            Self::Linux
        } else if cfg!(target_os = "macos") {
            Self::MacOs
        } else if cfg!(windows) {
            Self::Windows
        } else {
            Self::Other
        }
    }
}

/// The part of the Host's layer-1 policy (plan 6.5) codex-acp can express.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostPolicy {
    /// Session authorization root; becomes the ACP `cwd`.
    pub cwd: PathBuf,
    /// The root may be written.
    pub writable: bool,
    /// Local network access for commands.
    pub network: bool,
}

/// Map the Host policy onto a codex-acp mode. Only the two modes whose
/// approvals go to the Host are ever chosen. codex-acp has no mode with
/// network on and the sandbox still in place (only `agent-full-access` turns
/// network on, and it removes the sandbox and all approvals), so a policy that
/// wants network is refused rather than widened.
pub fn mode_for_policy(policy: &HostPolicy) -> Result<CodexMode, PortError> {
    if policy.network {
        return Err(PortError::Unsupported(
            "codex-acp offers network access only together with full disk access and no approvals",
        ));
    }
    Ok(if policy.writable {
        CodexMode::WorkspaceWrite
    } else {
        CodexMode::ReadOnly
    })
}

/// Session variables the Host injects into the codex-acp process on top of
/// the allowlist (plan 6.5 "Host 显式注入的会话变量"):
/// * `INITIAL_AGENT_MODE`: without it codex-acp starts in `agent` (automatic
///   reviewer) [M1: `currentModeId":"agent"`]; with it, `read-only` [M1].
/// * `NO_BROWSER=1`: hides the browser ChatGPT login (README).
/// * `CODEX_CONFIG`: merged into the Codex session config [S]. Turns off the
///   model's server-side web search, which the local network sandbox does not
///   cover. Requested only; not yet measured to take effect.
///
/// Authentication is deliberately absent: codex-acp needs an `authenticate`
/// call (or `DEFAULT_AUTH_REQUEST`) even when `OPENAI_API_KEY` is set [M1:
/// fresh CODEX_HOME + key -> session/new "Authentication required"], and
/// `authenticate {methodId:"api-key"}` writes the key into
/// `$CODEX_HOME/auth.json` [M1]. That is a copy of a credential into the native
/// store (plan 6.5 forbids it), so the Host relies on the user's own Codex
/// login and only reads `_meta.authStatus` / `_auth/status_update`.
///
/// The shared launch path does not inject extra variables yet; see the
/// change request in `docs/codex.md`.
pub fn session_env(policy: &HostPolicy) -> Result<Vec<(String, String)>, PortError> {
    Ok(session_env_for_mode(mode_for_policy(policy)?))
}

/// [`session_env`] for a mode the user picked in the per-agent permission
/// list (`workspace-write` / `agent` / `agent-full-access`). The mode is
/// re-applied with `session/set_mode` after the session is opened or loaded
/// and whenever the user changes it mid-session.
pub fn session_env_for_mode(mode: CodexMode) -> Vec<(String, String)> {
    vec![
        ("INITIAL_AGENT_MODE".into(), mode.id().into()),
        ("NO_BROWSER".into(), "1".into()),
        ("CODEX_CONFIG".into(), json!({ "web_search": "disabled" }).to_string()),
    ]
}

/// `session/set_mode` params to (re)apply the Host's mode, e.g. when a
/// tightened policy must reach an existing session (plan 6.5).
pub fn set_mode_params(session_id: &str, mode: CodexMode) -> Value {
    json!({ "sessionId": session_id, "modeId": mode.id() })
}

/// Fail closed unless the session really is in the mode the Host asked for.
/// Reads `modes.currentModeId` (or the `mode` config option) from the
/// `session/new` / `session/load` / `session/resume` response [M1].
pub fn verify_session_mode(session_response: &Value, expected: CodexMode) -> Result<(), PortError> {
    let current = session_response
        .pointer("/modes/currentModeId")
        .and_then(Value::as_str)
        .or_else(|| {
            session_response
                .get("configOptions")
                .and_then(Value::as_array)
                .and_then(|options| options.iter().find(|option| option.get("id") == Some(&json!("mode"))))
                .and_then(|option| option.get("currentValue"))
                .and_then(Value::as_str)
        });
    match current {
        Some(id) if id == expected.id() => Ok(()),
        Some(id) => Err(PortError::Refused(format!(
            "codex session is in mode {id}, Host policy requires {}",
            expected.id()
        ))),
        None => Err(PortError::Refused("codex session did not report its mode".into())),
    }
}

/// Startup check (plan 7.3: incompatible version refuses to start): the agent
/// must be codex-acp at the pinned version speaking ACP v1.
pub fn check_initialize(initialize_response: &Value) -> Result<(), PortError> {
    let protocol = initialize_response.get("protocolVersion").and_then(Value::as_u64);
    if protocol != Some(1) {
        return Err(PortError::Refused(format!("codex-acp protocolVersion {protocol:?}, expected 1")));
    }
    let name = initialize_response.pointer("/agentInfo/name").and_then(Value::as_str);
    let version = initialize_response.pointer("/agentInfo/version").and_then(Value::as_str);
    match (name, version) {
        (Some(AGENT_NAME), Some(CODEX_ACP_VERSION)) => Ok(()),
        _ => Err(PortError::Refused(format!(
            "expected {AGENT_NAME} {CODEX_ACP_VERSION}, got {} {}",
            name.unwrap_or("<none>"),
            version.unwrap_or("<none>")
        ))),
    }
}

/// codex-acp advertises steering as `_meta.steering.supported` on the
/// initialize result [M1], not as an ACP capability.
pub fn steer_supported(initialize_response: &Value) -> bool {
    initialize_response.pointer("/_meta/steering/supported") == Some(&Value::Bool(true))
}

/// `_session/steering` params: `{sessionId, prompt: ContentBlock[]}` [S]. The
/// result's `outcome` is `injected` (joined the live turn), `startedNewTurn`
/// (no live turn, a new one started) or `failed`; the Host must show which.
pub fn steering_params(session_id: &str, text: &str) -> Value {
    json!({ "sessionId": session_id, "prompt": [{ "type": "text", "text": text }] })
}

/// Session capabilities from `initialize`. Same reading as
/// `acp::capabilities_from_initialize` (absent or false = unsupported), kept
/// here so this module also builds without the `acp` feature; the
/// `codex_backend` test checks the two agree. `steer` comes from `_meta`.
fn session_capabilities(initialize_response: &Value) -> SessionCapabilities {
    let caps = initialize_response.get("agentCapabilities").cloned().unwrap_or(Value::Null);
    let session_caps = caps.get("sessionCapabilities").cloned().unwrap_or(Value::Null);
    let present = |value: &Value, key: &str| match value.get(key) {
        Some(Value::Bool(flag)) => *flag,
        Some(Value::Null) | None => false,
        Some(_) => true,
    };
    SessionCapabilities {
        load: present(&caps, "loadSession"),
        resume: present(&session_caps, "resume"),
        close: present(&session_caps, "close"),
        list: present(&session_caps, "list"),
        steer: steer_supported(initialize_response),
    }
}

/// Capability declaration for the mode Fox uses by default (read-only) on the
/// current platform.
pub fn capabilities(initialize_response: &Value) -> CapabilityDeclaration {
    capabilities_for(initialize_response, CodexMode::ReadOnly, Platform::current())
}

/// Four-dimension declaration for a given mode and platform. Anything without
/// evidence is `Unverified`; nothing here is upgraded on expectation alone.
pub fn capabilities_for(initialize_response: &Value, mode: CodexMode, platform: Platform) -> CapabilityDeclaration {
    use ApprovalGate::{None as NoGate, Unverified as GateUnverified};

    let approval_coverage = match mode {
        CodexMode::ReadOnly | CodexMode::WorkspaceWrite => vec![
            // [M2] reads anywhere succeed inside the sandbox; [S] no approval
            // path exists for reads. Not limited to the authorization root.
            (ToolCategory::FileRead, NoGate),
            // read-only: Codex is expected to ask (fileChange approval ->
            // request_permission) for every patch outside the writable roots
            // [S: CodexApprovalHandler.handleFileChange], but which writes ask
            // is decided inside the Codex binary and has not been run.
            // workspace-write: writes inside cwd run without asking [S: mode
            // description "Edit workspace files"].
            (
                ToolCategory::FileWrite,
                if mode == CodexMode::ReadOnly { GateUnverified } else { NoGate },
            ),
            // approvalPolicy `on-request` [S]: commands run inside the sandbox
            // without asking; only a model-requested escalation becomes a
            // request_permission. Not per call.
            (ToolCategory::Shell, NoGate),
            // Local network is a sandbox matter (see below). The model's
            // server-side web search is not approved by the Host at all [S:
            // webSearch items are reported, never asked]; it is only requested
            // off via CODEX_CONFIG (unmeasured).
            (ToolCategory::Network, NoGate),
            // Codex raises MCP tool approvals as elicitations mapped to
            // request_permission [S: buildMcpPermissionOptions], but when it
            // asks depends on Codex config/annotations - not measured. Fox's
            // business tools are gated by the Host's own MCP server anyway.
            (ToolCategory::McpBusinessTool, GateUnverified),
        ],
        // Approvals go to Codex's automatic reviewer / are disabled [S].
        CodexMode::AutoReview | CodexMode::FullAccess => [
            ToolCategory::FileRead,
            ToolCategory::FileWrite,
            ToolCategory::Shell,
            ToolCategory::Network,
            ToolCategory::McpBusinessTool,
        ]
        .into_iter()
        .map(|category| (category, NoGate))
        .collect(),
    };

    let (sandbox_filesystem, sandbox_network) = match (mode, platform) {
        (CodexMode::FullAccess, _) => (Enforcement::Absent, Enforcement::Absent),
        // [M2] Linux aarch64, kernel 6.12: write outside roots -> "Read-only
        // file system"; TCP to 1.1.1.1:443 -> "Could not connect"; control
        // without sandbox and with network_access=true -> HTTP 200. Measured
        // with `codex sandbox`, the mechanism codex-acp selects per turn
        // [S: sandboxPolicy in runTurn]; through a live turn still pending
        // (T04). Filesystem enforcement covers writes only.
        (_, Platform::Linux) => (Enforcement::Enforced, Enforcement::Enforced),
        // macOS Seatbelt expected, not run. Windows: Codex's Windows sandbox is
        // weaker/experimental and not run. Declared as such, never Enforced.
        _ => (Enforcement::Unverified, Enforcement::Unverified),
    };

    let mut declared = CapabilityDeclaration::unverified();
    declared.approval_coverage = approval_coverage;
    declared.sandbox_filesystem = sandbox_filesystem;
    declared.sandbox_network = sandbox_network;
    // [S] session/cancel -> turn/interrupt, then the prompt returns
    // stopReason "cancelled" once Codex reports the turn interrupted. But
    // "cancelled" is also returned when the client aborted before/after
    // completion, and no live turn was cancelled here: stays requested.
    declared.cancel_turn = CancelReach::TurnRequested;
    // Fox spawns codex-acp in its own process group; codex-acp spawns
    // `codex app-server` as an ordinary child (same group) [S:
    // startCodexConnection]. Group kill checked by the real smoke test on Unix.
    declared.cancel_process_tree = true;
    // ACP v1 has no acknowledgement of a permission response.
    declared.approval_answer_acknowledged = false;
    // [S] README + reporters: shell command, file change, MCP tool call, web
    // search are reported as tool_call / tool_call_update with start and end.
    // Processes started inside a command are not visible.
    declared.observability = Visibility::ToolLifecycle;
    declared.session = session_capabilities(initialize_response);
    declared
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_never_maps_to_modes_that_bypass_host_approval() {
        let mut policy = HostPolicy { cwd: "/w".into(), writable: false, network: false };
        assert_eq!(mode_for_policy(&policy), Ok(CodexMode::ReadOnly));
        policy.writable = true;
        assert_eq!(mode_for_policy(&policy), Ok(CodexMode::WorkspaceWrite));
        policy.network = true;
        assert!(matches!(mode_for_policy(&policy), Err(PortError::Unsupported(_))));
        for mode in [CodexMode::ReadOnly, CodexMode::WorkspaceWrite, CodexMode::AutoReview, CodexMode::FullAccess] {
            assert_eq!(CodexMode::parse(mode.id()), Some(mode));
        }
    }
}
