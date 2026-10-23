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
    /// Settings batch, item 6: what the unified login (Fox「模型服务」) does
    /// for this backend at the next launch; `None` when 模型服务 has no
    /// usable default provider. Names only.
    pub unified_login: Option<crate::unified_login::UnifiedReport>,
}

/// Names of Host environment variables that are set (non-empty). Values are
/// never looked at beyond emptiness.
pub type EnvPresence<'a> = &'a dyn Fn(&str) -> bool;

pub(crate) const DSH_ENV: &str = ".dsh/.env";
pub(crate) const DSH_PATCH_ACP: &str = ".dsh/profiles/acp/cordis.patch.yml";
pub(crate) const DSH_PATCH: &str = ".dsh/cordis.patch.yml";
pub(crate) const CODEX_AUTH: &str = ".codex/auth.json";
pub(crate) const CODEX_CONFIG: &str = ".codex/config.toml";
pub(crate) const PI_AUTH: &str = ".pi/agent/auth.json";
pub(crate) const PI_MODELS: &str = ".pi/agent/models.json";
pub(crate) const PI_SETTINGS: &str = ".pi/agent/settings.json";

pub(crate) fn rel(home: &Path, relative: &str) -> PathBuf {
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

pub(crate) fn read_small(path: &Path) -> Option<String> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_IMPORT_FILE_BYTES {
        return None;
    }
    std::fs::read_to_string(path).ok()
}

pub(crate) fn is_present_file(path: &Path) -> bool {
    std::fs::symlink_metadata(path).map(|meta| meta.is_file() && meta.len() > 0).unwrap_or(false)
}

/// Codex `auth.json` holds a usable login: a non-empty `OPENAI_API_KEY` or a
/// `tokens` object.
pub(crate) fn codex_auth_present(text: &str) -> bool {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else { return false };
    let key = value.get("OPENAI_API_KEY").and_then(serde_json::Value::as_str).is_some_and(|key| !key.trim().is_empty());
    key || value.get("tokens").is_some_and(serde_json::Value::is_object)
}

/// pi `auth.json`: provider names whose entry is an API key with a key, or
/// an OAuth credential.
pub(crate) fn pi_auth_providers(text: &str) -> Vec<String> {
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
pub(crate) fn json_has_api_key(value: &serde_json::Value) -> bool {
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
                    content: "- id: llm-deepseek\n  config:\n    baseURL: https://api.deepseek.com/anthropic\n- id: acp\n  config:\n    provider: deepseek-official\n    model: deepseek-flash\n",
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
        unified_login: None,
    }
}

/// Whether a launch should be refused BEFORE the backend starts because the
/// probe found no login (`agent_backend_missing_credentials`), so the user
/// gets the login card instead of the backend's own raw error.
///
/// Only where the probe is authoritative:
/// * DeepSeek: dsh needs `DEEPSEEK_API_KEY` (private `.dsh/.env`, or the Host
///   environment where it reaches the backend);
/// * Codex: no usable `auth.json`, no `OPENAI_API_KEY` reaching the backend
///   AND no `config.toml` (a config may name another provider / key the probe
///   does not model, so its presence lets Codex start and decide);
/// * pi: never (pi reads provider keys from many places, e.g. the Host
///   environment; the card still shows, but the launch is not blocked).
pub fn launch_blocked_by_missing_login(probe: &CredentialProbe) -> bool {
    if probe.state != CredentialState::MissingCredentials {
        return false;
    }
    match probe.backend.as_str() {
        "deepseek" => true,
        "codex" => !probe.present_files.iter().any(|file| file == CODEX_CONFIG),
        _ => false,
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

/// One private HOME an import writes to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportTarget {
    /// Project UUID.
    pub project: String,
    /// What the user calls the project (its name; the UUID when unknown).
    pub label: String,
    pub home: PathBuf,
}

/// How an import treats a login that is already there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImportOptions<'a> {
    /// "覆盖已有登录" (off by default): replace a file / key the private HOME
    /// already has, after backing it up. Off: keep it and report the skip.
    pub overwrite: bool,
    /// Name of the backup directory under [`BACKUP_DIR`] (`[A-Za-z0-9_-]`,
    /// e.g. `20261022-084512`); used only when something is overwritten.
    pub backup_stamp: &'a str,
}

impl Default for ImportOptions<'_> {
    fn default() -> Self {
        Self { overwrite: false, backup_stamp: "backup" }
    }
}

/// Directory of the private HOME that holds overwritten logins
/// (`<private HOME>/.fox-credential-backups/<stamp>/<relative path>`).
pub const BACKUP_DIR: &str = ".fox-credential-backups";

/// A project whose private HOME already had some of the items: kept, not
/// copied (names only).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportSkip {
    pub project: String,
    pub label: String,
    /// File destinations (`.codex/auth.json`) and key names (`DEEPSEEK_API_KEY`).
    pub items: Vec<String>,
}

/// Where the overwritten items of one project were backed up.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportBackup {
    pub project: String,
    pub label: String,
    /// Absolute backup directory (Host only: never sent to the UI or the
    /// audit, it contains the user's SID / profile path).
    #[serde(skip)]
    pub directory: PathBuf,
    /// The backup directory relative to the private HOME
    /// (`.fox-credential-backups/<stamp>`): what the receipt shows and the
    /// audit records (settings batch, item 3).
    #[serde(rename = "directory")]
    pub relative_directory: String,
    /// Files backed up, relative to the private HOME.
    pub items: Vec<String>,
}

/// Audit record of one import: names only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportReport {
    pub backend: String,
    /// Private HOMEs targeted.
    pub homes: usize,
    /// Private HOMEs where at least one item was written.
    pub homes_written: usize,
    /// Whether "覆盖已有登录" was chosen.
    pub overwrite: bool,
    /// Files / keys written to at least one private HOME.
    pub files: Vec<String>,
    pub keys: Vec<String>,
    /// Ids the user confirmed that are not (or no longer) in the whitelist.
    pub skipped: Vec<String>,
    /// Projects that already had some items (kept, not copied).
    pub skipped_existing: Vec<ImportSkip>,
    /// Backups made before overwriting (overwrite only).
    pub backups: Vec<ImportBackup>,
}

impl ImportReport {
    /// The audit event (`agent_backend.credentials_imported`): names only,
    /// backup directories relative to the private HOME.
    pub fn audit_event(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "agent_backend.credentials_imported",
            "agentBackend": self.backend,
            "homes": self.homes,
            "homesWritten": self.homes_written,
            "overwrite": self.overwrite,
            "files": self.files,
            "keys": self.keys,
            "skipped": self.skipped,
            "skippedExisting": self.skipped_existing.iter().map(|skip| serde_json::json!({ "project": skip.project, "items": skip.items })).collect::<Vec<_>>(),
            "backups": self.backups.iter().map(|backup| serde_json::json!({
                "project": backup.project,
                "items": backup.items,
                "directory": backup.relative_directory,
            })).collect::<Vec<_>>(),
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
pub(crate) fn ensure_private_dir(home: &Path, dir: &Path) -> std::io::Result<()> {
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
pub(crate) fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
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

/// `.fox-credential-backups/<stamp>` for a backup directory: the form the
/// receipt and the audit use (no absolute path, no SID).
pub fn relative_backup_directory(directory: &Path) -> String {
    match directory.file_name() {
        Some(name) => format!("{BACKUP_DIR}/{}", name.to_string_lossy()),
        None => BACKUP_DIR.to_owned(),
    }
}

/// Backup directory name for `now`, UTC: `YYYYMMDD-HHMMSS`.
pub fn backup_stamp(now: std::time::SystemTime) -> String {
    let secs = now.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let (days, rem) = ((secs / 86_400) as i64, secs % 86_400);
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}{month:02}{day:02}-{:02}{:02}{:02}", rem / 3600, rem % 3600 / 60, rem % 60)
}

fn valid_stamp(stamp: &str) -> bool {
    !stamp.is_empty() && stamp.len() <= 64 && stamp.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn is_link(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink())
}

/// Per-home backup of the files about to be overwritten.
struct Backup<'a> {
    home: &'a Path,
    stamp: &'a str,
    directory: Option<PathBuf>,
    items: Vec<String>,
}

impl Backup<'_> {
    /// Copy `<home>/<relative>` into the backup directory (created on first
    /// use, private; a fresh `-2`, `-3` ... suffix if the stamp is taken).
    fn save(&mut self, relative: &str) -> Result<(), String> {
        let source = rel(self.home, relative);
        let bytes = std::fs::read(&source).map_err(|error| format!("cannot back up {relative}: {error}"))?;
        if self.directory.is_none() {
            let root = self.home.join(BACKUP_DIR);
            ensure_private_dir(self.home, &root).map_err(|error| format!("cannot create {BACKUP_DIR}: {error}"))?;
            let mut n = 1;
            let directory = loop {
                let name = if n == 1 { self.stamp.to_owned() } else { format!("{}-{n}", self.stamp) };
                let candidate = root.join(&name);
                match std::fs::create_dir(&candidate) {
                    Ok(()) => break candidate,
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists && n < 100 => n += 1,
                    Err(error) => return Err(format!("cannot create the backup directory: {error}")),
                }
            };
            set_mode(&directory, 0o700).map_err(|error| format!("cannot protect the backup directory: {error}"))?;
            self.directory = Some(directory);
        }
        let directory = self.directory.clone().expect("created above");
        let dest = rel(&directory, relative);
        ensure_private_dir(self.home, dest.parent().unwrap_or(&directory)).map_err(|error| format!("cannot create the backup of {relative}: {error}"))?;
        write_private(&dest, &bytes).map_err(|error| format!("cannot back up {relative}: {error}"))?;
        self.items.push(relative.to_owned());
        Ok(())
    }
}

/// Copy the items with ids in `selected` (re-checked against
/// [`import_candidates`]; anything else is skipped) into every target's
/// private HOME. A copy, never a link; regular files only.
///
/// A login already there is KEPT by default: a file the private HOME has, or
/// a `.env` key it already assigns, is not touched and the project is listed
/// in [`ImportReport::skipped_existing`] (Windows acceptance of 80b1622:
/// "套用到我所有项目" silently replaced the main project's Codex
/// `auth.json`). With [`ImportOptions::overwrite`] it is replaced after the
/// old file is copied to `<home>/.fox-credential-backups/<stamp>/`. Other
/// `.env` lines are always kept.
pub fn import_into(backend: AgentBackend, real: &RealHome, selected: &[String], targets: &[ImportTarget], options: ImportOptions<'_>) -> Result<ImportReport, String> {
    if options.overwrite && !valid_stamp(options.backup_stamp) {
        return Err(format!("invalid backup name {:?}", options.backup_stamp));
    }
    let candidates = import_candidates(backend, real);
    let chosen: Vec<&ImportItem> = candidates.iter().filter(|item| selected.contains(&item.id)).collect();
    let skipped: Vec<String> = selected.iter().filter(|id| !candidates.iter().any(|item| &item.id == *id)).cloned().collect();
    let mut files: Vec<String> = Vec::new();
    let mut keys: Vec<String> = Vec::new();
    let mut skipped_existing: Vec<ImportSkip> = Vec::new();
    let mut backups: Vec<ImportBackup> = Vec::new();
    let mut homes_written = 0;
    for target in targets {
        let home = target.home.as_path();
        ensure_private_dir(home, home).map_err(|error| format!("cannot create the private HOME: {error}"))?;
        let mut kept: Vec<String> = Vec::new();
        let mut backup = Backup { home, stamp: options.backup_stamp, directory: None, items: Vec::new() };
        let mut wrote = false;
        // Whole files.
        for item in chosen.iter().filter(|item| item.kind == ImportKind::File) {
            let dest = rel(home, &item.destination);
            let exists = std::fs::symlink_metadata(&dest).is_ok();
            if exists && !options.overwrite {
                kept.push(item.destination.clone());
                continue;
            }
            if is_link(&dest) || (exists && !dest.is_file()) {
                return Err(format!("{} in the private HOME is not a regular file; refusing to overwrite it", item.destination));
            }
            let bytes = read_small(&item.source)
                .map(String::into_bytes)
                .or_else(|| {
                    let meta = std::fs::symlink_metadata(&item.source).ok()?;
                    (meta.is_file() && meta.len() <= MAX_IMPORT_FILE_BYTES).then(|| std::fs::read(&item.source).ok()).flatten()
                })
                .ok_or_else(|| format!("{} is not a regular file of at most 1 MiB", item.name))?;
            if exists {
                backup.save(&item.destination)?;
            }
            ensure_private_dir(home, dest.parent().unwrap_or(home)).map_err(|error| format!("cannot create {}: {error}", item.destination))?;
            write_private(&dest, &bytes).map_err(|error| format!("cannot write {}: {error}", item.destination))?;
            wrote = true;
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
            if is_link(&dest) {
                return Err(format!("{destination} in the private HOME is a link; refusing to write through it"));
            }
            let existing = if std::fs::symlink_metadata(&dest).is_ok() {
                read_small(&dest).ok_or_else(|| format!("{destination} in the private HOME is not a regular text file of at most 1 MiB; refusing to rewrite it"))?
            } else {
                String::new()
            };
            let present = dotenv_key_names(&existing);
            let mut names: Vec<&str> = Vec::new();
            let mut replaced = false;
            for item in &group {
                let name = item.name.as_str();
                if present.iter().any(|known| known == name) {
                    if !options.overwrite {
                        kept.push(name.to_owned());
                        continue;
                    }
                    replaced = true;
                }
                names.push(name);
            }
            if names.is_empty() {
                continue;
            }
            if replaced {
                backup.save(destination)?;
            }
            ensure_private_dir(home, dest.parent().unwrap_or(home)).map_err(|error| format!("cannot create {destination}: {error}"))?;
            write_private(&dest, merge_env_lines(&existing, &source, &names).as_bytes()).map_err(|error| format!("cannot write {destination}: {error}"))?;
            wrote = true;
            for name in names {
                if !keys.iter().any(|known| known == name) {
                    keys.push(name.to_owned());
                }
            }
        }
        if wrote {
            homes_written += 1;
        }
        if !kept.is_empty() {
            skipped_existing.push(ImportSkip { project: target.project.clone(), label: target.label.clone(), items: kept });
        }
        if let Some(directory) = backup.directory {
            let relative_directory = relative_backup_directory(&directory);
            backups.push(ImportBackup { project: target.project.clone(), label: target.label.clone(), directory, relative_directory, items: backup.items });
        }
    }
    Ok(ImportReport {
        backend: backend.as_str().to_owned(),
        homes: targets.len(),
        homes_written,
        overwrite: options.overwrite,
        files,
        keys,
        skipped,
        skipped_existing,
        backups,
    })
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

    /// Targets named after the `homes/<project>/<backend>` layout of the tests.
    fn targets(homes: &[PathBuf]) -> Vec<ImportTarget> {
        homes
            .iter()
            .map(|home| {
                let project = home.parent().and_then(Path::file_name).unwrap().to_string_lossy().into_owned();
                ImportTarget { label: format!("Project {project}"), project, home: home.clone() }
            })
            .collect()
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
        // An existing private .env keeps its other lines AND its own key (kept by default).
        write(&homes[0], DSH_ENV, "DEEPSEEK_API_KEY=old\nOTHER=1\n");
        let selected = vec![
            "env:.dsh/.env#DEEPSEEK_API_KEY".to_owned(),
            format!("file:{DSH_PATCH_ACP}"),
            "file:.ssh/id_ed25519".to_owned(),
            "env:.dsh/.env#UNRELATED_TOKEN".to_owned(),
        ];
        let report = import_into(AgentBackend::Deepseek, &real, &selected, &targets(&homes), ImportOptions::default()).unwrap();
        assert_eq!(report.homes, 2);
        assert_eq!(report.homes_written, 2);
        assert_eq!(report.skipped_existing, [ImportSkip { project: "p1".into(), label: "Project p1".into(), items: vec!["DEEPSEEK_API_KEY".into()] }]);
        assert!(report.backups.is_empty() && !rel(&homes[0], BACKUP_DIR).exists());
        assert_eq!(report.files, [DSH_PATCH_ACP]);
        assert_eq!(report.keys, ["DEEPSEEK_API_KEY"]);
        assert_eq!(report.skipped, ["file:.ssh/id_ed25519", "env:.dsh/.env#UNRELATED_TOKEN"]);
        assert_no_secret(&report);
        assert_no_secret(&report.audit_event());
        let first = std::fs::read_to_string(rel(&homes[0], DSH_ENV)).unwrap();
        assert_eq!(first, "DEEPSEEK_API_KEY=old\nOTHER=1\n", "the existing key is kept");
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
                if home == &homes[1] {
                    assert_eq!(mode(&rel(home, DSH_ENV)), 0o600, "written by the import");
                }
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
        let report = import_into(AgentBackend::Codex, &custom, &["file:.codex/auth.json".to_owned()], &targets(std::slice::from_ref(&home)), ImportOptions::default()).unwrap();
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

    /// 80b1622 acceptance: "套用到我所有项目" replaced the main project's Codex
    /// auth.json (3886 B -> 3948 B). Default: an existing file / key is kept
    /// and reported by project; "覆盖已有登录" backs it up first and says where.
    /// The current project follows the same rule. The audit has names only.
    #[test]
    fn import_keeps_existing_logins_unless_overwrite_and_backs_them_up() {
        let root = scratch("import-keep");
        let real = root.join("real");
        write(&real, CODEX_AUTH, r#"{"OPENAI_API_KEY":"sk-openai-real-0003"}"#);
        write(&real, CODEX_CONFIG, "model = \"o4\"\n");
        write(&real, DSH_ENV, "DEEPSEEK_API_KEY=sk-real-deepseek-0001\nMAGPIE_GATEWAY_KEY=magpie-real-0002\n");
        let real = RealHome { home: real, codex_home: None };
        let homes = vec![root.join("homes/main/codex"), root.join("homes/fresh/codex")];
        let main_auth = r#"{"tokens":{"refresh_token":"chatgpt-refresh-0004"}}"#;
        write(&homes[0], CODEX_AUTH, main_auth);
        let selected = vec![format!("file:{CODEX_AUTH}"), format!("file:{CODEX_CONFIG}")];

        // Default: main keeps its auth.json (gets the missing config.toml), fresh gets both.
        let report = import_into(AgentBackend::Codex, &real, &selected, &targets(&homes), ImportOptions::default()).unwrap();
        assert_eq!(std::fs::read_to_string(rel(&homes[0], CODEX_AUTH)).unwrap(), main_auth, "never silently replaced");
        assert!(rel(&homes[0], CODEX_CONFIG).is_file());
        assert!(std::fs::read_to_string(rel(&homes[1], CODEX_AUTH)).unwrap().contains("sk-openai-real-0003"));
        assert_eq!(report.homes_written, 2);
        assert!(!report.overwrite);
        assert_eq!(report.skipped_existing, [ImportSkip { project: "main".into(), label: "Project main".into(), items: vec![CODEX_AUTH.into()] }]);
        assert!(report.backups.is_empty());
        assert!(!rel(&homes[0], BACKUP_DIR).exists());
        assert_no_secret(&report);
        assert_no_secret(&report.audit_event());

        // Again, default: everything already there, nothing written anywhere.
        let again = import_into(AgentBackend::Codex, &real, &selected, &targets(&homes), ImportOptions::default()).unwrap();
        assert_eq!(again.homes_written, 0);
        assert!(again.files.is_empty());
        assert_eq!(again.skipped_existing.len(), 2);
        assert_eq!(again.skipped_existing[1].items, [CODEX_AUTH, CODEX_CONFIG]);

        // Overwrite: backed up under the private HOME with the stamp, then replaced.
        let report = import_into(AgentBackend::Codex, &real, &selected[..1], &targets(&homes[..1]), ImportOptions { overwrite: true, backup_stamp: "20261022-084512" }).unwrap();
        assert!(report.overwrite && report.skipped_existing.is_empty());
        assert_eq!(report.backups.len(), 1);
        let backup = &report.backups[0];
        assert_eq!(backup.directory, homes[0].join(BACKUP_DIR).join("20261022-084512"));
        // Settings batch, item 3: the UI and the audit get the relative form only.
        assert_eq!(backup.relative_directory, ".fox-credential-backups/20261022-084512");
        let wire = serde_json::to_value(&report).unwrap();
        assert_eq!(wire["backups"][0]["directory"], ".fox-credential-backups/20261022-084512");
        assert!(!wire.to_string().contains(&homes[0].display().to_string()), "no absolute path on the wire");
        assert_eq!(report.audit_event()["backups"][0]["directory"], ".fox-credential-backups/20261022-084512");
        assert_eq!(backup.items, [CODEX_AUTH]);
        assert_eq!(std::fs::read_to_string(rel(&backup.directory, CODEX_AUTH)).unwrap(), main_auth);
        assert!(std::fs::read_to_string(rel(&homes[0], CODEX_AUTH)).unwrap().contains("sk-openai-real-0003"));
        let audit = report.audit_event();
        assert_eq!(audit["backups"][0]["directory"], format!("{BACKUP_DIR}/20261022-084512"));
        assert!(!serde_json::to_string(&audit).unwrap().contains(&root.display().to_string()), "no absolute path in the audit");
        assert_no_secret(&audit);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&homes[0].join(BACKUP_DIR)), 0o700);
            assert_eq!(mode(&backup.directory), 0o700);
            assert_eq!(mode(&rel(&backup.directory, CODEX_AUTH)), 0o600);
        }
        // Same stamp again: a fresh directory, the first backup untouched.
        let second = import_into(AgentBackend::Codex, &real, &selected[..1], &targets(&homes[..1]), ImportOptions { overwrite: true, backup_stamp: "20261022-084512" }).unwrap();
        assert_eq!(second.backups[0].directory, homes[0].join(BACKUP_DIR).join("20261022-084512-2"));
        assert_eq!(std::fs::read_to_string(rel(&backup.directory, CODEX_AUTH)).unwrap(), main_auth);
        assert!(import_into(AgentBackend::Codex, &real, &selected, &targets(&homes), ImportOptions { overwrite: true, backup_stamp: "../x" }).is_err());

        // .env merge by key: existing keys kept by default, missing ones added.
        let ds = vec![root.join("homes/main/deepseek")];
        write(&ds[0], DSH_ENV, "DEEPSEEK_API_KEY=mine\nOTHER=1\n");
        let keys = vec![format!("env:{DSH_ENV}#DEEPSEEK_API_KEY"), format!("env:{DSH_ENV}#MAGPIE_GATEWAY_KEY")];
        let report = import_into(AgentBackend::Deepseek, &real, &keys, &targets(&ds), ImportOptions::default()).unwrap();
        assert_eq!(std::fs::read_to_string(rel(&ds[0], DSH_ENV)).unwrap(), "DEEPSEEK_API_KEY=mine\nOTHER=1\nMAGPIE_GATEWAY_KEY=magpie-real-0002\n");
        assert_eq!(report.keys, ["MAGPIE_GATEWAY_KEY"]);
        assert_eq!(report.skipped_existing[0].items, ["DEEPSEEK_API_KEY"]);
        assert_no_secret(&report.audit_event());
        // Overwrite: the whole .env is backed up first, other lines kept.
        let report = import_into(AgentBackend::Deepseek, &real, &keys[..1], &targets(&ds), ImportOptions { overwrite: true, backup_stamp: "s1" }).unwrap();
        assert_eq!(std::fs::read_to_string(rel(&ds[0], DSH_ENV)).unwrap(), "OTHER=1\nMAGPIE_GATEWAY_KEY=magpie-real-0002\nDEEPSEEK_API_KEY=sk-real-deepseek-0001\n");
        assert_eq!(std::fs::read_to_string(rel(&report.backups[0].directory, DSH_ENV)).unwrap(), "DEEPSEEK_API_KEY=mine\nOTHER=1\nMAGPIE_GATEWAY_KEY=magpie-real-0002\n");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn backup_stamp_is_utc_and_path_safe() {
        let at = |secs: u64| backup_stamp(std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs));
        assert_eq!(at(0), "19700101-000000");
        assert_eq!(at(1_792_658_712), "20261022-084512");
        assert_eq!(at(951_782_400), "20000229-000000");
        assert!(valid_stamp(&at(1_792_658_712)));
    }

    /// The pre-launch refusal only where the probe is authoritative.
    #[test]
    fn launch_is_blocked_only_for_a_missing_deepseek_or_codex_login() {
        let root = scratch("launch-blocked");
        let empty = root.join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        let blocked = |backend| launch_blocked_by_missing_login(&probe(backend, &empty, PermissionMode::Sandboxed, HostOs::Windows, &no_env));
        assert!(blocked(AgentBackend::Codex));
        assert!(blocked(AgentBackend::Deepseek));
        assert!(!blocked(AgentBackend::Pi), "pi may read keys the probe does not see");
        assert!(!blocked(AgentBackend::Fox));
        // Codex with a config.toml (may name another provider): let it start.
        let configured = root.join("configured");
        write(&configured, CODEX_CONFIG, "model_provider = \"azure\"\n");
        assert!(!launch_blocked_by_missing_login(&probe(AgentBackend::Codex, &configured, PermissionMode::Sandboxed, HostOs::Windows, &no_env)));
        // A login present: never blocked.
        let ready = root.join("ready");
        write(&ready, CODEX_AUTH, r#"{"OPENAI_API_KEY":"sk-openai-real-0003"}"#);
        assert!(!launch_blocked_by_missing_login(&probe(AgentBackend::Codex, &ready, PermissionMode::Sandboxed, HostOs::Windows, &no_env)));
        let _ = std::fs::remove_dir_all(&root);
    }
}
