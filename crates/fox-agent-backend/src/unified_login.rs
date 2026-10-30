//! Unified login (settings batch, item 6): before an external backend starts,
//! Fox writes the backend's own config files into the project's private HOME
//! from the selected protocol profile of Fox「模型服务」(base URL, API type, key,
//! default model), so pi / DeepSeek / Codex work without a separate login.
//!
//! Rules:
//!
//! * **The user's own files are never touched.** A file is Fox-managed only
//!   while the ledger [`MANAGED_LEDGER`] in the private HOME lists it with the
//!   fingerprint of its current content; a file the user created, imported or
//!   edited (fingerprint differs) is the user's from then on.
//! * **An existing login wins.** When the private HOME already holds a login
//!   of the user's (Codex `auth.json` / `config.toml`, a dsh `.env` key, pi
//!   `auth.json` / `models.json` with a key), nothing is generated.
//! * **Regenerated when 模型服务 changes.** Fox-managed files are rewritten
//!   when the generated content changes, and removed
//!   ([`remove_managed`]) when 模型服务 has no usable default provider, so
//!   the missing-login card shows again as the fallback.
//! * **Secrets.** The key is only ever in the generated file (pi
//!   `models.json`, dsh `.env`; 0600 on Unix, the private root's DACL on
//!   Windows) or, for Codex, in the backend's environment
//!   ([`CODEX_KEY_ENV`], never in `config.toml`). [`Secret`] never prints its
//!   value; reports, logs and audits carry file NAMES only.
//!
//! Formats (checked against the pinned versions, docs/pi.md, deepseek.md,
//! sandbox.md 13.12):
//!
//! * pi 0.84.2 (`docs/models.md`, `docs/settings.md`): `.pi/agent/models.json`
//!   `{"providers":{"fox":{"baseUrl","api","apiKey","models":[{"id"}]}}}`
//!   (`api` = Fox's API type, the same names pi uses; a literal `apiKey` is
//!   escaped so a leading `!` / `$` is never run or interpolated) and
//!   `.pi/agent/settings.json` `{"defaultProvider":"fox","defaultModel"}`.
//! * DeepSeek (dsh 0.2.1-rc.2): `.dsh/profiles/acp/cordis.patch.yml` with the
//!   `llm-deepseek` base URL and the `acp` row (`provider: deepseek-official`,
//!   the default model), `.dsh/.env` with `DEEPSEEK_API_KEY`. dsh speaks
//!   Anthropic Messages. The selected profile is used as is; Fox never guesses
//!   an alternate vendor endpoint from a Chat Completions address.
//! * Codex (codex-acp): `.codex/config.toml` with `model`, `model_provider =
//!   "fox"` and `[model_providers.fox]` (`base_url`, `wire_api = "responses"`,
//!   `env_key = "FOX_CODEX_API_KEY"`). Requires an explicitly selected
//!   `openai-responses` profile. Protocol matching is not a successful model run.

use crate::onboarding::{
    codex_auth_present, dotenv_key_names, ensure_private_dir, is_present_file, json_has_api_key, pi_auth_providers, read_small, rel, write_private, CODEX_AUTH,
    CODEX_CONFIG, DSH_ENV, DSH_PATCH, DSH_PATCH_ACP, PI_AUTH, PI_MODELS, PI_SETTINGS,
};
use crate::port::AgentBackend;
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::Path;

/// Ledger of Fox-managed files, at the private HOME root.
pub const MANAGED_LEDGER: &str = ".fox-managed.json";
/// Environment variable that carries the key to Codex (`env_key`).
pub const CODEX_KEY_ENV: &str = "FOX_CODEX_API_KEY";
/// Provider id Fox registers in pi and Codex.
pub const FOX_PROVIDER_ID: &str = "fox";
/// First line of every generated text file that allows comments.
pub const MANAGED_HEADER: &str = "# Fox 管理：由 Fox「模型服务」的协议配置自动生成，模型服务改动后会重新生成。改动本文件后 Fox 不再管理它。";

/// A secret value. `Debug` never shows it; read it with [`Secret::expose`].
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

/// Fox「模型服务」's default provider, as the Host read it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnifiedLogin {
    /// Provider name shown to the user (no secret).
    pub provider_name: String,
    /// Normalized base URL (no trailing `/`).
    pub base_url: String,
    /// Fox API type: `openai-completions` / `anthropic-messages` / ...
    pub api_type: String,
    /// Default model id.
    pub model: String,
    pub api_key: Secret,
    /// Whether `POST {base}/responses` exists (Host probe, cached). `None`:
    /// not probed / undetermined.
    pub responses_supported: Option<bool>,
}

/// What [`apply`] did for one backend. Names only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// Fox-managed files were written (or rewritten).
    Generated,
    /// The Fox-managed files were already current.
    Unchanged,
    /// The private HOME holds the user's own login: nothing generated.
    ExistingLogin,
    /// This provider cannot drive this backend (reason in the report).
    Unsupported,
}

/// Report of [`apply`] / [`plan`]. File names relative to the private HOME;
/// never a value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UnifiedReport {
    pub backend: String,
    pub outcome: Outcome,
    pub provider: String,
    /// Files Fox manages for this backend after the call.
    pub files: Vec<String>,
    /// Files left alone because they are the user's.
    pub user_files: Vec<String>,
    pub reason: Option<String>,
    /// Variables the launch must add to the backend's environment (Codex
    /// key). Never serialized.
    #[serde(skip)]
    pub env: Vec<(String, Secret)>,
}

/// FNV-1a 64 of `bytes`: change detection for the ledger (not security).
pub fn fingerprint(bytes: &[u8]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

fn read_ledger(home: &Path) -> BTreeMap<String, String> {
    read_small(&home.join(MANAGED_LEDGER))
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .and_then(|value| value.get("files").cloned())
        .and_then(|files| serde_json::from_value::<BTreeMap<String, String>>(files).ok())
        .unwrap_or_default()
}

fn write_ledger(home: &Path, ledger: &BTreeMap<String, String>) -> std::io::Result<()> {
    let path = home.join(MANAGED_LEDGER);
    if ledger.is_empty() {
        return match std::fs::remove_file(&path) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        };
    }
    let value = serde_json::json!({
        "version": 1,
        "note": "Fox 管理的文件清单：只有内容与这里的指纹一致的文件才由 Fox 重新生成或删除；你改过的文件 Fox 不再动。",
        "files": ledger,
    });
    write_private(&path, serde_json::to_string_pretty(&value).expect("ledger serializes").as_bytes())
}

/// Whether `relative` exists in `home` and is Fox-managed (ledger
/// fingerprint equals the current content).
fn is_managed(home: &Path, ledger: &BTreeMap<String, String>, relative: &str) -> bool {
    let Some(expected) = ledger.get(relative) else { return false };
    let path = rel(home, relative);
    match std::fs::symlink_metadata(&path) {
        Ok(meta) if meta.is_file() => std::fs::read(&path).map(|bytes| &fingerprint(&bytes) == expected).unwrap_or(false),
        _ => false,
    }
}

/// `relative` exists (any type, a link included) and is not Fox-managed.
fn is_users(home: &Path, ledger: &BTreeMap<String, String>, relative: &str) -> bool {
    std::fs::symlink_metadata(rel(home, relative)).is_ok() && !is_managed(home, ledger, relative)
}

/// The files Fox may generate for `backend`.
pub fn managed_candidates(backend: AgentBackend) -> &'static [&'static str] {
    match backend {
        AgentBackend::Codex => &[CODEX_CONFIG],
        AgentBackend::Deepseek => &[DSH_PATCH_ACP, DSH_ENV],
        AgentBackend::Pi => &[PI_MODELS, PI_SETTINGS],
        AgentBackend::Fox => &[],
    }
}

/// Whether the user's own files in `home` already give `backend` a login
/// (`host_login`: a login variable of the Host environment reaches it).
pub fn user_login_present(backend: AgentBackend, home: &Path, host_login: bool) -> bool {
    let ledger = read_ledger(home);
    let users = |relative: &str| is_users(home, &ledger, relative) && is_present_file(&rel(home, relative));
    match backend {
        AgentBackend::Codex => {
            host_login
                || (users(CODEX_AUTH) && read_small(&rel(home, CODEX_AUTH)).is_some_and(|text| codex_auth_present(&text)))
                // A config of the user's may name its own provider and key.
                || users(CODEX_CONFIG)
        }
        AgentBackend::Deepseek => {
            host_login || (users(DSH_ENV) && read_small(&rel(home, DSH_ENV)).is_some_and(|text| dotenv_key_names(&text).iter().any(|name| name == "DEEPSEEK_API_KEY")))
        }
        AgentBackend::Pi => {
            (users(PI_AUTH) && read_small(&rel(home, PI_AUTH)).is_some_and(|text| !pi_auth_providers(&text).is_empty()))
                || (users(PI_MODELS)
                    && read_small(&rel(home, PI_MODELS)).and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok()).is_some_and(|value| json_has_api_key(&value)))
        }
        AgentBackend::Fox => true,
    }
}

fn valid_secret(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 4096 && !value.chars().any(|c| c.is_control() || c.is_whitespace() || c == '"' || c == '\'' || c == '\\')
}

fn yaml_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn toml_string(value: &str) -> String {
    serde_json::to_string(value).expect("string serializes")
}

/// pi resolves `!cmd` and `$VAR` in `apiKey`; escape them so the key is a
/// literal (`$$` -> `$`, a leading `$!` -> `!`).
pub fn pi_literal(value: &str) -> String {
    let escaped = value.replace('$', "$$");
    match escaped.strip_prefix('!') {
        Some(rest) => format!("$!{rest}"),
        None => escaped,
    }
}

/// The dsh base URL for this provider, or why there is none.
pub fn deepseek_base_url(login: &UnifiedLogin) -> Result<String, String> {
    let base = login.base_url.trim_end_matches('/');
    if login.api_type == "anthropic-messages" {
        Ok(base.to_owned())
    } else {
        Err("请在模型服务中为 DeepSeek 配置 Anthropic Messages 协议".to_owned())
    }
}

/// Protocol compatibility is separate from connection verification.
pub fn codex_unsupported_reason(login: &UnifiedLogin) -> Option<String> {
    (login.api_type != "openai-responses").then(|| "请在模型服务中为 Codex 配置 OpenAI Responses 协议".to_owned())
}

/// The files to generate for `backend` from `login` (content includes the
/// key where the backend reads it from a file), or why not.
fn generated_files(backend: AgentBackend, login: &UnifiedLogin) -> Result<(Vec<(&'static str, String)>, Vec<(String, Secret)>), String> {
    if !valid_secret(login.api_key.expose()) {
        return Err("模型服务默认 provider 的 key 含有空白或引号等字符，Fox 不自动写入".to_owned());
    }
    if login.model.trim().is_empty() || login.base_url.trim().is_empty() {
        return Err("模型服务没有默认模型或地址".to_owned());
    }
    let key = login.api_key.expose();
    match backend {
        AgentBackend::Pi => {
            let models = serde_json::json!({
                "providers": {
                    FOX_PROVIDER_ID: {
                        "baseUrl": login.base_url.trim_end_matches('/'),
                        "api": login.api_type,
                        "apiKey": pi_literal(key),
                        "models": [{ "id": login.model }],
                    }
                }
            });
            let settings = serde_json::json!({ "defaultProvider": FOX_PROVIDER_ID, "defaultModel": login.model });
            Ok((
                vec![
                    (PI_MODELS, serde_json::to_string_pretty(&models).expect("json") + "\n"),
                    (PI_SETTINGS, serde_json::to_string_pretty(&settings).expect("json") + "\n"),
                ],
                Vec::new(),
            ))
        }
        AgentBackend::Deepseek => {
            let base = deepseek_base_url(login)?;
            let patch = format!(
                "{MANAGED_HEADER}\n- id: llm-deepseek\n  config:\n    baseURL: {}\n{}",
                yaml_quote(&base),
                crate::backends::deepseek::acp_model_patch("deepseek-official", &login.model)
            );
            let env = format!("{MANAGED_HEADER}\nDEEPSEEK_API_KEY={key}\n");
            Ok((vec![(DSH_PATCH_ACP, patch), (DSH_ENV, env)], Vec::new()))
        }
        AgentBackend::Codex => {
            if let Some(reason) = codex_unsupported_reason(login) {
                return Err(reason);
            }
            let config = format!(
                "{MANAGED_HEADER}\n# key 不写进本文件：Fox 启动 Codex 时通过环境变量 {CODEX_KEY_ENV} 注入。\nmodel = {}\nmodel_provider = \"{FOX_PROVIDER_ID}\"\n\n[model_providers.{FOX_PROVIDER_ID}]\nname = {}\nbase_url = {}\nwire_api = \"responses\"\nenv_key = \"{CODEX_KEY_ENV}\"\n# 网络不通时不长时间重试（Reconnecting…）：Fox 在沙箱里会立即结束本轮并给出被拦截的主机。\nrequest_max_retries = 2\nstream_max_retries = 2\n",
                toml_string(&login.model),
                toml_string(&format!("Fox 模型服务：{}", login.provider_name)),
                toml_string(login.base_url.trim_end_matches('/')),
            );
            Ok((vec![(CODEX_CONFIG, config)], vec![(CODEX_KEY_ENV.to_owned(), login.api_key.clone())]))
        }
        AgentBackend::Fox => Err("Fox 不需要外部登录".to_owned()),
    }
}

fn report(backend: AgentBackend, login: &UnifiedLogin, outcome: Outcome) -> UnifiedReport {
    UnifiedReport { backend: backend.as_str().to_owned(), outcome, provider: login.provider_name.clone(), files: Vec::new(), user_files: Vec::new(), reason: None, env: Vec::new() }
}

/// What [`apply`] would do, without writing anything (login probe of a
/// draft or a conversation before its next launch).
pub fn plan(backend: AgentBackend, home: &Path, login: &UnifiedLogin, host_login: bool) -> UnifiedReport {
    run(backend, home, login, host_login, false).unwrap_or_else(|error| {
        let mut out = report(backend, login, Outcome::Unsupported);
        out.reason = Some(error);
        out
    })
}

/// Generate the Fox-managed files of `backend` in `home` from `login`
/// (see the module docs). `Err` only for an I/O failure.
pub fn apply(backend: AgentBackend, home: &Path, login: &UnifiedLogin, host_login: bool) -> Result<UnifiedReport, String> {
    run(backend, home, login, host_login, true)
}

fn run(backend: AgentBackend, home: &Path, login: &UnifiedLogin, host_login: bool, write: bool) -> Result<UnifiedReport, String> {
    let ledger = read_ledger(home);
    let candidates = managed_candidates(backend);
    let user_files: Vec<String> = candidates.iter().filter(|relative| is_users(home, &ledger, relative)).map(|relative| (*relative).to_owned()).collect();
    if user_login_present(backend, home, host_login) {
        let mut out = report(backend, login, Outcome::ExistingLogin);
        out.user_files = user_files;
        out.files = candidates.iter().filter(|relative| is_managed(home, &ledger, relative)).map(|relative| (*relative).to_owned()).collect();
        return Ok(out);
    }
    let (files, env) = match generated_files(backend, login) {
        Ok(generated) => generated,
        Err(reason) => {
            let mut out = report(backend, login, Outcome::Unsupported);
            out.reason = Some(reason);
            out.user_files = user_files;
            return Ok(out);
        }
    };
    // The file that holds the key (pi models.json, dsh .env) or the config
    // that names the provider (Codex) must be writable; an optional file
    // (pi settings.json, the dsh patch) of the user's is just left alone.
    let essential = match backend {
        AgentBackend::Pi => PI_MODELS,
        AgentBackend::Deepseek => DSH_ENV,
        _ => CODEX_CONFIG,
    };
    let mut blocking: Vec<String> = user_files.iter().filter(|file| file.as_str() == essential).cloned().collect();
    if backend == AgentBackend::Deepseek && is_users(home, &ledger, DSH_PATCH) {
        // dsh's other patch would override the generated base URL / model.
        blocking.push(DSH_PATCH.to_owned());
    }
    if !blocking.is_empty() {
        let mut out = report(backend, login, Outcome::Unsupported);
        out.reason = Some(format!("私有 HOME 里已有你自己的文件（{}），Fox 不改动它，也就不自动生成登录", blocking.join("、")));
        out.user_files = user_files;
        return Ok(out);
    }
    let mut ledger_out = ledger.clone();
    let mut written = false;
    let mut managed = Vec::new();
    for (relative, content) in &files {
        if user_files.iter().any(|file| file == relative) {
            continue;
        }
        managed.push((*relative).to_owned());
        let path = rel(home, relative);
        let current = std::fs::read(&path).ok();
        if current.as_deref() == Some(content.as_bytes()) && is_managed(home, &ledger, relative) {
            continue;
        }
        written = true;
        if write {
            ensure_private_dir(home, path.parent().unwrap_or(home)).map_err(|error| format!("cannot create the directory of {relative}: {error}"))?;
            write_private(&path, content.as_bytes()).map_err(|error| format!("cannot write {relative}: {error}"))?;
            ledger_out.insert((*relative).to_owned(), fingerprint(content.as_bytes()));
        }
    }
    if write && ledger_out != ledger {
        write_ledger(home, &ledger_out).map_err(|error| format!("cannot write {MANAGED_LEDGER}: {error}"))?;
    }
    let mut out = report(backend, login, if written { Outcome::Generated } else { Outcome::Unchanged });
    out.files = managed;
    out.user_files = user_files;
    out.env = env;
    Ok(out)
}

/// Remove the Fox-managed files of `backend` (模型服务 has no usable default
/// provider any more), so the missing-login card shows again. Files the
/// user changed are kept. Returns the removed names.
pub fn remove_managed(backend: AgentBackend, home: &Path) -> Result<Vec<String>, String> {
    if home.join(MANAGED_LEDGER).exists() {
        let text = read_small(&home.join(MANAGED_LEDGER)).ok_or("无法读取 Fox 生成文件清单，未执行清理")?;
        let value: serde_json::Value = serde_json::from_str(&text).map_err(|_| "Fox 生成文件清单无效，未执行清理")?;
        if serde_json::from_value::<BTreeMap<String, String>>(value.get("files").cloned().unwrap_or_default()).is_err() {
            return Err("Fox 生成文件清单无效，未执行清理".into());
        }
    }
    let mut ledger = read_ledger(home);
    let mut removed = Vec::new();
    for relative in managed_candidates(backend) {
        if is_managed(home, &ledger, relative) {
            std::fs::remove_file(rel(home, relative)).map_err(|error| format!("cannot remove {relative}: {error}"))?;
            ledger.remove(*relative);
            removed.push((*relative).to_owned());
        } else if ledger.contains_key(*relative) && std::fs::symlink_metadata(rel(home, relative)).is_err() {
            ledger.remove(*relative);
        }
    }
    if !removed.is_empty() || ledger != read_ledger(home) {
        write_ledger(home, &ledger).map_err(|error| format!("cannot write {MANAGED_LEDGER}: {error}"))?;
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::external_home::{HostOs, PermissionMode};

    const KEY: &str = "sk-unified-0123456789abcdef";

    #[test]
    fn corrupted_ownership_ledger_reports_cleanup_failure() {
        let home = temp_home("corrupt-ledger");
        std::fs::write(home.join(MANAGED_LEDGER), "not-json").unwrap();
        assert!(remove_managed(AgentBackend::Pi, &home).is_err());
        assert_eq!(std::fs::read_to_string(home.join(MANAGED_LEDGER)).unwrap(), "not-json");
        let _ = std::fs::remove_dir_all(home);
    }

    fn temp_home(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("fox-unified-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn login(api_type: &str, base: &str, responses: Option<bool>) -> UnifiedLogin {
        UnifiedLogin {
            provider_name: "DeepSeek".into(),
            base_url: base.into(),
            api_type: api_type.into(),
            model: "deepseek-v4-flash".into(),
            api_key: Secret::new(KEY),
            responses_supported: responses,
        }
    }

    fn no_env(_: &str) -> bool {
        false
    }

    fn probe(backend: AgentBackend, home: &Path) -> crate::onboarding::CredentialProbe {
        crate::onboarding::probe(backend, home, PermissionMode::FullAccess, HostOs::Linux, &no_env)
    }

    #[test]
    fn secret_and_reports_never_show_the_key() {
        let mut login = login("openai-completions", "https://api.deepseek.com", Some(true));
        assert!(!format!("{login:?}").contains(KEY));
        let home = temp_home("report");
        for backend in [AgentBackend::Pi, AgentBackend::Deepseek, AgentBackend::Codex] {
            login.api_type = match backend { AgentBackend::Codex => "openai-responses", AgentBackend::Deepseek => "anthropic-messages", _ => "openai-completions" }.into();
            let report = apply(backend, &home, &login, false).unwrap();
            assert_eq!(report.outcome, Outcome::Generated, "{backend:?}");
            let text = serde_json::to_string(&report).unwrap();
            assert!(!text.contains(KEY), "{text}");
            assert!(!format!("{report:?}").contains(KEY));
        }
    }

    #[test]
    fn pi_gets_models_and_settings_with_a_literal_key() {
        let home = temp_home("pi");
        let report = apply(AgentBackend::Pi, &home, &login("openai-completions", "https://api.deepseek.com/", None), false).unwrap();
        assert_eq!(report.outcome, Outcome::Generated);
        assert_eq!(report.files, [PI_MODELS, PI_SETTINGS]);
        let models: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(rel(&home, PI_MODELS)).unwrap()).unwrap();
        assert_eq!(models["providers"]["fox"]["baseUrl"], "https://api.deepseek.com");
        assert_eq!(models["providers"]["fox"]["api"], "openai-completions");
        assert_eq!(models["providers"]["fox"]["apiKey"], KEY);
        assert_eq!(models["providers"]["fox"]["models"][0]["id"], "deepseek-v4-flash");
        let settings: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(rel(&home, PI_SETTINGS)).unwrap()).unwrap();
        assert_eq!(settings, serde_json::json!({ "defaultProvider": "fox", "defaultModel": "deepseek-v4-flash" }));
        assert_eq!(probe(AgentBackend::Pi, &home).state, crate::onboarding::CredentialState::Ready);
        assert_eq!(pi_literal("!op read x"), "$!op read x");
        assert_eq!(pi_literal("a$B"), "a$$B");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for file in [PI_MODELS, PI_SETTINGS, MANAGED_LEDGER] {
                assert_eq!(std::fs::metadata(rel(&home, file)).unwrap().permissions().mode() & 0o777, 0o600, "{file}");
            }
            assert_eq!(std::fs::metadata(home.join(".pi/agent")).unwrap().permissions().mode() & 0o777, 0o700);
        }
    }

    #[test]
    fn deepseek_gets_the_anthropic_entry_and_a_dotenv_key() {
        let home = temp_home("dsh");
        let report = apply(AgentBackend::Deepseek, &home, &login("anthropic-messages", "https://api.deepseek.com/anthropic", None), false).unwrap();
        assert_eq!(report.outcome, Outcome::Generated);
        let patch = std::fs::read_to_string(rel(&home, DSH_PATCH_ACP)).unwrap();
        assert!(patch.starts_with("# Fox 管理"));
        assert!(patch.contains("baseURL: 'https://api.deepseek.com/anthropic'"), "{patch}");
        assert!(patch.contains("provider: 'deepseek-official'") && patch.contains("model: 'deepseek-v4-flash'"), "{patch}");
        let env = std::fs::read_to_string(rel(&home, DSH_ENV)).unwrap();
        assert!(env.contains(&format!("DEEPSEEK_API_KEY={KEY}")));
        assert_eq!(probe(AgentBackend::Deepseek, &home).state, crate::onboarding::CredentialState::Ready);
        // An Anthropic-compatible gateway (Magpie) is used as is; the loopback
        // refusal of the sandbox then applies unchanged.
        let magpie = temp_home("dsh-magpie");
        apply(AgentBackend::Deepseek, &magpie, &login("anthropic-messages", "http://127.0.0.1:3425/v1", None), false).unwrap();
        let sandboxed = crate::onboarding::probe(AgentBackend::Deepseek, &magpie, PermissionMode::Sandboxed, HostOs::Linux, &no_env);
        assert_eq!(sandboxed.state, crate::onboarding::CredentialState::LoopbackRequiresFullAccess);
        // Another OpenAI-compatible vendor cannot drive dsh.
        let other = apply(AgentBackend::Deepseek, &temp_home("dsh-other"), &login("openai-completions", "https://dashscope.aliyuncs.com/compatible-mode/v1", None), false).unwrap();
        assert_eq!(other.outcome, Outcome::Unsupported);
        assert!(other.reason.unwrap().contains("Anthropic Messages"));
    }

    #[test]
    fn codex_config_names_the_provider_and_keeps_the_key_in_the_environment() {
        let home = temp_home("codex");
        let report = apply(AgentBackend::Codex, &home, &login("openai-responses", "https://api.deepseek.com", Some(true)), false).unwrap();
        assert_eq!(report.outcome, Outcome::Generated);
        let config = std::fs::read_to_string(rel(&home, CODEX_CONFIG)).unwrap();
        assert!(config.contains("model_provider = \"fox\"") && config.contains("[model_providers.fox]"), "{config}");
        assert!(config.contains("base_url = \"https://api.deepseek.com\"") && config.contains("wire_api = \"responses\"") && config.contains("env_key = \"FOX_CODEX_API_KEY\""), "{config}");
        assert!(!config.contains(KEY), "the key never goes into config.toml");
        assert_eq!(report.env.len(), 1);
        assert_eq!(report.env[0].0, CODEX_KEY_ENV);
        assert_eq!(report.env[0].1.expose(), KEY);
        let refused = apply(AgentBackend::Codex, &temp_home("codex-no"), &login("openai-completions", "https://api.example.com/v1", Some(false)), false).unwrap();
        assert_eq!(refused.outcome, Outcome::Unsupported);
        assert!(refused.reason.unwrap().contains("Responses"));
        assert!(refused.env.is_empty());
    }

    #[test]
    fn the_users_own_login_and_files_are_never_touched() {
        let login = login("openai-completions", "https://api.deepseek.com", Some(true));
        // pi: the user's auth.json login wins.
        let home = temp_home("own-pi");
        std::fs::create_dir_all(home.join(".pi/agent")).unwrap();
        std::fs::write(rel(&home, PI_AUTH), r#"{"anthropic":{"type":"api_key","key":"sk-user"}}"#).unwrap();
        let report = apply(AgentBackend::Pi, &home, &login, false).unwrap();
        assert_eq!(report.outcome, Outcome::ExistingLogin);
        assert!(!rel(&home, PI_MODELS).exists());
        // DeepSeek: the user's .env without a key is kept, nothing generated.
        let home = temp_home("own-dsh");
        std::fs::create_dir_all(home.join(".dsh")).unwrap();
        std::fs::write(rel(&home, DSH_ENV), "MAGPIE_GATEWAY_KEY=m\n").unwrap();
        let report = apply(AgentBackend::Deepseek, &home, &login, false).unwrap();
        assert_eq!(report.outcome, Outcome::Unsupported);
        assert_eq!(std::fs::read_to_string(rel(&home, DSH_ENV)).unwrap(), "MAGPIE_GATEWAY_KEY=m\n");
        assert!(!rel(&home, DSH_PATCH_ACP).exists());
        // Codex: the user's config.toml counts as their login.
        let home = temp_home("own-codex");
        std::fs::create_dir_all(home.join(".codex")).unwrap();
        std::fs::write(rel(&home, CODEX_CONFIG), "model = \"o3\"\n").unwrap();
        assert_eq!(apply(AgentBackend::Codex, &home, &login, false).unwrap().outcome, Outcome::ExistingLogin);
        assert_eq!(std::fs::read_to_string(rel(&home, CODEX_CONFIG)).unwrap(), "model = \"o3\"\n");
        // A login variable of the Host environment counts too.
        assert_eq!(apply(AgentBackend::Deepseek, &temp_home("own-host"), &login, true).unwrap().outcome, Outcome::ExistingLogin);
    }

    #[test]
    fn managed_files_follow_the_model_service_until_the_user_edits_them() {
        let home = temp_home("regen");
        let first = login("openai-completions", "https://api.deepseek.com", None);
        assert_eq!(apply(AgentBackend::Pi, &home, &first, false).unwrap().outcome, Outcome::Generated);
        assert_eq!(apply(AgentBackend::Pi, &home, &first, false).unwrap().outcome, Outcome::Unchanged);
        let mut second = first.clone();
        second.model = "deepseek-v4-pro".into();
        assert_eq!(plan(AgentBackend::Pi, &home, &second, false).outcome, Outcome::Generated, "plan reports without writing");
        assert!(std::fs::read_to_string(rel(&home, PI_SETTINGS)).unwrap().contains("deepseek-v4-flash"));
        assert_eq!(apply(AgentBackend::Pi, &home, &second, false).unwrap().outcome, Outcome::Generated);
        assert!(std::fs::read_to_string(rel(&home, PI_SETTINGS)).unwrap().contains("deepseek-v4-pro"));
        // The user edits settings.json: it is theirs from now on.
        std::fs::write(rel(&home, PI_SETTINGS), "{\"defaultModel\":\"mine\"}\n").unwrap();
        let mut third = second.clone();
        third.model = "deepseek-v5".into();
        let report = apply(AgentBackend::Pi, &home, &third, false).unwrap();
        assert_eq!(report.user_files, [PI_SETTINGS]);
        assert_eq!(std::fs::read_to_string(rel(&home, PI_SETTINGS)).unwrap(), "{\"defaultModel\":\"mine\"}\n");
        // 模型服务 cleared: Fox removes only its own files.
        assert_eq!(remove_managed(AgentBackend::Pi, &home).unwrap(), [PI_MODELS]);
        assert!(!rel(&home, PI_MODELS).exists());
        assert!(rel(&home, PI_SETTINGS).exists(), "the user's file stays");
        assert_eq!(probe(AgentBackend::Pi, &home).state, crate::onboarding::CredentialState::MissingCredentials);
    }
}
