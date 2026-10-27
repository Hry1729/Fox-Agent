//! Per-agent native permission list (权限按 Agent 原生列出).
//!
//! Each external Agent offers only the permission modes it really has, under
//! its own names, and Fox stops translating its own 询问 / 自动 / 只读 onto
//! Codex or dsh:
//!
//! * **Codex** (codex-acp 2.2.2 session modes): `workspace-write`
//!   (「请求审批」, approvals `on-request` + reviewer `user`), `agent`
//!   (「帮我批准」, `on-request` + `auto_review`: Codex asks its OpenAI
//!   `codex-auto-review` model, so it is greyed for any other provider) and
//!   `agent-full-access` (「完全访问权限」). Switched mid-session with
//!   `session/set_mode`. `read-only` exists natively but is not offered.
//! * **DeepSeek (dsh 0.2.1-rc.2)**: the preset / approval combinations of
//!   [`crate::backends::deepseek::DshPreset`]. dsh has no `session/set_mode`
//!   (-32601) and `set_config_option` covers only model / reasoning effort, so
//!   a change takes effect when dsh restarts at the next turn.
//! * **pi**: pi has no permission system; Fox's approval gate extension
//!   raises every tool call and the Host answers it: 只读 / 询问 / 自动, plus
//!   完全访问.
//! * **Fox**: unchanged (its own execution mode in the composer).
//!
//! The full-access option of each external Agent IS Fox's「完全访问」(no srt
//! around the backend): choosing it goes through the existing grant /
//! confirmation / audit, choosing anything else while it is on revokes it.
//! Every other option runs inside srt.
//!
//! The Host's automatic answer to `session/request_permission`
//! ([`answer`]) is only a fail-closed fallback: a request the native layer
//! let through although the mode says nothing should ask is rejected, a mode
//! that asks shows the card. Fox never auto-allows on behalf of Codex or dsh.

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

/// One entry of an Agent's permission list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NativePermissionOption {
    /// Stored id (the Agent's own mode / preset name where it has one).
    pub id: &'static str,
    /// Chinese label shown in the picker.
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
    pub options: Vec<NativePermissionOption>,
    /// The sandboxed option a new session starts in when nothing was chosen.
    pub default_mode: &'static str,
    pub switch_timing: SwitchTiming,
    /// What the UI must say about when a change applies.
    pub switch_note: &'static str,
}

/// What the catalog depends on besides the Agent.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CatalogContext {
    /// Base URL of the provider the Agent uses through Fox's unified login:
    /// the protocol address bound for this Agent, else the provider's own
    /// base URL (13.19 fix ④). `None` (or a URL without a host): the provider
    /// cannot be determined, and Codex「帮我批准」is greyed fail-closed
    /// ([`CODEX_AUTO_REVIEW_UNKNOWN_REASON`]).
    pub provider_base_url: Option<String>,
    /// `FOX_EXTERNAL_FULL_ACCESS_FORBIDDEN`: the full-access option is greyed.
    pub full_access_forbidden: bool,
}

pub const CODEX_AUTO_REVIEW_REASON: &str = "「帮我批准」由 Codex 调用 OpenAI 的 codex-auto-review 模型自动审查；当前供应商不是 OpenAI，会被拒绝";
pub const CODEX_AUTO_REVIEW_UNKNOWN_REASON: &str = "「帮我批准」由 Codex 调用 OpenAI 的 codex-auto-review 模型自动审查；无法确定当前供应商的地址（模型服务未设默认供应商，或供应商没有填写地址），按安全默认不可用";
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

fn option(id: &'static str, label: &'static str, description: &'static str, native: &'static str, full_access: bool) -> NativePermissionOption {
    NativePermissionOption { id, label, description, native, full_access, disabled_reason: None }
}

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
                option(CODEX_ASK, "请求审批", "工作区内可写；越出沙箱的操作逐个请你批准", "workspace-write（on-request · 审批人 user）", false),
                option(CODEX_AUTO_REVIEW, "帮我批准", "工作区内可写；越出沙箱的操作交给 Codex 的自动审查模型批准", "agent（on-request · 审批人 auto_review）", false),
                option(CODEX_FULL, "完全访问权限", "不套沙箱、不再询问；即 Fox 的「完全访问」", "agent-full-access", true),
            ],
            CODEX_ASK,
            SwitchTiming::Live,
            "会话中切换立即生效（session/set_mode）",
        ),
        AgentBackend::Deepseek => (
            vec![
                option(DshPreset::ReadOnly.id(), "只读", "只读工作区，不询问，越权操作直接拒绝", "sandbox read-only · approval never", false),
                option(DshPreset::WorkspaceWriteAsk.id(), "工作区可写（询问）", "工作区内可写；只在越出工作区时询问", "preset workspace-write（sandbox workspace-write · approval ask）", false),
                option(DshPreset::WorkspaceWriteNever.id(), "工作区可写（不询问）", "工作区内可写；越出工作区的操作直接拒绝", "sandbox workspace-write · approval never", false),
                option(DshPreset::DangerFullAccess.id(), "完全访问", "不套沙箱、不再询问；即 Fox 的「完全访问」", "preset danger-full-access（approval never）", true),
            ],
            // Default = what every dsh session ran in before the list
            // (approval never), so an existing runtime package keeps working.
            DshPreset::WorkspaceWriteNever.id(),
            SwitchTiming::NextTurnRestart,
            "DeepSeek 不支持会话中途切换：下一轮开始时重启 dsh 生效",
        ),
        AgentBackend::Pi => (
            vec![
                option(PI_READ_ONLY, "只读", "只放行读取类工具，其余由 Fox 拒绝", "Fox 审批拦截扩展 · 只读", false),
                option(PI_ASK, "询问", "每个工具调用都由你批准", "Fox 审批拦截扩展 · 询问", false),
                option(PI_ALLOW, "自动", "Fox 自动允许一次，仍在沙箱内", "Fox 审批拦截扩展 · 自动", false),
                option(PI_FULL, "完全访问", "不套沙箱，Fox 自动允许；即 Fox 的「完全访问」", "完全访问（无 srt）", true),
            ],
            PI_ASK,
            SwitchTiming::HostAnswered,
            "由 Fox 作答：切换后从下一次工具调用起生效",
        ),
        AgentBackend::Fox => (Vec::new(), "", SwitchTiming::HostAnswered, ""),
    };
    for option in &mut options {
        if option.full_access && context.full_access_forbidden {
            option.disabled_reason = Some(FULL_ACCESS_FORBIDDEN_NOTE.to_owned());
        }
        if backend == AgentBackend::Codex && option.id == CODEX_AUTO_REVIEW {
            // Fail-closed: only a provider known to be OpenAI keeps it.
            match context.provider_base_url.as_deref().and_then(host_of) {
                Some(host) if host == "openai.com" || host.ends_with(".openai.com") => {}
                Some(_) => option.disabled_reason = Some(CODEX_AUTO_REVIEW_REASON.to_owned()),
                None => option.disabled_reason = Some(CODEX_AUTO_REVIEW_UNKNOWN_REASON.to_owned()),
            }
        }
    }
    NativePermissionCatalog { backend: backend.as_str(), display_name: display_name(backend), options, default_mode, switch_timing, switch_note }
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

/// `id` names an option of `backend`'s list (enabled or not).
pub fn is_known(backend: AgentBackend, id: &str) -> bool {
    catalog(backend, &CatalogContext::default()).options.iter().any(|option| option.id == id.trim())
}

/// The mode a session really runs in: the stored choice when it is a known,
/// enabled option (including the Agent's own full-access option), else the
/// Agent's default. An unknown or greyed choice never widens.
///
/// `full_access` is retained for old callers: it may select the full option
/// when no stored choice exists. It is no longer tied to Fox's removed outer
/// SRT layer and cannot rewrite an existing native choice.
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
/// * Codex `workspace-write` / `agent`: the card (the user is the approver,
///   or the automatic reviewer escalated to the user).
/// * dsh sandboxed presets with approval `never` (read-only,
///   workspace-write-never): reject; the mode itself says escalations are
///   refused, so one the native layer let through is refused (fail-closed).
/// * dsh `workspace-write` (approval `ask`): the card.
/// * The full-access options (Codex `agent-full-access`, dsh
///   `danger-full-access`): nothing should ask; if something does, the card
///   (the mode does not require a refusal, and Fox never allows on Codex's or
///   dsh's behalf).
/// * pi: Fox's gate, as before: 只读 / 询问 / 自动; 完全访问 = 自动.
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
    fn each_agent_lists_only_its_own_modes() {
        let context = CatalogContext::default();
        let ids = |backend| catalog(backend, &context).options.iter().map(|option| option.id).collect::<Vec<_>>();
        assert_eq!(ids(AgentBackend::Codex), ["workspace-write", "agent", "agent-full-access"]);
        assert_eq!(ids(AgentBackend::Deepseek), ["read-only", "workspace-write", "workspace-write-never", "danger-full-access"]);
        assert_eq!(ids(AgentBackend::Pi), ["read_only", "ask", "allow", "full_access"]);
        assert!(ids(AgentBackend::Fox).is_empty());
        let labels = catalog(AgentBackend::Codex, &context).options.iter().map(|option| option.label).collect::<Vec<_>>();
        assert_eq!(labels, ["请求审批", "帮我批准", "完全访问权限"]);
        assert_eq!(catalog(AgentBackend::Codex, &context).switch_timing, SwitchTiming::Live);
        assert_eq!(catalog(AgentBackend::Deepseek, &context).switch_timing, SwitchTiming::NextTurnRestart);
        assert!(catalog(AgentBackend::Deepseek, &context).switch_note.contains("下一轮"));
        // Display names used in UI text (13.19 fix ①: no hard-coded "dsh").
        let names = [AgentBackend::Codex, AgentBackend::Deepseek, AgentBackend::Pi].map(|backend| catalog(backend, &context).display_name);
        assert_eq!(names, ["Codex", "DeepSeek", "pi"]);
        let json = serde_json::to_value(catalog(AgentBackend::Pi, &context)).unwrap();
        assert_eq!(json["displayName"], "pi");
    }

    #[test]
    fn codex_auto_review_is_greyed_for_non_openai_providers() {
        let deepseek = CatalogContext { provider_base_url: Some("https://api.deepseek.com/v1".into()), ..Default::default() };
        let codex = catalog(AgentBackend::Codex, &deepseek);
        let auto = codex.options.iter().find(|option| option.id == "agent").unwrap();
        assert!(auto.disabled_reason.as_deref().unwrap().contains("codex-auto-review"));
        let openai = CatalogContext { provider_base_url: Some("https://api.openai.com/v1".into()), ..Default::default() };
        assert!(catalog(AgentBackend::Codex, &openai).options.iter().all(|option| option.disabled_reason.is_none()));
        // 13.19 fix ④: provider unknown (no default provider, no address,
        // no host) = fail-closed, greyed with its own reason; the other two
        // options stay available.
        for unknown in [None, Some(String::new()), Some("not a url".to_owned())] {
            let context = CatalogContext { provider_base_url: unknown.clone(), ..Default::default() };
            let codex = catalog(AgentBackend::Codex, &context);
            let reasons = codex.options.iter().map(|option| option.disabled_reason.as_deref()).collect::<Vec<_>>();
            assert_eq!(reasons, [None, Some(CODEX_AUTO_REVIEW_UNKNOWN_REASON), None], "{unknown:?}");
            assert_eq!(effective_mode(AgentBackend::Codex, Some("agent"), false, &context), "workspace-write", "{unknown:?}");
        }
        // dsh / pi lists do not depend on the provider.
        assert!(catalog(AgentBackend::Deepseek, &CatalogContext::default()).options.iter().all(|option| option.disabled_reason.is_none()));
        assert!(catalog(AgentBackend::Pi, &CatalogContext::default()).options.iter().all(|option| option.disabled_reason.is_none()));
        // A greyed stored choice falls back to the default.
        assert_eq!(effective_mode(AgentBackend::Codex, Some("agent"), false, &deepseek), "workspace-write");
        assert_eq!(effective_mode(AgentBackend::Codex, Some("agent"), false, &openai), "agent");
        assert!(!provider_is_openai("https://openai.com.evil.example/v1"));
        assert!(provider_is_openai("https://API.OpenAI.com:443/v1"));
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
