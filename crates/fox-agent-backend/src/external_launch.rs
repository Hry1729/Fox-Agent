//! The real launcher for external backends (feature `acp`): Fox-managed
//! runtime layout, per-backend launch specs, the private HOME, and the
//! Agent-native permission mode. External Agents (pi / Codex / DeepSeek) run
//! in their own native sandboxes and approvals; Fox adds no sandbox around
//! them. The desktop builds one [`HostLauncher`] at startup and hands it to
//! [`crate::external::ExternalRuntime`].
//!
//! Managed runtime layout (the same on every OS; on Windows it is the package
//! the installer puts in place):
//!
//! ```text
//! <runtime>/node/node.exe | node/bin/node     Node (Linux / macOS: else `node` on PATH)
//! <runtime>/backends/<pi|codex|deepseek>/node_modules/.bin/<pi-acp|codex-acp|dsh>[.cmd]
//! <runtime>/dsh-patches/<variant>.yml          DeepSeek Host patch (verified byte for byte)
//! <runtime>/pi-gate/pi-gated.{sh,cmd}, fox-approval-gate.ts   optional pi approval gate
//! ```
//!
//! `<runtime>`: `FOX_AGENT_RUNTIME_ROOT` when set; Windows
//! `%ProgramFiles%\Fox\sandbox-runtime\<version>` (`FOX_RUNTIME_ROOT`, else
//! `FOX_RUNTIME_VERSION`, else the newest installed version); Linux / macOS
//! `<fox data>/agent-runtime`.
//!
//! The backend must be installed (managed runtime, or an explicit
//! `FOX_*_PROGRAM` override for development); the Host never launches through
//! `npx` (`agent_backend_not_installed`). The full-access switches
//! (`FOX_EXTERNAL_FULL_ACCESS`, `FOX_EXTERNAL_FULL_ACCESS_FORBIDDEN`) are still
//! read: they feed the recorded mode and the new-conversation UI.

use crate::capability_view::PiLaunch;
use crate::config::HostPolicy;
use crate::external::{ExternalLauncher, LaunchFailure, LaunchRequest, Launched};
use crate::launch::LaunchSpec;
use crate::port::AgentBackend;
use crate::external_home::{prepare_launch, HostOs, PermissionMode, PrivateDirs, PrivateHomes};
use futures::future::BoxFuture;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Override of the managed runtime directory (every OS).
pub const AGENT_RUNTIME_ROOT_ENV: &str = "FOX_AGENT_RUNTIME_ROOT";
/// Windows: the runtime version directory under `%ProgramFiles%\Fox\sandbox-runtime`.
pub const RUNTIME_VERSION_ENV: &str = "FOX_RUNTIME_VERSION";
pub use crate::launch::RUNTIME_PACKAGE_VERSION;
/// Engineering full-access grant (see the module docs).
pub const FULL_ACCESS_ENV: &str = "FOX_EXTERNAL_FULL_ACCESS";
/// Enterprise switch: no external session may record full access.
pub const FULL_ACCESS_FORBIDDEN_ENV: &str = "FOX_EXTERNAL_FULL_ACCESS_FORBIDDEN";
/// pi providers whose built-in model API hosts the Host approves
/// (`anthropic,openai`).
pub const PI_PROVIDERS_ENV: &str = "FOX_PI_PROVIDERS";
/// Extra model endpoints per backend the Host approves
/// (`pi=api.example.com;codex=...`). A local model gateway is written as
/// `127.0.0.1:<port>`.
pub const EXTRA_DOMAINS_ENV: &str = "FOX_EXTERNAL_EXTRA_DOMAINS";
/// Host variables (e.g. `RUSTUP_HOME,CARGO_HOME`) passed to the backend on
/// top of the environment allowlist.
pub const ENV_PASSTHROUGH_ENV: &str = "FOX_EXTERNAL_ENV_PASSTHROUGH";

/// Full-access request + grant, parsed from [`FULL_ACCESS_ENV`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum FullAccessSwitch {
    #[default]
    Off,
    All,
    Keys(Vec<String>),
}

impl FullAccessSwitch {
    pub fn parse(value: Option<&str>) -> Self {
        let Some(raw) = value.map(str::trim).filter(|raw| !raw.is_empty()) else {
            return Self::Off;
        };
        match raw.to_ascii_lowercase().as_str() {
            "0" | "off" | "false" | "no" | "none" => Self::Off,
            "1" | "on" | "true" | "yes" | "all" => Self::All,
            _ => Self::Keys(raw.split([',', ';']).map(str::trim).filter(|key| !key.is_empty()).map(str::to_owned).collect()),
        }
    }
}

/// Everything the launcher needs from the Host environment.
#[derive(Debug, Clone)]
pub struct HostLaunchConfig {
    pub os: HostOs,
    /// Fox data directory (Linux / macOS private HOMEs live under it).
    pub fox_data: PathBuf,
    /// Fox state directory (model endpoint grants, login source choice,
    /// session temp directories).
    pub state_root: PathBuf,
    pub runtime_root: PathBuf,
    pub native_login_home: Option<crate::onboarding::RealHome>,
    pub full_access: FullAccessSwitch,
    pub full_access_forbidden: bool,
    pub pi_providers: Vec<String>,
    pub extra_domains: BTreeMap<AgentBackend, Vec<String>>,
    /// Host variables passed to the backend ([`ENV_PASSTHROUGH_ENV`]).
    pub env_passthrough: Vec<String>,
}

/// [`FULL_ACCESS_FORBIDDEN_ENV`] as the Host reads it (`1` / `true` / `on` /
/// `yes`). Shared by the launcher and the new-conversation command, so the
/// UI's disabled option and the backend refusal can never disagree.
pub fn full_access_forbidden_from_env() -> bool {
    matches!(env(FULL_ACCESS_FORBIDDEN_ENV).as_deref().map(str::trim), Some("1" | "true" | "on" | "yes"))
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.trim().is_empty())
}

/// Newest version directory under `parent` (lexicographic), if any.
fn newest_version_dir(parent: &Path) -> Option<PathBuf> {
    let mut versions: Vec<PathBuf> = std::fs::read_dir(parent)
        .ok()?
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false))
        .filter(|entry| !entry.file_name().to_string_lossy().starts_with('.'))
        .map(|entry| entry.path())
        .collect();
    versions.sort();
    versions.pop()
}

pub fn parse_extra_domains(value: Option<&str>) -> BTreeMap<AgentBackend, Vec<String>> {
    let mut out: BTreeMap<AgentBackend, Vec<String>> = BTreeMap::new();
    for part in value.unwrap_or_default().split(';') {
        let Some((backend, domains)) = part.split_once('=') else { continue };
        let Some(backend) = AgentBackend::parse(backend.trim()) else { continue };
        out.entry(backend)
            .or_default()
            .extend(domains.split(',').map(str::trim).filter(|domain| !domain.is_empty()).map(str::to_owned));
    }
    out
}

impl HostLaunchConfig {
    /// The pi-acp turn-outcome adaptation of the managed runtime matches its
    /// build-time pin ([`crate::runtime_adapters::PI_TURN_OUTCOME_ADAPTER`]).
    pub fn pi_turn_adapter_verified(&self) -> bool {
        crate::runtime_adapters::verified_files(&self.runtime_root, &crate::runtime_adapters::PI_TURN_OUTCOME_ADAPTER)
    }

    /// What an external pi needs beyond being installed (plan C1/C2), checked
    /// after [`Self::launch_spec_with`] found the program, so a pi that is not
    /// installed is reported as such first:
    ///
    /// 1. Fox's approval gate (`pi-gate/`): pi has no permission system of its
    ///    own, so without the gate its tools would run unasked and no Fox tier
    ///    (只读 / 询问 / 自动 / 完全访问) could be enforced
    ///    (`agent_backend_pi_gate_missing`).
    /// 2. The pinned turn-outcome adaptation of pi-acp, so a model failure is
    ///    reported instead of ending the turn silently
    ///    (`agent_backend_runtime_adapter_missing`). An explicit
    ///    `FOX_PI_ACP_PROGRAM` (development) is the developer's own pi-acp and
    ///    is not checked against the package pins.
    pub fn pi_preflight(&self, pi_launch: PiLaunch, adapter_verified: impl FnOnce() -> bool) -> Result<(), LaunchFailure> {
        if pi_launch != PiLaunch::Gated {
            return Err(LaunchFailure::new("agent_backend_pi_gate_missing", "pi 运行时缺少 Fox 审批拦截扩展（pi-gate），Fox 无法按所选权限档位应答 pi 的工具调用，已拒绝启动")
                .remedy("安装或修复 Fox 外部运行时（安装包自带 pi-gate）后重试")
                .detail(serde_json::json!({ "gate": self.pi_gate_script() })));
        }
        if crate::launch::program_override(AgentBackend::Pi).is_none() && !adapter_verified() {
            return Err(LaunchFailure::new("agent_backend_runtime_adapter_missing", "pi 运行时缺少已验证的模型错误适配（pi-acp fox-turn-outcome）")
                .remedy("安装或修复 Fox 外部运行时后重试")
                .detail(serde_json::json!({ "runtimeRoot": self.runtime_root, "files": crate::runtime_adapters::PI_TURN_OUTCOME_ADAPTER })));
        }
        Ok(())
    }
    /// Read the configuration from the process environment. `fox_data` is the
    /// app data directory.
    pub fn from_env(fox_data: &Path) -> Self {
        let os = HostOs::current();
        let runtime_root = env(AGENT_RUNTIME_ROOT_ENV).map(PathBuf::from).unwrap_or_else(|| match os {
            HostOs::Windows => {
                if env(crate::launch::RUNTIME_ROOT_ENV).is_some() || env(RUNTIME_VERSION_ENV).is_some() {
                    crate::launch::windows_runtime_root(&env(RUNTIME_VERSION_ENV).unwrap_or_default())
                } else {
                    let parent = crate::launch::default_windows_runtime_root("");
                    let expected = parent.join(RUNTIME_PACKAGE_VERSION);
                    if expected.is_dir() { expected } else { newest_version_dir(&parent).unwrap_or(parent) }
                }
            }
            _ => fox_data.join("agent-runtime"),
        });
        let state_root = match os {
            HostOs::Windows => std::env::var_os("LOCALAPPDATA")
                .map(|base| PathBuf::from(base).join("Fox").join("state"))
                .unwrap_or_else(|| fox_data.join("agent-state")),
            _ => fox_data.join("agent-state"),
        };
        Self {
            os,
            fox_data: fox_data.to_path_buf(),
            state_root,
            runtime_root,
            native_login_home: crate::login_source::real_home(),
            full_access: FullAccessSwitch::parse(env(FULL_ACCESS_ENV).as_deref()),
            full_access_forbidden: full_access_forbidden_from_env(),
            pi_providers: env(PI_PROVIDERS_ENV)
                .map(|value| value.split(',').map(str::trim).filter(|item| !item.is_empty()).map(str::to_owned).collect())
                .unwrap_or_default(),
            extra_domains: parse_extra_domains(env(EXTRA_DOMAINS_ENV).as_deref()),
            env_passthrough: env(ENV_PASSTHROUGH_ENV)
                .map(|value| value.split([',', ';']).map(str::trim).filter(|item| !item.is_empty()).map(str::to_owned).collect())
                .unwrap_or_default(),
        }
    }

    /// The layer-1 Host policy this configuration amounts to.
    pub fn host_policy(&self, conversation_id: &str, project_root: &Path) -> HostPolicy {
        let mut policy = HostPolicy { full_access_forbidden: self.full_access_forbidden, ..HostPolicy::default() };
        match &self.full_access {
            FullAccessSwitch::Off => {}
            FullAccessSwitch::All => {
                policy.full_access_grants.insert(crate::external_home::FullAccessGrant::Conversation(conversation_id.to_owned()).key());
            }
            FullAccessSwitch::Keys(keys) => policy.full_access_grants.extend(keys.iter().cloned()),
        }
        let _ = project_root;
        policy
    }

    /// Whether the conversation runs in its Agent's own full access (plan
    /// C2): [`PermissionMode::FullAccess`] exactly when the effective native
    /// mode is the Agent's full-access option (Codex `agent-full-access`, dsh
    /// `danger-full-access`, pi `full_access`). Fox adds no outer layer; this
    /// only records what the native mode is, so leaving full access can be
    /// applied the Agent's way (dsh restarts, see
    /// [`crate::native_permission::full_access_needs_restart`]).
    ///
    /// The legacy `full_access_granted` flag and the engineering switch no
    /// longer decide anything here.
    pub fn permission_mode(&self, request: &LaunchRequest) -> PermissionMode {
        if crate::native_permission::is_full_access(request.backend, request.native_mode()) {
            PermissionMode::FullAccess
        } else {
            PermissionMode::Standard
        }
    }

    fn exe(&self, name: &str) -> String {
        if self.os == HostOs::Windows {
            format!("{name}.cmd")
        } else {
            name.to_owned()
        }
    }

    /// The installed entry point of `backend`: [`crate::launch::program_override_env`]
    /// when set, else the first existing of
    /// `<runtime>/backends/<dir>/node_modules/.bin/<bin>[.cmd]` and
    /// `<runtime>/backends/<dir>/<bin>[.cmd]`. Never `npx`.
    pub fn managed_program(&self, backend: AgentBackend) -> Option<PathBuf> {
        if let Some(program) = crate::launch::program_override(backend) {
            // An override that points at nothing is "not installed", not a
            // launch that fails later.
            let program = PathBuf::from(program);
            return program.is_file().then_some(program);
        }
        crate::launch::managed_entry(&self.runtime_root, backend, self.os == HostOs::Windows)
    }

    pub fn managed_node(&self) -> Option<PathBuf> {
        let candidate = if self.os == HostOs::Windows {
            self.runtime_root.join("node").join("node.exe")
        } else {
            self.runtime_root.join("node").join("bin").join("node")
        };
        if candidate.is_file() {
            return Some(candidate);
        }
        if self.os == HostOs::Windows {
            return None;
        }
        crate::external_home::which("node")
    }

    /// Which pi launch a session would get: gated when the managed runtime
    /// ships the Fox approval gate (`pi-gate/`), else stock (no Host approvals).
    pub fn pi_launch(&self) -> PiLaunch {
        if self.pi_gate_script().is_file() { PiLaunch::Gated } else { PiLaunch::Stock }
    }

    fn pi_gate_script(&self) -> PathBuf {
        self.runtime_root.join("pi-gate").join(if self.os == HostOs::Windows { "pi-gated.cmd" } else { "pi-gated.sh" })
    }

    /// The launch spec for `backend` in the Agent's default native mode,
    /// always an installed program ([`Self::managed_program`]); a backend that
    /// is not installed is refused (`agent_backend_not_installed`). Production
    /// never launches through `npx`.
    pub fn launch_spec(&self, backend: AgentBackend, project_root: &Path) -> Result<(LaunchSpec, PiLaunch), LaunchFailure> {
        let native = crate::native_permission::effective_mode(backend, None, false, &Default::default());
        self.launch_spec_with(backend, native, project_root)
    }

    /// [`Self::launch_spec`] in the Agent's own permission mode `native`
    /// ([`crate::native_permission`]):
    /// Codex starts in that codex-acp mode (`INITIAL_AGENT_MODE`), dsh with
    /// the `--patch` of that preset (approval written per preset), pi
    /// unchanged (the Host answers its gate).
    pub fn launch_spec_with(&self, backend: AgentBackend, native: &str, project_root: &Path) -> Result<(LaunchSpec, PiLaunch), LaunchFailure> {
        if backend == AgentBackend::Fox {
            return Err(LaunchFailure::new("agent_backend_invalid", "Fox 不经外部运行时启动"));
        }
        let program = self.managed_program(backend).ok_or_else(|| {
            LaunchFailure::new(
                "agent_backend_not_installed",
                format!("外部后端「{}」未安装在 Fox 受管运行时 {}", backend.as_str(), self.runtime_root.display()),
            )
            .remedy("安装或修复 Fox 的外部后端运行时（随 Fox 安装包一并安装）；开发调试可用环境变量指定已安装的入口")
            .detail(serde_json::json!({ "runtimeRoot": self.runtime_root, "overrideEnv": crate::launch::program_override_env(backend) }))
        })?;
        let program_text = program.to_string_lossy().into_owned();
        let mut pi_launch = PiLaunch::Stock;
        let mut spec = match backend {
            AgentBackend::Deepseek => {
                use crate::backends::deepseek::{default_preset_patch_path, preset_file_name, preset_patch, verify_installed_preset_patch};
                // The preset the user picked in dsh's own permission list.
                // Fox no longer translates its old full-access flag onto it.
                let preset = crate::native_permission::dsh_preset(native);
                // Fox adds no network sandbox around dsh, so dsh's own
                // workspace preset keeps its native web tools.
                let network = true;
                let managed_patch = default_preset_patch_path(&self.runtime_root, preset, network);
                let patch = if managed_patch.is_file() {
                    managed_patch
                } else {
                    // A program override outside the runtime package: the Host
                    // patch is written to Fox state.
                    let dir = self.state_root.join(crate::backends::deepseek::PATCH_DIR);
                    let patch = dir.join(preset_file_name(preset, network));
                    std::fs::create_dir_all(&dir)
                        .and_then(|_| std::fs::write(&patch, preset_patch(preset, network).patch_yaml))
                        .map_err(|error| LaunchFailure::new("agent_backend_launch_invalid", format!("cannot write the dsh patch: {error}")))?;
                    patch
                };
                verify_installed_preset_patch(&patch, preset, network).map_err(|error| {
                    LaunchFailure::new("agent_backend_not_installed", error.to_string())
                        .remedy("重新安装（重新启用）Fox 的外部后端运行时以带上这个权限的 dsh 补丁；或先改用「工作区可写（不询问）」")
                        .detail(serde_json::json!({ "preset": preset.id(), "patch": preset_file_name(preset, network) }))
                })?;
                crate::backends::deepseek::launch_spec(&program_text, &patch)
                    .map_err(|error| LaunchFailure::new("agent_backend_launch_invalid", error.to_string()))?
            }
            _ => {
                let mut spec = crate::launch::launch_spec(backend).expect("external backend");
                spec.program = program_text;
                spec.args.clear();
                spec
            }
        };
        match backend {
            AgentBackend::Codex => {
                let _ = project_root;
                // The codex-acp mode the user picked. Codex's own sandbox and
                // approval policy stay exactly in that mode; Fox never turns
                // it into full access.
                spec.session_env = crate::backends::codex::session_env_for_mode(crate::native_permission::codex_mode(native));
            }
            AgentBackend::Pi => {
                let gate = self.pi_gate_script();
                if gate.is_file() {
                    let pi_bin = program.parent().map(|dir| dir.join(self.exe("pi"))).unwrap_or_else(|| PathBuf::from(self.exe("pi")));
                    spec.session_env = vec![
                        ("PI_ACP_PI_COMMAND".into(), gate.to_string_lossy().into_owned()),
                        ("FOX_PI_BIN".into(), pi_bin.to_string_lossy().into_owned()),
                    ];
                    pi_launch = PiLaunch::Gated;
                }
            }
            _ => {}
        }
        // Tool-chain variables (RUSTUP_HOME, CARGO_HOME ...) the Host chose
        // to pass: an explicit Host allowlist, not the whole environment.
        for name in &self.env_passthrough {
            if let Some(value) = std::env::var_os(name) {
                spec.session_env.retain(|(key, _)| key != name);
                spec.session_env.push((name.clone(), value.to_string_lossy().into_owned()));
            }
        }
        Ok((spec, pi_launch))
    }

    /// The host of the unified login's base URL when the Fox-managed login
    /// is (or will be) what the backend reads in `home`; `None` for the
    /// user's own login, no 模型服务 provider, or a loopback / malformed URL.
    pub fn unified_login_host(&self, request: &LaunchRequest, home: &Path) -> Option<String> {
        let login = request.unified_login.as_ref()?;
        let plan = crate::unified_login::plan(request.backend, home, login, false);
        if !matches!(plan.outcome, crate::unified_login::Outcome::Generated | crate::unified_login::Outcome::Unchanged) {
            return None;
        }
        let base = match request.backend {
            AgentBackend::Deepseek => crate::unified_login::deepseek_base_url(login).ok()?,
            _ => login.base_url.clone(),
        };
        crate::model_endpoint::Endpoint::parse(&base).ok().map(|endpoint| endpoint.entry)
    }

    /// Native files select the destination, but never grant network access.
    /// A Host-selected supplier or an explicitly confirmed import does that.
    pub fn model_endpoint(&self, request: &LaunchRequest, home: &Path) -> Result<Option<crate::model_endpoint::Endpoint>, LaunchFailure> {
        let invalid = |message| LaunchFailure::new("agent_backend_model_endpoint_invalid", message)
            .remedy("在私有模型配置中明确选择静态地址和模型，然后从真实 HOME 重新导入配置");
        let native = crate::model_endpoint::resolve(request.backend, home, &request.project_root).map_err(invalid)?;
        let trusted = request.unified_login.as_ref().and_then(|login| {
            let base = if request.backend == AgentBackend::Deepseek { crate::unified_login::deepseek_base_url(login).ok()? } else { login.base_url.clone() };
            crate::model_endpoint::Endpoint::parse(&base).ok()
        });
        if let Some(endpoint) = native.endpoint.as_ref() {
            let imported = crate::model_endpoint::import_approved(&self.state_root, request.backend, home, &native).map_err(invalid)?;
            let explicit = self.extra_domains.get(&request.backend).is_some_and(|entries| entries.iter().any(|entry| entry.eq_ignore_ascii_case(&endpoint.entry) || entry.eq_ignore_ascii_case(&endpoint.host)));
            let built_in = request.backend == AgentBackend::Deepseek && endpoint.entry == "api.deepseek.com:443";
            let selected_pi_builtin = request.backend == AgentBackend::Pi && native.provider.as_ref().is_some_and(|provider| self.pi_providers.contains(provider))
                && crate::model_endpoint::pi_provider_domains(native.provider.as_deref().unwrap_or_default()).is_some_and(|hosts| hosts.contains(&endpoint.host.as_str())) && endpoint.port == 443;
            if trusted.as_ref() == Some(endpoint) || imported || explicit || built_in || selected_pi_builtin { return Ok(Some(endpoint.clone())); }
            return Err(LaunchFailure::new("agent_backend_model_endpoint_unapproved", format!("原生模型配置选择了 {}，但没有匹配的 Host 网络授权，或导入后配置已变化", endpoint.entry))
                .remedy("保留当前原生配置。在登录卡片中重新导入真实 HOME 的模型和设置文件；已有文件需明确选择覆盖。也可在 Fox 模型服务中选择同一端点")
                .detail(serde_json::json!({"endpoint":endpoint.entry,"credentialsAreNotReachability":true})));
        }
        Ok(self.unified_login_host(request, home).and_then(|_| trusted))
    }
}

/// Error code: the backend's private HOME has no login it reads
/// ([`crate::onboarding::launch_blocked_by_missing_login`]); refused before
/// the backend starts so the UI shows the login card, not a raw backend error.
pub const MISSING_CREDENTIALS: &str = "agent_backend_missing_credentials";

/// Remedy of [`MISSING_CREDENTIALS`].
pub const MISSING_CREDENTIALS_REMEDY: &str = "在输入框上方的登录卡片里打开私有 HOME 放入登录文件，或从真实 HOME 导入";

/// Why the full-access choice is unavailable, for the new-conversation UI
/// (`None` when it can be chosen).
pub const FULL_ACCESS_FORBIDDEN_REASON: &str = "本机已禁止完全访问（FOX_EXTERNAL_FULL_ACCESS_FORBIDDEN=1）";

impl HostLaunchConfig {
    /// The private HOME and session temp root of an external backend.
    ///
    /// On Windows the per-user root earlier releases kept under
    /// `%ProgramData%\Fox\sandbox\<SID>` is reused when it already exists, so
    /// logins made there keep working; otherwise the per-user
    /// `<fox data>/agent-homes` is used.
    pub fn private_dirs(&self, request: &LaunchRequest) -> Result<PrivateDirs, LaunchFailure> {
        let project_uuid = request.project_uuid.clone().unwrap_or_default();
        let (homes_root, tmp_root) = self.external_private_roots();
        let home = PrivateHomes::new(homes_root).home_for(&project_uuid, request.backend)?;
        Ok(PrivateDirs { private_home: home.path, tmp_root })
    }

    /// External backends' private roots (see [`Self::private_dirs`]).
    fn external_private_roots(&self) -> (PathBuf, PathBuf) {
        if self.os == HostOs::Windows {
            if let Some(root) = crate::external_home::legacy_windows_private_root() {
                return (root.join("homes"), root.join("tmp"));
            }
        }
        (self.fox_data.join("agent-homes"), self.state_root.join("agent-tmp"))
    }

    /// The private HOME of `backend` in project `project_uuid` (used by every
    /// external launch). Reads only; nothing is created.
    pub fn private_home_path(&self, project_uuid: &str, backend: AgentBackend) -> Result<PathBuf, LaunchFailure> {
        let (root, _) = self.external_private_roots();
        Ok(PrivateHomes::new(root).home_for(project_uuid, backend)?.path)
    }

    /// Login probe for a conversation's backend (UX batch ①): which login
    /// files / key names its private HOME has. Names only
    /// ([`crate::onboarding::probe`]).
    pub fn credential_probe(&self, request: &LaunchRequest) -> Result<crate::onboarding::CredentialProbe, LaunchFailure> {
        let project_uuid = request.project_uuid.clone().unwrap_or_default();
        let home = self.private_home_path(&project_uuid, request.backend)?;
        let host_env = |name: &str| std::env::var_os(name).is_some_and(|value| !value.is_empty());
        let mut probe = crate::onboarding::probe(request.backend, &home, &host_env);
        if request.backend == AgentBackend::Pi {
            let mut source = crate::login_source::status(&self.state_root, &home, self.native_login_home.as_ref())
                .map_err(|e| LaunchFailure::new("agent_backend_source_failed", e))?;
            let choice_required = source.choice_required;
            if !choice_required && source.current == "fox" {
                if let Some(login) = &request.unified_login {
                    source.provider = Some(login.provider_name.clone());
                    source.model = Some(login.model.clone());
                    source.model_endpoint = crate::model_endpoint::Endpoint::parse(&login.base_url).ok().map(|endpoint| endpoint.entry);
                }
            }
            probe.source = Some(source);
            if choice_required { return Ok(probe); }
        }
        if let Some(login) = &request.unified_login {
            let plan = crate::unified_login::plan(request.backend, &home, login, host_login_reaches(&probe));
            if matches!(plan.outcome, crate::unified_login::Outcome::Generated | crate::unified_login::Outcome::Unchanged) {
                // The launch will generate (or already generated) a login
                // from 模型服务: ready.
                probe.credentials_present = true;
                probe.state = crate::onboarding::CredentialState::Ready;
            }
            probe.unified_login = Some(plan);
        }
        Ok(probe)
    }

    /// Settings batch, item 6: generate the backend's config from Fox「模型
    /// 服务」 in the private HOME the launch uses (the user's own login and
    /// files are never touched), or remove Fox-managed files when 模型服务
    /// has no usable default provider. Windows: the private root is prepared
    /// before and its DACL re-applied after, as for an import.
    pub fn apply_unified_login(&self, request: &LaunchRequest) -> Result<Option<crate::unified_login::UnifiedReport>, LaunchFailure> {
        if request.backend == AgentBackend::Fox {
            return Ok(None);
        }
        let private = self.private_dirs(request)?;
        let home = private.private_home;
        crate::external_home::ensure_backend_home_dirs(request.backend, &home)
            .map_err(|error| LaunchFailure::new("agent_backend_unified_login_failed", format!("无法创建私有 HOME：{error}")))?;
        if request.backend == AgentBackend::Pi {
            crate::login_source::prepare(&self.state_root, &home, self.native_login_home.as_ref())
                .map_err(|e| LaunchFailure::new("agent_backend_source_required", e))?;
        }
        let report = match &request.unified_login {
            Some(login) => {
                let host_env = |name: &str| std::env::var_os(name).is_some_and(|value| !value.is_empty());
                let probe = crate::onboarding::probe(request.backend, &home, &host_env);
                let report = crate::unified_login::apply(request.backend, &home, login, host_login_reaches(&probe))
                    .map_err(|error| LaunchFailure::new("agent_backend_unified_login_failed", format!("无法按模型服务生成登录配置：{error}")))?;
                eprintln!(
                    "fox.external unified_login agent_backend={} outcome={:?} files={:?} user_files={:?}",
                    request.backend.as_str(),
                    report.outcome,
                    report.files,
                    report.user_files
                );
                Some(report)
            }
            None => {
                let removed = crate::unified_login::remove_managed(request.backend, &home)
                    .map_err(|error| LaunchFailure::new("agent_backend_unified_login_failed", error))?;
                if !removed.is_empty() {
                    eprintln!("fox.external unified_login agent_backend={} removed={removed:?}", request.backend.as_str());
                }
                None
            }
        };
        if self.os == HostOs::Windows {
            self.private_dirs(request)?;
        }
        Ok(report)
    }

    /// [`MISSING_CREDENTIALS`] when the probe of this request says the
    /// backend has no login ([`crate::onboarding::launch_blocked_by_missing_login`]);
    /// `None` otherwise, and when the probe itself cannot run (the launch then
    /// reports its own error). Names only in the detail.
    pub fn missing_login_refusal(&self, request: &LaunchRequest) -> Option<LaunchFailure> {
        let probe = self.credential_probe(request).ok()?;
        if !crate::onboarding::launch_blocked_by_missing_login(&probe) {
            return None;
        }
        Some(
            LaunchFailure::new(
                MISSING_CREDENTIALS,
                format!("外部后端「{}」还没有登录：Fox 为本项目准备的私有 HOME 里没有它读取的登录（{}）", request.backend.as_str(), probe.required_files.join("、")),
            )
            .remedy(MISSING_CREDENTIALS_REMEDY)
            .detail(serde_json::json!({
                "state": probe.state.as_str(),
                "privateHome": probe.private_home,
                "requiredKeys": probe.required_keys,
                "requiredFiles": probe.required_files,
                "presentFiles": probe.present_files,
            })),
        )
    }

    /// Copy the confirmed login items from the real HOME into the private
    /// HOME of `backend` in every project of `projects` (`(uuid, label)`; UX
    /// batch ①). A login already there is kept unless `overwrite`, in which
    /// case it is backed up first ([`crate::onboarding::import_into`]).
    /// Windows: the private root is created / verified before and its DACL
    /// re-applied after the copy (the same [`prepare_windows_private_root`]
    /// the launch uses); Unix: files 0600, directories 0700.
    pub fn import_credentials(
        &self,
        backend: AgentBackend,
        projects: &[(String, String)],
        real: &crate::onboarding::RealHome,
        selected: &[String],
        overwrite: bool,
    ) -> Result<crate::onboarding::ImportReport, LaunchFailure> {
        if backend == AgentBackend::Fox || projects.is_empty() {
            return Err(LaunchFailure::new("agent_backend_import_invalid", "没有可导入的外部后端或项目"));
        }
        // Import into the same private HOME a launch will read.
        let (homes_root, _) = self.external_private_roots();
        std::fs::create_dir_all(&homes_root).map_err(|error| {
            LaunchFailure::new("agent_backend_import_failed", format!("cannot create {}: {error}", homes_root.display()))
        })?;
        let homes = PrivateHomes::new(homes_root);
        let targets = projects
            .iter()
            .map(|(uuid, label)| {
                homes
                    .home_for(uuid, backend)
                    .map(|home| crate::onboarding::ImportTarget { project: uuid.clone(), label: label.clone(), home: home.path })
                    .map_err(LaunchFailure::from)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let stamp = crate::onboarding::backup_stamp(std::time::SystemTime::now());
        let options = crate::onboarding::ImportOptions { overwrite, backup_stamp: &stamp };
        let trusted_import = crate::model_endpoint::confirmed_import_model(backend, real, selected)
            .map_err(|error| LaunchFailure::new("agent_backend_model_endpoint_import_failed", error))?;
        let report = crate::onboarding::import_into(backend, real, selected, &targets, options).map_err(|error| LaunchFailure::new("agent_backend_import_failed", error))?;
        for target in &targets {
            if let Some(native) = &trusted_import {
                let copied = crate::model_endpoint::resolve(backend, &target.home, &target.home.join(".fox-no-project-settings"))
                    .map_err(|error| LaunchFailure::new("agent_backend_model_endpoint_import_failed", error))?;
                if copied.fingerprint != native.fingerprint { continue; }
                crate::model_endpoint::approve_import_snapshot(&self.state_root, backend, &target.home, native)
                    .map_err(|error| LaunchFailure::new("agent_backend_model_endpoint_import_failed", format!("文件已导入，但模型端点授权未完成：{error}")))?;
            }
        }
        Ok(report)
    }


}

/// Whether a login variable of the Host environment reaches the backend
/// (the probe lists it as `NAME（Host 环境）`).
fn host_login_reaches(probe: &crate::onboarding::CredentialProbe) -> bool {
    probe.present_keys.iter().any(|key| key.ends_with("（Host 环境）"))
}

/// The production [`ExternalLauncher`].
#[derive(Clone)]
pub struct HostLauncher {
    pub config: HostLaunchConfig,
    /// Plan C3: the Host's Fox tool MCP bridge. Every launch gets a fresh
    /// token and the bridge is injected the way its Agent supports
    /// ([`crate::fox_mcp::injection_for`]). `None` only in tests that do not
    /// exercise the bridge.
    pub mcp: Option<crate::fox_mcp::McpBridge>,
}

/// Revokes a launch's bridge token unless the launch succeeded (then the
/// token lives until the process stops: [`Launched::on_closed`]).
struct McpTokenGuard {
    bridge: crate::fox_mcp::McpBridge,
    token: String,
    armed: bool,
}

impl McpTokenGuard {
    fn into_on_closed(mut self) -> Box<dyn FnOnce() + Send> {
        self.armed = false;
        let bridge = self.bridge.clone();
        let token = std::mem::take(&mut self.token);
        Box::new(move || bridge.revoke(&token))
    }
}

impl Drop for McpTokenGuard {
    fn drop(&mut self) {
        if self.armed {
            self.bridge.revoke(&self.token);
        }
    }
}

/// What a launch hands its Agent for the Fox tools.
struct McpLaunch {
    guard: McpTokenGuard,
    stdio: Option<crate::fox_mcp::McpStdioServer>,
}

impl HostLauncher {
    pub fn new(config: HostLaunchConfig) -> Self {
        Self { config, mcp: None }
    }

    /// [`Self::new`] with the Fox tool MCP bridge (plan C3).
    pub fn with_mcp(config: HostLaunchConfig, bridge: crate::fox_mcp::McpBridge) -> Self {
        Self { config, mcp: Some(bridge) }
    }

    /// Issue the launch's token and put the bridge where its Agent reads it:
    /// Codex / dsh an ACP stdio MCP server (the relay, run by the runtime's
    /// Node); pi the environment of the gated pi, which loads the Fox tool
    /// extension. Nothing is launched without it (no silent "no tools").
    fn prepare_mcp(config: &HostLaunchConfig, bridge: &crate::fox_mcp::McpBridge, request: &LaunchRequest, spec: &mut LaunchSpec) -> Result<McpLaunch, LaunchFailure> {
        use crate::fox_mcp::{injection_for, write_assets, McpInjection, PI_EXTENSION_ENV};
        let unavailable = |message: String| {
            LaunchFailure::new("agent_backend_fox_tools_unavailable", message).remedy("安装或修复 Fox 外部运行时后重试")
        };
        let node = config.managed_node().ok_or_else(|| unavailable(format!("找不到运行 Fox 工具桥（MCP）的 Node：{}", config.runtime_root.display())))?;
        if request.backend == AgentBackend::Pi {
            // The installed gate must know how to load the Fox tool extension
            // (runtime packages before C3 do not).
            let gate = config.pi_gate_script();
            let supports = std::fs::read_to_string(&gate).map(|text| text.contains(PI_EXTENSION_ENV)).unwrap_or(false);
            if !supports {
                return Err(LaunchFailure::new("agent_backend_fox_tools_unsupported", "已安装的 pi-gate 不支持加载 Fox 工具扩展（MCP 桥），pi 将无法使用交付文件、知识库和提问工具，已拒绝启动")
                    .remedy("升级 Fox 外部运行时（新版运行时包自带支持 Fox 工具的 pi-gate）后重试")
                    .detail(serde_json::json!({ "gate": gate })));
            }
        }
        let assets = write_assets(&config.state_root).map_err(|error| unavailable(format!("无法准备 Fox 工具桥：{error}")))?;
        let grant = bridge.issue(&request.conversation_id, request.backend).map_err(|error| unavailable(format!("无法签发 Fox 工具桥令牌：{error}")))?;
        let guard = McpTokenGuard { bridge: bridge.clone(), token: grant.token.clone(), armed: true };
        let stdio = match injection_for(request.backend, &grant, &node, &assets) {
            Some(McpInjection::AcpStdio(server)) => Some(server),
            Some(McpInjection::PiExtension { env }) => {
                for (name, value) in env {
                    spec.session_env.retain(|(key, _)| key != &name);
                    spec.session_env.push((name, value));
                }
                None
            }
            None => return Err(LaunchFailure::new("agent_backend_invalid", "Fox 不经外部运行时启动")),
        };
        Ok(McpLaunch { guard, stdio })
    }

    /// Check the pinned adapter answered `initialize` (plan 7.3). A mismatch
    /// refuses the launch.
    fn check_initialize(backend: AgentBackend, response: &serde_json::Value) -> Result<(), LaunchFailure> {
        // Plan C4: a protocol mismatch is its own code, before the pin check.
        let protocol = response.get("protocolVersion").and_then(serde_json::Value::as_u64);
        if protocol != Some(1) {
            return Err(LaunchFailure::new(crate::external::failure_codes::PROTOCOL_MISMATCH, format!("外部 Agent 使用 ACP 协议版本 {protocol:?}，本版 Fox 只支持版本 1"))
                .remedy("把 Fox 外部运行时升级到与本版 Fox 配套的版本")
                .detail(serde_json::json!({ "protocolVersion": response.get("protocolVersion"), "agentInfo": response.get("agentInfo") })));
        }
        let result = match backend {
            AgentBackend::Pi => crate::backends::pi::check_initialize(response),
            AgentBackend::Codex => crate::backends::codex::check_initialize(response).map_err(|error| error.to_string()),
            AgentBackend::Deepseek => crate::backends::deepseek::check_initialize(response).map_err(|error| error.to_string()),
            AgentBackend::Fox => Ok(()),
        };
        let pinned = match backend {
            AgentBackend::Pi => format!("pi-acp {}", crate::launch::PI_ACP_VERSION),
            AgentBackend::Codex => format!("codex-acp {}", crate::launch::CODEX_ACP_VERSION),
            AgentBackend::Deepseek => format!("dsh {}", crate::launch::DSH_VERSION),
            AgentBackend::Fox => String::new(),
        };
        result.map_err(|error| {
            LaunchFailure::new(crate::external::failure_codes::VERSION_MISMATCH, error)
                .remedy(format!("升级或重新安装 Fox 外部运行时（本版 Fox 锁定 {pinned}）"))
                .detail(serde_json::json!({ "pinned": pinned, "agentInfo": response.get("agentInfo") }))
        })
    }

    async fn launch_now(config: HostLaunchConfig, bridge: Option<crate::fox_mcp::McpBridge>, request: LaunchRequest) -> Result<Launched, LaunchFailure> {
        // Fox adds no sandbox around an external Agent: its own native
        // sandbox, approval and permission settings remain authoritative.
        // The private HOME is Fox-managed. `mode` only records whether the
        // native mode is the Agent's full access.
        let mode = config.permission_mode(&request);
        // The native mode is the user's stored per-Agent choice. A pi that is
        // not installed is refused here first (`agent_backend_not_installed`).
        let (mut spec, pi_launch) = config.launch_spec_with(request.backend, request.native_mode(), &request.project_root)?;
        if request.backend == AgentBackend::Pi {
            config.pi_preflight(pi_launch, || config.pi_turn_adapter_verified())?;
        }
        // Unified login first: the Codex key goes into the backend's
        // environment only (never config.toml).
        if let Some(report) = config.apply_unified_login(&request)? {
            for (name, value) in &report.env {
                spec.session_env.retain(|(key, _)| key != name);
                spec.session_env.push((name.clone(), value.expose().to_owned()));
            }
        }
        // No login at all: refuse before anything starts, so the first run of
        // a new project shows the login card instead of the backend's own
        // error (codex-acp exits before initialize).
        if let Some(refusal) = config.missing_login_refusal(&request) {
            return Err(refusal);
        }
        let mcp = match bridge.as_ref() {
            Some(bridge) => Some(Self::prepare_mcp(&config, bridge, &request, &mut spec)?),
            None => None,
        };
        let os = config.os;
        // The external process is a normal child of the Fox Host (Job Object /
        // process group) and keeps whatever native protection its Agent
        // provides.
        let private = config.private_dirs(&request)?;
        let prepared = prepare_launch(spec, &request.session_key, &private, os)?;
        let declaration = prepared.declaration.clone();
        let cwd = request.project_root.clone();
        let connection = AcpConnection::spawn_prepared_detailed(prepared, &cwd).await?;
        let mut connection = connection;
        let (on_closed, stdio, fox_tools) = match mcp {
            Some(McpLaunch { guard, stdio }) => {
                // pi: the runtime checks the extension listed the tools once
                // the session opened (fox-fix-pi).
                let probe = (request.backend == AgentBackend::Pi).then(|| guard.bridge.probe(&guard.token));
                (Some(guard.into_on_closed()), stdio, probe)
            }
            None => (None, None, None),
        };
        if let Some(server) = stdio {
            connection.set_mcp_servers(vec![server]);
        }
        // Peek at initialize (it is the first queued event) for the version
        // check without consuming it for the runtime.
        let mut events = connection.take_events();
        let first = events.recv().await;
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        if let Some(crate::acp::BackendEvent::Initialized { initialize_response, .. }) = &first {
            if let Err(failure) = Self::check_initialize(request.backend, initialize_response) {
                let _ = connection.close_with_report().await;
                if let Some(callback) = on_closed {
                    callback();
                }
                return Err(failure);
            }
        }
        if let Some(event) = first {
            let _ = tx.send(event);
        }
        // Forward the rest unchanged.
        tokio::spawn(async move {
            while let Some(event) = events.recv().await {
                if tx.send(event).is_err() {
                    break;
                }
            }
        });
        connection.events = rx;
        Ok(Launched { connection, mode, declaration, pi_launch, on_closed, fox_tools })
    }
}

use crate::acp::AcpConnection;

impl ExternalLauncher for HostLauncher {
    fn model_host(&self, request: &LaunchRequest) -> Option<String> {
        let home = self.config.private_home_path(request.project_uuid.as_deref().unwrap_or_default(), request.backend).ok()?;
        crate::model_endpoint::resolve(request.backend, &home, &request.project_root).ok()?.endpoint.map(|endpoint| endpoint.entry)
            .or_else(|| self.config.unified_login_host(request, &home))
    }
    fn selected_model(&self, request: &LaunchRequest) -> Option<String> {
        if request.backend != AgentBackend::Pi { return None; }
        let home = self.config.private_home_path(request.project_uuid.as_deref().unwrap_or_default(), request.backend).ok()?;
        let native = crate::model_endpoint::resolve(request.backend, &home, &request.project_root).ok()?;
        Some(format!("{}/{}", native.provider?, native.model?))
    }
    fn connection_key(&self, request: &LaunchRequest) -> Result<Option<String>, LaunchFailure> {
        use sha2::{Digest, Sha256};
        let home = self.config.private_home_path(request.project_uuid.as_deref().unwrap_or_default(), request.backend)?;
        let mut hash = Sha256::new();
        hash.update(crate::model_endpoint::source_fingerprint(request.backend, &home).map_err(|error| LaunchFailure::new("agent_backend_model_endpoint_invalid", error))?);
        if let Some(login) = &request.unified_login {
            for part in [&login.base_url, &login.api_type, &login.model] { hash.update((part.len() as u64).to_le_bytes()); hash.update(part.as_bytes()); }
            hash.update(login.api_key.expose().as_bytes());
        }
        hash.update(serde_json::to_vec(&self.config.extra_domains).expect("policy serializes"));
        hash.update(serde_json::to_vec(&self.config.pi_providers).expect("providers serialize"));
        Ok(Some(format!("{:x}", hash.finalize())))
    }
    fn desired_mode(&self, request: &LaunchRequest) -> PermissionMode {
        self.config.permission_mode(request)
    }

    fn launch(&self, request: LaunchRequest) -> BoxFuture<'static, Result<Launched, LaunchFailure>> {
        let config = self.config.clone();
        Box::pin(Self::launch_now(config, self.mcp.clone(), request))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(root: &Path) -> HostLaunchConfig {
        HostLaunchConfig {
            os: HostOs::Linux,
            fox_data: root.join("data"),
            state_root: root.join("data").join("agent-state"),
            runtime_root: root.join("runtime"),
            native_login_home: None,
            full_access: FullAccessSwitch::Off,
            full_access_forbidden: false,
            pi_providers: vec![],
            extra_domains: BTreeMap::new(),
            env_passthrough: vec![],
        }
    }

    fn request(backend: AgentBackend, root: &Path) -> LaunchRequest {
        LaunchRequest {
            conversation_id: "c1".into(),
            backend,
            project_uuid: Some("8f14e45f-ceea-467a-9a36-dedd4bea2543".into()),
            project_root: root.join("proj"),
            session_key: "s1".into(),
            full_access_granted: false,
            unified_login: None,
            native_permission: None,
        }
    }

    #[test]
    fn full_access_switch_parsing_is_compatible_but_does_not_add_a_wrapper() {
        let root = std::env::temp_dir().join("fox-ext-launch-mode");
        let mut config = config(&root);
        let mut req = request(AgentBackend::Pi, &root);
        // The engineering switch no longer decides anything: only the native mode.
        assert_eq!(config.permission_mode(&req), PermissionMode::Standard);
        config.full_access = FullAccessSwitch::parse(Some("1"));
        assert_eq!(config.permission_mode(&req), PermissionMode::Standard);
        req.native_permission = Some("full_access".into());
        assert_eq!(config.permission_mode(&req), PermissionMode::FullAccess);
        config.full_access_forbidden = true;
        assert_eq!(config.permission_mode(&req), PermissionMode::FullAccess, "FORBIDDEN is enforced by the catalog / the Host commands, not here");
        assert_eq!(FullAccessSwitch::parse(Some(" off ")), FullAccessSwitch::Off);
        assert_eq!(FullAccessSwitch::parse(None), FullAccessSwitch::Off);
    }

    /// UX batch ①: the Host glue probes the private HOME a launch would use
    /// (mode included) and imports into every listed project's private HOME.
    #[cfg(unix)]
    #[test]
    fn credential_probe_and_import_use_the_launch_private_homes() {
        let root = std::env::temp_dir().join(format!("fox-ext-launch-onboarding-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let config = config(&root);
        let mut req = request(AgentBackend::Deepseek, &root);
        let probe = config.credential_probe(&req).unwrap();
        assert_eq!(probe.state, crate::onboarding::CredentialState::MissingCredentials);
        assert_eq!(probe.private_home, root.join("data/agent-homes/8f14e45f-ceea-467a-9a36-dedd4bea2543/deepseek"));
        let real = root.join("real");
        std::fs::create_dir_all(real.join(".dsh/profiles/acp")).unwrap();
        std::fs::write(real.join(".dsh/.env"), "DEEPSEEK_API_KEY=sk-glue-secret-1
").unwrap();
        std::fs::write(real.join(".dsh/profiles/acp/cordis.patch.yml"), "- id: llm-deepseek\n  config:\n    baseURL: http://127.0.0.1:3425/v1\n").unwrap();
        let real = crate::onboarding::RealHome { home: real, codex_home: None };
        let projects = vec![
            ("8f14e45f-ceea-467a-9a36-dedd4bea2543".to_owned(), "主项目".to_owned()),
            ("c9f0f895-fb98-4b91-a5c3-2b5f3e3a7a11".to_owned(), "c9f0f895-fb98-4b91-a5c3-2b5f3e3a7a11".to_owned()),
        ];
        let selected = vec!["env:.dsh/.env#DEEPSEEK_API_KEY".to_owned(), "file:.dsh/profiles/acp/cordis.patch.yml".to_owned()];
        let report = config.import_credentials(AgentBackend::Deepseek, &projects, &real, &selected, false).unwrap();
        assert_eq!(report.homes, 2);
        assert_eq!(report.homes_written, 2);
        // Again: both projects already have both items; kept, nothing written.
        let again = config.import_credentials(AgentBackend::Deepseek, &projects, &real, &selected, false).unwrap();
        assert_eq!(again.homes_written, 0);
        assert_eq!(again.skipped_existing.iter().map(|skip| skip.label.as_str()).collect::<Vec<_>>(), ["主项目", projects[1].0.as_str()]);
        // Overwrite: backed up under each private HOME.
        let over = config.import_credentials(AgentBackend::Deepseek, &projects[..1], &real, &selected[1..], true).unwrap();
        assert_eq!(over.backups.len(), 1);
        assert!(over.backups[0].directory.starts_with(root.join("data/agent-homes/8f14e45f-ceea-467a-9a36-dedd4bea2543/deepseek").join(crate::onboarding::BACKUP_DIR)));
        assert!(!report.audit_event().to_string().contains("sk-glue-secret-1"));
        // A loopback base URL is not refused: Fox adds no layer around dsh.
        let probe = config.credential_probe(&req).unwrap();
        assert_eq!(probe.state, crate::onboarding::CredentialState::Ready);
        req.full_access_granted = true;
        assert_eq!(config.credential_probe(&req).unwrap().state, crate::onboarding::CredentialState::Ready);
        let mut second = request(AgentBackend::Deepseek, &root);
        second.project_uuid = Some(projects[1].0.clone());
        second.full_access_granted = true;
        assert_eq!(config.credential_probe(&second).unwrap().state, crate::onboarding::CredentialState::Ready, "applied to every project");
        assert!(config.import_credentials(AgentBackend::Deepseek, &[], &real, &selected, false).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn pi_source_choice_blocks_generation_and_skipped_import_cannot_grant_a_new_endpoint() {
        let root = tempfile::tempdir().unwrap();
        let mut config = config(root.path());
        let real = crate::onboarding::RealHome { home: root.path().join("real"), codex_home: None };
        let write_pi = |home: &Path, endpoint: &str| {
            std::fs::create_dir_all(home.join(".pi/agent")).unwrap();
            std::fs::write(home.join(".pi/agent/settings.json"), r#"{"defaultProvider":"magpie","defaultModel":"m"}"#).unwrap();
            std::fs::write(home.join(".pi/agent/models.json"), serde_json::json!({"providers":{"magpie":{"baseUrl":endpoint,"apiKey":"test-secret","models":[{"id":"m"}]}}}).to_string()).unwrap();
        };
        write_pi(&real.home, "http://127.0.0.1:3425/v1");
        config.native_login_home = Some(real.clone());
        let req = request(AgentBackend::Pi, root.path());
        let probe = config.credential_probe(&req).unwrap();
        let source = probe.source.unwrap();
        assert!(source.choice_required);
        assert_eq!(source.model_endpoint.as_deref(), Some("127.0.0.1:3425"));
        assert_eq!(config.apply_unified_login(&req).unwrap_err().code, "agent_backend_source_required");
        let home = config.private_home_path(req.project_uuid.as_deref().unwrap(), AgentBackend::Pi).unwrap();
        assert!(!home.join(".pi/agent/models.json").exists());
        write_pi(&home, "https://existing.example/v1");
        let selected = vec!["file:.pi/agent/models.json".into(), "file:.pi/agent/settings.json".into()];
        let projects = [(req.project_uuid.unwrap(), "当前项目".into())];
        let kept = config.import_credentials(AgentBackend::Pi, &projects, &real, &selected, false).unwrap();
        assert_eq!(kept.homes_written, 0);
        assert!(!config.state_root.join("model-endpoint-grants.json").exists());
        let imported = config.import_credentials(AgentBackend::Pi, &projects, &real, &selected, true).unwrap();
        assert_eq!(imported.backups.len(), 1);
        let native = crate::model_endpoint::resolve(AgentBackend::Pi, &home, root.path()).unwrap();
        assert!(crate::model_endpoint::import_approved(&config.state_root, AgentBackend::Pi, &home, &native).unwrap());
    }

    /// Plan C2: the recorded mode is FullAccess exactly when the native mode
    /// is the Agent's own full access. The old full-access flag remains
    /// persisted data; it never decides the mode and never rewrites a native
    /// choice.
    #[test]
    fn external_permission_mode_follows_the_agents_native_full_access() {
        let root = std::env::temp_dir().join("fox-ext-launch-grant");
        let mut config = config(&root);
        let mut req = request(AgentBackend::Deepseek, &root);
        assert_eq!(config.permission_mode(&req), PermissionMode::Standard, "dsh default: workspace-write-never");
        req.full_access_granted = true;
        config.full_access = FullAccessSwitch::All;
        assert_eq!(config.permission_mode(&req), PermissionMode::Standard, "the legacy grant does not widen");
        req.native_permission = Some("workspace-write-never".into());
        assert_eq!(req.native_mode(), "workspace-write-never");
        req.native_permission = Some("danger-full-access".into());
        assert_eq!(config.permission_mode(&req), PermissionMode::FullAccess);
        req.backend = AgentBackend::Codex;
        req.native_permission = Some("agent-full-access".into());
        assert_eq!(req.native_mode(), "agent-full-access");
        assert_eq!(config.permission_mode(&req), PermissionMode::FullAccess);
        req.native_permission = Some("read-only".into());
        assert_eq!(config.permission_mode(&req), PermissionMode::Standard);
        req.backend = AgentBackend::Pi;
        req.native_permission = Some("allow".into());
        assert_eq!(config.permission_mode(&req), PermissionMode::Standard);
    }

    /// C1: an external pi is refused for what it really lacks, in order: not
    /// installed, then Fox's approval gate, then the pinned turn-outcome
    /// adaptation. (Before C1 the adaptation was checked before anything,
    /// so a pi that was simply not installed read as "adapter missing".)
    #[test]
    fn pi_is_refused_for_what_it_really_lacks_in_order() {
        let root = std::env::temp_dir().join(format!("fox-ext-launch-pi-preflight-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let config = config(&root);
        if crate::launch::program_override(AgentBackend::Pi).is_some() {
            eprintln!("SKIPPED: FOX_PI_ACP_PROGRAM is set");
            return;
        }
        let failure = config.launch_spec_with(AgentBackend::Pi, "ask", &root.join("proj")).unwrap_err();
        assert_eq!(failure.code, "agent_backend_not_installed", "not installed is said first");
        let bin = root.join("runtime/backends/pi/node_modules/.bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("pi-acp"), "#!/bin/sh\n").unwrap();
        let (_, launch) = config.launch_spec_with(AgentBackend::Pi, "ask", &root.join("proj")).unwrap();
        assert_eq!(launch, PiLaunch::Stock);
        let failure = config.pi_preflight(launch, || true).unwrap_err();
        assert_eq!(failure.code, "agent_backend_pi_gate_missing", "no gate: no tier could be enforced");
        std::fs::create_dir_all(root.join("runtime/pi-gate")).unwrap();
        std::fs::write(root.join("runtime/pi-gate/pi-gated.sh"), "#!/bin/sh\n").unwrap();
        let (spec, launch) = config.launch_spec_with(AgentBackend::Pi, "ask", &root.join("proj")).unwrap();
        assert_eq!(launch, PiLaunch::Gated);
        assert!(spec.session_env.iter().any(|(key, value)| key == "PI_ACP_PI_COMMAND" && value.ends_with("pi-gated.sh")));
        assert_eq!(config.pi_preflight(launch, || false).unwrap_err().code, "agent_backend_runtime_adapter_missing");
        assert!(!config.pi_turn_adapter_verified(), "the fake runtime does not carry the pinned adaptation");
        assert_eq!(config.pi_preflight(launch, || true), Ok(()));
        let _ = std::fs::remove_dir_all(&root);
    }

    struct NoTools;
    impl crate::fox_mcp::FoxToolHost for NoTools {
        fn call_tool(&self, _context: crate::fox_mcp::ToolCallContext, _tool: String, _arguments: serde_json::Value) -> BoxFuture<'static, Result<serde_json::Value, crate::fox_mcp::ToolError>> {
            Box::pin(async { Err(crate::fox_mcp::ToolError::new("test", "unused")) })
        }
    }

    /// Plan C3: what each Agent gets for the Fox tools, from the real launcher.
    #[tokio::test(flavor = "multi_thread")]
    async fn launches_inject_the_fox_tool_bridge_per_agent_and_revoke_on_failure() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let config = config(root);
        let node = root.join("runtime/node/bin/node");
        std::fs::create_dir_all(node.parent().unwrap()).unwrap();
        std::fs::write(&node, "").unwrap();
        let bridge = crate::fox_mcp::McpBridge::start(std::sync::Arc::new(NoTools), &tokio::runtime::Handle::current()).unwrap();
        // Codex / dsh: one ACP stdio server (the relay, run by the runtime's Node).
        for backend in [AgentBackend::Codex, AgentBackend::Deepseek] {
            let mut spec = crate::launch::launch_spec(backend).unwrap();
            let before = spec.session_env.clone();
            let launch = HostLauncher::prepare_mcp(&config, &bridge, &request(backend, root), &mut spec).unwrap();
            let server = launch.stdio.clone().expect("stdio server");
            assert_eq!(server.name, crate::fox_mcp::SERVER_NAME);
            assert_eq!(server.command, node);
            assert!(Path::new(&server.args[0]).is_file(), "relay written to Fox state");
            assert!(server.args[0].starts_with(config.state_root.to_string_lossy().as_ref()));
            assert_eq!(spec.session_env, before, "nothing leaks into the Agent's own environment");
            assert_eq!(bridge.live_tokens(), 1);
            drop(launch);
            assert_eq!(bridge.live_tokens(), 0, "a launch that did not start revokes its token");
        }
        // pi: refused while the installed gate cannot load the extension.
        let gate = root.join("runtime/pi-gate/pi-gated.sh");
        std::fs::create_dir_all(gate.parent().unwrap()).unwrap();
        std::fs::write(&gate, "#!/bin/sh\nexec pi -e gate.ts \"$@\"\n").unwrap();
        let mut spec = crate::launch::launch_spec(AgentBackend::Pi).unwrap();
        let failure = HostLauncher::prepare_mcp(&config, &bridge, &request(AgentBackend::Pi, root), &mut spec).err().expect("old gate refused");
        assert_eq!(failure.code, "agent_backend_fox_tools_unsupported");
        assert_eq!(bridge.live_tokens(), 0);
        // pi with the C3 gate: environment for the gated pi, no ACP server.
        std::fs::write(&gate, include_str!("../fixtures/pi/pi-gated.sh")).unwrap();
        let launch = HostLauncher::prepare_mcp(&config, &bridge, &request(AgentBackend::Pi, root), &mut spec).unwrap();
        assert!(launch.stdio.is_none());
        let env: std::collections::HashMap<_, _> = spec.session_env.iter().cloned().collect();
        assert!(Path::new(&env[crate::fox_mcp::PI_EXTENSION_ENV]).is_file(), "extension written to Fox state");
        assert_eq!(env[crate::fox_mcp::ENDPOINT_ENV], bridge.endpoint());
        assert_eq!(env[crate::fox_mcp::TOKEN_ENV].len(), 64);
        let on_closed = launch.guard.into_on_closed();
        assert_eq!(bridge.live_tokens(), 1, "a started launch keeps its token");
        on_closed();
        assert_eq!(bridge.live_tokens(), 0, "revoked when the process stops");
        // No Node: refused, never launched without the tools.
        std::fs::remove_file(&node).unwrap();
        let mut config_without_node = config.clone();
        config_without_node.os = HostOs::Windows;
        let mut spec = crate::launch::launch_spec(AgentBackend::Codex).unwrap();
        let failure = HostLauncher::prepare_mcp(&config_without_node, &bridge, &request(AgentBackend::Codex, root), &mut spec).err().expect("no node");
        assert_eq!(failure.code, "agent_backend_fox_tools_unavailable");
    }

    #[test]
    fn specs_use_the_managed_install_only_never_npx() {
        let root = std::env::temp_dir().join(format!("fox-ext-launch-spec-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mut config = config(&root);
        // Nothing installed: refused, never an npx fallback.
        let error = config.launch_spec(AgentBackend::Pi, &root).unwrap_err();
        assert_eq!(error.code, "agent_backend_not_installed");
        // Paths are built and compared per component (`Path` equality), so the
        // test means the same on Windows, where `join("a/b")` would mix
        // separators and a string comparison would fail.
        let under = |base: &Path, parts: &[&str]| parts.iter().fold(base.to_path_buf(), |path, part| path.join(part));
        // Managed install: absolute program, no npx arguments; the pi gate when shipped.
        let bin = under(&root, &["runtime", "backends", "pi", "node_modules", ".bin"]);
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("pi-acp"), "").unwrap();
        let (spec, launch) = config.launch_spec(AgentBackend::Pi, &root).unwrap();
        assert_eq!(Path::new(&spec.program), bin.join("pi-acp"));
        assert!(spec.args.is_empty());
        assert_eq!(launch, PiLaunch::Stock);
        let gate_dir = under(&root, &["runtime", "pi-gate"]);
        std::fs::create_dir_all(&gate_dir).unwrap();
        std::fs::write(gate_dir.join("pi-gated.sh"), "").unwrap();
        let (spec, launch) = config.launch_spec(AgentBackend::Pi, &root).unwrap();
        assert_eq!(launch, PiLaunch::Gated);
        assert!(spec.session_env.iter().any(|(key, value)| key == "FOX_PI_BIN" && Path::new(value) == bin.join("pi")));
        assert!(spec.session_env.iter().any(|(key, value)| key == "PI_ACP_PI_COMMAND" && Path::new(value) == gate_dir.join("pi-gated.sh")));
        // Codex: the Agent's own mode is passed through untouched, and the
        // tool-chain passthrough is an explicit Host allowlist.
        let codex = under(&root, &["runtime", "backends", "codex", "node_modules", ".bin"]);
        std::fs::create_dir_all(&codex).unwrap();
        std::fs::write(codex.join("codex-acp"), "").unwrap();
        config.env_passthrough = vec!["PATH".into()];
        let (spec, _) = config.launch_spec(AgentBackend::Codex, &root).unwrap();
        assert!(spec.session_env.iter().any(|(key, value)| key == "INITIAL_AGENT_MODE" && value == "workspace-write"), "the default native mode, never Codex full access");
        assert!(spec.session_env.iter().any(|(key, _)| key == "PATH"), "explicit Host passthrough");
        let (spec, _) = config.launch_spec_with(AgentBackend::Codex, "agent", &root).unwrap();
        assert!(spec.session_env.iter().any(|(key, value)| key == "INITIAL_AGENT_MODE" && value == "agent"));
        let (spec, _) = config.launch_spec_with(AgentBackend::Codex, "agent-full-access", &root).unwrap();
        assert!(spec.session_env.iter().any(|(key, value)| key == "INITIAL_AGENT_MODE" && value == "agent-full-access"), "the Agent's native full access is not rewritten");
        assert!(spec.session_env.iter().any(|(key, _)| key == "PATH"), "passthrough is unchanged");
        // DeepSeek managed (runtime package 0.1.1 layout: `backends/dsh`): the
        // installed patch must match byte for byte.
        let dsh = under(&root, &["runtime", "backends", "dsh", "node_modules", ".bin"]);
        std::fs::create_dir_all(&dsh).unwrap();
        std::fs::write(dsh.join("dsh"), "").unwrap();
        let patch_policy = crate::backends::deepseek::HostPolicy { workspace_write: true, network: true };
        let patch = crate::backends::deepseek::default_patch_path(&config.runtime_root, &patch_policy);
        std::fs::create_dir_all(patch.parent().unwrap()).unwrap();
        std::fs::write(&patch, crate::backends::deepseek::policy_patch(&patch_policy).patch_yaml).unwrap();
        let (spec, _) = config.launch_spec(AgentBackend::Deepseek, &root).unwrap();
        assert!(spec.args.iter().any(|arg| Path::new(arg) == patch));
        assert_eq!(Path::new(&spec.program), dsh.join("dsh"));
        // The ask preset is generated into Fox state when the runtime
        // package does not carry that variant.
        let (spec, _) = config.launch_spec_with(AgentBackend::Deepseek, "workspace-write", &root).unwrap();
        let ask = spec.args.iter().find(|arg| arg.ends_with("workspace-write-ask.yml")).expect("ask patch argument");
        assert!(std::fs::read_to_string(ask).unwrap().contains("policy: ask"));
        // Native dsh danger-full-access is selected only when the user stored
        // it.
        let (spec, _) = config.launch_spec_with(AgentBackend::Deepseek, "danger-full-access", &root).unwrap();
        assert!(spec.args.iter().any(|arg| arg.ends_with("danger-full-access.yml")), "{:?}", spec.args);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A DeepSeek session with a local gateway is started as a normal child
    /// (and here fails on the empty dsh entry, never on a loopback refusal).
    #[tokio::test(flavor = "current_thread")]
    async fn host_launcher_does_not_refuse_a_native_deepseek_loopback_gateway() {
        let root = std::env::temp_dir().join(format!("fox-ext-launch-dsh-refuse-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let config = config(&root);
        let dsh = root.join("runtime").join("backends").join("dsh").join("node_modules").join(".bin");
        std::fs::create_dir_all(&dsh).unwrap();
        std::fs::write(dsh.join("dsh"), "").unwrap();
        let patch_policy = crate::backends::deepseek::HostPolicy { workspace_write: true, network: true };
        let patch = crate::backends::deepseek::default_patch_path(&config.runtime_root, &patch_policy);
        std::fs::create_dir_all(patch.parent().unwrap()).unwrap();
        std::fs::write(&patch, crate::backends::deepseek::policy_patch(&patch_policy).patch_yaml).unwrap();
        let request = request(AgentBackend::Deepseek, &root);
        std::fs::create_dir_all(&request.project_root).unwrap();
        let home = PrivateHomes::new(config.fox_data.join("agent-homes")).home_for(request.project_uuid.as_deref().unwrap(), AgentBackend::Deepseek).unwrap();
        std::fs::create_dir_all(home.path.join(".dsh")).unwrap();
        let launcher = HostLauncher::new(config.clone());
        std::fs::write(home.path.join(".dsh").join("cordis.patch.yml"), "- id: llm-deepseek\n  config:\n    baseURL: http://127.0.0.1:3425/anthropic\n").unwrap();
        let error = launcher.launch(request.clone()).await.err().expect("empty dsh exits");
        assert_ne!(error.code, "agent_backend_loopback_requires_full_access", "{error:?}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 80b1622 acceptance: a new project's first Codex run must not reach
    /// codex-acp without a login (it exited before initialize with a raw
    /// CODEX_HOME path error). The probe runs first and refuses with
    /// `agent_backend_missing_credentials`; with a login the launch gets past
    /// the refusal (and here fails on the empty codex-acp entry).
    #[tokio::test(flavor = "current_thread")]
    async fn host_launcher_refuses_a_codex_launch_without_login_before_starting() {
        if std::env::var_os("OPENAI_API_KEY").is_some_and(|value| !value.is_empty()) {
            eprintln!("skipped: OPENAI_API_KEY is set in this environment (it counts as a login)");
            return;
        }
        let root = std::env::temp_dir().join(format!("fox-ext-launch-codex-login-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let config = config(&root);
        let codex = root.join("runtime").join("backends").join("codex").join("node_modules").join(".bin");
        std::fs::create_dir_all(&codex).unwrap();
        std::fs::write(codex.join("codex-acp"), "").unwrap();
        let request = request(AgentBackend::Codex, &root);
        std::fs::create_dir_all(&request.project_root).unwrap();
        let launcher = HostLauncher::new(config.clone());
        let error = launcher.launch(request.clone()).await.err().expect("refused");
        assert_eq!(error.code, MISSING_CREDENTIALS, "{error:?}");
        assert_eq!(error.remedy.as_deref(), Some(MISSING_CREDENTIALS_REMEDY));
        let detail = serde_json::to_string(&error.detail).unwrap();
        assert!(detail.contains(".codex/auth.json") && detail.contains("missing_credentials"), "{detail}");
        // A login in the private HOME: past the refusal.
        let home = PrivateHomes::new(config.fox_data.join("agent-homes")).home_for(request.project_uuid.as_deref().unwrap(), AgentBackend::Codex).unwrap().path;
        std::fs::create_dir_all(home.join(".codex")).unwrap();
        std::fs::write(home.join(".codex").join("auth.json"), r#"{"OPENAI_API_KEY":"sk-fox-test-not-real"}"#).unwrap();
        let error = launcher.launch(request.clone()).await.err().expect("empty codex-acp exits");
        assert_ne!(error.code, MISSING_CREDENTIALS, "{error:?}");
        assert!(!format!("{error:?}").contains("sk-fox-test-not-real"), "never a value");
        // pi is never blocked by the probe (keys may come from elsewhere).
        let probe = crate::onboarding::probe(AgentBackend::Pi, &root.join("nothing"), &|_| false);
        assert!(!crate::onboarding::launch_blocked_by_missing_login(&probe));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Full access through the production launcher: the backend program runs
    /// itself (nothing in front of it) and sees HOME, XDG_* and
    /// TMPDIR in the private HOME of this project x backend, which is created
    /// on the way. A mock `pi-acp` script records what it got under its HOME
    /// and exits (the launch then fails initialize; only the env matters).
    #[cfg(unix)]
    #[tokio::test(flavor = "current_thread")]
    async fn host_launcher_full_access_runs_unwrapped_with_the_private_home() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!("fox-ext-launch-full-home-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mut config = config(&root);
        config.full_access = FullAccessSwitch::All;
        // Codex: no Fox runtime adapter is required, only a login in the
        // private HOME.
        let bin = root.join("runtime").join("backends").join("codex").join("node_modules").join(".bin");
        std::fs::create_dir_all(&bin).unwrap();
        let script = bin.join("codex-acp");
        std::fs::write(&script, "#!/bin/sh\nprintf 'argv0=%s\\nHOME=%s\\nTMPDIR=%s\\nXDG_CONFIG_HOME=%s\\nCODEX_HOME=%s\\nPROXY=%s%s\\n' \"$0\" \"$HOME\" \"$TMPDIR\" \"$XDG_CONFIG_HOME\" \"$CODEX_HOME\" \"$NODE_USE_ENV_PROXY\" \"$CLAUDE_CODE_TMPDIR\" > \"$HOME/fox-env.txt\"\nexit 0\n").unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut request = request(AgentBackend::Codex, &root);
        request.native_permission = Some("agent-full-access".into());
        std::fs::create_dir_all(&request.project_root).unwrap();
        assert_eq!(config.permission_mode(&request), PermissionMode::FullAccess);
        let home = PrivateHomes::new(config.fox_data.join("agent-homes")).home_for(request.project_uuid.as_deref().unwrap(), AgentBackend::Codex).unwrap().path;
        std::fs::create_dir_all(home.join(".codex")).unwrap();
        std::fs::write(home.join(".codex").join("auth.json"), r#"{"OPENAI_API_KEY":"sk-fox-test-not-real"}"#).unwrap();
        let _ = HostLauncher::new(config.clone()).launch(request.clone()).await;
        let recorded = std::fs::read_to_string(home.join("fox-env.txt")).expect("the backend wrote under the private HOME");
        let tmp = config.state_root.join("agent-tmp").join("s1");
        assert!(recorded.contains(&format!("argv0={}\n", script.display())), "not wrapped: {recorded}");
        assert!(recorded.contains(&format!("HOME={}\n", home.display())), "{recorded}");
        assert!(recorded.contains(&format!("TMPDIR={}\n", tmp.display())), "{recorded}");
        assert!(recorded.contains(&format!("XDG_CONFIG_HOME={}\n", home.join(".config").display())), "{recorded}");
        assert!(recorded.contains(&format!("CODEX_HOME={}\n", home.join(".codex").display())), "{recorded}");
        assert!(recorded.contains("PROXY=\n"), "no proxy variables added: {recorded}");
        assert!(!tmp.exists(), "the session temp goes away with the failed launch");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Windows counterpart of the real-process check above: the codex-acp
    /// entry that Fox launches is the Agent adapter itself, and it receives
    /// the private HOME and no proxy variables.
    #[cfg(windows)]
    #[tokio::test(flavor = "current_thread")]
    async fn host_launcher_external_process_is_not_wrapped_on_windows() {
        let root = std::env::temp_dir().join(format!("fox-ext-launch-win-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let config = config(&root);
        let config = HostLaunchConfig { os: HostOs::Windows, ..config };
        let bin = root.join("runtime").join("backends").join("codex").join("node_modules").join(".bin");
        std::fs::create_dir_all(&bin).unwrap();
        let script = bin.join("codex-acp.cmd");
        std::fs::write(&script, r#"@echo off
> "%HOME%\fox-launch.txt" (
  echo argv0=%0
  echo HOME=%HOME%
  echo CODEX_HOME=%CODEX_HOME%
  echo NODE_USE_ENV_PROXY=%NODE_USE_ENV_PROXY%
)
exit /b 0
"#).unwrap();
        let request = request(AgentBackend::Codex, &root);
        std::fs::create_dir_all(&request.project_root).unwrap();
        let home = config.private_home_path(request.project_uuid.as_deref().unwrap(), AgentBackend::Codex).unwrap();
        std::fs::create_dir_all(home.join(".codex")).unwrap();
        std::fs::write(home.join(".codex").join("auth.json"), r#"{"OPENAI_API_KEY":"sk-fox-test-not-real"}"#).unwrap();
        let result = HostLauncher::new(config.clone()).launch(request).await;
        if let Err(error) = &result { eprintln!("launch error after process exit: {error:?}"); }
        let recorded = std::fs::read_to_string(home.join("fox-launch.txt")).unwrap_or_else(|error| {
            panic!("the backend wrote no recording under {}: {error}", home.display())
        });
        assert!(recorded.to_ascii_lowercase().contains("argv0=") && recorded.to_ascii_lowercase().contains("codex-acp.cmd"), "not the Agent adapter: {recorded}");
        assert!(recorded.contains(&format!("HOME={}\r\n", home.display())) || recorded.contains(&format!("HOME={}\n", home.display())), "{recorded}");
        assert!(recorded.contains("NODE_USE_ENV_PROXY=\r\n") || recorded.contains("NODE_USE_ENV_PROXY=\n"), "no proxy variables added: {recorded}");
        assert!(!recorded.contains("sk-fox-test-not-real"), "no credential value in the recording");
        let _ = std::fs::remove_dir_all(&home);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn native_pi_endpoint_needs_host_authorization_and_preserves_private_precedence() {
        use crate::unified_login::{Secret, UnifiedLogin};
        let root = std::env::temp_dir().join(format!("fox-endpoint-policy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let config = config(&root);
        let homes = config.fox_data.join("agent-homes");
        let mut request = request(AgentBackend::Pi, &root);
        request.unified_login = Some(UnifiedLogin { provider_name: "DeepSeek".into(), base_url: "https://api.deepseek.com".into(), api_type: "openai-completions".into(), model: "deepseek-v4-pro".into(), api_key: Secret::new("sk-host-test"), responses_supported: None });
        let home = PrivateHomes::new(homes.clone()).home_for(request.project_uuid.as_deref().unwrap(), request.backend).unwrap().path;
        std::fs::create_dir_all(home.join(".pi/agent")).unwrap();
        let models = home.join(".pi/agent/models.json");
        let original = r#"{"providers":{"magpie":{"baseUrl":"http://127.0.0.1:3425/v1","apiKey":"private-key","models":[{"id":"m"}]}}}"#;
        std::fs::write(&models, original).unwrap();
        std::fs::write(home.join(".pi/agent/settings.json"), r#"{"defaultProvider":"magpie","defaultModel":"m"}"#).unwrap();
        let launcher = HostLauncher::new(config.clone());
        assert_eq!(launcher.model_host(&request).as_deref(), Some("127.0.0.1:3425"));
        assert_eq!(launcher.selected_model(&request).as_deref(), Some("magpie/m"));
        assert_eq!(config.model_endpoint(&request, &home).unwrap_err().code, "agent_backend_model_endpoint_unapproved");
        assert_eq!(std::fs::read_to_string(&models).unwrap(), original, "the supplier default must not overwrite native Magpie");
        let snapshot = crate::model_endpoint::resolve(request.backend, &home, &request.project_root).unwrap();
        crate::model_endpoint::approve_import_snapshot(&config.state_root, request.backend, &home, &snapshot).unwrap();
        assert_eq!(config.model_endpoint(&request, &home).unwrap().map(|endpoint| endpoint.entry).as_deref(), Some("127.0.0.1:3425"));
        std::fs::write(&models, original.replace(":3425", ":3426")).unwrap();
        assert_eq!(config.model_endpoint(&request, &home).unwrap_err().code, "agent_backend_model_endpoint_unapproved");
        let _ = std::fs::remove_dir_all(root);
    }
}
