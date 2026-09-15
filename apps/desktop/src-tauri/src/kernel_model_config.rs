//! Host-owned model configuration; credentials are not part of the snapshot.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

pub(crate) const KERNEL_MODEL_ADAPTER: &str = "pi-0.84.2/fox-kernel-worker-v1";

fn pi_engine() -> String { "pi".into() }
fn is_pi(value: &str) -> bool { value == "pi" }

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct NativeAdapterConfig {
    pub command: String,
    pub binary_hash: String,
}

pub(crate) fn configured_engine() -> Result<String, String> {
    let name = std::env::var("FOX_KERNEL_ENGINE").unwrap_or_else(|_| "pi".into());
    match name.as_str() {
        "pi" | "codex" | "deepseek_harness" => Ok(name),
        _ => Err("FOX_KERNEL_ENGINE must name an installed supported adapter: pi, codex or deepseek_harness".into()),
    }
}

pub(crate) fn capture_native_adapter(engine: &str) -> Result<Option<NativeAdapterConfig>, String> {
    if matches!(engine, "pi" | "deepseek_harness") { return Ok(None); }
    if engine != "codex" { return Err("Kernel engine is not installed".into()); }
    let command = std::env::var("FOX_KERNEL_CODEX_BINARY").map_err(|_| "Set FOX_KERNEL_CODEX_BINARY to the verified Codex executable before selecting this engine")?;
    let path = std::path::Path::new(&command);
    if !path.is_absolute() { return Err("native engine executable path must be absolute".into()); }
    let path = path.canonicalize().map_err(|_| "native engine executable is missing")?;
    let metadata = path.metadata().map_err(|_| "native engine executable cannot be read")?;
    if !metadata.is_file() || metadata.len() > 536_870_912 { return Err("native engine executable has an invalid size".into()); }
    let bytes = std::fs::read(&path).map_err(|_| "native engine executable cannot be read")?;
    Ok(Some(NativeAdapterConfig { command: path.to_string_lossy().into_owned(),
        binary_hash: format!("sha256:{}", hex::encode(Sha256::digest(bytes))) }))
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct KernelModelConfig {
    pub execution_profile_id: String,
    pub model_service: Value,
    pub system_prompt: String,
    pub proposal_tools: Vec<Value>,
    #[serde(default = "pi_engine", skip_serializing_if = "is_pi")]
    pub engine_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_adapter: Option<NativeAdapterConfig>,
}

fn secret_field(value: &Value) -> bool {
    match value {
        Value::Object(fields) => fields.iter().any(|(key, value)| {
            matches!(key.to_ascii_lowercase().replace('_', "").as_str(),
                "apikey" | "authorization" | "credentials" | "accesstoken" | "password" | "secret")
                || secret_field(value)
        }),
        Value::Array(values) => values.iter().any(secret_field),
        _ => false,
    }
}

impl KernelModelConfig {
    pub(crate) fn adapter_version(&self) -> Result<&'static str, String> {
        match (self.engine_id.as_str(), self.native_adapter.as_ref()) {
            ("pi", None) => Ok(KERNEL_MODEL_ADAPTER),
            ("deepseek_harness", None) if self.model_service["apiType"] == "openai-completions" => Ok("deepseek-harness-0.1.2-rc.1/fox-kernel-worker-v1"),
            ("codex", Some(native)) if self.model_service["apiType"] == "openai-responses"
                && std::path::Path::new(&native.command).is_absolute()
                && fox_engine_protocol::valid_hash(&native.binary_hash) => Ok("codex-app-server/fox-kernel-worker-v1"),
            _ => Err("unsupported or incomplete Kernel engine adapter".into()),
        }
    }

    pub(crate) fn hash(&self) -> Result<String, String> {
        self.adapter_version()?;
        let service = self.model_service.as_object().ok_or("invalid Kernel model service")?;
        if secret_field(&self.model_service) { return Err("Kernel credentials must be supplied separately".into()); }
        if self.execution_profile_id.trim().is_empty()
            || !service.get("modelId").and_then(Value::as_str).is_some_and(|id| !id.trim().is_empty()) {
            return Err("incomplete Kernel model configuration".into());
        }
        let url = service.get("baseUrl").and_then(Value::as_str)
            .and_then(|value| reqwest::Url::parse(value).ok()).ok_or("invalid Kernel model base URL")?;
        if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none()
            || !url.username().is_empty() || url.password().is_some() || url.query().is_some() || url.fragment().is_some() {
            return Err("Kernel model URL must not contain credentials or query parameters".into());
        }
        if service.keys().any(|key| !matches!(key.as_str(), "apiType" | "modelId" | "baseUrl" | "modelProfile"
            | "reasoning" | "thinkingLevel" | "contextWindow" | "maxOutputTokens" | "supportsImageInput"
            | "fauxResponses" | "fauxTokensPerSecond")) {
            return Err("unsupported Kernel model service field".into());
        }
        if let Some(api) = service.get("apiType") {
            if !matches!(api.as_str(), Some("faux" | "openai-completions" | "openai-responses" | "anthropic-messages")) {
                return Err("unsupported Kernel model API".into());
            }
        }
        if service.get("modelProfile").is_some_and(|value| !value.is_object()) {
            return Err("invalid Kernel model profile".into());
        }
        // Definitions are bounded by the registered catalog and serialized
        // configuration size, independently of the per-batch proposal limit.
        let mut names = std::collections::HashSet::new();
        for tool in &self.proposal_tools {
            let name = tool["name"].as_str().ok_or("missing Kernel proposal tool name")?;
            if !names.insert(name) || fox_engine_protocol::canonical_runtime_tool_contract(name).is_none()
                || !tool["description"].as_str().is_some_and(|text| !text.trim().is_empty())
                || !tool["parameters"].is_object() || tool["parameters"]["type"] != "object" {
                return Err("invalid Kernel proposal tool schema".into());
            }
        }
        let bytes = serde_json::to_vec(self).map_err(|_| "invalid Kernel model configuration")?;
        if bytes.len() > 1_048_576 { return Err("Kernel model configuration exceeds frame limit".into()); }
        Ok(format!("sha256:{}", hex::encode(Sha256::digest(bytes))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn config() -> KernelModelConfig {
        KernelModelConfig { engine_id: "pi".into(), native_adapter: None, execution_profile_id: "legacy".into(),
            model_service: json!({"apiType":"openai-completions","modelId":"test","baseUrl":"https://example.com/v1"}),
            system_prompt: "Host instructions".into(), proposal_tools: vec![] }
    }

    #[test]
    fn all_registered_tools_fit_but_duplicates_unknown_names_and_oversized_config_do_not() {
        let mut all = config();
        all.proposal_tools = fox_engine_protocol::TOOL_CONTRACTS.iter().map(|tool|
            json!({"name":tool.0,"description":"Registered Host tool","parameters":{"type":"object"}})).collect();
        assert!(all.hash().is_ok());
        let first = all.proposal_tools[0].clone();
        all.proposal_tools.push(first);
        assert!(all.hash().is_err());
        all.proposal_tools.pop();
        all.system_prompt = "x".repeat(1_048_576);
        assert!(all.hash().is_err());
    }

    #[test]
    fn configuration_rejects_credentials_unknown_fields_and_invalid_tool_schemas() {
        let good = config();
        assert!(good.hash().unwrap().starts_with("sha256:"));
        for (key,value) in [
            ("apiKey",json!("secret")), ("modelProfile",json!({"compat":{"Authorization":"secret"}})),
            ("headers",json!({"X-Api-Key":"secret"})), ("apiType",json!("unknown")),
            ("baseUrl",json!("https://user:pass@example.com")), ("baseUrl",json!("https://example.com?key=secret")),
        ] {
            let mut bad = good.clone(); bad.model_service[key] = value;
            assert!(bad.hash().is_err(), "{key}");
        }
        for tool in [json!({"name":"unknown","description":"bad","parameters":{"type":"object"}}),
            json!({"name":"read","description":"bad","parameters":{"type":"array"}})] {
            let mut bad = good.clone(); bad.proposal_tools.push(tool);
            assert!(bad.hash().is_err());
        }
        let mut wire = serde_json::to_value(&good).unwrap(); wire["extra"] = json!(true);
        assert!(serde_json::from_value::<KernelModelConfig>(wire).is_err());
    }
}
