//! The real launcher for external backends (feature `acp`): Fox-managed
//! runtime layout, per-backend launch specs, the session's permission mode
//! (sandboxed by default, never degraded), `prepare_launch` + srt, and on
//! Windows the private layout, the ACP bridge and the ancestor ACEs. The
//! desktop builds one [`HostLauncher`] at startup and hands it to
//! [`crate::external::ExternalRuntime`].
//!
//! Managed runtime layout (the same on every OS; on Windows it is the package
//! `enable-sandbox.ps1` installs, docs/sandbox.md 7.5):
//!
//! ```text
//! <runtime>/node/node.exe | node/bin/node     Node (Linux / macOS: else `node` on PATH)
//! <runtime>/srt/node_modules/@anthropic-ai/sandbox-runtime/...
//! <runtime>/backends/<pi|codex|deepseek>/node_modules/.bin/<pi-acp|codex-acp|dsh>[.cmd]
//! <runtime>/dsh-patches/<variant>.yml          DeepSeek Host patch (verified byte for byte)
//! <runtime>/pi-gate/pi-gated.{sh,cmd}, fox-approval-gate.ts   optional pi approval gate
//! <runtime>/acp-bridge/bridge.mjs             Windows sandboxed ACP bridge
//! ```
//!
//! `<runtime>`: `FOX_AGENT_RUNTIME_ROOT` when set; Windows
//! `%ProgramFiles%\Fox\sandbox-runtime\<version>` (`FOX_RUNTIME_ROOT`, else
//! `FOX_RUNTIME_VERSION`, else the newest installed version); Linux / macOS
//! `<fox data>/agent-runtime`.
//!
//! Permission mode (plan v0.3.2 addendum): `sandboxed` unless the Host policy
//! both requests and grants full access. In this batch the only grant source
//! is the engineering switch `FOX_EXTERNAL_FULL_ACCESS` (`1` = every external
//! conversation, or a list of `conversation:<id>` / `project:<root>` keys),
//! always overridden by `FOX_EXTERNAL_FULL_ACCESS_FORBIDDEN=1`. Whatever the
//! mode, the backend must be installed (managed runtime, or an explicit
//! `FOX_*_PROGRAM` override for development); the Host never launches through
//! `npx` (`agent_backend_not_installed`).

use crate::capability_view::PiLaunch;
use crate::config::{HostPolicy, SessionSnapshot};
use crate::external::{ExternalLauncher, LaunchFailure, LaunchRequest, Launched};
use crate::launch::LaunchSpec;
use crate::port::AgentBackend;
use crate::sandbox::{
    effective_permission_mode, prepare_full_access, prepare_launch, FullAccessPrivate, preflight, resolve_allowed_domains, AuthorizedDir, HostOs, PermissionMode, PrivateHomes,
    RealProbe, SandboxError, SandboxErrorCode, SandboxPolicy, SandboxSetup, SrtInstall, SrtLauncher, SETTING_PERMISSION_MODE,
};
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
/// Enterprise switch: no external session may run in full access.
pub const FULL_ACCESS_FORBIDDEN_ENV: &str = "FOX_EXTERNAL_FULL_ACCESS_FORBIDDEN";
/// pi providers whose model API domains the sandbox allows (`anthropic,openai`).
pub const PI_PROVIDERS_ENV: &str = "FOX_PI_PROVIDERS";
/// Extra sandbox domains per backend (`pi=api.example.com;codex=...`), Host
/// layer. A local model gateway is written as `127.0.0.1:<port>`.
pub const EXTRA_DOMAINS_ENV: &str = "FOX_EXTERNAL_EXTRA_DOMAINS";
/// Host variables (e.g. `RUSTUP_HOME,CARGO_HOME`) passed to the backend in
/// full access only; ignored in sandboxed mode.
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
    /// Fox state directory (per-session srt settings, ledgers); never
    /// readable or writable inside the sandbox.
    pub state_root: PathBuf,
    pub runtime_root: PathBuf,
    pub full_access: FullAccessSwitch,
    pub full_access_forbidden: bool,
    pub pi_providers: Vec<String>,
    pub extra_domains: BTreeMap<AgentBackend, Vec<String>>,
    /// Host variables passed to the backend in full access only
    /// ([`ENV_PASSTHROUGH_ENV`]).
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
    /// Read the configuration from the process environment. `fox_data` is the
    /// app data directory.
    pub fn from_env(fox_data: &Path) -> Self {
        let os = HostOs::current();
        let runtime_root = env(AGENT_RUNTIME_ROOT_ENV).map(PathBuf::from).unwrap_or_else(|| match os {
            HostOs::Windows => {
                if env(crate::winruntime::RUNTIME_ROOT_ENV).is_some() || env(RUNTIME_VERSION_ENV).is_some() {
                    crate::winruntime::windows_runtime_root(&env(RUNTIME_VERSION_ENV).unwrap_or_default())
                } else {
                    let parent = crate::winruntime::default_windows_runtime_root("");
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
                policy.full_access_grants.insert(crate::sandbox::FullAccessGrant::Conversation(conversation_id.to_owned()).key());
            }
            FullAccessSwitch::Keys(keys) => policy.full_access_grants.extend(keys.iter().cloned()),
        }
        let _ = project_root;
        for (backend, domains) in &self.extra_domains {
            policy.sandbox_extra_domains.insert(*backend, domains.iter().cloned().collect());
        }
        policy
    }

    /// Effective permission mode of a conversation: sandboxed unless the Host
    /// both requests and grants full access (and policy does not forbid it).
    /// Grant sources (OR): the engineering switch [`FULL_ACCESS_ENV`] and the
    /// user's per-conversation choice at creation
    /// ([`LaunchRequest::full_access_granted`], persisted as
    /// `conversation:<id>`). [`FULL_ACCESS_FORBIDDEN_ENV`] beats both.
    pub fn permission_mode(&self, request: &LaunchRequest) -> PermissionMode {
        let mut policy = self.host_policy(&request.conversation_id, &request.project_root);
        if request.full_access_granted {
            policy.full_access_grants.insert(crate::sandbox::FullAccessGrant::Conversation(request.conversation_id.clone()).key());
        }
        let requested = if self.full_access == FullAccessSwitch::Off && !request.full_access_granted { "sandboxed" } else { "full_access" };
        let snapshot = SessionSnapshot {
            backend: request.backend,
            project_root: request.project_root.to_string_lossy().into_owned(),
            egress_allowed: false,
            settings: BTreeMap::from([(SETTING_PERMISSION_MODE.to_owned(), requested.to_owned())]),
        };
        effective_permission_mode(&policy, &snapshot, &request.conversation_id).mode
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
            // launch that fails later inside the sandbox.
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
        crate::sandbox::SystemProbe::which(&RealProbe, "node")
    }

    pub fn srt_install(&self) -> Option<SrtInstall> {
        Some(SrtInstall::under_prefix(self.managed_node()?, &self.runtime_root.join("srt")))
    }

    /// Which pi launch a session would get: gated when the managed runtime
    /// ships the Fox approval gate (`pi-gate/`), else stock (no Host approvals).
    pub fn pi_launch(&self) -> PiLaunch {
        if self.pi_gate_script().is_file() { PiLaunch::Gated } else { PiLaunch::Stock }
    }

    fn pi_gate_script(&self) -> PathBuf {
        self.runtime_root.join("pi-gate").join(if self.os == HostOs::Windows { "pi-gated.cmd" } else { "pi-gated.sh" })
    }

    /// The launch spec for `backend` in `mode`, always an installed program
    /// ([`Self::managed_program`]); a backend that is not installed is refused
    /// (`agent_backend_not_installed`) in every mode. Production never
    /// launches through `npx`.
    pub fn launch_spec(&self, backend: AgentBackend, mode: PermissionMode, project_root: &Path) -> Result<(LaunchSpec, PiLaunch), LaunchFailure> {
        if backend == AgentBackend::Fox {
            return Err(LaunchFailure::new("agent_backend_invalid", "Fox 不经外部运行时启动"));
        }
        let program = self.managed_program(backend).ok_or_else(|| {
            LaunchFailure::new(
                "agent_backend_not_installed",
                format!("外部后端「{}」未安装在 Fox 受管运行时 {}", backend.as_str(), self.runtime_root.display()),
            )
            .remedy("安装或修复 Fox 的外部后端运行时（启用沙箱时一并安装）；开发调试可用环境变量指定已安装的入口")
            .detail(serde_json::json!({ "runtimeRoot": self.runtime_root, "overrideEnv": crate::launch::program_override_env(backend) }))
        })?;
        let program_text = program.to_string_lossy().into_owned();
        let mut pi_launch = PiLaunch::Stock;
        let mut spec = match backend {
            AgentBackend::Deepseek => {
                let patch_policy = crate::backends::deepseek::HostPolicy { workspace_write: true, network: false };
                let managed_patch = crate::backends::deepseek::default_patch_path(&self.runtime_root, &patch_policy);
                let patch = if managed_patch.is_file() || mode == PermissionMode::Sandboxed {
                    managed_patch
                } else {
                    // Full access with a program outside the runtime package
                    // (override): the Host patch is written to Fox state.
                    let dir = self.state_root.join(crate::backends::deepseek::PATCH_DIR);
                    let patch = dir.join(crate::backends::deepseek::patch_file_name(&patch_policy));
                    std::fs::create_dir_all(&dir)
                        .and_then(|_| std::fs::write(&patch, crate::backends::deepseek::policy_patch(&patch_policy).patch_yaml))
                        .map_err(|error| LaunchFailure::new("agent_backend_launch_invalid", format!("cannot write the dsh patch: {error}")))?;
                    patch
                };
                crate::backends::deepseek::verify_installed_patch(&patch, &patch_policy)
                    .map_err(|error| LaunchFailure::new("agent_backend_not_installed", error.to_string()).remedy("重新安装 Fox 的外部后端运行时"))?;
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
                let policy = crate::backends::codex::HostPolicy { cwd: crate::launch::backend_visible_path(project_root), writable: true, network: false };
                spec.session_env = crate::backends::codex::session_env(&policy)
                    .map_err(|error| LaunchFailure::new("agent_backend_launch_invalid", error.to_string()))?;
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
        // Tool-chain variables (RUSTUP_HOME, CARGO_HOME ...) the Host chose to
        // pass: full access only. In sandboxed mode they would point outside
        // the readable roots and widen nothing safely; they are dropped.
        if mode == PermissionMode::FullAccess {
            for name in &self.env_passthrough {
                if let Some(value) = std::env::var_os(name) {
                    spec.session_env.retain(|(key, _)| key != name);
                    spec.session_env.push((name.clone(), value.to_string_lossy().into_owned()));
                }
            }
        }
        // A loopback model gateway (e.g. 127.0.0.1:3425) on the allowlist:
        // srt on Linux / macOS sets NO_PROXY=localhost,127.0.0.1, so the
        // backend would bypass the proxy and reach nothing inside the network
        // namespace. Clear it for the backend so the request goes through the
        // srt proxy, which enforces the allowlist (srt on Windows already
        // drops NO_PROXY).
        if mode == PermissionMode::Sandboxed && self.os != HostOs::Windows && self.allows_loopback(backend) {
            spec.session_env.push(("NO_PROXY".into(), String::new()));
            spec.session_env.push(("no_proxy".into(), String::new()));
        }
        Ok((spec, pi_launch))
    }

    /// Whether the Host allowlist for `backend` names a loopback endpoint.
    pub fn allows_loopback(&self, backend: AgentBackend) -> bool {
        self.extra_domains.get(&backend).is_some_and(|domains| {
            domains.iter().any(|domain| {
                let host = domain.rsplit_once(':').map_or(domain.as_str(), |(host, _)| host);
                host == "127.0.0.1" || host == "localhost"
            })
        })
    }

    /// The sandbox policy for one session (authorized = the project root,
    /// writable; private HOME per project x backend; runtime read-only;
    /// domains per backend, only narrowed below the Host layer).
    pub fn sandbox_policy(&self, request: &LaunchRequest, homes_root: PathBuf) -> Result<SandboxPolicy, SandboxError> {
        let project_uuid = request.project_uuid.clone().unwrap_or_default();
        let home = PrivateHomes::new(homes_root).home_for(&project_uuid, request.backend)?;
        let policy = self.host_policy(&request.conversation_id, &request.project_root);
        let providers: Vec<&str> = self.pi_providers.iter().map(String::as_str).collect();
        let domains = resolve_allowed_domains(request.backend, &providers, &policy, &[]);
        let mut extra = vec![self.runtime_root.clone()];
        if let Some(node) = self.managed_node().and_then(|node| node.parent().map(Path::to_path_buf)) {
            extra.push(node);
        }
        extra.dedup();
        // DeepSeek: allow exactly the loopback port of the model gateway its
        // own base URL names (dsh config in the private HOME), the
        // same IP-literal allowlist entry the round-6 scripted model uses.
        // Remote base URLs add nothing; an undeterminable port allows nothing
        // (diagnostic in the declaration).
        //
        // Since the f2bf567 re-test the port is NOT allowed: dsh dials
        // loopback directly, never through the srt proxy, so the entry would
        // open nothing that works; a loopback base URL is refused before
        // launch instead ([`Self::loopback_refusal`]). The derivation is kept
        // for the declaration's diagnostics and for a dsh with an upstream
        // "loopback through the proxy" switch
        // ([`crate::backends::deepseek::LOOPBACK_VIA_PROXY_SUPPORTED`]).
        let derivation = match request.backend {
            AgentBackend::Deepseek => {
                let derived = crate::loopback::derive_loopback_gateway(&crate::backends::deepseek::base_url_candidates(&home.path, &request.project_root));
                if crate::backends::deepseek::LOOPBACK_VIA_PROXY_SUPPORTED {
                    derived
                } else {
                    let mut diagnostics = derived.diagnostics;
                    if let Some(gateway) = derived.gateway {
                        diagnostics.insert(
                            0,
                            format!(
                                "{} 指向本机回环 {}；dsh 对回环地址直连、不经 srt 代理，放行端口无效，因此不放行（沙箱模式下此类会话在启动前即被拒绝）",
                                gateway.source, gateway.entry
                            ),
                        );
                    }
                    crate::loopback::LoopbackDerivation { gateway: None, diagnostics }
                }
            }
            _ => crate::loopback::LoopbackDerivation::default(),
        };
        Ok(SandboxPolicy::new(
            vec![AuthorizedDir { path: request.project_root.clone(), writable: true }],
            home,
            extra,
            domains.domains,
            self.os,
        )
        .with_loopback_derivation(derivation))
    }
}

/// Error code: a sandboxed DeepSeek session whose base URL is a loopback
/// gateway (e.g. Magpie on 127.0.0.1), refused before launch.
pub const LOOPBACK_REQUIRES_FULL_ACCESS: &str = "agent_backend_loopback_requires_full_access";

/// Remedy of [`LOOPBACK_REQUIRES_FULL_ACCESS`]: full access is chosen when a
/// conversation is created (it cannot be turned on mid-session).
pub const LOOPBACK_REFUSAL_REMEDY: &str = "需要完全访问权限：新建会话时选择“完全访问”（会话中不能开启），或改用官方 api.deepseek.com";

/// [`LOOPBACK_REFUSAL_REMEDY`] when [`FULL_ACCESS_FORBIDDEN_ENV`] is set.
pub const LOOPBACK_REFUSAL_REMEDY_FORBIDDEN: &str = "本机已禁止完全访问（FOX_EXTERNAL_FULL_ACCESS_FORBIDDEN=1），请改用官方 api.deepseek.com";

/// Why the full-access choice is unavailable, for the new-conversation UI
/// (`None` when it can be chosen).
pub const FULL_ACCESS_FORBIDDEN_REASON: &str = "本机已禁止完全访问（FOX_EXTERNAL_FULL_ACCESS_FORBIDDEN=1），外部后端只能在沙箱中运行";

impl HostLaunchConfig {
    /// Full access: the private HOME and session temp root a sandboxed launch
    /// of the same request would use. Linux / macOS:
    /// `<fox data>/sandbox-homes/<project uuid>/<backend>` and
    /// `<state>/sandbox-tmp`. Windows: the per-user private root
    /// (`%ProgramData%\Fox\sandbox\<SID>\{homes,tmp}`), created and
    /// verified exactly as for the sandbox (its base comes from "启用沙箱").
    pub fn full_access_private(&self, request: &LaunchRequest) -> Result<FullAccessPrivate, LaunchFailure> {
        let project_uuid = request.project_uuid.clone().unwrap_or_default();
        let (homes_root, tmp_root) = if self.os == HostOs::Windows {
            use crate::winacl::RealWindowsAcl;
            use crate::winruntime::{known_srt_sids, prepare_windows_private_root, WindowsPrivateLayout};
            let layout = WindowsPrivateLayout::for_current_user(&RealWindowsAcl)?;
            let known = known_srt_sids(&self.state_root)?;
            prepare_windows_private_root(&layout, &RealWindowsAcl, &RealWindowsAcl, &known)?;
            (layout.homes_root(), layout.tmp_root())
        } else {
            (self.fox_data.join("sandbox-homes"), self.state_root.join("sandbox-tmp"))
        };
        let home = PrivateHomes::new(homes_root).home_for(&project_uuid, request.backend)?;
        Ok(FullAccessPrivate { private_home: home.path, tmp_root })
    }

    /// The private HOME of `backend` in project `project_uuid` (the one both
    /// modes use). Reads only; nothing is created.
    pub fn private_home_path(&self, project_uuid: &str, backend: AgentBackend) -> Result<PathBuf, LaunchFailure> {
        let root = self
            .sandbox_homes_root()
            .ok_or_else(|| LaunchFailure::new("sandbox_runtime_not_installed", "Fox 私有 HOME 的根目录不可用（Windows 需先启用一次沙箱）"))?;
        Ok(PrivateHomes::new(root).home_for(project_uuid, backend)?.path)
    }

    /// Login probe for a conversation's backend (UX batch ①): which login
    /// files / key names its private HOME has, in the mode a launch would
    /// use now. Names only ([`crate::onboarding::probe`]).
    pub fn credential_probe(&self, request: &LaunchRequest) -> Result<crate::onboarding::CredentialProbe, LaunchFailure> {
        let project_uuid = request.project_uuid.clone().unwrap_or_default();
        let home = self.private_home_path(&project_uuid, request.backend)?;
        let mode = self.permission_mode(request);
        let host_env = |name: &str| std::env::var_os(name).is_some_and(|value| !value.is_empty());
        Ok(crate::onboarding::probe(request.backend, &home, mode, self.os, &host_env))
    }

    /// Copy the confirmed login items from the real HOME into the private
    /// HOME of `backend` in every project of `project_uuids` (UX batch ①).
    /// Windows: the private root is created / verified before and its DACL
    /// re-applied after the copy (the same [`prepare_windows_private_root`]
    /// the launch uses); Unix: files 0600, directories 0700.
    pub fn import_credentials(
        &self,
        backend: AgentBackend,
        project_uuids: &[String],
        real: &crate::onboarding::RealHome,
        selected: &[String],
    ) -> Result<crate::onboarding::ImportReport, LaunchFailure> {
        if backend == AgentBackend::Fox || project_uuids.is_empty() {
            return Err(LaunchFailure::new("agent_backend_import_invalid", "没有可导入的外部后端或项目"));
        }
        let prepare_windows = || -> Result<PathBuf, LaunchFailure> {
            use crate::winacl::RealWindowsAcl;
            use crate::winruntime::{known_srt_sids, prepare_windows_private_root, WindowsPrivateLayout};
            let layout = WindowsPrivateLayout::for_current_user(&RealWindowsAcl)?;
            let known = known_srt_sids(&self.state_root)?;
            prepare_windows_private_root(&layout, &RealWindowsAcl, &RealWindowsAcl, &known)?;
            Ok(layout.homes_root())
        };
        let homes_root = if self.os == HostOs::Windows { prepare_windows()? } else { self.fox_data.join("sandbox-homes") };
        let homes = PrivateHomes::new(homes_root);
        let paths = project_uuids
            .iter()
            .map(|uuid| homes.home_for(uuid, backend).map(|home| home.path).map_err(LaunchFailure::from))
            .collect::<Result<Vec<_>, _>>()?;
        let report = crate::onboarding::import_into(backend, real, selected, &paths).map_err(|error| LaunchFailure::new("agent_backend_import_failed", error))?;
        if self.os == HostOs::Windows {
            prepare_windows()?;
        }
        Ok(report)
    }

    /// Root of the private HOMEs a sandboxed launch on this OS uses (Windows:
    /// the per-user private root; elsewhere `<fox data>/sandbox-homes`).
    /// Reads only; nothing is created.
    fn sandbox_homes_root(&self) -> Option<PathBuf> {
        if self.os == HostOs::Windows {
            crate::winruntime::WindowsPrivateLayout::for_current_user(&crate::winacl::RealWindowsAcl).ok().map(|layout| layout.homes_root())
        } else {
            Some(self.fox_data.join("sandbox-homes"))
        }
    }

    /// Refuse a sandboxed DeepSeek launch whose base URL (the private HOME's
    /// dsh config, [`crate::backends::deepseek::base_url_candidates`]) names a
    /// loopback address. dsh's proxy layer always connects to loopback
    /// directly and srt 0.0.80 cannot allow a direct loopback port, so such a
    /// session would only fail later with "Connection error". Full access and
    /// remote base URLs are not affected; other backends are never refused.
    pub fn loopback_refusal(&self, request: &LaunchRequest, mode: PermissionMode, homes_root: PathBuf) -> Option<LaunchFailure> {
        if request.backend != AgentBackend::Deepseek || mode != PermissionMode::Sandboxed || crate::backends::deepseek::LOOPBACK_VIA_PROXY_SUPPORTED {
            return None;
        }
        let project_uuid = request.project_uuid.clone().unwrap_or_default();
        let home = PrivateHomes::new(homes_root).home_for(&project_uuid, request.backend).ok()?;
        let loopback: Vec<crate::loopback::BaseUrlCandidate> = crate::backends::deepseek::base_url_candidates(&home.path, &request.project_root)
            .into_iter()
            .filter(|candidate| crate::loopback::names_loopback(&candidate.value))
            .collect();
        if loopback.is_empty() {
            return None;
        }
        let named = loopback.iter().map(|candidate| format!("{}（来自 {}）", candidate.value, candidate.source)).collect::<Vec<_>>().join("；");
        Some(
            LaunchFailure::new(
                LOOPBACK_REQUIRES_FULL_ACCESS,
                format!("本机网关在沙箱模式下不可达：DeepSeek 的 base URL 指向本机回环地址 {named}，dsh 对回环地址总是直连、不经沙箱代理"),
            )
            .remedy(if self.full_access_forbidden { LOOPBACK_REFUSAL_REMEDY_FORBIDDEN } else { LOOPBACK_REFUSAL_REMEDY })
            .detail(serde_json::json!({
                "reason": "dsh_proxy_bypasses_loopback",
                "mode": mode.as_str(),
                "baseUrls": loopback.iter().map(|candidate| serde_json::json!({ "value": candidate.value, "source": candidate.source })).collect::<Vec<_>>(),
            })),
        )
    }
}

/// The production [`ExternalLauncher`].
#[derive(Clone)]
pub struct HostLauncher {
    pub config: HostLaunchConfig,
}

impl HostLauncher {
    pub fn new(config: HostLaunchConfig) -> Self {
        Self { config }
    }

    /// Check the pinned adapter answered `initialize` (plan 7.3). A mismatch
    /// refuses the launch.
    fn check_initialize(backend: AgentBackend, response: &serde_json::Value) -> Result<(), LaunchFailure> {
        let result = match backend {
            AgentBackend::Pi => crate::backends::pi::check_initialize(response),
            AgentBackend::Codex => crate::backends::codex::check_initialize(response).map_err(|error| error.to_string()),
            AgentBackend::Deepseek => crate::backends::deepseek::check_initialize(response).map_err(|error| error.to_string()),
            AgentBackend::Fox => Ok(()),
        };
        result.map_err(|error| LaunchFailure::new("agent_backend_version_mismatch", error))
    }

    async fn launch_now(config: HostLaunchConfig, request: LaunchRequest) -> Result<Launched, LaunchFailure> {
        let mode = config.permission_mode(&request);
        let (spec, pi_launch) = config.launch_spec(request.backend, mode, &request.project_root)?;
        // DeepSeek + loopback gateway + sandbox: refuse now, not after the
        // first prompt fails with "Connection error".
        if let Some(refusal) = config.sandbox_homes_root().and_then(|homes| config.loopback_refusal(&request, mode, homes)) {
            return Err(refusal);
        }
        let os = config.os;
        // Windows sandboxed: private layout + srt registry + ancestor ACEs.
        let mut windows: Option<WindowsSandboxContext> = None;
        let prepared = match mode {
            // Full access lifts the isolation only: HOME (Windows
            // USERPROFILE / APPDATA / LOCALAPPDATA), XDG_*, temp and the
            // backend's own home still point at the private HOME of this
            // project x backend, the one the sandbox uses.
            PermissionMode::FullAccess => {
                let private = config.full_access_private(&request)?;
                prepare_full_access(spec, &request.session_key, &private, os)?
            }
            PermissionMode::Sandboxed => {
                let install = config.srt_install().ok_or_else(|| {
                    LaunchFailure::from(
                        SandboxError::new(SandboxErrorCode::SandboxDependencyMissing, format!("Node is not installed in {}", config.runtime_root.display()))
                            .remedy("安装 Fox 的外部后端运行时（含 Node、srt 与后端）"),
                    )
                })?;
                let ready = preflight(&install, &RealProbe);
                let (homes_root, launcher) = match ready {
                    Err(error) => (config.fox_data.join("sandbox-homes"), Err(error)),
                    Ok(ready) => {
                        if os == HostOs::Windows {
                            let context = WindowsSandboxContext::prepare(&config.state_root)?;
                            let launcher = SrtLauncher::new(ready, config.state_root.clone())
                                .with_managed_runtime_root(config.runtime_root.clone())
                                .with_tmp_root(context.layout.tmp_root())
                                .with_srt_sandbox_sid(context.sid.clone());
                            let homes = context.layout.homes_root();
                            windows = Some(context);
                            (homes, Ok(launcher))
                        } else {
                            (config.fox_data.join("sandbox-homes"), Ok(SrtLauncher::new(ready, config.state_root.clone())))
                        }
                    }
                };
                let policy = config.sandbox_policy(&request, homes_root);
                let setup_error;
                let setup = match (&launcher, &policy) {
                    (Ok(launcher), Ok(policy)) => SandboxSetup::Ready { launcher, policy },
                    (Err(error), _) | (_, Err(error)) => {
                        setup_error = error.clone();
                        SandboxSetup::Unavailable(setup_error)
                    }
                };
                prepare_launch(mode, spec, &request.session_key, setup, os)?
            }
        };
        let declaration = prepared.declaration.clone();
        let cwd = request.project_root.clone();
        let (connection, on_closed) = match windows {
            None => (AcpConnection::spawn_prepared_detailed(prepared, &cwd).await?, None),
            Some(context) => context.spawn(prepared, cwd, request.session_key.clone()).await?,
        };
        let mut connection = connection;
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
        Ok(Launched { connection, mode, declaration, pi_launch, on_closed })
    }
}

use crate::acp::AcpConnection;

impl ExternalLauncher for HostLauncher {
    fn desired_mode(&self, request: &LaunchRequest) -> PermissionMode {
        self.config.permission_mode(request)
    }

    fn launch(&self, request: LaunchRequest) -> BoxFuture<'static, Result<Launched, LaunchFailure>> {
        let config = self.config.clone();
        Box::pin(Self::launch_now(config, request))
    }
}

/// Windows sandboxed launch context (docs/sandbox.md 7.5, 7.6): the per-user
/// private root, the srt sandbox SID and the ancestor-ACE coordinator. The
/// spawn runs inside the global srt transition on a blocking thread.
struct WindowsSandboxContext {
    layout: crate::winruntime::WindowsPrivateLayout,
    sid: String,
    coordinator: std::sync::Arc<crate::winancestor::AncestorAceCoordinator>,
}

impl WindowsSandboxContext {
    fn prepare(state_root: &Path) -> Result<Self, LaunchFailure> {
        use crate::winacl::RealWindowsAcl;
        use crate::winruntime::{prepare_windows_private_root, read_srt_registry, record_srt_sid, WindowsPrivateLayout};
        let registry = read_srt_registry(&RealProbe)?;
        let known = record_srt_sid(state_root, &registry)?;
        let layout = WindowsPrivateLayout::for_current_user(&RealWindowsAcl)?;
        prepare_windows_private_root(&layout, &RealWindowsAcl, &RealWindowsAcl, &known)?;
        let acl = std::sync::Arc::new(RealWindowsAcl);
        let coordinator = crate::winancestor::AncestorAceCoordinator::new(state_root.to_path_buf(), acl.clone(), acl)
            .with_srt_acl_lock(crate::winancestor::srt_acl_lock_path());
        Ok(Self { layout, sid: registry.sandbox_user_sid, coordinator: std::sync::Arc::new(coordinator) })
    }

    /// Ancestor ACEs + spawn inside one srt transition; the returned callback
    /// ends the ancestor session inside another transition after close.
    async fn spawn(
        self,
        prepared: crate::sandbox::PreparedLaunch,
        cwd: PathBuf,
        session_key: String,
    ) -> Result<(AcpConnection, Option<Box<dyn FnOnce() + Send>>), LaunchFailure> {
        let handle = tokio::runtime::Handle::current();
        let coordinator = std::sync::Arc::clone(&self.coordinator);
        let sid = self.sid.clone();
        let key = session_key.clone();
        let spawned = tokio::task::spawn_blocking(move || -> Result<AcpConnection, LaunchFailure> {
            let mut transition = coordinator.transition()?;
            if let Err(error) = transition.begin_session(&key, &sid, &prepared.windows_ancestor_aces) {
                transition.finish();
                return Err(error.into());
            }
            let result = handle.block_on(AcpConnection::spawn_prepared_detailed(prepared, &cwd));
            if result.is_err() {
                let _ = transition.end_session(&key);
            }
            transition.finish();
            result.map_err(LaunchFailure::from)
        })
        .await
        .map_err(|error| LaunchFailure::new("spawn_failed", format!("launch task failed: {error}")))??;
        let coordinator = std::sync::Arc::clone(&self.coordinator);
        let on_closed: Box<dyn FnOnce() + Send> = Box::new(move || {
            if let Ok(mut transition) = coordinator.transition() {
                let _ = transition.end_session(&session_key);
                transition.finish();
            }
        });
        Ok((spawned, Some(on_closed)))
    }
}

/// Startup crash recovery on Windows: remove every ancestor ACE left by a
/// previous Fox process (no session is alive at startup). No-op elsewhere.
pub fn recover_at_startup(config: &HostLaunchConfig) -> Result<(), String> {
    if config.os != HostOs::Windows || !config.state_root.join(crate::winancestor::ANCESTOR_LEDGER_FILE).exists() {
        return Ok(());
    }
    let acl = std::sync::Arc::new(crate::winacl::RealWindowsAcl);
    let coordinator = crate::winancestor::AncestorAceCoordinator::new(config.state_root.clone(), acl.clone(), acl)
        .with_srt_acl_lock(crate::winancestor::srt_acl_lock_path());
    crate::winancestor::recover_ancestor_aces(&coordinator, &Default::default()).map(|_| ()).map_err(|error| error.to_string())
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
        }
    }

    #[test]
    fn mode_is_sandboxed_unless_requested_granted_and_not_forbidden() {
        let root = std::env::temp_dir().join("fox-ext-launch-mode");
        let mut config = config(&root);
        let req = request(AgentBackend::Pi, &root);
        assert_eq!(config.permission_mode(&req), PermissionMode::Sandboxed);
        config.full_access = FullAccessSwitch::parse(Some("1"));
        assert_eq!(config.permission_mode(&req), PermissionMode::FullAccess);
        config.full_access = FullAccessSwitch::parse(Some("conversation:other"));
        assert_eq!(config.permission_mode(&req), PermissionMode::Sandboxed, "a grant for another conversation does not apply");
        config.full_access = FullAccessSwitch::parse(Some("conversation:c1"));
        assert_eq!(config.permission_mode(&req), PermissionMode::FullAccess);
        config.full_access_forbidden = true;
        assert_eq!(config.permission_mode(&req), PermissionMode::Sandboxed, "enterprise switch wins");
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
        assert_eq!(probe.private_home, root.join("data/sandbox-homes/8f14e45f-ceea-467a-9a36-dedd4bea2543/deepseek"));
        let real = root.join("real");
        std::fs::create_dir_all(real.join(".dsh/profiles/acp")).unwrap();
        std::fs::write(real.join(".dsh/.env"), "DEEPSEEK_API_KEY=sk-glue-secret-1
").unwrap();
        std::fs::write(real.join(".dsh/profiles/acp/cordis.patch.yml"), "- id: llm-deepseek\n  config:\n    baseURL: http://127.0.0.1:3425/v1\n").unwrap();
        let real = crate::onboarding::RealHome { home: real, codex_home: None };
        let projects = vec!["8f14e45f-ceea-467a-9a36-dedd4bea2543".to_owned(), "c9f0f895-fb98-4b91-a5c3-2b5f3e3a7a11".to_owned()];
        let selected = vec!["env:.dsh/.env#DEEPSEEK_API_KEY".to_owned(), "file:.dsh/profiles/acp/cordis.patch.yml".to_owned()];
        let report = config.import_credentials(AgentBackend::Deepseek, &projects, &real, &selected).unwrap();
        assert_eq!(report.homes, 2);
        assert!(!report.audit_event().to_string().contains("sk-glue-secret-1"));
        // Sandboxed + loopback base URL: refused category; full access: ready.
        let probe = config.credential_probe(&req).unwrap();
        assert_eq!(probe.state, crate::onboarding::CredentialState::LoopbackRequiresFullAccess);
        req.full_access_granted = true;
        assert_eq!(config.credential_probe(&req).unwrap().state, crate::onboarding::CredentialState::Ready);
        let mut second = request(AgentBackend::Deepseek, &root);
        second.project_uuid = Some(projects[1].clone());
        second.full_access_granted = true;
        assert_eq!(config.credential_probe(&second).unwrap().state, crate::onboarding::CredentialState::Ready, "applied to every project");
        assert!(config.import_credentials(AgentBackend::Deepseek, &[], &real, &selected).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// UX batch ②: the user's per-conversation choice (persisted, carried as
    /// `LaunchRequest::full_access_granted`) grants full access to exactly
    /// that conversation, OR-ed with the engineering switch; FORBIDDEN beats
    /// both.
    #[test]
    fn the_conversation_grant_from_the_ui_or_the_switch_and_forbidden_wins() {
        let root = std::env::temp_dir().join("fox-ext-launch-grant");
        let mut config = config(&root);
        let mut req = request(AgentBackend::Deepseek, &root);
        assert_eq!(config.permission_mode(&req), PermissionMode::Sandboxed, "sandboxed by default");
        req.full_access_granted = true;
        assert_eq!(config.permission_mode(&req), PermissionMode::FullAccess, "chosen at creation");
        let mut other = request(AgentBackend::Deepseek, &root);
        other.conversation_id = "c2".into();
        assert_eq!(config.permission_mode(&other), PermissionMode::Sandboxed, "the grant is per conversation");
        req.full_access_granted = false;
        config.full_access = FullAccessSwitch::All;
        assert_eq!(config.permission_mode(&req), PermissionMode::FullAccess, "engineering switch alone");
        req.full_access_granted = true;
        config.full_access_forbidden = true;
        assert_eq!(config.permission_mode(&req), PermissionMode::Sandboxed, "FORBIDDEN beats the UI grant and the switch");
        config.full_access = FullAccessSwitch::Off;
        assert_eq!(config.permission_mode(&req), PermissionMode::Sandboxed, "FORBIDDEN beats the UI grant alone");
    }

    #[test]
    fn specs_use_the_managed_install_only_never_npx() {
        let root = std::env::temp_dir().join(format!("fox-ext-launch-spec-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mut config = config(&root);
        // Nothing installed: refused in both modes, never an npx fallback.
        for mode in [PermissionMode::Sandboxed, PermissionMode::FullAccess] {
            let error = config.launch_spec(AgentBackend::Pi, mode, &root).unwrap_err();
            assert_eq!(error.code, "agent_backend_not_installed");
        }
        // Paths are built and compared per component (`Path` equality), so the
        // test means the same on Windows, where `join("a/b")` would mix
        // separators and a string comparison would fail.
        let under = |base: &Path, parts: &[&str]| parts.iter().fold(base.to_path_buf(), |path, part| path.join(part));
        // Managed install: absolute program, no npx arguments; the pi gate when shipped.
        let bin = under(&root, &["runtime", "backends", "pi", "node_modules", ".bin"]);
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("pi-acp"), "").unwrap();
        let (spec, launch) = config.launch_spec(AgentBackend::Pi, PermissionMode::Sandboxed, &root).unwrap();
        assert_eq!(Path::new(&spec.program), bin.join("pi-acp"));
        assert!(spec.args.is_empty());
        assert_eq!(launch, PiLaunch::Stock);
        let gate_dir = under(&root, &["runtime", "pi-gate"]);
        std::fs::create_dir_all(&gate_dir).unwrap();
        std::fs::write(gate_dir.join("pi-gated.sh"), "").unwrap();
        let (spec, launch) = config.launch_spec(AgentBackend::Pi, PermissionMode::Sandboxed, &root).unwrap();
        assert_eq!(launch, PiLaunch::Gated);
        assert!(spec.session_env.iter().any(|(key, value)| key == "FOX_PI_BIN" && Path::new(value) == bin.join("pi")));
        assert!(spec.session_env.iter().any(|(key, value)| key == "PI_ACP_PI_COMMAND" && Path::new(value) == gate_dir.join("pi-gated.sh")));
        // Codex: workspace-write mode; tool-chain passthrough only in full access.
        let codex = under(&root, &["runtime", "backends", "codex", "node_modules", ".bin"]);
        std::fs::create_dir_all(&codex).unwrap();
        std::fs::write(codex.join("codex-acp"), "").unwrap();
        config.env_passthrough = vec!["PATH".into()];
        let (spec, _) = config.launch_spec(AgentBackend::Codex, PermissionMode::FullAccess, &root).unwrap();
        assert!(spec.session_env.iter().any(|(key, value)| key == "INITIAL_AGENT_MODE" && value == "workspace-write"));
        assert!(spec.session_env.iter().any(|(key, _)| key == "PATH"), "passthrough in full access");
        let (spec, _) = config.launch_spec(AgentBackend::Codex, PermissionMode::Sandboxed, &root).unwrap();
        assert!(!spec.session_env.iter().any(|(key, _)| key == "PATH"), "never in sandboxed mode");
        // DeepSeek managed (runtime package 0.1.1 layout: `backends/dsh`): the
        // installed patch must match byte for byte.
        let dsh = under(&root, &["runtime", "backends", "dsh", "node_modules", ".bin"]);
        std::fs::create_dir_all(&dsh).unwrap();
        std::fs::write(dsh.join("dsh"), "").unwrap();
        let error = config.launch_spec(AgentBackend::Deepseek, PermissionMode::Sandboxed, &root).unwrap_err();
        assert_eq!(error.code, "agent_backend_not_installed");
        let patch_policy = crate::backends::deepseek::HostPolicy { workspace_write: true, network: false };
        let patch = crate::backends::deepseek::default_patch_path(&config.runtime_root, &patch_policy);
        std::fs::create_dir_all(patch.parent().unwrap()).unwrap();
        std::fs::write(&patch, crate::backends::deepseek::policy_patch(&patch_policy).patch_yaml).unwrap();
        let (spec, _) = config.launch_spec(AgentBackend::Deepseek, PermissionMode::Sandboxed, &root).unwrap();
        assert!(spec.args.iter().any(|arg| Path::new(arg) == patch));
        assert_eq!(Path::new(&spec.program), dsh.join("dsh"));
        // A local gateway on the pi allowlist clears NO_PROXY for the backend (Linux / macOS).
        config.extra_domains = parse_extra_domains(Some("pi=127.0.0.1:3425"));
        assert!(config.allows_loopback(AgentBackend::Pi));
        let (spec, _) = config.launch_spec(AgentBackend::Pi, PermissionMode::Sandboxed, &root).unwrap();
        assert!(spec.session_env.iter().any(|(key, value)| key == "NO_PROXY" && value.is_empty()));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The production launcher refuses a sandboxed DeepSeek session with a
    /// loopback base URL before preflight / srt (here no srt is installed:
    /// without the refusal the error would be `sandbox_dependency_missing`).
    #[tokio::test(flavor = "current_thread")]
    async fn host_launcher_refuses_a_sandboxed_deepseek_loopback_gateway_before_starting() {
        let root = std::env::temp_dir().join(format!("fox-ext-launch-dsh-refuse-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let config = config(&root);
        let dsh = root.join("runtime").join("backends").join("dsh").join("node_modules").join(".bin");
        std::fs::create_dir_all(&dsh).unwrap();
        std::fs::write(dsh.join("dsh"), "").unwrap();
        let patch_policy = crate::backends::deepseek::HostPolicy { workspace_write: true, network: false };
        let patch = crate::backends::deepseek::default_patch_path(&config.runtime_root, &patch_policy);
        std::fs::create_dir_all(patch.parent().unwrap()).unwrap();
        std::fs::write(&patch, crate::backends::deepseek::policy_patch(&patch_policy).patch_yaml).unwrap();
        let request = request(AgentBackend::Deepseek, &root);
        std::fs::create_dir_all(&request.project_root).unwrap();
        let home = PrivateHomes::new(config.fox_data.join("sandbox-homes")).home_for(request.project_uuid.as_deref().unwrap(), AgentBackend::Deepseek).unwrap();
        std::fs::create_dir_all(home.path.join(".dsh")).unwrap();
        let launcher = HostLauncher::new(config.clone());
        // Remote base URL: gets past the refusal (and then fails on the missing srt).
        std::fs::write(home.path.join(".dsh").join("cordis.patch.yml"), "- id: llm-deepseek\n  config:\n    baseURL: https://api.deepseek.com/anthropic\n").unwrap();
        let error = launcher.launch(request.clone()).await.err().expect("no srt here");
        assert_ne!(error.code, LOOPBACK_REQUIRES_FULL_ACCESS);
        // Loopback base URL: refused with the dedicated code.
        std::fs::write(home.path.join(".dsh").join("cordis.patch.yml"), "- id: llm-deepseek\n  config:\n    baseURL: http://127.0.0.1:3425/anthropic\n").unwrap();
        let error = launcher.launch(request.clone()).await.err().expect("refused");
        assert_eq!(error.code, LOOPBACK_REQUIRES_FULL_ACCESS, "{error:?}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Full access through the production launcher: the backend program runs
    /// itself (no srt / node cli.js in front of it) and sees HOME, XDG_* and
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
        let bin = root.join("runtime").join("backends").join("pi").join("node_modules").join(".bin");
        std::fs::create_dir_all(&bin).unwrap();
        let script = bin.join("pi-acp");
        std::fs::write(&script, "#!/bin/sh\nprintf 'argv0=%s\\nHOME=%s\\nTMPDIR=%s\\nXDG_CONFIG_HOME=%s\\nPROXY=%s%s\\n' \"$0\" \"$HOME\" \"$TMPDIR\" \"$XDG_CONFIG_HOME\" \"$NODE_USE_ENV_PROXY\" \"$CLAUDE_CODE_TMPDIR\" > \"$HOME/fox-env.txt\"\nexit 0\n").unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let request = request(AgentBackend::Pi, &root);
        std::fs::create_dir_all(&request.project_root).unwrap();
        assert_eq!(config.permission_mode(&request), PermissionMode::FullAccess);
        let home = PrivateHomes::new(config.fox_data.join("sandbox-homes")).home_for(request.project_uuid.as_deref().unwrap(), AgentBackend::Pi).unwrap().path;
        assert!(!home.exists());
        let _ = HostLauncher::new(config.clone()).launch(request.clone()).await;
        let recorded = std::fs::read_to_string(home.join("fox-env.txt")).expect("the backend wrote under the private HOME");
        let tmp = config.state_root.join("sandbox-tmp").join("s1");
        assert!(recorded.contains(&format!("argv0={}\n", script.display())), "not wrapped: {recorded}");
        assert!(recorded.contains(&format!("HOME={}\n", home.display())), "{recorded}");
        assert!(recorded.contains(&format!("TMPDIR={}\n", tmp.display())), "{recorded}");
        assert!(recorded.contains(&format!("XDG_CONFIG_HOME={}\n", home.join(".config").display())), "{recorded}");
        assert!(recorded.contains("PROXY=\n"), "no srt-only variables: {recorded}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn sandbox_policy_keys_the_private_home_by_project_uuid_and_backend() {
        let root = std::env::temp_dir().join("fox-ext-launch-policy");
        let mut config = config(&root);
        config.pi_providers = vec!["anthropic".into()];
        config.extra_domains = parse_extra_domains(Some("pi=llm.example.com,127.0.0.1:3425; codex=x.example.com"));
        let policy = config.sandbox_policy(&request(AgentBackend::Pi, &root), root.join("homes")).unwrap();
        assert_eq!(policy.private_home, root.join("homes/8f14e45f-ceea-467a-9a36-dedd4bea2543/pi"));
        assert_eq!(policy.authorized, vec![AuthorizedDir { path: root.join("proj"), writable: true }]);
        assert!(policy.allowed_domains.contains(&"api.anthropic.com".to_owned()));
        assert!(policy.allowed_domains.contains(&"llm.example.com".to_owned()));
        assert!(policy.allowed_domains.contains(&"127.0.0.1:3425".to_owned()), "local gateway port is allowed");
        assert!(!policy.allowed_domains.contains(&"x.example.com".to_owned()));
        assert!(policy.extra_read_only.contains(&root.join("runtime")));
        let mut bad = request(AgentBackend::Pi, &root);
        bad.project_uuid = None;
        assert_eq!(config.sandbox_policy(&bad, root.join("homes")).unwrap_err().code, SandboxErrorCode::SandboxPolicyInvalid);
    }

    /// DeepSeek with a loopback model gateway in its own base URL: since the
    /// f2bf567 re-test no loopback port is allowed (dsh dials loopback
    /// directly), the declaration says why, and a sandboxed launch is refused
    /// before it starts; full access and remote base URLs are not refused.
    #[test]
    fn deepseek_loopback_gateway_is_refused_in_the_sandbox_and_never_allowed() {
        use crate::sandbox::SandboxDeclaration;
        let root = std::env::temp_dir().join(format!("fox-ext-launch-dsh-loopback-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let config = config(&root);
        let homes = root.join("homes");
        let request_for = request;
        let request = request_for(AgentBackend::Deepseek, &root);
        std::fs::create_dir_all(&request.project_root).unwrap();
        // No base URL configured: only the default domain, no loopback.
        let policy = config.sandbox_policy(&request, homes.clone()).unwrap();
        assert_eq!(policy.allowed_domains, ["api.deepseek.com"]);
        assert!(policy.loopback_gateway.is_none() && policy.loopback_diagnostics.is_empty());
        // The private HOME's dsh config names the Magpie gateway.
        let dsh_home = policy.private_home.join(".dsh");
        std::fs::create_dir_all(&dsh_home).unwrap();
        let write_base_url = |url: &str| std::fs::write(dsh_home.join("cordis.patch.yml"), format!("- id: llm-deepseek\n  config:\n    baseURL: {url}\n")).unwrap();
        assert!(config.loopback_refusal(&request, PermissionMode::Sandboxed, homes.clone()).is_none(), "no base URL: launches");
        write_base_url("http://127.0.0.1:3425/anthropic");
        let policy = config.sandbox_policy(&request, homes.clone()).unwrap();
        assert_eq!(policy.allowed_domains, ["api.deepseek.com"], "the gateway port is no longer allowed");
        assert!(policy.loopback_gateway.is_none());
        assert!(policy.loopback_diagnostics[0].contains("127.0.0.1:3425") && policy.loopback_diagnostics[0].contains("直连"), "{:?}", policy.loopback_diagnostics);
        let settings = policy.to_srt_settings();
        assert_eq!(settings["network"]["allowedDomains"], serde_json::json!(["api.deepseek.com"]));
        let declaration = SandboxDeclaration::declare(PermissionMode::Sandboxed, Some(&policy), HostOs::Linux);
        let view = crate::capability_view::capability_view(
            AgentBackend::Deepseek,
            Some(&serde_json::json!({"agentInfo": {"name": "deepseek-harness-acp"}})),
            &declaration,
            crate::capability_view::PiLaunch::Stock,
        );
        assert!(view.sandbox.detail.contains("未放行本机回环端口"), "{}", view.sandbox.detail);
        assert!(view.sandbox.detail.contains("完全访问"), "{}", view.sandbox.detail);
        // Sandboxed launch: refused before anything starts, with the new code.
        let refusal = config.loopback_refusal(&request, PermissionMode::Sandboxed, homes.clone()).expect("refused in the sandbox");
        assert_eq!(refusal.code, LOOPBACK_REQUIRES_FULL_ACCESS);
        assert!(refusal.message.contains("本机网关在沙箱模式下不可达") && refusal.message.contains("http://127.0.0.1:3425/anthropic"), "{}", refusal.message);
        assert_eq!(refusal.remedy.as_deref(), Some(LOOPBACK_REFUSAL_REMEDY));
        assert!(refusal.remedy.as_deref().unwrap().contains("新建会话时选择“完全访问”"));
        assert!(!refusal.remedy.as_deref().unwrap().contains("请切换到") && !refusal.remedy.as_deref().unwrap().contains("FOX_EXTERNAL_FULL_ACCESS"));
        // Forbidden on this machine: the remedy no longer offers full access.
        let mut forbidden = config.clone();
        forbidden.full_access_forbidden = true;
        let refusal = forbidden.loopback_refusal(&request, PermissionMode::Sandboxed, homes.clone()).expect("refused");
        assert_eq!(refusal.remedy.as_deref(), Some(LOOPBACK_REFUSAL_REMEDY_FORBIDDEN));
        assert!(refusal.remedy.as_deref().unwrap().contains("已禁止完全访问") && !refusal.remedy.as_deref().unwrap().contains("新建会话时选择"));
        assert_eq!(refusal.detail["reason"], "dsh_proxy_bypasses_loopback");
        // Full access: not refused.
        assert!(config.loopback_refusal(&request, PermissionMode::FullAccess, homes.clone()).is_none());
        // A loopback base URL whose port is computed is refused as well.
        write_base_url("http://localhost:${MAGPIE_PORT}/anthropic");
        assert!(config.loopback_refusal(&request, PermissionMode::Sandboxed, homes.clone()).is_some());
        // Port cannot be determined: nothing allowed, diagnostic declared.
        write_base_url("!!js process.env.GW");
        let policy = config.sandbox_policy(&request, homes.clone()).unwrap();
        assert_eq!(policy.allowed_domains, ["api.deepseek.com"]);
        assert!(policy.loopback_gateway.is_none());
        assert!(!policy.loopback_diagnostics.is_empty());
        let declaration = SandboxDeclaration::declare(PermissionMode::Sandboxed, Some(&policy), HostOs::Linux);
        assert!(!declaration.loopback_diagnostics.is_empty());
        assert!(config.loopback_refusal(&request, PermissionMode::Sandboxed, homes.clone()).is_none(), "not loopback: not refused");
        // A remote base URL: no loopback port, no diagnostic, not refused.
        write_base_url("https://llm.example.com/anthropic");
        let policy = config.sandbox_policy(&request, homes.clone()).unwrap();
        assert_eq!(policy.allowed_domains, ["api.deepseek.com"]);
        assert!(policy.loopback_gateway.is_none() && policy.loopback_diagnostics.is_empty());
        assert!(config.loopback_refusal(&request, PermissionMode::Sandboxed, homes.clone()).is_none());
        // Other backends never read dsh configuration and are never refused.
        write_base_url("http://127.0.0.1:3425");
        let pi = config.sandbox_policy(&request_for(AgentBackend::Pi, &root), homes.clone()).unwrap();
        assert!(!pi.allowed_domains.iter().any(|domain| domain.starts_with("127.0.0.1")));
        assert!(config.loopback_refusal(&request_for(AgentBackend::Pi, &root), PermissionMode::Sandboxed, homes).is_none());
        let _ = std::fs::remove_dir_all(&root);
    }
}
