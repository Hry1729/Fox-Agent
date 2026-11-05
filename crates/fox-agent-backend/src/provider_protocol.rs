use serde::{Deserialize, Serialize};

/// A protocol connection within a provider. Credentials stay in the OS keyring.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProtocolProfile {
    #[serde(default)]
    pub preset_id: Option<String>,
    pub id: String,
    pub api_type: String,
    pub base_url: String,
    pub model_id: String,
    #[serde(default)]
    pub separate_credential: bool,
    #[serde(default, skip_deserializing)]
    pub credential_configured: bool,
}

pub fn supports(api_type: &str, backend: &str) -> bool {
    match backend {
        "fox" => matches!(api_type, "openai-completions" | "anthropic-messages"),
        "pi" => matches!(api_type, "openai-completions" | "anthropic-messages" | "openai-responses"),
        "codex" => api_type == "openai-responses",
        "deepseek" => api_type == "anthropic-messages",
        _ => false,
    }
}

pub fn credential_id(provider_id: &str, profile_id: &str) -> String {
    format!("fox-protocol:{provider_id}:{profile_id}")
}

pub fn validate(profiles: &[ProtocolProfile]) -> Result<(), String> {
    let mut ids = std::collections::HashSet::new();
    for profile in profiles {
        if profile.id.is_empty() || !profile.id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') || !ids.insert(&profile.id) {
            return Err("协议配置 ID 无效或重复".into());
        }
        if !supports(&profile.api_type, "pi") || profile.base_url.trim().is_empty() {
            return Err("请选择支持的协议，并填写地址".into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn profile(id: &str, api: &str) -> ProtocolProfile {
        ProtocolProfile { preset_id: None, id: id.into(), api_type: api.into(), base_url: "https://example.com/v1".into(), model_id: "model".into(), separate_credential: false, credential_configured: false }
    }
    #[test]
    fn validates_endpoints_independently_of_agent_bindings() {
        assert!(validate(&[profile("a", "anthropic-messages"), profile("b", "openai-responses")]).is_ok());
        assert!(validate(&[profile("a", "anthropic-messages"), profile("a", "openai-responses")]).is_err());
        assert!(validate(&[profile("a", "unsupported")]).is_err());
        assert!(!supports("openai-completions", "codex"));
        assert!(!supports("openai-responses", "fox"));
    }
    #[test]
    fn credential_identity_is_per_provider_and_profile() {
        assert_ne!(credential_id("one", "a"), credential_id("two", "a"));
        assert_ne!(credential_id("one", "a"), credential_id("one", "b"));
    }
}
