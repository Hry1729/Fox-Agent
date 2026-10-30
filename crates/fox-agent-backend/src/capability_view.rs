//! What the UI shows for an external backend's effective capabilities: the
//! four dimensions of plan section 2 (approval coverage, sandbox scope,
//! cancel reach, observability), one line each, built from the backend's
//! measured declaration (`backends::*::capabilities`). Fox adds no outer
//! sandbox around an external backend, so the sandbox scope is the Agent's
//! own native one, exactly as measured.
//!
//! Nothing is upgraded here: an `Unverified` gate is shown as "未验证", never
//! as "逐次审批".

use crate::port::{AgentBackend, ApprovalGate, CancelReach, CapabilityDeclaration, Enforcement, ToolCategory, Visibility};
use crate::external_home::LaunchDeclaration;
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
    /// Warning the UI shows next to the scope (none since Fox stopped adding
    /// its own sandbox; kept in the payload the UI reads).
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

/// External backends get no Fox sandbox. This describes only the Fox layer;
/// the Agent's own sandbox and approval policy are shown from its native
/// measurement and remain authoritative.
pub const NO_FOX_LAYER_DETAIL: &str = "Fox 不额外包装外部 Agent；Agent 自身的沙箱、权限模式和审批保持不变，Fox 仍提供私有 HOME 与会话管理";

fn sandbox_dimension(declared: &CapabilityDeclaration) -> Dimension {
    Dimension {
        summary: format!(
            "沙箱：Fox 外层无；原生文件{}、网络{}",
            enforcement_label(declared.sandbox_filesystem),
            enforcement_label(declared.sandbox_network)
        ),
        detail: format!(
            "{}；原生文件边界：{}；原生网络边界：{}",
            NO_FOX_LAYER_DETAIL,
            enforcement_label(declared.sandbox_filesystem),
            enforcement_label(declared.sandbox_network)
        ),
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
    declaration: &LaunchDeclaration,
    pi_launch: PiLaunch,
) -> CapabilityView {
    let launched = initialize_response.is_some();
    let declared = backend_declaration(backend, initialize_response.unwrap_or(&Value::Null), pi_launch);
    CapabilityView {
        backend: backend.as_str().to_owned(),
        mode: declaration.mode.as_str().to_owned(),
        source: if launched { "live" } else { "planned" },
        approval: approval_dimension(&declared),
        sandbox: sandbox_dimension(&declared),
        cancel: cancel_dimension(&declared),
        observability: observability_dimension(&declared),
        warning: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::external_home::PermissionMode;

    #[test]
    fn stock_pi_never_claims_per_call_approval_and_gated_pi_does() {
        let planned = LaunchDeclaration::declare(PermissionMode::Sandboxed);
        let stock = capability_view(AgentBackend::Pi, None, &planned, PiLaunch::Stock);
        assert_eq!(stock.approval.summary, "审批：无逐次审批");
        assert!(stock.approval.detail.contains("不经 Fox 审批直接执行：读文件、写文件、命令、网络"));
        assert_eq!(stock.source, "planned");
        assert!(stock.sandbox.summary.starts_with("沙箱：Fox 外层无"), "{}", stock.sandbox.summary);
        assert_eq!(stock.cancel.summary, "取消：可确认");
        assert_eq!(stock.observability.summary, "可观测：工具起止");
        let gated = capability_view(AgentBackend::Pi, None, &planned, PiLaunch::Gated);
        assert_eq!(gated.approval.summary, "审批：读文件、写文件、命令逐次");
    }

    #[test]
    fn no_outer_layer_keeps_the_agents_native_sandbox_declaration() {
        let full = LaunchDeclaration::declare(PermissionMode::FullAccess);
        let view = capability_view(AgentBackend::Codex, Some(&serde_json::json!({})), &full, PiLaunch::Stock);
        assert_eq!(view.mode, "full_access");
        assert!(view.sandbox.summary.contains("Fox 外层无"), "{}", view.sandbox.summary);
        // The Agent's native declaration is never turned into "无". Linux has
        // a measured enforced write boundary; Windows / macOS remain unverified.
        let label = if cfg!(target_os = "linux") { "原生文件强制" } else { "原生文件未验证" };
        assert!(view.sandbox.summary.contains(label), "{}", view.sandbox.summary);
        assert_eq!(view.source, "live");
        assert_eq!(view.warning, None);
    }

    #[test]
    fn deepseek_cancel_is_request_only_and_observability_unverified() {
        let planned = LaunchDeclaration::declare(PermissionMode::Sandboxed);
        let view = capability_view(AgentBackend::Deepseek, None, &planned, PiLaunch::Stock);
        assert_eq!(view.cancel.summary, "取消：仅发送请求");
        assert_eq!(view.observability.summary, "可观测：未验证");
        assert!(view.cancel.detail.contains("未知"));
        assert!(!view.sandbox.detail.contains("本机回环网关"), "{}", view.sandbox.detail);
    }
}
