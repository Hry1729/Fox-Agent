//! Native configuration selects the model; only Host configuration or an
//! explicit import grants its network destination. Grants live outside the
//! writable backend HOME and expire when the selected source changes.

use crate::port::AgentBackend;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Endpoint {
    pub scheme: String,
    pub host: String,
    pub port: u16,
    pub entry: String,
    pub loopback: bool,
}

impl Endpoint {
    /// Never include the input URL in an error: it may contain credentials.
    pub fn parse(value: &str) -> Result<Self, String> {
        if value.trim() != value
            || value.chars().any(char::is_whitespace)
            || value.contains(['$', '\\'])
            || value.contains("process.env")
        {
            return Err("模型地址必须是静态的 HTTP 或 HTTPS URL".into());
        }
        let url = url::Url::parse(value).map_err(|_| "模型地址不是有效 URL")?;
        if !matches!(url.scheme(), "http" | "https")
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return Err("模型地址必须使用 HTTP 或 HTTPS，且不能包含登录信息".into());
        }
        let host = match url.host().ok_or("模型地址缺少主机")? {
            url::Host::Domain(host) => host.trim_end_matches('.').to_owned(),
            url::Host::Ipv4(host) => {
                if host.is_unspecified() {
                    return Err("模型地址不能使用未指定的 IP".into());
                }
                host.to_string()
            }
            url::Host::Ipv6(host) => {
                if host.is_unspecified() {
                    return Err("模型地址不能使用未指定的 IP".into());
                }
                host.to_string()
            }
        };
        if host.is_empty() || host.contains('*') {
            return Err("模型地址缺少精确主机".into());
        }
        let port = url
            .port_or_known_default()
            .filter(|port| *port != 0)
            .ok_or("模型地址缺少有效端口")?;
        let loopback = crate::loopback::is_loopback_host(&host)
            || host
                .parse::<std::net::Ipv6Addr>()
                .ok()
                .and_then(|ip| ip.to_ipv4_mapped())
                .is_some_and(|ip| ip.is_loopback());
        Ok(Self {
            scheme: url.scheme().to_owned(),
            entry: crate::loopback::allow_entry(&host, port),
            host,
            port,
            loopback,
        })
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NativeModel {
    pub endpoint: Option<Endpoint>,
    pub provider: Option<String>,
    pub model: Option<String>,
    /// A digest, never the source content or credentials.
    pub fingerprint: String,
}

fn source_files(backend: AgentBackend) -> &'static [&'static str] {
    match backend {
        AgentBackend::Pi => &[".pi/agent/models.json", ".pi/agent/settings.json"],
        AgentBackend::Deepseek => &[
            ".dsh/cordis.patch.yml",
            ".dsh/profiles/acp/cordis.patch.yml",
        ],
        AgentBackend::Codex => &[".codex/config.toml"],
        AgentBackend::Fox => &[],
    }
}

fn read(path: &Path) -> Result<Option<Vec<u8>>, String> {
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("无法读取模型配置".into()),
        Ok(metadata)
            if !metadata.is_file()
                || metadata.file_type().is_symlink()
                || metadata.len() > 1024 * 1024 =>
        {
            return Err("模型配置必须是小于 1 MiB 的普通文件".into())
        }
        _ => {}
    }
    std::fs::read(path)
        .map(Some)
        .map_err(|_| "无法读取模型配置".into())
}

fn file(home: &Path, relative: &str) -> PathBuf {
    relative
        .split('/')
        .fold(home.to_path_buf(), |path, part| path.join(part))
}

pub fn source_fingerprint(backend: AgentBackend, home: &Path) -> Result<String, String> {
    let mut digest = Sha256::new();
    for relative in source_files(backend) {
        digest.update(relative.as_bytes());
        match read(&file(home, relative))? {
            Some(bytes) => {
                digest.update([1]);
                digest.update((bytes.len() as u64).to_le_bytes());
                digest.update(bytes);
            }
            None => digest.update([0]),
        }
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn json(home: &Path, relative: &str) -> Result<Value, String> {
    read(&file(home, relative))?
        .map(|bytes| {
            serde_json::from_slice(&bytes).map_err(|_| format!("{relative} 不是有效 JSON"))
        })
        .transpose()
        .map(|value| value.unwrap_or(Value::Null))
}

fn pi_model(home: &Path, project: &Path) -> Result<NativeModel, String> {
    let settings = json(home, ".pi/agent/settings.json")?;
    let project_settings = json(project, ".pi/settings.json")?;
    for key in ["defaultProvider", "defaultModel", "httpProxy"] {
        if project_settings
            .get(key)
            .is_some_and(|value| Some(value) != settings.get(key))
        {
            return Err(format!(
                "项目 .pi/settings.json 覆盖了 {key}；请在 Fox 私有模型配置中明确选择后重新导入"
            ));
        }
    }
    let mut out = NativeModel {
        provider: settings
            .get("defaultProvider")
            .and_then(Value::as_str)
            .map(str::to_owned),
        model: settings
            .get("defaultModel")
            .and_then(Value::as_str)
            .map(str::to_owned),
        ..Default::default()
    };
    let models = json(home, ".pi/agent/models.json")?;
    let providers = models.get("providers").and_then(Value::as_object);
    let Some(provider) = out.provider.as_deref() else {
        if providers.is_some_and(|providers| !providers.is_empty()) {
            return Err(
                "pi 有多个或未选定的模型配置；请明确 defaultProvider 和 defaultModel".into(),
            );
        }
        return Ok(out);
    };
    if let Some(config) = providers.and_then(|providers| providers.get(provider)) {
        let model = out.model.as_deref().ok_or("pi 未指定 defaultModel")?;
        let model_config = config
            .get("models")
            .and_then(Value::as_array)
            .and_then(|models| {
                models
                    .iter()
                    .find(|value| value.get("id").and_then(Value::as_str) == Some(model))
            });
        let base = model_config
            .and_then(|value| value.get("baseUrl"))
            .or_else(|| config.get("baseUrl"));
        if let Some(base) = base {
            out.endpoint = Some(Endpoint::parse(
                base.as_str().ok_or("pi baseUrl 不是静态字符串")?,
            )?);
        } else if crate::sandbox::pi_provider_domains(provider).is_none() {
            return Err("所选 pi 模型缺少静态 baseUrl".into());
        }
        if config.get("oauth").is_some() {
            return Err("动态 OAuth 模型地址需由 Host 明确授权，当前无法确定实际端点".into());
        }
    }
    if out.endpoint.is_none() {
        if let Some(hosts) = crate::sandbox::pi_provider_domains(provider) {
            if hosts.len() == 1 {
                out.endpoint = Some(Endpoint::parse(&format!("https://{}", hosts[0]))?);
            }
        }
    }
    Ok(out)
}

pub fn resolve(backend: AgentBackend, home: &Path, project: &Path) -> Result<NativeModel, String> {
    let mut out = match backend {
        AgentBackend::Pi => pi_model(home, project)?,
        AgentBackend::Deepseek => {
            let candidates = crate::backends::deepseek::base_url_candidates(home, project);
            let mut endpoints = Vec::new();
            for candidate in candidates {
                let endpoint = Endpoint::parse(&candidate.value)?;
                if !endpoints.contains(&endpoint) {
                    endpoints.push(endpoint);
                }
            }
            if endpoints.len() > 1 {
                return Err("DeepSeek 配置有多个模型端点，无法确定实际地址".into());
            }
            NativeModel {
                endpoint: endpoints.pop(),
                ..Default::default()
            }
        }
        AgentBackend::Codex => {
            let mut out = NativeModel::default();
            if let Some(bytes) = read(&project.join(".codex/config.toml"))? {
                let text = std::str::from_utf8(&bytes).map_err(|_| "项目 Codex 配置不是 UTF-8")?;
                let config: toml::Value =
                    toml::from_str(text).map_err(|_| "项目 Codex 配置不是有效 TOML")?;
                if ["model_provider", "model_providers", "profile", "profiles"]
                    .iter()
                    .any(|key| config.get(key).is_some())
                {
                    return Err(
                        "项目 Codex 配置包含模型路由；请把选定地址放入私有配置后由 Host 重新授权"
                            .into(),
                    );
                }
            }
            if let Some(bytes) = read(&file(home, ".codex/config.toml"))? {
                let text = std::str::from_utf8(&bytes).map_err(|_| "Codex 配置不是 UTF-8")?;
                let config: toml::Value =
                    toml::from_str(text).map_err(|_| "Codex 配置不是有效 TOML")?;
                if config.get("profile").is_some() {
                    return Err(
                        "Codex 的 profile 路由尚未明确；请在私有配置中直接指定模型供应商".into(),
                    );
                }
                out.provider = config
                    .get("model_provider")
                    .and_then(toml::Value::as_str)
                    .map(str::to_owned);
                out.model = config
                    .get("model")
                    .and_then(toml::Value::as_str)
                    .map(str::to_owned);
                if let Some(provider) = &out.provider {
                    if let Some(base) = config
                        .get("model_providers")
                        .and_then(|value| value.get(provider))
                        .and_then(|value| value.get("base_url"))
                    {
                        out.endpoint = Some(Endpoint::parse(
                            base.as_str().ok_or("Codex base_url 不是静态字符串")?,
                        )?);
                    } else if provider != "openai" {
                        return Err("所选 Codex 供应商缺少静态 base_url".into());
                    }
                }
            }
            out
        }
        AgentBackend::Fox => NativeModel::default(),
    };
    // Network permission depends on the route, not themes, keys, or unused
    // providers. The separate source digest is only for connection reuse.
    out.fingerprint = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&out.endpoint).expect("endpoint serializes"))
    );
    Ok(out)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Grant {
    fingerprint: String,
    endpoint: Endpoint,
}

fn grant_key(backend: AgentBackend, home: &Path) -> String {
    format!("{}:{}", backend.as_str(), home.to_string_lossy())
}
fn grant_path(state: &Path) -> PathBuf {
    state.join("model-endpoint-grants.json")
}

/// Call only after a user-authorized Host import. Reading a backend-written
/// file on launch must never create or refresh this grant.
pub fn approve_import_snapshot(
    state: &Path,
    backend: AgentBackend,
    home: &Path,
    native: &NativeModel,
) -> Result<(), String> {
    let path = grant_path(state);
    let _lock = crate::statefile::StateLock::acquire(&crate::statefile::lock_path_for(&path))
        .map_err(|_| "无法锁定模型端点授权记录")?;
    let mut grants: BTreeMap<String, Grant> = crate::statefile::read_json(&path)?;
    let key = grant_key(backend, home);
    if let Some(endpoint) = native.endpoint.as_ref() {
        grants.insert(
            key,
            Grant {
                fingerprint: native.fingerprint.clone(),
                endpoint: endpoint.clone(),
            },
        );
    } else {
        grants.remove(&key);
    }
    crate::statefile::write_json(&path, &grants)
}

/// A confirmation of auth.json alone cannot approve model configuration
/// left behind by a backend. Every present address/selection source must
/// have been selected by the user and match its trusted import source.
pub fn confirmed_import_model(
    backend: AgentBackend,
    real: &crate::onboarding::RealHome,
    selected: &[String],
) -> Result<Option<NativeModel>, String> {
    for relative in source_files(backend) {
        let source = if backend == AgentBackend::Codex {
            real.codex_home
                .clone()
                .unwrap_or_else(|| real.home.join(".codex"))
                .join("config.toml")
        } else {
            file(&real.home, relative)
        };
        if read(&source)?.is_some() && !selected.contains(&format!("file:{relative}")) {
            return Ok(None);
        }
    }
    if backend == AgentBackend::Codex
        && real
            .codex_home
            .as_ref()
            .is_some_and(|path| path != &real.home.join(".codex"))
    {
        // Imported CODEX_HOME may live anywhere; a grant must be parsed from
        // the exact source, so require the normal location for now.
        return Ok(None);
    }
    resolve(
        backend,
        &real.home,
        &real.home.join(".fox-no-project-settings"),
    )
    .map(|native| native.endpoint.is_some().then_some(native))
}

pub fn import_approved(
    state: &Path,
    backend: AgentBackend,
    home: &Path,
    native: &NativeModel,
) -> Result<bool, String> {
    let grants: BTreeMap<String, Grant> = crate::statefile::read_json(&grant_path(state))?;
    Ok(grants.get(&grant_key(backend, home)).is_some_and(|grant| {
        grant.fingerprint == native.fingerprint && native.endpoint.as_ref() == Some(&grant.endpoint)
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn root(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!("fox-model-endpoint-{}-{label}", std::process::id()))
    }
    fn pi(home: &Path, base: &str) {
        std::fs::create_dir_all(home.join(".pi/agent")).unwrap();
        std::fs::write(
            home.join(".pi/agent/settings.json"),
            r#"{"defaultProvider":"magpie","defaultModel":"m"}"#,
        )
        .unwrap();
        std::fs::write(home.join(".pi/agent/models.json"), serde_json::json!({"providers":{"magpie":{"baseUrl":base,"apiKey":"test-secret","models":[{"id":"m"}]},"unused":{"baseUrl":"http://localhost:9876"}}}).to_string()).unwrap();
    }
    #[test]
    fn endpoint_is_exact_and_diagnostics_do_not_echo_secrets() {
        for (url, entry) in [
            ("http://127.0.0.1:3425/v1", "127.0.0.1:3425"),
            ("http://LOCALHOST/v1", "localhost:80"),
            ("http://[::1]:3425", "[::1]:3425"),
            ("https://Model.Example/v1", "model.example:443"),
        ] {
            assert_eq!(Endpoint::parse(url).unwrap().entry, entry);
        }
        for url in [
            "http://user:secret@localhost:1",
            "http://localhost:${PORT}",
            "http://localhost:0",
            "http://localhost:99999",
            "https://*.example",
            "file:///secret",
            "http://[::]:34",
            "http://127.0.0.1:34\\evil",
        ] {
            let error = Endpoint::parse(url).unwrap_err();
            assert!(!error.contains("secret"));
        }
        assert!(
            Endpoint::parse("http://[::ffff:127.0.0.1]:3425")
                .unwrap()
                .loopback
        );
    }
    #[test]
    fn selected_network_target_only_and_grant_expires_on_route_change() {
        let root = root("grant");
        let _ = std::fs::remove_dir_all(&root);
        let home = root.join("home");
        let state = root.join("state");
        let project = root.join("project");
        pi(&home, "http://127.0.0.1:3425/v1");
        let native = resolve(AgentBackend::Pi, &home, &project).unwrap();
        assert_eq!(native.endpoint.as_ref().unwrap().entry, "127.0.0.1:3425");
        assert!(!import_approved(&state, AgentBackend::Pi, &home, &native).unwrap());
        approve_import_snapshot(&state, AgentBackend::Pi, &home, &native).unwrap();
        assert!(import_approved(&state, AgentBackend::Pi, &home, &native).unwrap());
        assert!(!std::fs::read_to_string(grant_path(&state))
            .unwrap()
            .contains("test-secret"));
        std::fs::write(home.join(".pi/agent/settings.json"), r#"{"defaultProvider":"magpie","defaultModel":"m","theme":"light","compaction":{"enabled":false}}"#).unwrap();
        let models = home.join(".pi/agent/models.json");
        let harmless = std::fs::read_to_string(&models)
            .unwrap()
            .replace("test-secret", "rotated-key")
            .replace("localhost:9876", "unused.example:9988");
        std::fs::write(&models, harmless).unwrap();
        assert!(
            import_approved(
                &state,
                AgentBackend::Pi,
                &home,
                &resolve(AgentBackend::Pi, &home, &project).unwrap()
            )
            .unwrap(),
            "themes, credentials, and unused providers do not widen the route"
        );
        pi(&home, "https://unapproved.example:8443/v1");
        assert!(!import_approved(
            &state,
            AgentBackend::Pi,
            &home,
            &resolve(AgentBackend::Pi, &home, &project).unwrap()
        )
        .unwrap());
        std::fs::create_dir_all(project.join(".pi")).unwrap();
        std::fs::write(
            project.join(".pi/settings.json"),
            r#"{"defaultProvider":"unused"}"#,
        )
        .unwrap();
        assert!(resolve(AgentBackend::Pi, &home, &project)
            .unwrap_err()
            .contains("覆盖"));
        let _ = std::fs::remove_dir_all(root);
    }
    #[test]
    fn confirmed_import_grants_only_the_trusted_snapshot_not_later_home_edits() {
        let root = root("snapshot");
        let _ = std::fs::remove_dir_all(&root);
        let real = crate::onboarding::RealHome {
            home: root.join("real"),
            codex_home: None,
        };
        let target = root.join("private");
        pi(&real.home, "http://localhost:3425/v1");
        let selected = vec![
            "file:.pi/agent/models.json".into(),
            "file:.pi/agent/settings.json".into(),
        ];
        assert!(
            confirmed_import_model(AgentBackend::Pi, &real, &selected[..1])
                .unwrap()
                .is_none()
        );
        let snapshot = confirmed_import_model(AgentBackend::Pi, &real, &selected)
            .unwrap()
            .unwrap();
        pi(&target, "http://localhost:7777/v1");
        approve_import_snapshot(&root.join("state"), AgentBackend::Pi, &target, &snapshot).unwrap();
        let actual = resolve(AgentBackend::Pi, &target, &root.join("project")).unwrap();
        assert!(!import_approved(&root.join("state"), AgentBackend::Pi, &target, &actual).unwrap());
        pi(&target, "http://localhost:3425/v1");
        let actual = resolve(AgentBackend::Pi, &target, &root.join("project")).unwrap();
        assert!(import_approved(&root.join("state"), AgentBackend::Pi, &target, &actual).unwrap());
        let _ = std::fs::remove_dir_all(root);
    }
}
