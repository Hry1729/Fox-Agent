//! Per-agent approval mode lists, one option per mode the Agent really has
//! (plan C2, revised by the composer batch 2026-10-31).
//!
//! This module is the ONE source of every backend's list: id, full name, the
//! two-character short name of the composer button, the Fox tier used to
//! carry a choice across backends, whether the option needs the first-time
//! second confirmation, and when a change applies. The desktop UI only shows
//! it ([`approval_modes`]).
//!
//! | 选项（全名 / 缩写） | id | 实际执行 |
//! |---|---|---|
//! | Fox / PiNative：只读 / 询问 / 自动 / 完全访问（只读 询问 自动 完全） | `read_only` `ask` `allow` `full_access` | Fox 审批引擎四档 |
//! | pi（pi-acp）：只读 / 询问 / 自动 / 完全访问（同上） | `read_only` `ask` `allow` `full_access` | pi-gate 把每个工具调用交给 Fox 审批引擎，四档与 PiNative 同一张表 |
//! | Codex：请求审批 / 帮我批准 / 完全访问权限（审批 批准 完全） | `workspace-write` `agent` `agent-full-access` | codex-acp 2.2.2 `AgentMode`：on-request·user / on-request·auto_review / never·dangerFullAccess |
//! | dsh：仅可查看 / 工作区完全修改 / 完全权限（查看 工作 完全） | `read-only` `workspace-write` `danger-full-access` | dsh 预设：read-only / workspace-write（approval ask）/ danger-full-access |
//!
//! * pi-acp 0.0.34 does expose ACP session modes, but they are pi's thinking
//!   levels (`setSessionMode` -> `set_thinking_level`), not permissions, and
//!   pi 1.4.0 has no permission system. pi's list is therefore exactly what
//!   the Fox pi-gate can enforce: every tool call held in pi's `tool_call`
//!   hook and answered by the PiNative approval engine (read_only / ask /
//!   allow / full_access). A change applies from pi's next request.
//! * Codex keeps its own sandbox and approval. `agent` (Codex "Auto review",
//!   approvals by the codex-auto-review model) is offered as「帮我批准」and
//!   greyed with the reason when the provider is not OpenAI (measured: the
//!   official DeepSeek endpoint does not serve that model, writes are
//!   refused). `read-only` is a real codex-acp mode but not listed; a stored
//!   one keeps running (never widened) and is shown as an old setting.
//!   Codex switches at once (`session/set_mode`).
//! * dsh keeps its own sandbox and approval; it has no `session/set_mode`
//!   (-32601), so a change applies when dsh restarts at the next turn. Fox's
//!   own `workspace-write-never` combination is no longer listed; a stored
//!   one keeps running and is shown as an old setting.
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
    /// Fox / PiNative: the running turn uses the new mode from its next tool call.
    NextToolCall,
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
    /// Full name shown in the picker (the Agent's own name, in Chinese).
    pub label: &'static str,
    /// Two-character name on the composer button.
    pub short_label: &'static str,
    /// One-line explanation.
    pub description: &'static str,
    /// The Agent's own name of what is configured (shown as a hint).
    pub native: &'static str,
    /// This option is the Agent's full access, bound to Fox's「完全访问」.
    pub full_access: bool,
    /// Picking it needs the first-time second confirmation (per project).
    pub needs_confirmation: bool,
    /// Offered in the picker. An unlisted option is a mode an older build
    /// could store: it keeps running (never widened) and the picker shows it
    /// only while it is the current one.
    pub listed: bool,
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
    /// The Agent's own modes, listed ones first in the picker order.
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
    /// Codex「帮我批准」(`agent`) is enabled only for an OpenAI endpoint.
    pub provider_base_url: Option<String>,
    /// `FOX_EXTERNAL_FULL_ACCESS_FORBIDDEN`: the full-access option is greyed.
    pub full_access_forbidden: bool,
}

/// Why Codex「帮我批准」is greyed off OpenAI.
pub const CODEX_NO_AUTO_TIER_REASON: &str = "「帮我批准」由 Codex 调用 OpenAI 的 codex-auto-review 模型自动审查；当前供应商不是 OpenAI，会被拒绝";
/// Fox / PiNative ids (the composer's stored execution modes).
pub const FOX_READ_ONLY: &str = "read_only";
pub const FOX_ASK: &str = "ask";
pub const FOX_ALLOW: &str = "allow";
pub const FOX_FULL: &str = "full_access";
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

#[allow(clippy::too_many_arguments)]
fn option(id: &'static str, tier: FoxTier, label: &'static str, short_label: &'static str, description: &'static str, native: &'static str) -> NativePermissionOption {
    let full_access = tier == FoxTier::FullAccess;
    NativePermissionOption { id, tier, label, short_label, description, native, full_access, needs_confirmation: full_access, listed: true, disabled_reason: None }
}

fn unlisted(mut option: NativePermissionOption) -> NativePermissionOption {
    option.listed = false;
    option
}

/// The four Fox tiers as options (Fox / PiNative and pi through pi-gate).
fn fox_tier_options() -> Vec<NativePermissionOption> {
    vec![
        option(FOX_READ_ONLY, FoxTier::ReadOnly, "只读", "只读", "只读取，不写文件、不执行命令", "Fox 审批引擎 · 只读"),
        option(FOX_ASK, FoxTier::Ask, "询问", "询问", "写入、命令和联网前都先问你；命中项目规则的照规则处理", "Fox 审批引擎 · 询问"),
        option(FOX_ALLOW, FoxTier::Auto, "自动", "自动", "项目内写入和低风险命令直接执行；高风险命令和受保护路径仍会询问", "Fox 审批引擎 · 自动"),
        option(FOX_FULL, FoxTier::FullAccess, "完全访问", "完全", "所有操作直接执行，包括项目外和受保护路径；内置禁止规则和项目禁止规则仍然生效", "Fox 审批引擎 · 完全访问"),
    ]
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
                option(CODEX_ASK, FoxTier::Ask, "请求审批", "审批", "工作区内可写；越出工作区或联网前由 Codex 发起审批，Fox 转成审批卡请你批准", "workspace-write（on-request · 审批人 user）"),
                option(CODEX_AUTO_REVIEW, FoxTier::Auto, "帮我批准", "批准", "工作区内可写；需要审批的操作由 Codex 的自动审查模型判断后批准，只有它判为不安全的才问你", "agent（on-request · 审批人 auto_review）"),
                option(CODEX_FULL, FoxTier::FullAccess, "完全访问权限", "完全", "不套沙箱、不再询问，可联网和访问电脑上的任何文件", "agent-full-access（never · danger-full-access）"),
                unlisted(option(CODEX_READ_ONLY, FoxTier::ReadOnly, "只读（旧设置）", "只读", "Codex 原生只读沙箱；越权时由 Codex 发起审批。不再提供，切换后不能再选回", "read-only（sandbox read-only · on-request · 审批人 user）")),
            ],
            CODEX_ASK,
            SwitchTiming::Live,
            "切换立即生效（Codex session/set_mode）",
        ),
        AgentBackend::Deepseek => (
            vec![
                option(DshPreset::ReadOnly.id(), FoxTier::ReadOnly, "仅可查看", "查看", "只读工作区，不询问，越权操作直接拒绝", "preset read-only（sandbox read-only · approval never）"),
                option(DshPreset::WorkspaceWriteAsk.id(), FoxTier::Ask, "工作区完全修改", "工作", "工作区内可自由修改；越出工作区时由 dsh 发起审批，Fox 转成审批卡", "preset workspace-write（sandbox workspace-write · approval ask）"),
                option(DshPreset::DangerFullAccess.id(), FoxTier::FullAccess, "完全权限", "完全", "不套沙箱、不再询问", "preset danger-full-access（approval never）"),
                unlisted(option(DshPreset::WorkspaceWriteNever.id(), FoxTier::Auto, "工作区可写·不询问（旧设置）", "工作", "工作区内可写、不询问；越出工作区的操作直接拒绝。不再提供，切换后不能再选回", "sandbox workspace-write · approval never")),
            ],
            // dsh's own default preset (workspace-write, approval ask).
            DshPreset::WorkspaceWriteAsk.id(),
            SwitchTiming::NextTurnRestart,
            "DeepSeek 不支持会话中途切换：下一轮开始时重启 dsh 生效",
        ),
        AgentBackend::Pi => (
            fox_tier_options(),
            PI_ASK,
            SwitchTiming::HostAnswered,
            "由 Fox 审批引擎经 pi-gate 作答：切换后从 pi 的下一次请求起生效",
        ),
        AgentBackend::Fox => (Vec::new(), "", SwitchTiming::HostAnswered, ""),
    };
    for option in &mut options {
        if option.full_access && context.full_access_forbidden {
            option.disabled_reason = Some(FULL_ACCESS_FORBIDDEN_NOTE.to_owned());
        }
        if backend == AgentBackend::Codex
            && option.id == CODEX_AUTO_REVIEW
            && !context.provider_base_url.as_deref().is_some_and(provider_is_openai)
        {
            // The codex-auto-review model is served by OpenAI only.
            option.disabled_reason = Some(CODEX_NO_AUTO_TIER_REASON.to_owned());
        }
    }
    NativePermissionCatalog { backend: backend.as_str(), display_name: display_name(backend), options, default_mode, switch_timing, switch_note }
}

/// The Fox (PiNative) conversation's list: Fox's own four execution modes,
/// with the composer's ids. A change applies from the next tool call.
pub fn fox_catalog(context: &CatalogContext) -> NativePermissionCatalog {
    let mut options = fox_tier_options();
    for option in &mut options {
        option.native = match option.tier {
            FoxTier::ReadOnly => "PiNative · 只读",
            FoxTier::Ask => "PiNative · 询问",
            FoxTier::Auto => "PiNative · 自动",
            FoxTier::FullAccess => "PiNative · 完全访问",
        };
    }
    let _ = context;
    NativePermissionCatalog {
        backend: AgentBackend::Fox.as_str(),
        display_name: display_name(AgentBackend::Fox),
        options,
        default_mode: FOX_ASK,
        switch_timing: SwitchTiming::NextToolCall,
        switch_note: "运行中切换，从下一次工具调用起生效",
    }
}

/// Every backend's approval mode list, the one source the composer shows
/// (Fox / PiNative included).
pub fn approval_modes(backend: AgentBackend, context: &CatalogContext) -> NativePermissionCatalog {
    match backend {
        AgentBackend::Fox => fox_catalog(context),
        other => catalog(other, context),
    }
}

/// The mode a new conversation of `backend` starts in when the user switches
/// to it from a choice of tier `from` (composer batch 2026-10-31):
/// * the equivalent listed, enabled option of the same tier;
/// * no such option, no previous choice (`None`), or full access: the
///   backend's default (询问 / 请求审批 / 工作区完全修改). Full access is never
///   chosen automatically; it always takes the user's own pick and the
///   first-time confirmation.
pub fn mode_for_tier(backend: AgentBackend, from: Option<FoxTier>, context: &CatalogContext) -> &'static str {
    let catalog = approval_modes(backend, context);
    match from {
        Some(tier) if tier != FoxTier::FullAccess => catalog
            .options
            .iter()
            .find(|option| option.tier == tier && option.listed && !option.full_access && option.disabled_reason.is_none())
            .map(|option| option.id)
            .unwrap_or(catalog.default_mode),
        _ => catalog.default_mode,
    }
}

impl FoxTier {
    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|tier| tier.as_str() == value.trim())
    }
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

    fn listed(backend: AgentBackend, context: &CatalogContext) -> Vec<(&'static str, &'static str, &'static str)> {
        approval_modes(backend, context).options.iter().filter(|option| option.listed).map(|option| (option.id, option.label, option.short_label)).collect()
    }

    /// Composer batch: each backend lists its own modes one to one, with the
    /// Agent's own names and a two-character short name.
    #[test]
    fn each_backend_lists_its_own_modes_with_full_and_short_names() {
        let context = CatalogContext::default();
        assert_eq!(listed(AgentBackend::Codex, &context), [("workspace-write", "请求审批", "审批"), ("agent", "帮我批准", "批准"), ("agent-full-access", "完全访问权限", "完全")]);
        assert_eq!(listed(AgentBackend::Deepseek, &context), [("read-only", "仅可查看", "查看"), ("workspace-write", "工作区完全修改", "工作"), ("danger-full-access", "完全权限", "完全")]);
        let fox = [("read_only", "只读", "只读"), ("ask", "询问", "询问"), ("allow", "自动", "自动"), ("full_access", "完全访问", "完全")];
        assert_eq!(listed(AgentBackend::Pi, &context), fox);
        assert_eq!(listed(AgentBackend::Fox, &context), fox);
        assert!(catalog(AgentBackend::Fox, &context).options.is_empty(), "external-only callers keep an empty Fox list");
        for backend in [AgentBackend::Fox, AgentBackend::Pi, AgentBackend::Codex, AgentBackend::Deepseek] {
            let catalog = approval_modes(backend, &context);
            let shorts = catalog.options.iter().filter(|option| option.listed).map(|option| option.short_label).collect::<Vec<_>>();
            assert!(shorts.iter().all(|short| short.chars().count() == 2), "{backend:?}: two characters");
            let mut unique = shorts.clone();
            unique.sort();
            unique.dedup();
            assert_eq!(unique.len(), shorts.len(), "{backend:?}: distinguishable");
            for option in &catalog.options {
                assert_eq!(option.needs_confirmation, option.full_access, "{backend:?} {}", option.id);
                assert_eq!(option.full_access, option.tier == FoxTier::FullAccess);
            }
            assert_eq!(tier_of_in(&catalog, catalog.default_mode), Some(FoxTier::Ask), "{backend:?}: default is the asking mode");
        }
        assert_eq!(approval_modes(AgentBackend::Fox, &context).switch_timing, SwitchTiming::NextToolCall);
        assert_eq!(catalog(AgentBackend::Codex, &context).switch_timing, SwitchTiming::Live);
        assert_eq!(catalog(AgentBackend::Deepseek, &context).switch_timing, SwitchTiming::NextTurnRestart);
        assert_eq!(catalog(AgentBackend::Pi, &context).switch_timing, SwitchTiming::HostAnswered);
        let names = [AgentBackend::Codex, AgentBackend::Deepseek, AgentBackend::Pi].map(|backend| catalog(backend, &context).display_name);
        assert_eq!(names, ["Codex", "DeepSeek", "pi"]);
        let json = serde_json::to_value(catalog(AgentBackend::Codex, &context)).unwrap();
        assert_eq!(json["options"][0]["shortLabel"], "审批");
        assert_eq!(json["options"][2]["needsConfirmation"], true);
        assert_eq!(json["options"][3]["listed"], false);
        assert_eq!(serde_json::to_value(SwitchTiming::NextToolCall).unwrap(), "next_tool_call");
        assert_eq!(codex_mode("read-only"), crate::backends::codex::CodexMode::ReadOnly);
        assert_eq!(codex_mode("agent"), crate::backends::codex::CodexMode::AutoReview);
        assert_eq!(codex_mode("agent-full-access"), crate::backends::codex::CodexMode::FullAccess);
        assert_eq!(codex_mode("workspace-write"), crate::backends::codex::CodexMode::WorkspaceWrite);
        assert_eq!(dsh_preset("workspace-write"), crate::backends::deepseek::DshPreset::WorkspaceWriteAsk);
    }

    fn tier_of_in(catalog: &NativePermissionCatalog, id: &str) -> Option<FoxTier> {
        catalog.options.iter().find(|option| option.id == id).map(|option| option.tier)
    }

    /// An option an older build stored but the list no longer offers keeps
    /// running: never silently widened to the default.
    #[test]
    fn unlisted_stored_modes_keep_running() {
        let context = CatalogContext::default();
        assert_eq!(effective_mode(AgentBackend::Codex, Some("read-only"), false, &context), "read-only");
        assert_eq!(effective_mode(AgentBackend::Deepseek, Some("workspace-write-never"), false, &context), "workspace-write-never");
    }

    /// Codex「帮我批准」(`agent`, auto review) is offered on OpenAI only.
    #[test]
    fn codex_auto_review_is_offered_only_on_openai() {
        for (url, enabled) in [(None, false), (Some("https://api.openai.com/v1"), true), (Some("https://api.deepseek.com/v1"), false), (Some("https://openai.com.evil.example/v1"), false)] {
            let context = CatalogContext { provider_base_url: url.map(str::to_owned), ..Default::default() };
            let codex = catalog(AgentBackend::Codex, &context);
            let reason = codex.options.iter().find(|option| option.id == "agent").unwrap().disabled_reason.clone();
            assert_eq!(reason.is_none(), enabled, "{url:?}");
            if !enabled {
                assert_eq!(reason.as_deref(), Some(CODEX_NO_AUTO_TIER_REASON));
            }
            let expected = if enabled { "agent" } else { "workspace-write" };
            assert_eq!(effective_mode(AgentBackend::Codex, Some("agent"), false, &context), expected, "{url:?}: a greyed choice never runs");
        }
        assert!(catalog(AgentBackend::Deepseek, &CatalogContext::default()).options.iter().all(|option| option.disabled_reason.is_none()));
        assert!(catalog(AgentBackend::Pi, &CatalogContext::default()).options.iter().all(|option| option.disabled_reason.is_none()));
        assert!(!provider_is_openai("https://openai.com.evil.example/v1"));
        assert!(provider_is_openai("https://API.OpenAI.com:443/v1"));
    }

    /// Switching backends carries the tier; full access never carries; a tier
    /// the new backend lacks falls back to its asking default.
    #[test]
    fn switching_backend_maps_the_tier_and_never_selects_full_access() {
        let none = CatalogContext::default();
        let openai = CatalogContext { provider_base_url: Some("https://api.openai.com/v1".into()), ..Default::default() };
        use FoxTier::*;
        assert_eq!(mode_for_tier(AgentBackend::Codex, None, &none), "workspace-write", "fresh: 请求审批");
        assert_eq!(mode_for_tier(AgentBackend::Codex, Some(Ask), &none), "workspace-write");
        assert_eq!(mode_for_tier(AgentBackend::Codex, Some(ReadOnly), &none), "workspace-write", "Codex lists no read-only");
        assert_eq!(mode_for_tier(AgentBackend::Codex, Some(Auto), &none), "workspace-write", "greyed 帮我批准 off OpenAI");
        assert_eq!(mode_for_tier(AgentBackend::Codex, Some(Auto), &openai), "agent");
        assert_eq!(mode_for_tier(AgentBackend::Deepseek, Some(ReadOnly), &none), "read-only");
        assert_eq!(mode_for_tier(AgentBackend::Deepseek, Some(Ask), &none), "workspace-write");
        assert_eq!(mode_for_tier(AgentBackend::Deepseek, Some(Auto), &none), "workspace-write");
        assert_eq!(mode_for_tier(AgentBackend::Pi, Some(Auto), &none), "allow");
        assert_eq!(mode_for_tier(AgentBackend::Pi, Some(ReadOnly), &none), "read_only");
        assert_eq!(mode_for_tier(AgentBackend::Fox, Some(Auto), &none), "allow");
        for backend in [AgentBackend::Fox, AgentBackend::Pi, AgentBackend::Codex, AgentBackend::Deepseek] {
            let mode = mode_for_tier(backend, Some(FullAccess), &openai);
            assert!(!approval_modes(backend, &openai).options.iter().any(|option| option.id == mode && option.full_access), "{backend:?}");
        }
        assert_eq!(FoxTier::parse("auto"), Some(Auto));
        assert_eq!(FoxTier::parse("junk"), None);
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
