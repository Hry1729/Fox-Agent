//! Login guidance for the external backends (UX batch ①).
//!
//! External backends run with a Fox private HOME in both permission modes
//! (one per project UUID × backend, [`crate::sandbox::PrivateHomes`]); a login
//! in the user's real HOME is never used. This module lets the Host tell the
//! user what is missing and copy a login over on request:
//!
//! * [`probe`] classifies a private HOME as [`CredentialState::Ready`],
//!   [`CredentialState::MissingCredentials`] or
//!   [`CredentialState::LoopbackRequiresFullAccess`] (DeepSeek with a loopback
//!   base URL in sandboxed mode, refused at launch). It looks at file and key
//!   NAMES only: values are read where a file has to be parsed, never
//!   returned, logged or serialized.
//! * [`import_candidates`] lists what the real HOME has that the backend
//!   would read (a fixed whitelist per backend: files and `.env` key names).
//! * [`import_into`] copies the items the user confirmed into one or more
//!   private HOMEs: a copy (never a link), regular files only, files 0600 and
//!   directories 0700 on Unix (Windows: the caller re-applies the private
//!   root's DACL, [`crate::winruntime::prepare_windows_private_root`]). The
//!   [`ImportReport`] is the audit record: file and key names, no value.
//!
//! Where each backend reads its login (checked against the pinned versions):
//! * DeepSeek (dsh 0.2.1-rc.2): `DSH_HOME=<home>/.dsh`; key `DEEPSEEK_API_KEY`
//!   in `.dsh/.env` (dsh refuses `DEEPSEEK_BASE_URL` there); base URL, provider
//!   and model in `.dsh/profiles/acp/cordis.patch.yml` or
//!   `.dsh/cordis.patch.yml` (docs/sandbox.md 13.12).
//! * Codex (codex-acp): `CODEX_HOME=<home>/.codex`; `auth.json`
//!   (`OPENAI_API_KEY` or ChatGPT `tokens`), `config.toml`; or the Host
//!   environment's `OPENAI_API_KEY` (not passed into the Windows sandbox).
//! * pi (pi-coding-agent 0.84.2, `dist/config.js` `getAgentDir()`):
//!   `<home>/.pi/agent/` (`PI_CODING_AGENT_DIR` is not set by Fox);
//!   `auth.json` (provider → `{type: "api_key", key}` / `{type: "oauth", ...}`),
//!   `models.json` (custom providers, may carry `apiKey`), `settings.json`.
use crate::port::AgentBackend;
use crate::sandbox::{HostOs, PermissionMode};
use serde::Serialize;
use std::path::{Path, PathBuf};

/// Largest file the import copies (logins and configs are small).
pub const MAX_IMPORT_FILE_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialState {
    /// A login / key the backend reads is present.
    Ready,
    /// Nothing the backend could log in with: show the guidance card.
    MissingCredentials,
    /// Credentials present, but the DeepSeek base URL is loopback (a local
    /// gateway such as Magpie) and the session is sandboxed: the launch is
    /// refused (`agent_backend_loopback_requires_full_access`). Different
    /// remedy: full access, or the official endpoint.
    LoopbackRequiresFullAccess,
}

impl CredentialState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::MissingCredentials => "missing_credentials",
            Self::LoopbackRequiresFullAccess => "loopback_requires_full_access",
        }
    }
}

/// A configuration template the guidance card shows (no secret: the key
/// lines carry placeholders).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigTemplate {
    pub id: &'static str,
    pub title: &'static str,
    /// Which permission modes it works in.
    pub note: &'static str,
    pub files: Vec<TemplateFile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplateFile {
    /// Relative to the private HOME, `/`-separated.
    pub path: &'static str,
    pub content: &'static str,
}

/// Result of [`probe`]. Names only, never a value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialProbe {
    pub backend: String,
    pub mode: String,
    pub state: CredentialState,
    pub private_home: PathBuf,
    /// Whether a login the backend reads exists (independent of the loopback
    /// problem, so both can be reported).
    pub credentials_present: bool,
    /// DeepSeek: a loopback base URL that sandboxed mode refuses.
    pub loopback_blocked: bool,
    /// Key names found (in private files, or the Host environment).
    pub present_keys: Vec<String>,
    /// Files found, relative to the private HOME.
    pub present_files: Vec<String>,
    /// Key names the backend needs (one of them is enough).
    pub required_keys: Vec<String>,
    /// Files the backend reads its login / config from, relative.
    pub required_files: Vec<String>,
    pub templates: Vec<ConfigTemplate>,
}

/// Names of Host environment variables that are set (non-empty). Values are
/// never looked at beyond emptiness.
pub type EnvPresence<'a> = &'a dyn Fn(&str) -> bool;

const DSH_ENV: &str = ".dsh/.env";
const DSH_PATCH_ACP: &str = ".dsh/profiles/acp/cordis.patch.yml";
const DSH_PATCH: &str = ".dsh/cordis.patch.yml";
const CODEX_AUTH: &str = ".codex/auth.json";
const CODEX_CONFIG: &str = ".codex/config.toml";
const PI_AUTH: &str = ".pi/agent/auth.json";
const PI_MODELS: &str = ".pi/agent/models.json";
const PI_SETTINGS: &str = ".pi/agent/settings.json";

fn rel(home: &Path, relative: &str) -> PathBuf {
    relative.split('/').fold(home.to_path_buf(), |path, part| path.join(part))
}

/// `KEY` names assigned a non-empty value in a dotenv text (`KEY=value`,
/// `export KEY=value`, quotes allowed). Values are dropped here.
pub fn dotenv_key_names(text: &str) -> Vec<String> {
    let mut names = Vec::new();
    for line in text.lines() {
        if let Some((name, value)) = dotenv_assignment(line) {
            let value = value.trim().trim_matches(|c| c == '"' || c == '\'').trim();
            if !value.is_empty() && !names.iter().any(|known| known == name) {
                names.push(name.to_owned());
            }
        }
    }
    names
}

/// `(name, raw value)` of one dotenv line, or `None` (comment, blank, junk).
fn dotenv_assignment(line: &str) -> Option<(&str, &str)> {
    let line = line.trim_start();
    if line.starts_with('#') {
        return None;
    }
    let line = line.strip_prefix("export ").map(str::trim_start).unwrap_or(line);
    let (name, value) = line.split_once('=')?;
    let name = name.trim();
    (!name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')).then_some((name, value))
}

fn read_small(path: &Path) -> Option<String> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_IMPORT_FILE_BYTES {
        return None;
    }
    std::fs::read_to_string(path).ok()
}

fn is_present_file(path: &Path) -> bool {
    std::fs::symlink_metadata(path).map(|meta| meta.is_file() && meta.len() > 0).unwrap_or(false)
}

/// Codex `auth.json` holds a usable login: a non-empty `OPENAI_API_KEY` or a
/// `tokens` object.
fn codex_auth_present(text: &str) -> bool {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else { return false };
    let key = value.get("OPENAI_API_KEY").and_then(serde_json::Value::as_str).is_some_and(|key| !key.trim().is_empty());
    key || value.get("tokens").is_some_and(serde_json::Value::is_object)
}

/// pi `auth.json`: provider names whose entry is an API key with a key, or
/// an OAuth credential.
fn pi_auth_providers(text: &str) -> Vec<String> {
    let Ok(serde_json::Value::Object(map)) = serde_json::from_str::<serde_json::Value>(text) else { return Vec::new() };
    map.iter()
        .filter(|(_, entry)| match entry.get("type").and_then(serde_json::Value::as_str) {
            Some("api_key") => entry.get("key").and_then(serde_json::Value::as_str).is_some_and(|key| !key.trim().is_empty()),
            Some("oauth") => entry.as_object().is_some_and(|object| object.len() > 1),
            _ => false,
        })
        .map(|(name, _)| name.clone())
        .collect()
}

/// pi `models.json`: whether any provider carries a non-empty `apiKey`.
fn json_has_api_key(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Object(map) => map.iter().any(|(key, value)| {
            (key == "apiKey" && value.as_str().is_some_and(|text| !text.trim().is_empty())) || json_has_api_key(value)
        }),
        serde_json::Value::Array(items) => items.iter().any(json_has_api_key),
        _ => false,
    }
}

/// DeepSeek templates (docs/sandbox.md 13.12): Magpie (local gateway, full
/// access only) and the official endpoint (both modes).
pub fn deepseek_templates() -> Vec<ConfigTemplate> {
    vec![
        ConfigTemplate {
            id: "deepseek_magpie",
            title: "经 Magpie 本机网关",
            note: "只能在完全访问模式下使用（沙箱模式会在启动前拒绝本机回环地址）。model 写网关上的全名。",
            files: vec![
                TemplateFile {
                    path: DSH_PATCH_ACP,
                    content: "- id: llm-deepseek\n  config:\n    baseURL: http://127.0.0.1:3425/v1\n- id: acp\n  config:\n    provider: deepseek-official\n    model: opendesign/deepseek-v4.1-flash\n",
                },
                TemplateFile { path: DSH_ENV, content: "MAGPIE_GATEWAY_KEY=<Magpie 的 key>\nDEEPSEEK_API_KEY=magpie\n" },
            ],
        },
        ConfigTemplate {
            id: "deepseek_official",
            title: "直连官方 api.deepseek.com",
            note: "沙箱模式和完全访问都可以使用。",
            files: vec![
                TemplateFile {
                    path: DSH_PATCH_ACP,
                    content: "- id: llm-deepseek\n  config:\n    baseURL: https://api.deepseek.com/anthropic\n- id: acp\n  config:\n    provider: deepseek-official\n    model: deepseek-v4-flash\n",
                },
                TemplateFile { path: DSH_ENV, content: "DEEPSEEK_API_KEY=<DeepSeek 平台的 key>\n" },
            ],
        },
    ]
}

/// Classify `private_home` for `backend` in `mode`. `host_env` says which
/// Host variables are set; on Windows sandboxed mode the Host environment
/// never reaches the backend, so it does not count there.
pub fn probe(backend: AgentBackend, private_home: &Path, mode: PermissionMode, os: HostOs, host_env: EnvPresence<'_>) -> CredentialProbe {
    let env_reaches_backend = !(os == HostOs::Windows && mode == PermissionMode::Sandboxed);
    let mut present_keys = Vec::new();
    let mut present_files = Vec::new();
    let mut credentials_present = false;
    let mut loopback_blocked = false;
    let (required_keys, required_files, templates): (Vec<&str>, Vec<&str>, Vec<ConfigTemplate>) = match backend {
        AgentBackend::Deepseek => {
            let env_path = rel(private_home, DSH_ENV);
            if is_present_file(&env_path) {
                present_files.push(DSH_ENV.to_owned());
            }
            let names = read_small(&env_path).map(|text| dotenv_key_names(&text)).unwrap_or_default();
            for name in ["DEEPSEEK_API_KEY", "MAGPIE_GATEWAY_KEY"] {
                if names.iter().any(|known| known == name) {
                    present_keys.push(name.to_owned());
                }
            }
            credentials_present = names.iter().any(|name| name == "DEEPSEEK_API_KEY");
            if env_reaches_backend && host_env("DEEPSEEK_API_KEY") && !credentials_present {
                present_keys.push("DEEPSEEK_API_KEY（Host 环境）".to_owned());
                credentials_present = true;
            }
            for file in [DSH_PATCH_ACP, DSH_PATCH] {
                if is_present_file(&rel(private_home, file)) {
                    present_files.push(file.to_owned());
                }
            }
            if mode == PermissionMode::Sandboxed && !crate::backends::deepseek::LOOPBACK_VIA_PROXY_SUPPORTED {
                // The value is classified here and dropped; only the flag leaves.
                loopback_blocked = crate::backends::deepseek::base_url_candidates(private_home, private_home)
                    .iter()
                    .any(|candidate| crate::loopback::names_loopback(&candidate.value));
            }
            (vec!["DEEPSEEK_API_KEY"], vec![DSH_ENV, DSH_PATCH_ACP], deepseek_templates())
        }
        AgentBackend::Codex => {
            let auth = rel(private_home, CODEX_AUTH);
            if is_present_file(&auth) {
                present_files.push(CODEX_AUTH.to_owned());
                credentials_present = read_small(&auth).is_some_and(|text| codex_auth_present(&text));
            }
            if is_present_file(&rel(private_home, CODEX_CONFIG)) {
                present_files.push(CODEX_CONFIG.to_owned());
            }
            if env_reaches_backend && host_env("OPENAI_API_KEY") {
                present_keys.push("OPENAI_API_KEY（Host 环境）".to_owned());
                credentials_present = true;
            }
            (vec!["OPENAI_API_KEY"], vec![CODEX_AUTH, CODEX_CONFIG], Vec::new())
        }
        AgentBackend::Pi => {
            let auth = rel(private_home, PI_AUTH);
            if is_present_file(&auth) {
                present_files.push(PI_AUTH.to_owned());
                let providers = read_small(&auth).map(|text| pi_auth_providers(&text)).unwrap_or_default();
                credentials_present = !providers.is_empty();
                present_keys.extend(providers.into_iter().map(|provider| format!("auth.json: {provider}")));
            }
            let models = rel(private_home, PI_MODELS);
            if is_present_file(&models) {
                present_files.push(PI_MODELS.to_owned());
                if read_small(&models).and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok()).is_some_and(|value| json_has_api_key(&value)) {
                    present_keys.push("models.json: apiKey".to_owned());
                    credentials_present = true;
                }
            }
            if is_present_file(&rel(private_home, PI_SETTINGS)) {
                present_files.push(PI_SETTINGS.to_owned());
            }
            (Vec::new(), vec![PI_AUTH, PI_MODELS, PI_SETTINGS], Vec::new())
        }
        AgentBackend::Fox => (Vec::new(), Vec::new(), Vec::new()),
    };
    let state = if backend == AgentBackend::Fox {
        CredentialState::Ready
    } else if !credentials_present {
        CredentialState::MissingCredentials
    } else if loopback_blocked {
        CredentialState::LoopbackRequiresFullAccess
    } else {
        CredentialState::Ready
    };
    CredentialProbe {
        backend: backend.as_str().to_owned(),
        mode: mode.as_str().to_owned(),
        state,
        private_home: private_home.to_path_buf(),
        credentials_present,
        loopback_blocked,
        present_keys,
        present_files,
        required_keys: required_keys.into_iter().map(str::to_owned).collect(),
        required_files: required_files.into_iter().map(str::to_owned).collect(),
        templates,
    }
}

/// Where the user's real login lives.
#[derive(Debug, Clone)]
pub struct RealHome {
    pub home: PathBuf,
    /// The Host's `CODEX_HOME`, when set (else `<home>/.codex`).
    pub codex_home: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportKind {
    /// A whole file.
    File,
    /// One `KEY=value` line of a dotenv file.
    EnvKey,
}

/// One thing the real HOME has that the backend would read. Shown to the
/// user by `name` (a file name or a key name); no value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportItem {
    /// Stable id the confirmation sends back (`file:<dest>` / `env:<dest>#<KEY>`).
    pub id: String,
    pub kind: ImportKind,
    /// File name (`auth.json`) or key name (`DEEPSEEK_API_KEY`).
    pub name: String,
    /// Destination, relative to the private HOME (`.codex/auth.json`).
    pub destination: String,
    #[serde(skip)]
    source: PathBuf,
}

fn file_item(source: PathBuf, destination: &str) -> Option<ImportItem> {
    is_present_file(&source).then(|| ImportItem {
        id: format!("file:{destination}"),
        kind: ImportKind::File,
        name: destination.rsplit('/').next().unwrap_or(destination).to_owned(),
        destination: destination.to_owned(),
        source,
    })
}

/// The whitelist of `backend` found in the real HOME. Nothing else in the
/// real HOME is looked at or listed.
pub fn import_candidates(backend: AgentBackend, real: &RealHome) -> Vec<ImportItem> {
    let mut items = Vec::new();
    match backend {
        AgentBackend::Deepseek => {
            let env_path = rel(&real.home, DSH_ENV);
            let names = read_small(&env_path).map(|text| dotenv_key_names(&text)).unwrap_or_default();
            for key in ["DEEPSEEK_API_KEY", "MAGPIE_GATEWAY_KEY"] {
                if names.iter().any(|name| name == key) {
                    items.push(ImportItem {
                        id: format!("env:{DSH_ENV}#{key}"),
                        kind: ImportKind::EnvKey,
                        name: key.to_owned(),
                        destination: DSH_ENV.to_owned(),
                        source: env_path.clone(),
                    });
                }
            }
            items.extend(file_item(rel(&real.home, DSH_PATCH_ACP), DSH_PATCH_ACP));
            items.extend(file_item(rel(&real.home, DSH_PATCH), DSH_PATCH));
        }
        AgentBackend::Codex => {
            let codex = real.codex_home.clone().unwrap_or_else(|| real.home.join(".codex"));
            items.extend(file_item(codex.join("auth.json"), CODEX_AUTH));
            items.extend(file_item(codex.join("config.toml"), CODEX_CONFIG));
        }
        AgentBackend::Pi => {
            for file in [PI_AUTH, PI_MODELS, PI_SETTINGS] {
                items.extend(file_item(rel(&real.home, file), file));
            }
        }
        AgentBackend::Fox => {}
    }
    items
}

/// Audit record of one import: names only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportReport {
    pub backend: String,
    /// Private HOMEs written.
    pub homes: usize,
    pub files: Vec<String>,
    pub keys: Vec<String>,
    /// Ids the user confirmed that are not (or no longer) in the whitelist.
    pub skipped: Vec<String>,
}

impl ImportReport {
    /// The audit event (`agent_backend.credentials_imported`).
    pub fn audit_event(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "agent_backend.credentials_imported",
            "agentBackend": self.backend,
            "homes": self.homes,
            "files": self.files,
            "keys": self.keys,
            "skipped": self.skipped,
        })
    }
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> std::io::Result<()> {
    Ok(())
}

/// Create `dir` (and parents up to `home`) and make every directory from
/// `home` down 0700 on Unix.
fn ensure_private_dir(home: &Path, dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let mut current = Some(dir);
    while let Some(path) = current {
        set_mode(path, 0o700)?;
        if path == home {
            break;
        }
        current = path.parent().filter(|parent| parent.starts_with(home));
    }
    Ok(())
}

/// Write `bytes` to `path` via a temp file in the same directory: 0600 before
/// the content lands, then rename (a reader never sees a half file or a
/// wider mode).
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let tmp = path.with_extension(format!("fox-import-{}", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    set_mode(&tmp, 0o600)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })?;
    set_mode(path, 0o600)
}

/// `KEY=value` lines of `keys` in `source` replace the same keys in `dest`
/// (other lines of `dest` are kept).
fn merge_env_lines(dest: &str, source: &str, keys: &[&str]) -> String {
    let mut out: Vec<String> = dest
        .lines()
        .filter(|line| dotenv_assignment(line).is_none_or(|(name, _)| !keys.contains(&name)))
        .map(str::to_owned)
        .collect();
    for key in keys {
        if let Some(line) = source.lines().filter(|line| dotenv_assignment(line).is_some_and(|(name, _)| name == *key)).last() {
            out.push(line.trim_start().strip_prefix("export ").unwrap_or(line.trim_start()).to_owned());
        }
    }
    let mut text = out.join("\n");
    text.push('\n');
    text
}

/// Copy the items with ids in `selected` (re-checked against
/// [`import_candidates`]; anything else is skipped) into every private HOME
/// in `homes`. A copy, never a link; regular files only.
pub fn import_into(backend: AgentBackend, real: &RealHome, selected: &[String], homes: &[PathBuf]) -> Result<ImportReport, String> {
    let candidates = import_candidates(backend, real);
    let chosen: Vec<&ImportItem> = candidates.iter().filter(|item| selected.contains(&item.id)).collect();
    let skipped: Vec<String> = selected.iter().filter(|id| !candidates.iter().any(|item| &item.id == *id)).cloned().collect();
    let mut files: Vec<String> = Vec::new();
    let mut keys: Vec<String> = Vec::new();
    for home in homes {
        ensure_private_dir(home, home).map_err(|error| format!("cannot create the private HOME: {error}"))?;
        // Whole files.
        for item in chosen.iter().filter(|item| item.kind == ImportKind::File) {
            let bytes = read_small(&item.source)
                .map(String::into_bytes)
                .or_else(|| {
                    let meta = std::fs::symlink_metadata(&item.source).ok()?;
                    (meta.is_file() && meta.len() <= MAX_IMPORT_FILE_BYTES).then(|| std::fs::read(&item.source).ok()).flatten()
                })
                .ok_or_else(|| format!("{} is not a regular file of at most 1 MiB", item.name))?;
            let dest = rel(home, &item.destination);
            ensure_private_dir(home, dest.parent().unwrap_or(home)).map_err(|error| format!("cannot create {}: {error}", item.destination))?;
            write_private(&dest, &bytes).map_err(|error| format!("cannot write {}: {error}", item.destination))?;
            if !files.contains(&item.destination) {
                files.push(item.destination.clone());
            }
        }
        // dotenv keys, grouped per destination file.
        let mut destinations: Vec<&str> = chosen.iter().filter(|item| item.kind == ImportKind::EnvKey).map(|item| item.destination.as_str()).collect();
        destinations.dedup();
        for destination in destinations {
            let group: Vec<&&ImportItem> = chosen.iter().filter(|item| item.kind == ImportKind::EnvKey && item.destination == destination).collect();
            let source = read_small(&group[0].source).ok_or_else(|| format!("{destination} is not a regular file of at most 1 MiB"))?;
            let dest = rel(home, destination);
            let existing = if std::fs::symlink_metadata(&dest).is_ok_and(|meta| meta.file_type().is_symlink()) {
                return Err(format!("{destination} in the private HOME is a link; refusing to write through it"));
            } else {
                read_small(&dest).unwrap_or_default()
            };
            let names: Vec<&str> = group.iter().map(|item| item.name.as_str()).collect();
            ensure_private_dir(home, dest.parent().unwrap_or(home)).map_err(|error| format!("cannot create {destination}: {error}"))?;
            write_private(&dest, merge_env_lines(&existing, &source, &names).as_bytes()).map_err(|error| format!("cannot write {destination}: {error}"))?;
            for name in names {
                if !keys.iter().any(|known| known == name) {
                    keys.push(name.to_owned());
                }
            }
        }
    }
    Ok(ImportReport { backend: backend.as_str().to_owned(), homes: homes.len(), files, keys, skipped })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRETS: [&str; 6] = ["sk-real-deepseek-0001", "magpie-real-0002", "sk-openai-real-0003", "chatgpt-refresh-0004", "sk-ant-real-0005", "pi-models-key-0006"];

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("fox-onboarding-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(home: &Path, relative: &str, text: &str) {
        let path = rel(home, relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn no_env(_: &str) -> bool {
        false
    }

    fn assert_no_secret(value: &impl Serialize) {
        let text = serde_json::to_string(value).unwrap();
        for secret in SECRETS {
            assert!(!text.contains(secret), "{secret} leaked: {text}");
        }
    }

    #[test]
    fn dotenv_names_only_non_empty_assignments() {
        let names = dotenv_key_names("# DEEPSEEK_API_KEY=commented\nexport DEEPSEEK_API_KEY=\"sk-real-deepseek-0001\"\nEMPTY=\nQUOTED_EMPTY=''\nMAGPIE_GATEWAY_KEY = magpie-real-0002\nnot a line\n");
        assert_eq!(names, ["DEEPSEEK_API_KEY", "MAGPIE_GATEWAY_KEY"]);
    }

    #[test]
    fn probe_classifies_the_three_states_and_never_returns_a_value() {
        let root = scratch("probe");
        // Missing: an empty private HOME, every backend.
        for backend in [AgentBackend::Deepseek, AgentBackend::Codex, AgentBackend::Pi] {
            let home = root.join(format!("empty-{}", backend.as_str()));
            std::fs::create_dir_all(&home).unwrap();
            let probe = probe(backend, &home, PermissionMode::Sandboxed, HostOs::Linux, &no_env);
            assert_eq!(probe.state, CredentialState::MissingCredentials, "{backend:?}");
            assert!(!probe.credentials_present && !probe.loopback_blocked);
            assert!(!probe.required_files.is_empty());
        }
        // DeepSeek ready (official endpoint) in the sandbox.
        let ds = root.join("ds-official");
        write(&ds, DSH_ENV, "DEEPSEEK_API_KEY=sk-real-deepseek-0001\n");
        write(&ds, DSH_PATCH_ACP, "- id: llm-deepseek\n  config:\n    baseURL: https://api.deepseek.com/anthropic\n");
        let ready = probe(AgentBackend::Deepseek, &ds, PermissionMode::Sandboxed, HostOs::Linux, &no_env);
        assert_eq!(ready.state, CredentialState::Ready);
        assert_eq!(ready.present_keys, ["DEEPSEEK_API_KEY"]);
        assert_eq!(ready.present_files, [DSH_ENV, DSH_PATCH_ACP]);
        assert_eq!(ready.templates.len(), 2);
        assert_no_secret(&ready);
        // DeepSeek + Magpie: refused in the sandbox, ready in full access.
        let magpie = root.join("ds-magpie");
        write(&magpie, DSH_ENV, "MAGPIE_GATEWAY_KEY=magpie-real-0002\nDEEPSEEK_API_KEY=magpie\n");
        write(&magpie, DSH_PATCH_ACP, "- id: llm-deepseek\n  config:\n    baseURL: http://127.0.0.1:3425/v1\n");
        let refused = probe(AgentBackend::Deepseek, &magpie, PermissionMode::Sandboxed, HostOs::Linux, &no_env);
        assert_eq!(refused.state, CredentialState::LoopbackRequiresFullAccess);
        assert!(refused.credentials_present && refused.loopback_blocked);
        let mut without_templates = refused.clone();
        without_templates.templates.clear();
        let text = serde_json::to_string(&without_templates).unwrap();
        assert!(!text.contains("127.0.0.1") && !text.contains("3425"), "the base URL value is not echoed: {text}");
        assert!(refused.present_keys.contains(&"MAGPIE_GATEWAY_KEY".to_owned()));
        assert_no_secret(&refused);
        let full = probe(AgentBackend::Deepseek, &magpie, PermissionMode::FullAccess, HostOs::Linux, &no_env);
        assert_eq!(full.state, CredentialState::Ready);
        assert!(!full.loopback_blocked);
        // Loopback but no key: the missing login is reported first, both flags kept.
        let both = root.join("ds-both");
        write(&both, DSH_PATCH_ACP, "- id: llm-deepseek\n  config:\n    baseURL: http://localhost:3425/v1\n");
        let both = probe(AgentBackend::Deepseek, &both, PermissionMode::Sandboxed, HostOs::Linux, &no_env);
        assert_eq!(both.state, CredentialState::MissingCredentials);
        assert!(both.loopback_blocked);
        // Codex: auth.json with a key or tokens; Host env except in the Windows sandbox.
        let codex = root.join("codex");
        write(&codex, CODEX_AUTH, r#"{"OPENAI_API_KEY":null,"tokens":{"refresh_token":"chatgpt-refresh-0004"}}"#);
        let ready = probe(AgentBackend::Codex, &codex, PermissionMode::Sandboxed, HostOs::Linux, &no_env);
        assert_eq!(ready.state, CredentialState::Ready);
        assert_no_secret(&ready);
        let bare = root.join("codex-env");
        std::fs::create_dir_all(&bare).unwrap();
        let env = |name: &str| name == "OPENAI_API_KEY";
        assert_eq!(probe(AgentBackend::Codex, &bare, PermissionMode::Sandboxed, HostOs::Linux, &env).state, CredentialState::Ready);
        assert_eq!(probe(AgentBackend::Codex, &bare, PermissionMode::Sandboxed, HostOs::Windows, &env).state, CredentialState::MissingCredentials);
        assert_eq!(probe(AgentBackend::Codex, &bare, PermissionMode::FullAccess, HostOs::Windows, &env).state, CredentialState::Ready);
        write(&bare, CODEX_AUTH, r#"{"OPENAI_API_KEY":""}"#);
        assert_eq!(probe(AgentBackend::Codex, &bare, PermissionMode::Sandboxed, HostOs::Linux, &no_env).state, CredentialState::MissingCredentials);
        // pi: auth.json providers / models.json apiKey.
        let pi = root.join("pi");
        write(&pi, PI_AUTH, r#"{"anthropic":{"type":"api_key","key":"sk-ant-real-0005"},"broken":{"type":"api_key","key":""}}"#);
        let ready = probe(AgentBackend::Pi, &pi, PermissionMode::Sandboxed, HostOs::Linux, &no_env);
        assert_eq!(ready.state, CredentialState::Ready);
        assert_eq!(ready.present_keys, ["auth.json: anthropic"]);
        assert_no_secret(&ready);
        let pi_models = root.join("pi-models");
        write(&pi_models, PI_MODELS, r#"{"providers":{"local":{"baseUrl":"http://x","apiKey":"pi-models-key-0006"}}}"#);
        let ready = probe(AgentBackend::Pi, &pi_models, PermissionMode::Sandboxed, HostOs::Linux, &no_env);
        assert_eq!(ready.state, CredentialState::Ready);
        assert_no_secret(&ready);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn import_copies_only_the_whitelist_to_every_home_with_private_modes() {
        let root = scratch("import");
        let real = root.join("real");
        write(&real, DSH_ENV, "DEEPSEEK_API_KEY=sk-real-deepseek-0001\nMAGPIE_GATEWAY_KEY=magpie-real-0002\nUNRELATED_TOKEN=keep-out\n");
        write(&real, DSH_PATCH_ACP, "- id: acp\n  config:\n    model: deepseek-v4-flash\n");
        write(&real, ".dsh/sessions/old.jsonl", "history");
        write(&real, ".ssh/id_ed25519", "private");
        let real = RealHome { home: real, codex_home: None };
        let candidates = import_candidates(AgentBackend::Deepseek, &real);
        let names: Vec<&str> = candidates.iter().map(|item| item.name.as_str()).collect();
        assert_eq!(names, ["DEEPSEEK_API_KEY", "MAGPIE_GATEWAY_KEY", "cordis.patch.yml"]);
        assert_no_secret(&candidates);
        assert!(!serde_json::to_string(&candidates).unwrap().contains(&real.home.display().to_string()), "source paths are not serialized");

        let homes = vec![root.join("homes/p1/deepseek"), root.join("homes/p2/deepseek")];
        // An existing private .env keeps its other lines; the imported key replaces its own.
        write(&homes[0], DSH_ENV, "DEEPSEEK_API_KEY=old\nOTHER=1\n");
        let selected = vec![
            "env:.dsh/.env#DEEPSEEK_API_KEY".to_owned(),
            format!("file:{DSH_PATCH_ACP}"),
            "file:.ssh/id_ed25519".to_owned(),
            "env:.dsh/.env#UNRELATED_TOKEN".to_owned(),
        ];
        let report = import_into(AgentBackend::Deepseek, &real, &selected, &homes).unwrap();
        assert_eq!(report.homes, 2);
        assert_eq!(report.files, [DSH_PATCH_ACP]);
        assert_eq!(report.keys, ["DEEPSEEK_API_KEY"]);
        assert_eq!(report.skipped, ["file:.ssh/id_ed25519", "env:.dsh/.env#UNRELATED_TOKEN"]);
        assert_no_secret(&report);
        assert_no_secret(&report.audit_event());
        let first = std::fs::read_to_string(rel(&homes[0], DSH_ENV)).unwrap();
        assert_eq!(first, "OTHER=1\nDEEPSEEK_API_KEY=sk-real-deepseek-0001\n");
        let second = std::fs::read_to_string(rel(&homes[1], DSH_ENV)).unwrap();
        assert_eq!(second, "DEEPSEEK_API_KEY=sk-real-deepseek-0001\n");
        for home in &homes {
            assert!(!rel(home, ".ssh").exists() && !rel(home, ".dsh/sessions").exists());
            let patch = rel(home, DSH_PATCH_ACP);
            assert!(!std::fs::symlink_metadata(&patch).unwrap().file_type().is_symlink(), "a copy, not a link");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
                assert_eq!(mode(&patch), 0o600);
                assert_eq!(mode(&rel(home, DSH_ENV)), 0o600);
                assert_eq!(mode(&rel(home, ".dsh")), 0o700);
                assert_eq!(mode(&rel(home, ".dsh/profiles/acp")), 0o700);
                assert_eq!(mode(home), 0o700);
            }
        }
        // The copy is independent of the source.
        write(&real.home, DSH_PATCH_ACP, "changed");
        assert_ne!(std::fs::read_to_string(rel(&homes[1], DSH_PATCH_ACP)).unwrap(), "changed");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn import_refuses_links_and_uses_the_codex_and_pi_whitelists() {
        let root = scratch("import-links");
        let real = root.join("real");
        write(&real, "elsewhere/secret.json", r#"{"OPENAI_API_KEY":"sk-openai-real-0003"}"#);
        std::fs::create_dir_all(real.join(".codex")).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(real.join("elsewhere/secret.json"), real.join(".codex/auth.json")).unwrap();
        write(&real, ".codex/config.toml", "model = \"o4\"\n");
        write(&real, ".codex/history.jsonl", "x");
        let real_home = RealHome { home: real.clone(), codex_home: None };
        let codex: Vec<String> = import_candidates(AgentBackend::Codex, &real_home).into_iter().map(|item| item.destination).collect();
        #[cfg(unix)]
        assert_eq!(codex, [CODEX_CONFIG], "a linked auth.json is not offered");
        // CODEX_HOME override.
        write(&root, "codex-home/auth.json", r#"{"OPENAI_API_KEY":"sk-openai-real-0003"}"#);
        let custom = RealHome { home: real.clone(), codex_home: Some(root.join("codex-home")) };
        let items = import_candidates(AgentBackend::Codex, &custom);
        assert_eq!(items.iter().map(|item| item.name.as_str()).collect::<Vec<_>>(), ["auth.json"]);
        let home = root.join("homes/p1/codex");
        let report = import_into(AgentBackend::Codex, &custom, &["file:.codex/auth.json".to_owned()], std::slice::from_ref(&home)).unwrap();
        assert_eq!(report.files, [CODEX_AUTH]);
        assert_no_secret(&report.audit_event());
        assert_eq!(probe(AgentBackend::Codex, &home, PermissionMode::Sandboxed, HostOs::Linux, &no_env).state, CredentialState::Ready);
        // pi whitelist.
        write(&real, PI_AUTH, r#"{"anthropic":{"type":"api_key","key":"sk-ant-real-0005"}}"#);
        write(&real, ".pi/agent/sessions/a.jsonl", "x");
        let pi: Vec<String> = import_candidates(AgentBackend::Pi, &real_home).into_iter().map(|item| item.destination).collect();
        assert_eq!(pi, [PI_AUTH]);
        let _ = std::fs::remove_dir_all(&root);
    }
}
