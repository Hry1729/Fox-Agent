use crate::port::AgentBackend;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum BindingSource {
    FoxService { provider_id: Option<String>, endpoint_id: String, model_id: String },
    Native,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentBinding {
    #[serde(flatten)]
    pub source: BindingSource,
    pub updated_at: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BindingRecord {
    pub backend: AgentBackend,
    pub enabled: bool,
    pub binding: Option<AgentBinding>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentConfigState {
    Ready, ConfiguredUnverified, NativeImportable, InstalledUnconfigured, NotInstalled, ReadFailed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallVia { Managed, System }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallBlocker { NotInstalled, SystemOnly, RuntimeIncomplete }

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallStatus {
    pub detected: bool,
    pub launchable: bool,
    pub via: Option<InstallVia>,
    pub blocker: Option<InstallBlocker>,
}

pub struct StateInputs {
    pub install: Result<InstallStatus, String>,
    pub native_available: bool,
    pub binding: Option<AgentBinding>,
    pub binding_valid: bool,
    pub model_verified: bool,
}

pub fn derive_state(inputs: &StateInputs) -> AgentConfigState {
    let Ok(install) = &inputs.install else { return AgentConfigState::ReadFailed };
    if !install.launchable { return AgentConfigState::NotInstalled; }
    if inputs.binding.is_some() {
        return if inputs.binding_valid && inputs.model_verified { AgentConfigState::Ready }
            else { AgentConfigState::ConfiguredUnverified };
    }
    if inputs.native_available { AgentConfigState::NativeImportable }
    else { AgentConfigState::InstalledUnconfigured }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VerifyDepth { Model, Agent, Tool }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationLayer { ConfigSaved, ModelRequest, AgentOutput, ToolCall }

impl VerificationLayer {
    pub const ALL: [Self; 4] = [Self::ConfigSaved, Self::ModelRequest, Self::AgentOutput, Self::ToolCall];
    pub fn as_str(self) -> &'static str {
        match self { Self::ConfigSaved => "config_saved", Self::ModelRequest => "model_request", Self::AgentOutput => "agent_output", Self::ToolCall => "tool_call" }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationStatus { Passed, Failed, Skipped, Running }

impl VerificationStatus {
    pub fn as_str(self) -> &'static str {
        match self { Self::Passed => "passed", Self::Failed => "failed", Self::Skipped => "skipped", Self::Running => "running" }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LayerResult {
    pub layer: VerificationLayer,
    pub status: VerificationStatus,
    pub latency_ms: Option<i64>,
    pub message: String,
    pub code: Option<String>,
    pub detail: Option<String>,
}

impl LayerResult {
    pub fn new(layer: VerificationLayer, status: VerificationStatus, code: Option<&str>, latency_ms: Option<i64>) -> Self {
        let message = match status {
            VerificationStatus::Skipped => "未进行",
            VerificationStatus::Running => "正在验证…",
            VerificationStatus::Passed => match layer {
                VerificationLayer::ConfigSaved => "配置已保存",
                VerificationLayer::ModelRequest => "模型请求通过",
                VerificationLayer::AgentOutput => "Agent 输出通过",
                VerificationLayer::ToolCall => "工具调用通过",
            },
            VerificationStatus::Failed => match code.unwrap_or_default() {
                "agent_verify.cancelled" => "已停止验证。",
                "agent_verify.timeout" => "验证超时，请稍后重试。",
                "agent_binding.provider_not_found" => "选择的服务已被删除，请重新选择。",
                "agent_binding.endpoint_not_found" => "这个服务没有可用入口，请重新选择。",
                "agent_binding.missing_key" => "这个服务还没有填写 API Key。",
                "agent_verify.model_failed" => "模型请求失败，请检查地址、模型和 Key。",
                "agent_verify.agent_no_output" => "Agent 没有输出。",
                "agent_verify.tool_failed" => "工具调用没有完成。",
                _ => "配置尚未就绪，请检查后重试。",
            },
        };
        Self { layer, status, latency_ms, message: message.into(), code: code.map(str::to_owned), detail: None }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VerificationReport {
    pub identity: String,
    pub checked_at: i64,
    pub layers: Vec<LayerResult>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_priority_keeps_installation_binding_and_verification_distinct() {
        let mut inputs = StateInputs {
            install: Ok(InstallStatus { detected: true, launchable: true, via: Some(InstallVia::Managed), blocker: None }),
            native_available: false, binding: None, binding_valid: false, model_verified: false,
        };
        assert_eq!(derive_state(&inputs), AgentConfigState::InstalledUnconfigured);
        inputs.native_available = true;
        assert_eq!(derive_state(&inputs), AgentConfigState::NativeImportable);
        inputs.binding = Some(AgentBinding { source: BindingSource::Native, updated_at: 1 });
        inputs.binding_valid = true;
        assert_eq!(derive_state(&inputs), AgentConfigState::ConfiguredUnverified);
        inputs.model_verified = true;
        assert_eq!(derive_state(&inputs), AgentConfigState::Ready);
        inputs.binding_valid = false;
        assert_eq!(derive_state(&inputs), AgentConfigState::ConfiguredUnverified);
        inputs.install.as_mut().unwrap().launchable = false;
        assert_eq!(derive_state(&inputs), AgentConfigState::NotInstalled);
        inputs.install = Err("unreadable runtime".into());
        assert_eq!(derive_state(&inputs), AgentConfigState::ReadFailed);
    }
}
