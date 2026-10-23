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
    #[serde(default)]
    pub backends: Vec<String>,
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
    let mut backends = std::collections::HashSet::new();
    for profile in profiles {
        if profile.id.is_empty() || !profile.id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') || !ids.insert(&profile.id) {
            return Err("协议配置 ID 无效或重复".into());
        }
        if !supports(&profile.api_type, "pi") || profile.base_url.trim().is_empty() {
            return Err("请选择支持的协议，并填写地址".into());
        }
        for backend in &profile.backends {
            if backend == "fox" || !supports(&profile.api_type, backend) {
                return Err(format!("协议 {} 不适用于 {backend}", profile.api_type));
            }
            if !backends.insert(backend) {
                return Err(format!("请只为 {backend} 选择一份默认协议配置"));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn profile(id: &str, api: &str, backends: &[&str]) -> ProtocolProfile {
        ProtocolProfile { preset_id: None, id: id.into(), api_type: api.into(), base_url: "https://example.com/v1".into(), model_id: "model".into(), separate_credential: false, backends: backends.iter().map(|v| (*v).into()).collect(), credential_configured: false }
    }
    #[test]
    fn two_explicit_protocols_cover_external_agents() {
        assert!(validate(&[profile("a", "anthropic-messages", &["pi", "deepseek"]), profile("b", "openai-responses", &["codex"])]).is_ok());
        assert!(!supports("openai-completions", "codex"));
        assert!(!supports("openai-responses", "fox"));
    }
    #[test]
    fn rejects_ambiguous_or_incompatible_bindings() {
        assert!(validate(&[profile("a", "anthropic-messages", &["codex"])]).is_err());
        assert!(validate(&[profile("a", "anthropic-messages", &["pi"]), profile("b", "openai-responses", &["pi"])]).is_err());
        assert!(validate(&[profile("a", "anthropic-messages", &[]), profile("a", "openai-responses", &[])]).is_err());
    }
    #[test]
    fn credential_identity_is_per_provider_and_profile() {
        assert_ne!(credential_id("one", "a"), credential_id("two", "a"));
        assert_ne!(credential_id("one", "a"), credential_id("one", "b"));
    }

}
