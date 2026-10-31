//! Per-agent native permission list mapped onto Fox's four tiers (plan C2).
//!
//! Fox speaks four tiers: 只读 / 询问 / 自动 / 完全访问 ([`FoxTier`]). Each
//! external Agent offers only the tiers it really has, each bound to one of
//! its own native modes; a tier it lacks is listed greyed with the reason:
//!
//! | Fox 档位 | pi | Codex (codex-acp) | dsh |
//! |---|---|---|---|
//! | 只读 | Host 应答：读放行、其余拒绝 | `read-only` | `read-only` |
//! | 询问 | Host 应答，规则同 PiNative | `workspace-write`，审批卡来自 request_permission | `workspace-write`（ask） |
//! | 自动 | Host 应答，规则同 PiNative | 不提供（置灰：`agent` 由 OpenAI 自动审查模型代批） | `workspace-write-never` |
//! | 完全访问 | Host 全部放行（block 规则仍生效） | `agent-full-access` | `danger-full-access` |
//!
//! * **pi** has no permission system of its own: Fox's approval gate
//!   extension raises every tool call and the Host answers it with the same
//!   approval engine as PiNative (`approval_policy`, project rules and block
//!   rules included; the desktop implements
//!   [`crate::external::ExternalSink::host_permission_answer`]). A change
//!   applies from pi's next request.
//! * **Codex** and **dsh** keep their own native sandbox and approval; Fox
//!   does not imitate them. Fox only turns a `session/request_permission`
//!   into an approval card. Codex switches at once (`session/set_mode`); dsh
//!   has no `session/set_mode` (-32601), so a change takes effect when dsh
//!   restarts at the next turn.
//! * **Fox**: unchanged (its own execution mode in the composer).
//!
//! The Host's automatic answer to `session/request_permission` for Codex and
//! dsh ([`answer`]) is only a fail-closed fallback: a request the native
//! layer let through although the mode says nothing should ask is rejected, a
//! mode that asks shows the card. Fox never auto-allows on behalf of Codex or
//! dsh.

use crate::port::AgentBackend;
use crate::projection::{auto_permission, AutoPermission, ExecutionMode, PermissionOption};
use serde::Serialize;
use serde_json::Value;

/// When a change of the mode reaches a running session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SwitchTiming {
    /// Applied to the live session at once (`session/set_mode`).
    Live,
    /// Applied when the backend restarts at the next turn.
    NextTurnRestart,
    /// The Host answers the requests itself; applies to the next request.
    HostAnswered,
}

/// Fox's four permission tiers, the vocabulary every Agent's list is mapped
/// onto (the same four as a Fox conversation's execution mode).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FoxTier {
    ReadOnly,
    Ask,
    Auto,
    FullAccess,
}

impl FoxTier {
    pub const ALL: [FoxTier; 4] = [Self::ReadOnly, Self::Ask, Self::Auto, Self::FullAccess];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::ReadOnly => "read_only",
            Self::Ask => "ask",
            Self::Auto => "auto",
            Self::FullAccess => "full_access",
        }
    }

    /// The tier's name in the UI.
    pub fn label(self) -> &'static str {
        match self {
            Self::ReadOnly => "只读",
            Self::Ask => "询问",
            Self::Auto => "自动",
            Self::FullAccess => "完全访问",
        }
    }
}

/// One entry of an Agent's permission list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NativePermissionOption {
    /// Stored id (the Agent's own mode / preset name where it has one).
    pub id: &'static str,
    /// The Fox tier this option is ([`FoxTier`]).
    pub tier: FoxTier,
    /// Chinese label shown in the picker (the tier's name).
    pub label: &'static str,
    /// One-line explanation.
    pub description: &'static str,
    /// The Agent's own name of what is configured (shown as a hint).
    pub native: &'static str,
    /// This option is the Agent's full access, bound to Fox's「完全访问」.
    pub full_access: bool,
    /// Greyed with this reason.
    pub disabled_reason: Option<String>,
}

/// The list for one Agent plus how a change is applied.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NativePermissionCatalog {
    pub backend: &'static str,
    /// The Agent's display name in UI text ([`display_name`]).
    pub display_name: &'static str,
    /// Always the four tiers in Fox order (只读, 询问, 自动, 完全访问).
    pub options: Vec<NativePermissionOption>,
    /// The (non-full-access) native option a new session starts in when
    /// nothing was chosen.
    pub default_mode: &'static str,
    pub switch_timing: SwitchTiming,
    /// What the UI must say about when a change applies.
    pub switch_note: &'static str,
}

/// What the catalog depends on besides the Agent.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CatalogContext {
    /// Base URL of the provider the Agent uses through Fox's unified login.
    /// No option depends on it since plan C2 (Codex's `agent` mode is never
    /// offered); kept for the callers that report it.
    pub provider_base_url: Option<String>,
    /// `FOX_EXTERNAL_FULL_ACCESS_FORBIDDEN`: the full-access option is greyed.
    pub full_access_forbidden: bool,
}

/// Why Codex has no 自动 tier.
pub const CODEX_NO_AUTO_TIER_REASON: &str = "Codex 没有与 Fox「自动」对等的档位：它的 agent 模式由 OpenAI 的 codex-auto-review 模型代你审批（需要 OpenAI），Fox 不提供";
pub const FULL_ACCESS_FORBIDDEN_NOTE: &str = "管理员已禁止完全访问（FOX_EXTERNAL_FULL_ACCESS_FORBIDDEN）";

/// Whether `base_url` is an OpenAI API endpoint (`api.openai.com` or another
/// `*.openai.com` host).
pub fn provider_is_openai(base_url: &str) -> bool {
    host_of(base_url).is_some_and(|host| host == "openai.com" || host.ends_with(".openai.com"))
}

/// Lower-case host of an `http(s)://` URL, without port / userinfo.
pub fn host_of(url: &str) -> Option<String> {
    let rest = url.trim().split_once("://").map(|(_, rest)| rest)?;
    let authority = rest.split(['/', '?', '#']).next()?;
    let authority = authority.rsplit_once('@').map_or(authority, |(_, host)| host);
    let host = if let Some(stripped) = authority.strip_prefix('[') {
        stripped.split_once(']').map(|(host, _)| host)?
    } else {
        authority.rsplit_once(':').filter(|(_, port)| port.chars().all(|c| c.is_ascii_digit())).map_or(authority, |(host, _)| host)
    };
    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    (!host.is_empty()).then_some(host)
}

/// The Agent's name in UI text (the same names as the picker / backend bar:
/// Codex, DeepSeek, pi; Fox for Fox).
pub fn display_name(backend: AgentBackend) -> &'static str {
    match backend {
        AgentBackend::Codex => "Codex",
        AgentBackend::Deepseek => "DeepSeek",
        AgentBackend::Pi => "pi",
        AgentBackend::Fox => "Fox",
    }
}

fn option(id: &'static str, tier: FoxTier, description: &'static str, native: &'static str) -> NativePermissionOption {
    NativePermissionOption { id, tier, label: tier.label(), description, native, full_access: tier == FoxTier::FullAccess, disabled_reason: None }
}

pub const CODEX_READ_ONLY: &str = "read-only";
pub const CODEX_ASK: &str = "workspace-write";
pub const CODEX_AUTO_REVIEW: &str = "agent";
pub const CODEX_FULL: &str = "agent-full-access";
pub const PI_READ_ONLY: &str = "read_only";
pub const PI_ASK: &str = "ask";
pub const PI_ALLOW: &str = "allow";
pub const PI_FULL: &str = "full_access";

/// The permission list of `backend` (empty for Fox).
pub fn catalog(backend: AgentBackend, context: &CatalogContext) -> NativePermissionCatalog {
    use crate::backends::deepseek::DshPreset;
    let (mut options, default_mode, switch_timing, switch_note) = match backend {
        AgentBackend::Codex => (
            vec![
                option(CODEX_READ_ONLY, FoxTier::ReadOnly, "Codex 原生只读沙箱；需要越权时由 Codex 发起审批，Fox 转成审批卡", "read-only（sandbox read-only · on-request · 审批人 user）"),
                option(CODEX_ASK, FoxTier::Ask, "工作区内可写；越出沙箱的操作由 Codex 发起审批，Fox 转成审批卡请你批准", "workspace-write（on-request · 审批人 user）"),
                option(CODEX_AUTO_REVIEW, FoxTier::Auto, "Codex 没有对等档位", "agent（on-request · 审批人 auto_review）"),
                option(CODEX_FULL, FoxTier::FullAccess, "不套沙箱、不再询问；即 Fox 的「完全访问」", "agent-full-access"),
            ],
            CODEX_ASK,
            SwitchTiming::Live,
            "切换立即生效（Codex session/set_mode）",
        ),
        AgentBackend::Deepseek => (
            vec![
                option(DshPreset::ReadOnly.id(), FoxTier::ReadOnly, "只读工作区，不询问，越权操作直接拒绝", "sandbox read-only · approval never"),
                option(DshPreset::WorkspaceWriteAsk.id(), FoxTier::Ask, "工作区内可写；越出工作区时由 dsh 发起审批，Fox 转成审批卡", "preset workspace-write（sandbox workspace-write · approval ask）"),
                option(DshPreset::WorkspaceWriteNever.id(), FoxTier::Auto, "工作区内可写、不询问；越出工作区的操作直接拒绝", "sandbox workspace-write · approval never"),
                option(DshPreset::DangerFullAccess.id(), FoxTier::FullAccess, "不套沙箱、不再询问；即 Fox 的「完全访问」", "preset danger-full-access（approval never）"),
            ],
            // Default = what every dsh session ran in before the list
            // (approval never), so an existing runtime package keeps working.
            DshPreset::WorkspaceWriteNever.id(),
            SwitchTiming::NextTurnRestart,
            "DeepSeek 不支持会话中途切换：下一轮开始时重启 dsh 生效",
        ),
        AgentBackend::Pi => (
            vec![
                option(PI_READ_ONLY, FoxTier::ReadOnly, "只放行读取类工具，其余由 Fox 拒绝", "Fox 审批引擎 · 只读"),
                option(PI_ASK, FoxTier::Ask, "与 Fox 内置会话同一套审批规则：写入和命令请你批准，命中项目规则的照规则处理", "Fox 审批引擎 · 询问"),
                option(PI_ALLOW, FoxTier::Auto, "与 Fox 内置会话同一套审批规则：项目内写入和低风险命令自动允许，高风险和越出项目的仍会询问", "Fox 审批引擎 · 自动"),
                option(PI_FULL, FoxTier::FullAccess, "Fox 全部自动允许（block 规则仍生效）；即 Fox 的「完全访问」", "Fox 审批引擎 · 完全访问"),
            ],
            PI_ASK,
            SwitchTiming::HostAnswered,
            "由 Fox 审批引擎作答：切换后从 pi 的下一次请求起生效",
        ),
        AgentBackend::Fox => (Vec::new(), "", SwitchTiming::HostAnswered, ""),
    };
    for option in &mut options {
        if option.full_access && context.full_access_forbidden {
            option.disabled_reason = Some(FULL_ACCESS_FORBIDDEN_NOTE.to_owned());
        }
        if backend == AgentBackend::Codex && option.id == CODEX_AUTO_REVIEW {
            // Plan C2: no peer of Fox's 自动; never offered, whatever the provider.
            option.disabled_reason = Some(CODEX_NO_AUTO_TIER_REASON.to_owned());
        }
    }
    NativePermissionCatalog { backend: backend.as_str(), display_name: display_name(backend), options, default_mode, switch_timing, switch_note }
}

/// The Fox tier of `backend`'s option `id` (`None` for an unknown id).
pub fn tier_of(backend: AgentBackend, id: &str) -> Option<FoxTier> {
    catalog(backend, &CatalogContext::default()).options.into_iter().find(|option| option.id == id.trim()).map(|option| option.tier)
}

/// The full-access option of `backend`.
pub fn full_access_mode(backend: AgentBackend) -> Option<&'static str> {
    match backend {
        AgentBackend::Codex => Some(CODEX_FULL),
        AgentBackend::Deepseek => Some(crate::backends::deepseek::DshPreset::DangerFullAccess.id()),
        AgentBackend::Pi => Some(PI_FULL),
        AgentBackend::Fox => None,
    }
}

pub fn is_full_access(backend: AgentBackend, id: &str) -> bool {
    full_access_mode(backend) == Some(id.trim())
}

/// Whether the Agent's full access lives in its process, so leaving it needs
/// a restart (dsh reads its preset only at start). Codex switches its own
/// sandbox live (`session/set_mode`) and pi's full access is the Host's
/// answer, so neither restarts.
pub fn full_access_needs_restart(backend: AgentBackend) -> bool {
    backend == AgentBackend::Deepseek
}

/// `id` names an option of `backend`'s list (enabled or not).
pub fn is_known(backend: AgentBackend, id: &str) -> bool {
    catalog(backend, &CatalogContext::default()).options.iter().any(|option| option.id == id.trim())
}

/// The mode a session really runs in: the stored choice when it is a known,
/// enabled option (including the Agent's own full-access option), else the
/// Agent's default. An unknown or greyed choice never widens.
///
/// `full_access` is retained for old callers: it may select the full option
/// when no stored choice exists. It cannot rewrite an existing native choice.
pub fn effective_mode(backend: AgentBackend, stored: Option<&str>, full_access: bool, context: &CatalogContext) -> &'static str {
    let catalog = catalog(backend, context);
    if let Some(option) = stored.and_then(|stored| {
        catalog.options.iter().find(|option| option.id == stored.trim() && option.disabled_reason.is_none())
    }) {
        return option.id;
    }
    if full_access {
        if let Some(full) = catalog.options.iter().find(|option| option.full_access && option.disabled_reason.is_none()) {
            return full.id;
        }
    }
    catalog.default_mode
}

/// The codex-acp mode of a Codex option id.
pub fn codex_mode(id: &str) -> crate::backends::codex::CodexMode {
    use crate::backends::codex::CodexMode;
    match id.trim() {
        CODEX_READ_ONLY => CodexMode::ReadOnly,
        CODEX_AUTO_REVIEW => CodexMode::AutoReview,
        CODEX_FULL => CodexMode::FullAccess,
        _ => CodexMode::WorkspaceWrite,
    }
}

/// The dsh preset of a DeepSeek option id (unknown = the default,
/// workspace-write with approval never).
pub fn dsh_preset(id: &str) -> crate::backends::deepseek::DshPreset {
    crate::backends::deepseek::DshPreset::parse(id).unwrap_or(crate::backends::deepseek::DshPreset::WorkspaceWriteNever)
}

/// How the Host answers a permission request that reached it in `mode`.
///
/// * Codex (every mode): the card. The user is the approver; in `read-only`
///   and `workspace-write` every escalation Codex raises reaches the Host.
/// * dsh sandboxed presets with approval `never` (read-only,
///   workspace-write-never): reject; the mode itself says escalations are
///   refused, so one the native layer let through is refused (fail-closed).
/// * dsh `workspace-write` (approval `ask`): the card.
/// * The full-access options (Codex `agent-full-access`, dsh
///   `danger-full-access`): nothing should ask; if something does, the card
///   (the mode does not require a refusal, and Fox never allows on Codex's or
///   dsh's behalf).
/// * pi: the built-in tier table ([`crate::projection::auto_permission`]):
///   只读 / 询问 / 自动; 完全访问 = 自动. This is only what a sink WITHOUT a
///   Host approval engine gets (tests); the desktop answers pi with the same
///   approval engine as PiNative
///   ([`crate::external::ExternalSink::host_permission_answer`]).
/// * Fox / unknown: the card.
pub fn answer(backend: AgentBackend, mode: &str, request: &Value, options: &[PermissionOption]) -> AutoPermission {
    let reject = || AutoPermission::Reject { option_id: crate::projection::reject_option(options).map(|option| option.option_id.clone()) };
    match backend {
        AgentBackend::Codex => AutoPermission::Ask,
        AgentBackend::Deepseek => {
            let preset = dsh_preset(mode);
            if preset.approval() == "ask" || preset == crate::backends::deepseek::DshPreset::DangerFullAccess {
                AutoPermission::Ask
            } else {
                reject()
            }
        }
        AgentBackend::Pi => {
            let execution = match mode.trim() {
                PI_READ_ONLY => ExecutionMode::ReadOnly,
                PI_ALLOW | PI_FULL => ExecutionMode::Allow,
                _ => ExecutionMode::Ask,
            };
            auto_permission(execution, request, options)
        }
        AgentBackend::Fox => AutoPermission::Ask,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn options() -> Vec<PermissionOption> {
        vec![
            PermissionOption { option_id: "once".into(), name: "Allow".into(), kind: "allow_once".into() },
            PermissionOption { option_id: "no".into(), name: "Reject".into(), kind: "reject_once".into() },
        ]
    }

    #[test]
    fn each_agent_lists_the_four_fox_tiers_mapped_to_its_own_modes() {
        let context = CatalogContext::default();
        let ids = |backend| catalog(backend, &context).options.iter().map(|option| option.id).collect::<Vec<_>>();
        assert_eq!(ids(AgentBackend::Codex), ["read-only", "workspace-write", "agent", "agent-full-access"]);
        assert_eq!(ids(AgentBackend::Deepseek), ["read-only", "workspace-write", "workspace-write-never", "danger-full-access"]);
        assert_eq!(ids(AgentBackend::Pi), ["read_only", "ask", "allow", "full_access"]);
        assert!(ids(AgentBackend::Fox).is_empty());
        for backend in [AgentBackend::Codex, AgentBackend::Deepseek, AgentBackend::Pi] {
            let catalog = catalog(backend, &context);
            assert_eq!(catalog.options.iter().map(|option| option.tier).collect::<Vec<_>>(), FoxTier::ALL, "{backend:?}: Fox order");
            assert_eq!(catalog.options.iter().map(|option| option.label).collect::<Vec<_>>(), ["只读", "询问", "自动", "完全访问"]);
            assert_eq!(catalog.options.iter().filter(|option| option.full_access).map(|option| option.tier).collect::<Vec<_>>(), [FoxTier::FullAccess]);
            assert_eq!(tier_of(backend, catalog.default_mode), Some(if backend == AgentBackend::Deepseek { FoxTier::Auto } else { FoxTier::Ask }));
        }
        assert_eq!(catalog(AgentBackend::Codex, &context).switch_timing, SwitchTiming::Live);
        assert!(catalog(AgentBackend::Codex, &context).switch_note.contains("立即生效"));
        assert_eq!(catalog(AgentBackend::Deepseek, &context).switch_timing, SwitchTiming::NextTurnRestart);
        assert!(catalog(AgentBackend::Deepseek, &context).switch_note.contains("下一轮"));
        assert_eq!(catalog(AgentBackend::Pi, &context).switch_timing, SwitchTiming::HostAnswered);
        assert!(catalog(AgentBackend::Pi, &context).switch_note.contains("下一次请求"));
        // Display names used in UI text (13.19 fix ①: no hard-coded "dsh").
        let names = [AgentBackend::Codex, AgentBackend::Deepseek, AgentBackend::Pi].map(|backend| catalog(backend, &context).display_name);
        assert_eq!(names, ["Codex", "DeepSeek", "pi"]);
        let json = serde_json::to_value(catalog(AgentBackend::Pi, &context)).unwrap();
        assert_eq!(json["displayName"], "pi");
        assert_eq!(json["options"][2]["tier"], "auto");
        assert_eq!(codex_mode("read-only"), crate::backends::codex::CodexMode::ReadOnly);
        assert_eq!(codex_mode("agent-full-access"), crate::backends::codex::CodexMode::FullAccess);
        assert_eq!(codex_mode("workspace-write"), crate::backends::codex::CodexMode::WorkspaceWrite);
    }

    /// Plan C2: Codex has no peer of Fox's 自动, whatever the provider.
    #[test]
    fn codex_auto_tier_is_always_greyed_with_its_reason() {
        for url in [None, Some("https://api.openai.com/v1".to_owned()), Some("https://api.deepseek.com/v1".to_owned())] {
            let context = CatalogContext { provider_base_url: url.clone(), ..Default::default() };
            let codex = catalog(AgentBackend::Codex, &context);
            let reasons = codex.options.iter().map(|option| option.disabled_reason.as_deref()).collect::<Vec<_>>();
            assert_eq!(reasons, [None, None, Some(CODEX_NO_AUTO_TIER_REASON), None], "{url:?}");
            // A stored `agent` (written by an older build) never runs: the default instead.
            assert_eq!(effective_mode(AgentBackend::Codex, Some("agent"), false, &context), "workspace-write", "{url:?}");
        }
        // dsh / pi offer all four tiers.
        assert!(catalog(AgentBackend::Deepseek, &CatalogContext::default()).options.iter().all(|option| option.disabled_reason.is_none()));
        assert!(catalog(AgentBackend::Pi, &CatalogContext::default()).options.iter().all(|option| option.disabled_reason.is_none()));
        assert!(!provider_is_openai("https://openai.com.evil.example/v1"));
        assert!(provider_is_openai("https://API.OpenAI.com:443/v1"));
    }

    #[test]
    fn only_dsh_restarts_to_leave_full_access() {
        assert!(full_access_needs_restart(AgentBackend::Deepseek));
        assert!(!full_access_needs_restart(AgentBackend::Codex));
        assert!(!full_access_needs_restart(AgentBackend::Pi));
    }

    #[test]
    fn native_full_access_is_a_stored_choice_not_a_fox_sandbox_mode() {
        let context = CatalogContext::default();
        for backend in [AgentBackend::Codex, AgentBackend::Deepseek, AgentBackend::Pi] {
            let full = full_access_mode(backend).unwrap();
            assert_eq!(effective_mode(backend, Some(full), false, &context), full, "{backend:?}: the Agent's own stored choice survives");
            assert_eq!(effective_mode(backend, None, true, &context), full, "legacy full-access callers still map explicitly");
            assert_eq!(effective_mode(backend, Some("junk"), false, &context), catalog(backend, &context).default_mode);
        }
        let forbidden = CatalogContext { full_access_forbidden: true, ..Default::default() };
        for backend in [AgentBackend::Codex, AgentBackend::Deepseek, AgentBackend::Pi] {
            let full = full_access_mode(backend).unwrap();
            assert!(catalog(backend, &forbidden).options.iter().find(|option| option.id == full).unwrap().disabled_reason.is_some());
            assert_eq!(effective_mode(backend, Some(full), false, &forbidden), catalog(backend, &forbidden).default_mode, "{backend:?}: FORBIDDEN still refuses");
        }
    }

    #[test]
    fn host_answers_only_as_a_fail_closed_fallback() {
        let request = json!({ "toolCall": { "kind": "edit" } });
        let read = json!({ "toolCall": { "kind": "read" } });
        let options = options();
        assert_eq!(answer(AgentBackend::Codex, "read-only", &request, &options), AutoPermission::Ask);
        assert_eq!(answer(AgentBackend::Codex, "workspace-write", &request, &options), AutoPermission::Ask);
        assert_eq!(answer(AgentBackend::Codex, "agent", &request, &options), AutoPermission::Ask);
        assert_eq!(answer(AgentBackend::Codex, "agent-full-access", &request, &options), AutoPermission::Ask);
        assert_eq!(answer(AgentBackend::Deepseek, "workspace-write", &request, &options), AutoPermission::Ask);
        assert_eq!(answer(AgentBackend::Deepseek, "danger-full-access", &request, &options), AutoPermission::Ask);
        for preset in ["read-only", "workspace-write-never"] {
            assert_eq!(answer(AgentBackend::Deepseek, preset, &request, &options), AutoPermission::Reject { option_id: Some("no".into()) }, "{preset}");
        }
        assert_eq!(answer(AgentBackend::Pi, "read_only", &request, &options), AutoPermission::Reject { option_id: Some("no".into()) });
        assert_eq!(answer(AgentBackend::Pi, "read_only", &read, &options), AutoPermission::Allow { option_id: "once".into() });
        assert_eq!(answer(AgentBackend::Pi, "ask", &request, &options), AutoPermission::Ask);
        assert_eq!(answer(AgentBackend::Pi, "allow", &request, &options), AutoPermission::Allow { option_id: "once".into() });
        // Never an automatic allow for Codex or dsh, whatever the mode.
        for mode in ["workspace-write", "agent", "agent-full-access", "read-only", "workspace-write-never", "danger-full-access", "junk"] {
            for backend in [AgentBackend::Codex, AgentBackend::Deepseek] {
                assert!(!matches!(answer(backend, mode, &request, &options), AutoPermission::Allow { .. }), "{backend:?} {mode}");
            }
        }
    }

    #[test]
    fn hosts_are_parsed_without_port_or_userinfo() {
        assert_eq!(host_of("https://api.deepseek.com/anthropic").as_deref(), Some("api.deepseek.com"));
        assert_eq!(host_of("http://user:pw@gw.example.org:8443/v1").as_deref(), Some("gw.example.org"));
        assert_eq!(host_of("http://[::1]:3425/v1").as_deref(), Some("::1"));
        assert_eq!(host_of("not a url"), None);
    }
}
