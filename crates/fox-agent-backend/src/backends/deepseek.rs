//! deepseek backend specifics. Owned by the deepseek integration branch.
//!
//! DeepSeekBackend = DeepSeek Harness `dsh --profile acp` (launch spec in
//! [`crate::launch`]) spoken to over ACP v1 by the shared client. This file only
//! holds what is specific to dsh: the pinned version check, the capability
//! declaration, the mapping of the Host policy onto a dsh `--patch` overlay, and
//! the section-8 floor verdict. No agent loop here. Experimental backend:
//! upstream labels it "developer preview, not security audited, breaking
//! changes expected".
//!
//! Evidence (details and raw recordings in `docs/deepseek.md`):
//! * [M1] real `initialize` / `session/new` / `session/prompt` against
//!   `@deepseek-ai/dsh@0.2.1-rc.2` through [`crate::acp::AcpConnection`] on
//!   Linux aarch64 / Debian 13 / Node 22.23, 2026-10-19, without `DEEPSEEK_API_KEY`;
//!   raw wire responses also captured with a Node JSON-RPC probe.
//! * [M2] `dsh --profile acp [--patch f] --dump-config` on the same install:
//!   the composed plugin tree, and that a `--patch` overlay wins over the
//!   home-level `$DSH_HOME/cordis.patch.yml`.
//! * [S] dsh 0.2.1-rc.2 package sources and READMEs: `dsh-acp` (approval
//!   bridge, initialize), `dsh-tools` (pre-execute gate), `dsh-tool-bash` /
//!   `dsh-tool-fs` / `dsh-sandbox` (escalation), `dsh-sandbox-policy`,
//!   `dsh-bash-sandbox`, `dsh-fs-sandbox`, `dsh-sandbox-local`,
//!   `dsh-subprocess` (env scrub), `dsh-credentials-local`,
//!   `dsh-session-telemetry-otel`.
//! No model turn has run (no DeepSeek credentials), so nothing that needs a
//! live turn (a tool call, an approval round trip, cancel confirmation, the
//! sandbox through a turn) is claimed as measured.

use crate::launch::LaunchSpec;
use crate::port::{
    AgentBackend, ApprovalGate, CancelReach, CapabilityDeclaration, Enforcement, PortError,
    SessionCapabilities, ToolCategory, Visibility,
};
use serde_json::Value;

/// `dsh --version` output of the install that was measured [M1]. dsh is a
/// developer preview with breaking changes between release candidates, so the
/// adapter runs only against this exact version.
pub const PINNED_VERSION: &str = "0.2.1-rc.2";
/// npm package the pin refers to.
pub const NPM_PACKAGE: &str = "@deepseek-ai/dsh";
/// `agentInfo.name` dsh reports in `initialize` [M1].
pub const AGENT_NAME: &str = "deepseek-harness-acp";
/// `agentInfo.version` in `initialize` [M1]. A hard-coded protocol-surface
/// version in `dsh-acp`, NOT the package version; the package version is
/// checked through `dsh --version`.
pub const AGENT_INFO_VERSION: &str = "0.0.1";
/// Option ids dsh offers in every `session/request_permission` [S: dsh-acp
/// `approval/request` bridge]. One-shot only: there is no "always" option.
pub const OPTION_ALLOW_ONCE: &str = "allow-once";
pub const OPTION_REJECT_ONCE: &str = "reject-once";

/// Refuse to start unless `dsh --version` printed exactly the pinned version.
/// The output is one line (`0.2.1-rc.2\n` [M1]); a leading `dsh`/`v` is
/// tolerated, anything else (other version, range, extra tokens) is rejected.
pub fn check_version(output_of_dsh_version: &str) -> Result<(), String> {
    let trimmed = output_of_dsh_version.trim();
    let mut tokens = trimmed.split_whitespace();
    let mut version = tokens.next().unwrap_or("");
    if version == "dsh" {
        version = tokens.next().unwrap_or("");
    }
    if tokens.next().is_some() {
        return Err(format!(
            "unexpected `dsh --version` output {trimmed:?}; DeepSeek backend is pinned to {PINNED_VERSION}"
        ));
    }
    let version = version.strip_prefix('v').unwrap_or(version);
    if version == PINNED_VERSION {
        Ok(())
    } else if version.is_empty() {
        Err(format!("`dsh --version` printed nothing; DeepSeek backend is pinned to {PINNED_VERSION}"))
    } else {
        Err(format!(
            "dsh {version} is not the validated version {PINNED_VERSION}; refusing to start the experimental DeepSeek backend"
        ))
    }
}

/// Check the `initialize` response is the dsh ACP bridge and speaks ACP v1.
pub fn check_initialize(initialize_response: &Value) -> Result<(), PortError> {
    let version = initialize_response.get("protocolVersion").and_then(Value::as_u64);
    if version != Some(1) {
        return Err(PortError::Refused(format!(
            "DeepSeek Harness answered ACP protocol version {version:?}, expected 1"
        )));
    }
    let name = initialize_response.pointer("/agentInfo/name").and_then(Value::as_str);
    if name != Some(AGENT_NAME) {
        return Err(PortError::Refused(format!(
            "expected agent {AGENT_NAME:?} from `dsh --profile acp`, got {name:?}"
        )));
    }
    Ok(())
}

fn declared(value: Option<&Value>) -> bool {
    match value {
        Some(Value::Bool(flag)) => *flag,
        Some(Value::Null) | None => false,
        Some(_) => true,
    }
}

/// Four-dimension capability declaration for dsh 0.2.1-rc.2 `--profile acp`
/// under the Host patch of [`policy_patch`]. Session capabilities are read from
/// the `initialize` response; everything else is fixed by evidence. Nothing is
/// claimed beyond the evidence; what needs a live turn stays `Unverified`.
pub fn capabilities(initialize_response: &Value) -> CapabilityDeclaration {
    let caps = initialize_response.get("agentCapabilities");
    let session_caps = caps.and_then(|caps| caps.get("sessionCapabilities"));
    let mut declared_caps = CapabilityDeclaration::unverified();

    // Approval coverage [S]. In the acp composition no `tools/pre-execute`
    // listener returns `ask` (only the unmounted experimental auto-review
    // does); `ctx.approval.request` — the only source of ACP
    // `session/request_permission` (dsh-acp `approval/request` bridge) — is
    // reached solely by the sandbox *escalation* path of bash / fs mutation
    // tools (`sandbox_permissions`, dsh-sandbox `approveEscalation`). So every
    // ordinary call runs without Host approval:
    // * FileRead: reads are never gated and never confined (dsh-fs-sandbox:
    //   "reads stay unconfined").
    // * FileWrite: writes inside the session workspace run unasked; outside it
    //   they are denied, and only an escalation retry asks. Under the Host
    //   patch the approval policy is `never`, so escalations are rejected by
    //   dsh itself and never reach the Host.
    // * Shell: bash runs unasked inside the sandbox mode; same escalation rule.
    // * Network: `web_fetch`/`web_search` tools and bash network are ungated.
    // * McpBusinessTool: MCP tools go through the same pipeline with no `ask`
    //   gate. Business authorisation must therefore stay in the Host's MCP
    //   server (plan 6.3 rule 8), which is where Fox enforces it anyway.
    declared_caps.approval_coverage = vec![
        (ToolCategory::FileRead, ApprovalGate::None),
        (ToolCategory::FileWrite, ApprovalGate::None),
        (ToolCategory::Shell, ApprovalGate::None),
        (ToolCategory::Network, ApprovalGate::None),
        (ToolCategory::McpBusinessTool, ApprovalGate::None),
    ];

    // Filesystem boundary: deliberately not a single "enforced". Per [S]:
    // writes are confined to the session cwd (+ temp dirs) by a kernel runner
    // for bash (bwrap / Landlock / Seatbelt; fails closed with
    // SANDBOX_UNAVAILABLE, may report `partial`) and by a user-space policy
    // fence for the fs tools ("a policy fence, not a kernel boundary"); reads
    // are not confined at all. Not measured through a live turn. See
    // [`filesystem_detail`].
    declared_caps.sandbox_filesystem = Enforcement::Unverified;

    // Network [S]: dsh-bash-sandbox "network stays unrestricted"; the bwrap
    // profile has no --unshare-net; web tools ship enabled. Disabling the web
    // tools in the patch does not stop bash / node subprocess network.
    declared_caps.sandbox_network = Enforcement::Absent;

    // Cancel: ACP `session/cancel` exists and the README says the prompt
    // settles `cancelled` after the root activity drains, but also that later
    // child messages "may start autonomous work" after cancel. Not measured
    // (needs a live turn) -> requested only.
    declared_caps.cancel_turn = CancelReach::TurnRequested;
    // Fox owns the dsh process group (acp.rs). dsh's own children use
    // systemd scopes / bwrap --die-with-parent [S]; that they all die with the
    // group is not measured, so plan 6.4 rule 4 "possible unverified side
    // effects" applies on kill.
    declared_caps.cancel_process_tree = true;
    // ACP v1 has no acknowledgement for a permission response.
    declared_caps.approval_answer_acknowledged = false;
    // README promises a generic tool lifecycle in `session/update`; no live
    // turn observed -> Unverified.
    declared_caps.observability = Visibility::Unverified;

    // Session [M1]: initialize declares close/list/resume, no loadSession.
    // README: no session/load, no transcript replay, resume does not replay.
    declared_caps.session = SessionCapabilities {
        load: declared(caps.and_then(|caps| caps.get("loadSession"))),
        resume: declared(session_caps.and_then(|caps| caps.get("resume"))),
        close: declared(session_caps.and_then(|caps| caps.get("close"))),
        list: declared(session_caps.and_then(|caps| caps.get("list"))),
        steer: false,
    };
    declared_caps
}

/// Finer-grained filesystem facts behind the `Unverified` aggregate, for the
/// settings/diagnostics page. All from [S]; none measured through a turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FilesystemDetail {
    /// Reads outside the workspace (including `~/.ssh`, other agents' auth
    /// files, `~/.dsh` credentials, `/proc/<pid>/environ` of same-user
    /// processes).
    pub read_outside_workspace: Enforcement,
    /// Bash/node writes outside the workspace: kernel runner (claimed).
    pub shell_write_outside_workspace: Enforcement,
    /// fs-tool writes outside the workspace: user-space path fence.
    pub tool_write_outside_workspace: Enforcement,
}

pub fn filesystem_detail() -> FilesystemDetail {
    FilesystemDetail {
        read_outside_workspace: Enforcement::Absent,
        shell_write_outside_workspace: Enforcement::Unverified,
        tool_write_outside_workspace: Enforcement::Advisory,
    }
}

/// What the Host allows for one DeepSeek session (layer 1 of plan 6.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HostPolicy {
    /// May write inside the session workspace. `false` = read-only.
    pub workspace_write: bool,
    /// Outbound network allowed.
    pub network: bool,
}

/// The dsh sandbox mode a policy maps to. `danger-full-access` is never
/// produced: the patch removes it from the preset table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxMode {
    ReadOnly,
    WorkspaceWrite,
}

impl SandboxMode {
    pub fn id(self) -> &'static str {
        match self {
            Self::ReadOnly => "read-only",
            Self::WorkspaceWrite => "workspace-write",
        }
    }
}

/// Result of mapping a Host policy onto dsh.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyMapping {
    pub mode: SandboxMode,
    /// `--patch` overlay content (YAML patch list). Built only from constants;
    /// no user input is interpolated.
    pub patch_yaml: String,
    /// Parts of the Host policy dsh cannot enforce. Non-empty means the Host
    /// must not present the session as confined by that policy.
    pub unenforced: Vec<&'static str>,
}

/// Map the Host policy onto a dsh `--patch` overlay [M2][S]:
/// * `sandbox-policy.mode` = read-only | workspace-write (the documented
///   `DSH_PERMISSION_MODE` env switch cannot be used: it is not in the Host
///   environment allowlist, and a patch is explicit anyway);
/// * the permission preset table is replaced by one preset (the chosen mode,
///   approval `never`), so a user-level `permission.defaultPreset` of
///   `danger-full-access` cannot be selected (fails closed instead);
/// * `approval.policy = never`: sandbox escalation requests are rejected by
///   dsh before any prompt, so no widening can be approved at all;
/// * session telemetry upload is disabled;
/// * without network permission the `web_fetch`/`web_search` tools are
///   disabled — bash/node subprocess network still works (listed in
///   `unenforced`).
///
/// A `--patch` overlay replaces the matched row's whole config and wins over
/// `$DSH_HOME/cordis.patch.yml` [M2]. It cannot neutralise rows a user-level
/// patch *inserts* (listed in `unenforced`).
pub fn policy_patch(policy: &HostPolicy) -> PolicyMapping {
    let mode = if policy.workspace_write { SandboxMode::WorkspaceWrite } else { SandboxMode::ReadOnly };
    let mut patch = String::from(
        "# Generated by Fox Host for the DeepSeek backend. Do not edit.\n",
    );
    patch.push_str(&format!(
        "- id: sandbox-policy\n  config:\n    mode: {mode}\n\
         - id: approval\n  config:\n    policy: never\n\
         - id: permission\n  config:\n    presets:\n      {mode}:\n        sandbox: {mode}\n        approval: never\n    defaultPreset: {mode}\n\
         - id: session-telemetry-otel\n  config:\n    mode: DISABLED\n",
        mode = mode.id()
    ));
    let mut unenforced = vec![
        "file reads outside the workspace are not confined",
        "fs-tool write confinement is a user-space path check, not a kernel boundary",
        "rows inserted by a user-level dsh patch ($DSH_HOME) are not neutralised",
    ];
    if !policy.network {
        patch.push_str("- id: tool-web\n  disabled: true\n");
        unenforced.push("network: bash and node subprocesses keep outbound network (dsh has no network sandbox)");
    }
    PolicyMapping { mode, patch_yaml: patch, unenforced }
}

/// Directory, inside Fox's managed runtime directory, holding every `--patch`
/// variant (`runtime-assets/dsh-patches` in this crate, copied there by the
/// one-time sandbox setup). The patch is a read-only asset the sandbox must
/// read, so on Windows it has to live in the managed runtime (Users
/// read+execute, administrator-owned), never under the user profile
/// (`sandbox_path_not_owned`).
pub const PATCH_DIR: &str = "dsh-patches";

/// File name of the variant for `policy`: `read-only.yml`,
/// `read-only-noweb.yml`, `workspace-write.yml`, `workspace-write-noweb.yml`.
pub fn patch_file_name(policy: &HostPolicy) -> String {
    let mode = if policy.workspace_write { SandboxMode::WorkspaceWrite } else { SandboxMode::ReadOnly };
    format!("{}{}.yml", mode.id(), if policy.network { "" } else { "-noweb" })
}

/// Every variant as `(file name, content)`; what the runtime package ships.
pub fn patch_variants() -> Vec<(String, String)> {
    let mut out = Vec::new();
    for workspace_write in [false, true] {
        for network in [false, true] {
            let policy = HostPolicy { workspace_write, network };
            out.push((patch_file_name(&policy), policy_patch(&policy).patch_yaml));
        }
    }
    out
}

/// Default `--patch` location: `<runtime_root>/dsh-patches/<variant>.yml`.
pub fn default_patch_path(runtime_root: &std::path::Path, policy: &HostPolicy) -> std::path::PathBuf {
    runtime_root.join(PATCH_DIR).join(patch_file_name(policy))
}

/// The installed patch must be byte-for-byte what Fox generates for
/// `policy` (fail-closed: a missing or edited file refuses the launch).
pub fn verify_installed_patch(path: &std::path::Path, policy: &HostPolicy) -> Result<(), PortError> {
    let expected = policy_patch(policy).patch_yaml;
    match std::fs::read_to_string(path) {
        Ok(found) if found == expected => Ok(()),
        Ok(_) => Err(PortError::Refused(format!("{} differs from the patch Fox generates; re-run the sandbox setup", path.display()))),
        Err(error) => Err(PortError::Refused(format!("dsh patch {} is not installed: {error}", path.display()))),
    }
}

/// Launch spec for a session: `dsh --profile acp --patch <abs path>`. The Host
/// writes [`PolicyMapping::patch_yaml`] to a Fox-owned file first and spawns
/// with the session root as the working directory (dsh's fallback workspace
/// root is `process.cwd()`).
pub fn launch_spec(program: &str, patch_path: &std::path::Path) -> Result<LaunchSpec, PortError> {
    if !patch_path.is_absolute() {
        return Err(PortError::Refused("dsh --patch path must be absolute".into()));
    }
    Ok(LaunchSpec {
        backend: AgentBackend::Deepseek,
        program: program.to_owned(),
        args: vec![
            "--profile".into(),
            "acp".into(),
            "--patch".into(),
            patch_path.to_string_lossy().into_owned(),
        ],
        version_pin: PINNED_VERSION,
        credential_env: &["DEEPSEEK_API_KEY"],
        experimental: true,
        session_env: vec![],
    })
}

/// Defence in depth for `session/request_permission`: under the Host patch
/// dsh never asks (approval policy `never`); if a request still arrives it can
/// only be a sandbox escalation [S], which the Host policy never grants. The
/// Host answers it with the native one-shot reject option, never "allow".
pub fn host_permission_answer(option_ids: &[String]) -> Option<&'static str> {
    option_ids
        .iter()
        .any(|id| id == OPTION_REJECT_ONCE)
        .then_some(OPTION_REJECT_ONCE)
}

/// Section-8 floor items for an experimental backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FloorStatus {
    Met { evidence: &'static str },
    NotMet { reasons: Vec<&'static str> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FloorVerdict {
    /// T04: out-of-scope path / network / business operations blocked.
    pub t04_isolation: FloorStatus,
    /// T05: approvals are never replayed.
    pub t05_no_replay: FloorStatus,
    /// Credential protection: env allowlist, native credentials never copied.
    pub credentials: FloorStatus,
}

impl FloorVerdict {
    /// The backend may be offered (even behind a manual switch) only when
    /// every floor item is met.
    pub fn may_enable(&self) -> bool {
        [&self.t04_isolation, &self.t05_no_replay, &self.credentials]
            .iter()
            .all(|status| matches!(status, FloorStatus::Met { .. }))
    }
}

/// Floor verdict for dsh 0.2.1-rc.2 as integrated today (dsh launched
/// directly by the Host, no outer OS sandbox around the dsh process).
pub fn floor_verdict() -> FloorVerdict {
    FloorVerdict {
        t04_isolation: FloorStatus::NotMet {
            reasons: vec![
                "reads outside the authorised root are not blocked in any dsh mode (read-only included)",
                "outbound network cannot be blocked: dsh has no network sandbox; bash/node keep network",
                "a read-only path fence is enforceable only for writes; T04 also needs read and network blocking",
            ],
        },
        t05_no_replay: FloorStatus::Met {
            evidence: "dsh options are one-shot (allow-once/reject-once) and an approval applies only to its request; \
                       Host client consumes each answer once and answers pending requests `cancelled` on cancel \
                       (acp_mock_agent tests); Host patch sets approval policy `never` so no grant exists to replay. \
                       Live round trip not measured.",
        },
        credentials: FloorStatus::Met {
            evidence: "Host side: child env = allowlist + DEEPSEEK_API_KEY by name; Host never copies dsh's \
                       credential store. dsh scrubs *KEY*/*TOKEN*/*SECRET* names from tool subprocesses. Residual risk \
                       tied to T04: unconfined reads let the agent read ~/.dsh credentials and same-user files.",
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floor_blocks_enabling_today() {
        assert!(!floor_verdict().may_enable());
    }

    #[test]
    fn shipped_patch_files_match_the_generated_variants_and_live_in_the_runtime() {
        let shipped = |name: &str| -> &'static str {
            match name {
                "read-only-noweb.yml" => include_str!("../../runtime-assets/dsh-patches/read-only-noweb.yml"),
                "read-only.yml" => include_str!("../../runtime-assets/dsh-patches/read-only.yml"),
                "workspace-write-noweb.yml" => include_str!("../../runtime-assets/dsh-patches/workspace-write-noweb.yml"),
                "workspace-write.yml" => include_str!("../../runtime-assets/dsh-patches/workspace-write.yml"),
                other => panic!("unexpected variant {other}"),
            }
        };
        let variants = patch_variants();
        assert_eq!(variants.len(), 4);
        for (name, content) in &variants {
            assert_eq!(shipped(name), content, "runtime-assets/dsh-patches/{name} is stale");
        }
        let runtime = std::path::Path::new("/opt/fox/sandbox-runtime/0.1.0");
        let policy = HostPolicy { workspace_write: false, network: false };
        assert_eq!(default_patch_path(runtime, &policy), runtime.join("dsh-patches").join("read-only-noweb.yml"));
        let dir = std::env::temp_dir().join(format!("fox-dsh-patch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("read-only-noweb.yml");
        assert!(verify_installed_patch(&file, &policy).is_err(), "missing");
        std::fs::write(&file, policy_patch(&policy).patch_yaml).unwrap();
        verify_installed_patch(&file, &policy).unwrap();
        std::fs::write(&file, "- id: approval\n  config:\n    policy: always\n").unwrap();
        assert!(verify_installed_patch(&file, &policy).is_err(), "edited");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn patch_never_widens() {
        let read_only = policy_patch(&HostPolicy { workspace_write: false, network: false });
        assert_eq!(read_only.mode, SandboxMode::ReadOnly);
        assert!(!read_only.patch_yaml.contains("danger-full-access"));
        assert!(read_only.patch_yaml.contains("policy: never"));
        assert!(read_only.patch_yaml.contains("- id: tool-web\n  disabled: true"));
    }
}
