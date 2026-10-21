//! What the UI shows for an external backend's effective capabilities: the
//! four dimensions of plan section 2 (approval coverage, sandbox scope,
//! cancel reach, observability), one line each, built from the backend's
//! measured declaration (`backends::*::capabilities`) folded with the
//! session's sandbox declaration (`SandboxDeclaration::apply_to`, which comes
//! from the effective policy actually written into the srt settings).
//!
//! Nothing is upgraded here: an `Unverified` gate is shown as "未验证", never
//! as "逐次审批"; a sandbox scope comes only from the declaration.

use crate::port::{AgentBackend, ApprovalGate, CancelReach, CapabilityDeclaration, Enforcement, ToolCategory, Visibility};
use crate::sandbox::{PermissionMode, SandboxDeclaration};
use serde::Serialize;
use serde_json::Value;

/// Which pi launch is in use: stock pi-acp (no Host approvals) or the Fox
/// approval gate extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PiLaunch {
    Stock,
    Gated,
}

/// The backend's own measured declaration for the launch Fox uses.
pub fn backend_declaration(backend: AgentBackend, initialize_response: &Value, pi_launch: PiLaunch) -> CapabilityDeclaration {
    match backend {
        AgentBackend::Pi => match pi_launch {
            PiLaunch::Stock => crate::backends::pi::capabilities(initialize_response),
            PiLaunch::Gated => crate::backends::pi::capabilities_with_approval_gate(initialize_response),
        },
        AgentBackend::Codex => crate::backends::codex::capabilities_for(
            initialize_response,
            crate::backends::codex::CodexMode::WorkspaceWrite,
            crate::backends::codex::Platform::current(),
        ),
        AgentBackend::Deepseek => crate::backends::deepseek::capabilities(initialize_response),
        AgentBackend::Fox => CapabilityDeclaration::unverified(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Dimension {
    /// One line for the chip.
    pub summary: String,
    /// Longer text for the tooltip.
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityView {
    pub backend: String,
    pub mode: String,
    /// `live`: from this session's `initialize` and launch; `planned`: before
    /// the first launch (static declaration + the mode Fox will use).
    pub source: &'static str,
    pub approval: Dimension,
    pub sandbox: Dimension,
    pub cancel: Dimension,
    pub observability: Dimension,
    /// Sandbox warning the UI must show next to the scope (Windows exposure).
    pub warning: Option<String>,
}

fn category_label(category: ToolCategory) -> &'static str {
    match category {
        ToolCategory::FileRead => "读文件",
        ToolCategory::FileWrite => "写文件",
        ToolCategory::Shell => "命令",
        ToolCategory::Network => "网络",
        ToolCategory::McpBusinessTool => "业务工具",
    }
}

fn enforcement_label(enforcement: Enforcement) -> &'static str {
    match enforcement {
        Enforcement::Enforced => "强制",
        Enforcement::Advisory => "仅约定",
        Enforcement::Absent => "无",
        Enforcement::Unverified => "未验证",
    }
}

fn approval_dimension(declared: &CapabilityDeclaration) -> Dimension {
    let mut per_call = Vec::new();
    let mut coarse = Vec::new();
    let mut none = Vec::new();
    let mut unverified = Vec::new();
    for (category, gate) in &declared.approval_coverage {
        let label = category_label(*category);
        match gate {
            ApprovalGate::PerCall => per_call.push(label),
            ApprovalGate::Coarse => coarse.push(label),
            ApprovalGate::None => none.push(label),
            ApprovalGate::Unverified => unverified.push(label),
        }
    }
    let summary = if per_call.is_empty() {
        "审批：无逐次审批".to_owned()
    } else {
        format!("审批：{}逐次", per_call.join("、"))
    };
    let mut parts = Vec::new();
    if !per_call.is_empty() {
        parts.push(format!("每次需审批：{}", per_call.join("、")));
    }
    if !coarse.is_empty() {
        parts.push(format!("粗粒度审批：{}", coarse.join("、")));
    }
    if !none.is_empty() {
        parts.push(format!("不经 Fox 审批直接执行：{}", none.join("、")));
    }
    if !unverified.is_empty() {
        parts.push(format!("未验证：{}", unverified.join("、")));
    }
    parts.push("审批答复只表示已写给后端，后端不回执".to_owned());
    Dimension { summary, detail: parts.join("；") }
}

/// DeepSeek in sandboxed mode: a loopback model gateway cannot be reached
/// (dsh dials loopback directly, never through the srt proxy), so such a
/// session is refused before launch.
pub const DEEPSEEK_LOOPBACK_LIMIT: &str = "限制：DeepSeek 的 base URL 若指向本机回环网关（如 Magpie 的 127.0.0.1:端口），沙箱模式下不可达（dsh 对回环地址总是直连、不经 srt 代理，srt 也无法按端口放行回环直连），Fox 会在启动前拒绝（agent_backend_loopback_requires_full_access）；需要完全访问权限（当前构建由 Host 配置 FOX_EXTERNAL_FULL_ACCESS 开启），或改用官方 api.deepseek.com";

/// Sandbox detail in full access: isolation is lifted, the private HOME is
/// not (see [`crate::sandbox::prepare_full_access`]).
pub const FULL_ACCESS_DETAIL: &str = "完全访问模式：不套沙箱，不承诺目录边界；使用 Fox 私有 HOME（与沙箱模式相同的登录与配置）";

fn sandbox_dimension(backend: AgentBackend, declared: &CapabilityDeclaration, declaration: &SandboxDeclaration, launched: bool) -> Dimension {
    let mut dimension = sandbox_dimension_base(declared, declaration, launched);
    if backend == AgentBackend::Deepseek && declaration.mode == PermissionMode::Sandboxed && !crate::backends::deepseek::LOOPBACK_VIA_PROXY_SUPPORTED {
        dimension.detail.push('；');
        dimension.detail.push_str(DEEPSEEK_LOOPBACK_LIMIT);
    }
    dimension
}

fn sandbox_dimension_base(declared: &CapabilityDeclaration, declaration: &SandboxDeclaration, launched: bool) -> Dimension {
    match declaration.mode {
        PermissionMode::FullAccess => Dimension {
            summary: "沙箱：无（完全访问）".to_owned(),
            detail: FULL_ACCESS_DETAIL.to_owned(),
        },
        PermissionMode::Sandboxed if !launched => Dimension {
            summary: "沙箱：srt（启动后确认）".to_owned(),
            detail: "默认沙箱模式；启动时按实际写入 srt 的配置确认范围，沙箱起不来则拒绝启动，不会退回完全访问".to_owned(),
        },
        PermissionMode::Sandboxed => {
            let scope = declaration.read_scope.ui_label(declaration.read_scope_verified);
            Dimension {
                summary: format!(
                    "沙箱：srt，文件{}、网络{}",
                    enforcement_label(declared.sandbox_filesystem),
                    enforcement_label(declared.sandbox_network)
                ),
                detail: format!(
                    "{scope}；可写：{}；网络白名单：{}{}",
                    if declaration.writable_roots.is_empty() { "无".to_owned() } else { declaration.writable_roots.join("、") },
                    if declaration.allowed_domains.is_empty() { "无".to_owned() } else { declaration.allowed_domains.join("、") },
                    loopback_detail(declaration)
                ),
            }
        }
    }
}

/// The loopback gateway sentence of the sandbox detail: which one loopback
/// port is allowed and from which base URL, or why none is.
fn loopback_detail(declaration: &SandboxDeclaration) -> String {
    match &declaration.loopback_gateway {
        Some(gateway) => format!("；放行本机回环端口：{}（仅此一个，来自 {}）", gateway.entry, gateway.source),
        None if !declaration.loopback_diagnostics.is_empty() => format!("；未放行本机回环端口：{}", declaration.loopback_diagnostics.join("；")),
        None => String::new(),
    }
}

fn cancel_dimension(declared: &CapabilityDeclaration) -> Dimension {
    let turn = match declared.cancel_turn {
        CancelReach::TurnConfirmed => "取消：可确认",
        CancelReach::TurnRequested => "取消：仅发送请求",
        CancelReach::Unsupported => "取消：不支持",
    };
    let detail = format!(
        "{}；{}；取消在限定时间内未得到后端确认时，结果记为“未知”并结束该后端进程",
        match declared.cancel_turn {
            CancelReach::TurnConfirmed => "后端会确认本轮已停止（session/cancel → cancelled）",
            CancelReach::TurnRequested => "发送 session/cancel，但无法确认本轮已停止",
            CancelReach::Unsupported => "后端不支持取消本轮",
        },
        if declared.cancel_process_tree { "Fox 可结束整个后端进程树" } else { "Fox 无法结束后端进程树" }
    );
    Dimension { summary: turn.to_owned(), detail }
}

fn observability_dimension(declared: &CapabilityDeclaration) -> Dimension {
    let (summary, detail) = match declared.observability {
        Visibility::ToolLifecycle => ("可观测：工具起止", "每个工具调用的开始与结束都会显示；命令内部的子进程行为不可见"),
        Visibility::FinalOnly => ("可观测：仅最终消息", "只能看到最终消息"),
        Visibility::Unverified => ("可观测：未验证", "后端声明会报告工具过程，未经实测"),
    };
    Dimension { summary: summary.to_owned(), detail: detail.to_owned() }
}

/// Build the view. `initialize_response` is `None` before the first launch
/// (`source = planned`, sandbox scope not yet known).
pub fn capability_view(
    backend: AgentBackend,
    initialize_response: Option<&Value>,
    declaration: &SandboxDeclaration,
    pi_launch: PiLaunch,
) -> CapabilityView {
    let launched = initialize_response.is_some();
    let mut declared = backend_declaration(backend, initialize_response.unwrap_or(&Value::Null), pi_launch);
    if launched {
        declaration.apply_to(&mut declared);
    } else if declaration.mode == PermissionMode::FullAccess {
        declaration.apply_to(&mut declared);
    }
    CapabilityView {
        backend: backend.as_str().to_owned(),
        mode: declaration.mode.as_str().to_owned(),
        source: if launched { "live" } else { "planned" },
        approval: approval_dimension(&declared),
        sandbox: sandbox_dimension(backend, &declared, declaration, launched),
        cancel: cancel_dimension(&declared),
        observability: observability_dimension(&declared),
        warning: declaration.exposure_warning.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sandbox::HostOs;

    #[test]
    fn stock_pi_never_claims_per_call_approval_and_gated_pi_does() {
        let planned = SandboxDeclaration::declare(PermissionMode::Sandboxed, None, HostOs::Linux);
        let stock = capability_view(AgentBackend::Pi, None, &planned, PiLaunch::Stock);
        assert_eq!(stock.approval.summary, "审批：无逐次审批");
        assert!(stock.approval.detail.contains("不经 Fox 审批直接执行：读文件、写文件、命令、网络"));
        assert_eq!(stock.source, "planned");
        assert_eq!(stock.sandbox.summary, "沙箱：srt（启动后确认）");
        assert_eq!(stock.cancel.summary, "取消：可确认");
        assert_eq!(stock.observability.summary, "可观测：工具起止");
        let gated = capability_view(AgentBackend::Pi, None, &planned, PiLaunch::Gated);
        assert_eq!(gated.approval.summary, "审批：读文件、写文件、命令逐次");
    }

    #[test]
    fn full_access_shows_no_sandbox_whatever_the_backend_has() {
        let full = SandboxDeclaration::declare(PermissionMode::FullAccess, None, HostOs::Linux);
        let view = capability_view(AgentBackend::Codex, Some(&serde_json::json!({})), &full, PiLaunch::Stock);
        assert_eq!(view.mode, "full_access");
        assert_eq!(view.sandbox.summary, "沙箱：无（完全访问）");
        assert!(view.sandbox.detail.contains("使用 Fox 私有 HOME（与沙箱模式相同的登录与配置）"), "{}", view.sandbox.detail);
        assert_eq!(view.source, "live");
    }

    #[test]
    fn deepseek_cancel_is_request_only_and_observability_unverified() {
        let planned = SandboxDeclaration::declare(PermissionMode::Sandboxed, None, HostOs::Linux);
        let view = capability_view(AgentBackend::Deepseek, None, &planned, PiLaunch::Stock);
        assert_eq!(view.cancel.summary, "取消：仅发送请求");
        assert_eq!(view.observability.summary, "可观测：未验证");
        assert!(view.cancel.detail.contains("未知"));
        assert!(view.sandbox.detail.contains("本机回环网关") && view.sandbox.detail.contains("需要完全访问权限（当前构建由 Host 配置 FOX_EXTERNAL_FULL_ACCESS 开启）"), "{}", view.sandbox.detail);
        assert!(!view.sandbox.detail.contains("请切换到"), "{}", view.sandbox.detail);
        let full = SandboxDeclaration::declare(PermissionMode::FullAccess, None, HostOs::Linux);
        let view = capability_view(AgentBackend::Deepseek, None, &full, PiLaunch::Stock);
        assert!(!view.sandbox.detail.contains("本机回环网关"), "full access is not limited: {}", view.sandbox.detail);
        let pi = capability_view(AgentBackend::Pi, None, &planned, PiLaunch::Stock);
        assert!(!pi.sandbox.detail.contains("本机回环网关"));
    }

    #[test]
    fn a_launched_sandbox_folds_srt_enforcement_into_the_scope() {
        let dir = std::env::temp_dir();
        let policy = crate::sandbox::SandboxPolicy::new(
            vec![crate::sandbox::AuthorizedDir { path: dir.join("proj"), writable: true }],
            dir.join("homes").join("pi"),
            vec![],
            vec!["api.anthropic.com".into()],
            HostOs::Linux,
        );
        let declaration = SandboxDeclaration::declare(PermissionMode::Sandboxed, Some(&policy), HostOs::Linux);
        let view = capability_view(AgentBackend::Pi, Some(&serde_json::json!({})), &declaration, PiLaunch::Stock);
        assert_eq!(view.sandbox.summary, "沙箱：srt，文件强制、网络强制");
        assert!(view.sandbox.detail.contains("api.anthropic.com"));
        assert!(view.sandbox.detail.contains("读取限定在授权文件夹"));
        assert_eq!(serde_json::to_value(&view).unwrap()["approval"]["summary"], "审批：无逐次审批");
    }
}
