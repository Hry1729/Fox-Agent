//! Host-owned model configuration; credentials are not part of the snapshot.
//!
//! ## One engine
//!
//! The single Pi mainline pins every Kernel run to the Pi engine. `codex` and
//! `deepseek_harness` are retired: their recorded `engine_id` values stay
//! readable (the column keeps its CHECK, old rows are never rewritten), but no
//! run can be started or frozen with them any more, and an explicit request is
//! refused with a recognizable code instead of falling back to another model
//! adapter. Removing an engine never removes the model service: Pi keeps
//! driving whichever provider the user configured, including DeepSeek.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

pub(crate) const KERNEL_MODEL_ADAPTER: &str = "pi-1.4.0/fox-kernel-worker-v1";

/// Refusal prefix of a retired Kernel engine. Stable, greppable, and never a
/// silent substitution: the Host reports it and starts nothing.
pub(crate) const KERNEL_ENGINE_RETIRED_CODE: &str = "kernel_engine_retired";

/// The only engine this build executes.
pub(crate) const SUPPORTED_KERNEL_ENGINE: &str = "pi";

/// Engines retired by the single Pi mainline. Readable, never executable.
pub(crate) const RETIRED_KERNEL_ENGINES: [&str; 2] = ["codex", "deepseek_harness"];

fn pi_engine() -> String { SUPPORTED_KERNEL_ENGINE.into() }
fn is_pi(value: &str) -> bool { value == SUPPORTED_KERNEL_ENGINE }

fn retired_engine_error(engine: &str) -> String {
    format!(
        "{KERNEL_ENGINE_RETIRED_CODE}: Kernel engine `{engine}` is retired in this build; only `{SUPPORTED_KERNEL_ENGINE}` runs. Old records stay readable, but no run is started with a retired engine."
    )
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct NativeAdapterConfig {
    pub command: String,
    pub binary_hash: String,
}

/// The engine a new run uses. Unset or empty means Pi.
///
/// `codex` / `deepseek_harness` are refused as retired: the process must not
/// silently select another adapter because an old deployment still exports
/// `FOX_KERNEL_ENGINE`.
pub(crate) fn configured_engine() -> Result<String, String> {
    let name = std::env::var("FOX_KERNEL_ENGINE").unwrap_or_else(|_| SUPPORTED_KERNEL_ENGINE.into());
    let name = name.trim();
    if name.is_empty() || name == SUPPORTED_KERNEL_ENGINE { return Ok(SUPPORTED_KERNEL_ENGINE.into()); }
    if RETIRED_KERNEL_ENGINES.contains(&name) { return Err(retired_engine_error(name)); }
    Err(format!(
        "{KERNEL_ENGINE_RETIRED_CODE}: FOX_KERNEL_ENGINE=`{name}` is not a supported engine; only `{SUPPORTED_KERNEL_ENGINE}` runs in this build."
    ))
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
    /// Only reachable through a record written before the single Pi mainline.
    /// Kept so an old snapshot still round-trips (readable) without ever being
    /// executable.
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

/// Whether a proposal tool's parameter schema is object-shaped at the root.
///
/// A tool may declare its arguments directly as an object, or as a union of
/// object variants when the call has genuinely mutually exclusive shapes:
/// `compute_job_start` takes either `{idempotencyKey, params}` or `{jobId}`, and
/// the runtime worker therefore emits `anyOf`. Both are object-rooted function
/// signatures. Requiring the literal `type == "object"` rejected the second form
/// and made the whole Kernel description fail for any Run whose tool catalogue
/// included a union-shaped tool — which is every attachment-capable Run, i.e.
/// exactly the data-analysis scenario. Anything that is not object-shaped at the
/// root is still rejected.
fn object_rooted_parameters(parameters: &Value) -> bool {
    if parameters["type"] == "object" {
        return true;
    }
    ["anyOf", "oneOf"].iter().any(|key| {
        parameters[*key].as_array().is_some_and(|variants| {
            !variants.is_empty() && variants.iter().all(object_rooted_parameters)
        })
    })
}

impl KernelModelConfig {
    pub(crate) fn adapter_version(&self) -> Result<&'static str, String> {
        match (self.engine_id.as_str(), self.native_adapter.as_ref()) {
            (SUPPORTED_KERNEL_ENGINE, None) => Ok(KERNEL_MODEL_ADAPTER),
            (SUPPORTED_KERNEL_ENGINE, Some(_)) => {
                Err("the Pi engine runs in the sidecar and cannot carry a native adapter".into())
            }
            (retired, _) if RETIRED_KERNEL_ENGINES.contains(&retired) => Err(retired_engine_error(retired)),
            (other, _) => Err(format!(
                "{KERNEL_ENGINE_RETIRED_CODE}: unsupported or incomplete Kernel engine adapter `{other}`"
            )),
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
            if !names.insert(name) {
                return Err(format!("invalid Kernel proposal tool schema: `{name}` is duplicated").into());
            }
            if fox_engine_protocol::canonical_runtime_tool_contract(name).is_none() {
                return Err(format!("invalid Kernel proposal tool schema: `{name}` is not a registered tool contract").into());
            }
            if !tool["description"].as_str().is_some_and(|text| !text.trim().is_empty()) {
                return Err(format!("invalid Kernel proposal tool schema: `{name}` has no description").into());
            }
            if !object_rooted_parameters(&tool["parameters"]) {
                return Err(format!("invalid Kernel proposal tool schema: `{name}` has no object parameter schema").into());
            }
        }
        let bytes = crate::canonical_json::to_vec(self).map_err(|_| "invalid Kernel model configuration")?;
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

    /// A union of object variants is an object-rooted function signature; the
    /// previous literal `type == "object"` check rejected the runtime worker's own
    /// `compute_job_start` definition and failed the whole description.
    #[test]
    fn a_union_of_object_variants_is_a_valid_tool_schema_but_anything_else_is_not() {
        let mut config = config();
        let union = json!({"name":"compute_job_start","description":"Start or resume",
            "parameters":{"anyOf":[
                {"type":"object","properties":{"idempotencyKey":{"type":"string"}}},
                {"type":"object","properties":{"jobId":{"type":"string"}}}]}});
        config.proposal_tools = vec![union];
        assert!(config.hash().is_ok(), "an anyOf of object variants must be accepted");

        let nested = json!({"name":"compute_job_start","description":"Start or resume",
            "parameters":{"oneOf":[{"anyOf":[{"type":"object"}]}]}});
        config.proposal_tools = vec![nested];
        assert!(config.hash().is_ok(), "nested unions of objects are still object-rooted");

        for rejected in [
            json!({"type":"array"}),
            json!({"anyOf":[{"type":"string"}]}),
            json!({"anyOf":[]}),
            json!({"type":"object"}),
        ] {
            let tool = json!({"name":"compute_job_start","description":"Start or resume",
                "parameters": rejected});
            config.proposal_tools = vec![tool];
            let outcome = config.hash();
            if rejected["type"] == "object" {
                assert!(outcome.is_ok());
            } else {
                assert!(
                    outcome.is_err(),
                    "a non object-rooted schema must still be rejected: {rejected}"
                );
            }
        }
    }

    #[test]
    fn configuration_rejects_credentials_unknown_fields_and_invalid_tool_schemas() {        let good = config();
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
    /// Serialize the tests that read or write the process environment.
    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock().unwrap_or_else(|error| error.into_inner())
    }

    fn with_engine_env<T>(value: Option<&str>, body: impl FnOnce() -> T) -> T {
        let _guard = env_lock();
        let original = std::env::var("FOX_KERNEL_ENGINE").ok();
        match value {
            Some(value) => std::env::set_var("FOX_KERNEL_ENGINE", value),
            None => std::env::remove_var("FOX_KERNEL_ENGINE"),
        }
        let outcome = body();
        match original {
            Some(original) => std::env::set_var("FOX_KERNEL_ENGINE", original),
            None => std::env::remove_var("FOX_KERNEL_ENGINE"),
        }
        outcome
    }

    /// Stage one of the single Pi mainline: one engine, chosen explicitly.
    #[test]
    fn the_only_engine_this_build_runs_is_pi() {
        assert_eq!(with_engine_env(None, configured_engine), Ok("pi".into()));
        assert_eq!(with_engine_env(Some("pi"), configured_engine), Ok("pi".into()));
        assert_eq!(with_engine_env(Some(" pi "), configured_engine), Ok("pi".into()));
        assert_eq!(with_engine_env(Some(""), configured_engine), Ok("pi".into()));
    }

    /// A retired engine is refused with a recognizable code. It must never
    /// quietly fall back to another adapter because an old deployment still
    /// exports `FOX_KERNEL_ENGINE`.
    #[test]
    fn a_retired_or_unknown_engine_is_refused_with_a_stable_code() {
        for engine in RETIRED_KERNEL_ENGINES {
            let refusal = with_engine_env(Some(engine), configured_engine).unwrap_err();
            assert!(refusal.starts_with(KERNEL_ENGINE_RETIRED_CODE), "{refusal}");
            assert!(refusal.contains(engine), "the refusal names the engine: {refusal}");
        }
        for engine in ["gpt", "codex-cli", "pi-1.4.0"] {
            let refusal = with_engine_env(Some(engine), configured_engine).unwrap_err();
            assert!(refusal.starts_with(KERNEL_ENGINE_RETIRED_CODE), "{refusal}");
        }
    }

    /// A retired run record stays readable (deserializable, unchanged engine
    /// id) but is not an executable Kernel configuration.
    #[test]
    fn an_old_retired_engine_record_stays_readable_but_is_never_executable() {
        for engine in RETIRED_KERNEL_ENGINES {
            let wire = json!({
                "executionProfileId": "legacy",
                "modelService": {"apiType": "openai-completions", "modelId": "old", "baseUrl": "https://example.com/v1"},
                "systemPrompt": "Host instructions",
                "proposalTools": [],
                "engineId": engine
            });
            let record: KernelModelConfig = serde_json::from_value(wire).expect("old record still reads");
            assert_eq!(record.engine_id, engine);
            let refusal = record.adapter_version().unwrap_err();
            assert!(refusal.starts_with(KERNEL_ENGINE_RETIRED_CODE), "{refusal}");
            assert!(record.hash().is_err(), "a retired engine hash must not freeze a run");
        }
        // A Pi record with a stale native adapter is refused too: the only
        // engine runs in the sidecar and has no binary to attest.
        let mut stale = config();
        stale.native_adapter = Some(NativeAdapterConfig { command: "/tmp/codex".into(), binary_hash: format!("sha256:{}", "0".repeat(64)) });
        assert!(stale.adapter_version().is_err());
        assert!(stale.hash().is_err());
    }

}
