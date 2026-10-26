//! Process launch specs for the external backends (plan 6.1, 6.7). Every
//! external backend is reached over ACP v1 through an existing entry point;
//! Fox does not implement per-vendor protocols.

use crate::port::AgentBackend;

/// Parent environment variables a backend process may inherit. Mirrors the
/// Host's `command_environment` allowlist; everything else (cloud keys,
/// tokens, NODE_OPTIONS, proxies) is dropped unless named per backend in
/// [`LaunchSpec::credential_env`].
#[cfg(windows)]
pub const ENV_ALLOWLIST: &[&str] = WINDOWS_ENV_ALLOWLIST;
#[cfg(not(windows))]
pub const ENV_ALLOWLIST: &[&str] = UNIX_ENV_ALLOWLIST;

/// [`ENV_ALLOWLIST`] on Windows (named so the Windows planning is testable
/// on any host).
pub const WINDOWS_ENV_ALLOWLIST: &[&str] = &[
    "PATH", "PATHEXT", "SystemRoot", "WINDIR", "COMSPEC", "TEMP", "TMP",
    "USERPROFILE", "HOMEDRIVE", "HOMEPATH", "LOCALAPPDATA", "APPDATA",
    "PROGRAMDATA", "PROGRAMFILES", "PROGRAMFILES(X86)", "PROGRAMW6432",
    "USERNAME", "USERDOMAIN", "NUMBER_OF_PROCESSORS", "PROCESSOR_ARCHITECTURE",
    "OS", "LANG", "LC_ALL", "LC_CTYPE", "TERM",
];

/// [`ENV_ALLOWLIST`] on Linux / macOS.
pub const UNIX_ENV_ALLOWLIST: &[&str] = &[
    "PATH", "HOME", "USER", "LOGNAME", "SHELL", "TMPDIR", "TEMP", "TMP",
    "LANG", "LC_ALL", "LC_CTYPE", "TERM", "XDG_CACHE_HOME", "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchSpec {
    pub backend: AgentBackend,
    pub program: String,
    pub args: Vec<String>,
    /// Exact package/binary version the adapter is validated against.
    pub version_pin: &'static str,
    /// Credential variables this backend's own authentication reads. Passed
    /// through by name only, never logged; the backend keeps managing its own
    /// login (plan 7.5). Empty means "the backend's own login store".
    pub credential_env: &'static [&'static str],
    /// Experimental: safety floor applies, function/stability gates do not.
    pub experimental: bool,
    /// Variables the Host itself sets for this session (for example codex-acp
    /// `INITIAL_AGENT_MODE`, pi-acp `PI_ACP_PI_COMMAND`). Added after the
    /// allowlist; never taken from the parent environment.
    pub session_env: Vec<(String, String)>,
}

fn npx() -> String {
    if cfg!(windows) { "npx.cmd".into() } else { "npx".into() }
}

pub const CODEX_ACP_VERSION: &str = "2.2.2";
pub const PI_ACP_VERSION: &str = "0.0.34";
/// DeepSeek Harness is a developer preview; pinned by `dsh --version` check.
pub const DSH_VERSION: &str = "0.2.1-rc.2";

/// Environment variable naming an installed entry point for `backend`
/// (`FOX_PI_ACP_PROGRAM`, `FOX_CODEX_ACP_PROGRAM`, `FOX_DSH_PROGRAM`). When
/// set, [`launch_spec`] runs that program with no `npx` arguments; the Host
/// launcher ([`crate::external_launch`]) also honours it before the managed
/// runtime. Lets the real-binary tests run against an installed runtime.
pub fn program_override_env(backend: AgentBackend) -> Option<&'static str> {
    match backend {
        AgentBackend::Pi => Some("FOX_PI_ACP_PROGRAM"),
        AgentBackend::Codex => Some("FOX_CODEX_ACP_PROGRAM"),
        AgentBackend::Deepseek => Some("FOX_DSH_PROGRAM"),
        AgentBackend::Fox => None,
    }
}

/// The override of [`program_override_env`], when set and non-empty.
pub fn program_override(backend: AgentBackend) -> Option<String> {
    let name = program_override_env(backend)?;
    std::env::var(name).ok().map(|value| value.trim().to_owned()).filter(|value| !value.is_empty())
}

/// `None` for FoxBackend, which runs in-process through the existing Kernel.
///
/// The `npx -y <pinned package>` entry points are for development and the
/// real-binary tests only; set [`program_override_env`] to use an installed
/// program instead. The desktop Host never launches through `npx` (it resolves
/// the Fox-managed runtime, see `crate::external_launch`).
pub fn launch_spec(backend: AgentBackend) -> Option<LaunchSpec> {
    let mut spec = default_launch_spec(backend)?;
    if let Some(program) = program_override(backend) {
        spec.program = program;
        if backend != AgentBackend::Deepseek {
            spec.args.clear();
        }
    }
    Some(spec)
}

/// Where a development / real-binary-test entry point came from
/// ([`dev_launch_spec`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DevEntrySource {
    /// [`program_override_env`] (`FOX_*_PROGRAM`).
    Override,
    /// `<runtime>/backends/<dir>/...` of the managed runtime ([`dev_runtime_root`]).
    Managed(std::path::PathBuf),
    /// `npx -y <pinned package>` (tests only; never the Host).
    Npx,
}

/// `<runtime>/backends/<dir>/node_modules/.bin/<bin>[.cmd]`, else
/// `<runtime>/backends/<dir>/<bin>[.cmd]`, whichever exists. The one layout
/// rule shared by the Host launcher ([`crate::external_launch`]) and the
/// real-binary tests.
pub fn managed_entry(runtime_root: &std::path::Path, backend: AgentBackend, windows: bool) -> Option<std::path::PathBuf> {
    let (dir, bin) = match backend {
        AgentBackend::Pi => ("pi", "pi-acp"),
        AgentBackend::Codex => ("codex", "codex-acp"),
        // Runtime package 0.1.1 layout: `backends\dsh` (not `deepseek`).
        AgentBackend::Deepseek => ("dsh", "dsh"),
        AgentBackend::Fox => return None,
    };
    let exe = if windows { format!("{bin}.cmd") } else { bin.to_owned() };
    let base = runtime_root.join("backends").join(dir);
    [base.join("node_modules").join(".bin").join(&exe), base.join(&exe)].into_iter().find(|candidate| candidate.is_file())
}

/// The managed runtime the real-binary tests look in: `FOX_AGENT_RUNTIME_ROOT`;
/// on Windows otherwise `FOX_RUNTIME_ROOT`, `FOX_RUNTIME_VERSION` or the
/// newest version under `%ProgramFiles%\Fox\sandbox-runtime`. Linux / macOS
/// have no default here (the Host's is `<fox data>/agent-runtime`).
pub fn dev_runtime_root() -> Option<std::path::PathBuf> {
    let set = |name: &str| std::env::var_os(name).filter(|value| !value.is_empty()).map(std::path::PathBuf::from);
    if let Some(root) = set("FOX_AGENT_RUNTIME_ROOT") {
        return Some(root);
    }
    if !cfg!(windows) {
        return None;
    }
    if set(crate::winruntime::RUNTIME_ROOT_ENV).is_some() {
        return Some(crate::winruntime::windows_runtime_root(""));
    }
    if let Some(version) = std::env::var("FOX_RUNTIME_VERSION").ok().filter(|value| !value.trim().is_empty()) {
        return Some(crate::winruntime::default_windows_runtime_root(version.trim()));
    }
    let parent = crate::winruntime::default_windows_runtime_root("");
    let expected = parent.join(RUNTIME_PACKAGE_VERSION);
    if expected.is_dir() {
        return Some(expected);
    }
    let mut versions: Vec<std::path::PathBuf> = std::fs::read_dir(&parent)
        .ok()?
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false))
        .filter(|entry| !entry.file_name().to_string_lossy().starts_with('.'))
        .map(|entry| entry.path())
        .collect();
    versions.sort();
    versions.pop()
}

/// Entry point for the real-binary tests and development, in order:
/// [`program_override_env`] (`FOX_CODEX_ACP_PROGRAM` / `FOX_PI_ACP_PROGRAM` /
/// `FOX_DSH_PROGRAM`), then the managed runtime entry under
/// [`dev_runtime_root`], then `npx -y <pinned package>` (dsh:
/// `@deepseek-ai/dsh@<pin>`). Tests only: the desktop Host resolves the
/// managed runtime through [`crate::external_launch::HostLaunchConfig`] and
/// never falls back to `npx`.
pub fn dev_launch_spec(backend: AgentBackend) -> Option<(LaunchSpec, DevEntrySource)> {
    let mut spec = default_launch_spec(backend)?;
    let (program, source) = if let Some(program) = program_override(backend) {
        (program, DevEntrySource::Override)
    } else if let Some((root, entry)) = dev_runtime_root().and_then(|root| managed_entry(&root, backend, cfg!(windows)).map(|entry| (root, entry))) {
        (entry.to_string_lossy().into_owned(), DevEntrySource::Managed(root))
    } else {
        if backend == AgentBackend::Deepseek {
            spec.program = npx();
            spec.args = vec!["-y".into(), format!("@deepseek-ai/dsh@{DSH_VERSION}"), "--profile".into(), "acp".into()];
        }
        return Some((spec, DevEntrySource::Npx));
    };
    spec.program = program;
    if backend != AgentBackend::Deepseek {
        spec.args.clear();
    }
    Some((spec, source))
}

fn default_launch_spec(backend: AgentBackend) -> Option<LaunchSpec> {
    match backend {
        AgentBackend::Fox => None,
        AgentBackend::Codex => Some(LaunchSpec {
            backend,
            program: npx(),
            args: vec!["-y".into(), format!("@agentclientprotocol/codex-acp@{CODEX_ACP_VERSION}")],
            version_pin: CODEX_ACP_VERSION,
            credential_env: &["OPENAI_API_KEY", "CODEX_HOME"],
            experimental: false,
            // Without this codex-acp starts in `agent` mode, where Codex's own
            // reviewer answers approvals and the Host never sees them.
            session_env: vec![("INITIAL_AGENT_MODE".into(), "read-only".into())],
        }),
        AgentBackend::Pi => Some(LaunchSpec {
            backend,
            program: npx(),
            args: vec!["-y".into(), format!("pi-acp@{PI_ACP_VERSION}")],
            version_pin: PI_ACP_VERSION,
            credential_env: &[],
            experimental: false,
            session_env: vec![],
        }),
        AgentBackend::Deepseek => Some(LaunchSpec {
            backend,
            program: if cfg!(windows) { "dsh.cmd".into() } else { "dsh".into() },
            args: vec!["--profile".into(), "acp".into()],
            version_pin: DSH_VERSION,
            credential_env: &["DEEPSEEK_API_KEY"],
            experimental: true,
            session_env: vec![],
        }),
    }
}

/// Node flags for every backend started inside the Windows sandbox (pi-acp /
/// pi, codex-acp, dsh are Node programs launched through `.cmd` shims, so
/// the flags travel as `NODE_OPTIONS`, which Node allows for both and passes
/// on to the Node children the backend starts). `--preserve-symlinks`
/// (`-main`) makes module resolution keep the path as given instead of
/// `realpath`ing it, which avoids part of the ancestor `lstat` calls the
/// sandbox account may not be allowed to make. Auxiliary only: the ancestor
/// metadata ACEs (crate::winancestor) are what makes the profile chain
/// stat-able; `fs.realpath` calls of the backend itself are unaffected.
pub const WINDOWS_SANDBOX_NODE_FLAGS: &[&str] = &["--preserve-symlinks", "--preserve-symlinks-main"];

/// [`WINDOWS_SANDBOX_NODE_FLAGS`] as one `NODE_OPTIONS` value.
pub fn windows_sandbox_node_options() -> String {
    WINDOWS_SANDBOX_NODE_FLAGS.join(" ")
}

/// The child environment: allowlisted parent variables plus this backend's
/// named credential variables. Case-insensitive on Windows.
pub fn child_environment(
    spec: &LaunchSpec,
    parent: impl IntoIterator<Item = (String, String)>,
) -> Vec<(String, String)> {
    child_environment_for(cfg!(windows), spec, parent)
}

/// [`child_environment`] with the Windows (`true`) or Unix rules.
pub fn child_environment_for(
    windows: bool,
    spec: &LaunchSpec,
    parent: impl IntoIterator<Item = (String, String)>,
) -> Vec<(String, String)> {
    let list = if windows { WINDOWS_ENV_ALLOWLIST } else { UNIX_ENV_ALLOWLIST };
    let allowed = |key: &str| {
        list.iter()
            .chain(spec.credential_env.iter())
            .any(|name| if windows { name.eq_ignore_ascii_case(key) } else { *name == key })
    };
    parent.into_iter().filter(|(key, _)| allowed(key)).collect()
}

/// The full environment a launched process gets: [`child_environment`] of the
/// parent plus the spec's own [`LaunchSpec::session_env`] (which wins). This
/// is exactly what [`crate::acp::AcpConnection`] passes to the process it
/// starts. For a sandboxed launch that process is the srt broker (`node
/// cli.js`), so the spec's `session_env` there holds only what srt itself
/// needs; the backend's private HOME and friends travel inside the command
/// (Windows `-c` with `set`, Linux / macOS `env K=V` after `--`), see
/// [`crate::sandbox::SrtLauncher`].
pub fn process_environment(spec: &LaunchSpec, parent: impl IntoIterator<Item = (String, String)>) -> Vec<(String, String)> {
    process_environment_for(cfg!(windows), spec, parent)
}

/// [`process_environment`] with the Windows (`true`) or Unix rules.
pub fn process_environment_for(windows: bool, spec: &LaunchSpec, parent: impl IntoIterator<Item = (String, String)>) -> Vec<(String, String)> {
    let mut env = child_environment_for(windows, spec, parent);
    for (key, value) in &spec.session_env {
        env.retain(|(existing, _)| if windows { !existing.eq_ignore_ascii_case(key) } else { existing != key });
        env.push((key.clone(), value.clone()));
    }
    env
}

/// The runtime package this Fox build ships with and checks against (its
/// `acp-bridge\bridge.mjs` must equal the embedded copy). Windows default
/// when `FOX_RUNTIME_VERSION` / `FOX_RUNTIME_ROOT` are unset: this version
/// directory if installed, else the newest installed one. Keep in step with
/// `scripts/windows/build-runtime-package.py` `DEFAULT_VERSION`.
pub const RUNTIME_PACKAGE_VERSION: &str = "0.1.4";

/// Environment variable carrying the bridge pipe name into the sandbox
/// (Windows sandboxed mode, docs/sandbox.md 7.12).
pub const BRIDGE_PIPE_ENV: &str = "FOX_ACP_PIPE";
/// Environment variable carrying the one-shot bridge token. The only
/// secret-shaped name [`crate::sandbox::windows_command_line`] puts on the
/// command line (it is worthless once the single connection is made; see
/// docs/sandbox.md 7.12 for who can read a command line).
pub const BRIDGE_TOKEN_ENV: &str = "FOX_ACP_TOKEN";
/// First line the bridge sends: `FOX-ACP-BRIDGE/1 <token>\n`.
pub const BRIDGE_PROTOCOL: &str = "FOX-ACP-BRIDGE/1";
/// The bridge script as shipped in the runtime package
/// (`<runtime>\acp-bridge\bridge.mjs`; source
/// `runtime-assets/acp-bridge/bridge.mjs` in this crate). The installed copy
/// must equal it byte for byte (line endings normalized), same rule as the dsh
/// patches.
pub const BRIDGE_SCRIPT: &str = include_str!("../runtime-assets/acp-bridge/bridge.mjs");
/// Location of the bridge inside the managed runtime directory.
pub const BRIDGE_RELATIVE_PATH: &[&str] = &["acp-bridge", "bridge.mjs"];

/// How long the Host waits for the bridge to connect and finish the handshake.
pub const BRIDGE_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// A one-shot local endpoint the in-sandbox bridge connects back to
/// (Windows: named pipe `\\.\pipe\fox-acp-<128-bit hex>`; Unix: a socket path,
/// used by tests only). Created by the launcher, consumed once by
/// [`crate::acp::AcpConnection`].
#[derive(Clone, PartialEq, Eq)]
pub struct BridgeEndpoint {
    /// Pipe name (Windows) or socket path (Unix).
    pub address: String,
    /// 128-bit random hex token the bridge must present.
    pub token: String,
    /// Windows: SID of the srt sandbox account (`HKLM\SOFTWARE\sandbox-runtime`
    /// `SandboxUserSid`), the only account besides the current user the pipe
    /// DACL admits. Required on Windows.
    pub allowed_sid: Option<String>,
    pub connect_timeout: std::time::Duration,
    /// The backend's environment, delivered to the bridge in the handshake
    /// answer (never on the srt `-c` command line: cmd.exe cannot carry
    /// quotes, `%` or line breaks safely, and a command line is visible in
    /// the process list). The bridge sets exactly these over its own
    /// environment when it starts the backend.
    pub child_env: Vec<(String, String)>,
    /// The backend's working directory (a plain path, see
    /// [`backend_visible_path`]); srt-win does not carry the caller's cwd into
    /// the sandbox, so the bridge sets it explicitly.
    pub child_cwd: Option<String>,
}

impl std::fmt::Debug for BridgeEndpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never print the token.
        f.debug_struct("BridgeEndpoint")
            .field("address", &self.address)
            .field("token", &"[redacted]")
            .field("allowed_sid", &self.allowed_sid)
            .field("connect_timeout", &self.connect_timeout)
            // Names only: values may be secrets.
            .field("child_env", &self.child_env.iter().map(|(key, _)| key.as_str()).collect::<Vec<_>>())
            .field("child_cwd", &self.child_cwd)
            .finish()
    }
}

impl BridgeEndpoint {
    /// Fresh endpoint with a random name and token. `Err` when the system
    /// random source is unavailable.
    pub fn new_local(allowed_sid: Option<String>) -> Result<Self, String> {
        let name = random_hex(16)?;
        let token = random_hex(16)?;
        let address = if cfg!(windows) {
            format!(r"\\.\pipe\fox-acp-{name}")
        } else {
            std::env::temp_dir().join(format!("fox-acp-{name}.sock")).to_string_lossy().into_owned()
        };
        Ok(Self { address, token, allowed_sid, connect_timeout: BRIDGE_CONNECT_TIMEOUT, child_env: Vec::new(), child_cwd: None })
    }

    /// The handshake answer Fox sends after a valid token:
    /// `OK {"cwd":..,"env":{..}}\n` (compact JSON, so one line; keys in a
    /// fixed order). The bridge applies `env` over its own environment and
    /// starts the backend in `cwd`.
    pub fn handshake_answer(&self) -> Vec<u8> {
        let env: serde_json::Map<String, serde_json::Value> = self
            .child_env
            .iter()
            .map(|(key, value)| (key.clone(), serde_json::Value::String(value.clone())))
            .collect();
        let mut payload = serde_json::Map::new();
        if let Some(cwd) = &self.child_cwd {
            payload.insert("cwd".into(), serde_json::Value::String(cwd.clone()));
        }
        payload.insert("env".into(), serde_json::Value::Object(env));
        let mut line = b"OK ".to_vec();
        line.extend(serde_json::to_vec(&serde_json::Value::Object(payload)).expect("bridge answer serializes"));
        line.push(b'\n');
        line
    }

    /// The two variables the bridge reads.
    pub fn env(&self) -> Vec<(String, String)> {
        vec![(BRIDGE_PIPE_ENV.to_owned(), self.address.clone()), (BRIDGE_TOKEN_ENV.to_owned(), self.token.clone())]
    }
}

/// `bytes` random bytes from the OS as lowercase hex.
pub fn random_hex(bytes: usize) -> Result<String, String> {
    let mut buffer = vec![0u8; bytes];
    #[cfg(windows)]
    {
        use windows_sys::Win32::Security::Cryptography::{BCryptGenRandom, BCRYPT_USE_SYSTEM_PREFERRED_RNG};
        let status = unsafe { BCryptGenRandom(std::ptr::null_mut(), buffer.as_mut_ptr(), buffer.len() as u32, BCRYPT_USE_SYSTEM_PREFERRED_RNG) };
        if status != 0 {
            return Err(format!("BCryptGenRandom failed (NTSTATUS {status:#x})"));
        }
    }
    #[cfg(not(windows))]
    {
        use std::io::Read;
        std::fs::File::open("/dev/urandom")
            .and_then(|mut file| file.read_exact(&mut buffer))
            .map_err(|error| format!("cannot read /dev/urandom: {error}"))?;
    }
    Ok(buffer.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// `path` as handed to a backend (ACP `cwd`, process cwd, bridge cwd, path
/// values in its environment): a Windows extended-length path
/// (`\\?\C:\x`, `\\?\UNC\server\share\x`, what `std::fs::canonicalize`
/// returns) becomes the ordinary form (`C:\x`, `\\server\share\x`) when that
/// is the same file; otherwise it is returned unchanged. Node-based backends
/// mis-handle the verbatim prefix (pi recorded `cwd = C:\Windows`, dsh failed
/// with `realpath 'D:' EISDIR`). The Host keeps comparing and auditing the
/// canonical form; only what crosses into a backend is simplified. Same
/// rules as `dunce::simplified`, as a pure string function so every OS
/// tests the Windows forms.
pub fn backend_visible_path(path: &std::path::Path) -> std::path::PathBuf {
    match path.to_str().and_then(simplify_verbatim_path) {
        Some(simple) => std::path::PathBuf::from(simple),
        None => path.to_path_buf(),
    }
}

/// [`backend_visible_path`] for text (an environment value): `Some` only
/// when `text` is a verbatim path that can be simplified.
pub fn simplify_verbatim_path(text: &str) -> Option<String> {
    const VERBATIM: &str = r"\\?\";
    let rest = text.strip_prefix(VERBATIM)?;
    let (prefix, tail) = if let Some(unc) = rest.strip_prefix("UNC\\").or_else(|| rest.strip_prefix("unc\\")) {
        // \\?\UNC\server\share[\rest]
        let mut parts = unc.splitn(3, '\\');
        let server = parts.next().filter(|part| !part.is_empty())?;
        let share = parts.next().filter(|part| !part.is_empty())?;
        if !plain_component(server) || !plain_component(share) {
            return None;
        }
        (format!(r"\\{server}\{share}"), parts.next().unwrap_or(""))
    } else {
        // \\?\X:[\rest]
        let bytes = rest.as_bytes();
        if bytes.len() < 2 || !bytes[0].is_ascii_alphabetic() || bytes[1] != b':' {
            return None;
        }
        let tail = &rest[2..];
        if !tail.is_empty() && !tail.starts_with('\\') {
            return None;
        }
        (rest[..2].to_owned(), tail.strip_prefix('\\').unwrap_or(""))
    };
    let mut out = prefix;
    let components: Vec<&str> = tail.split('\\').collect();
    let trailing_separator = tail.ends_with('\\');
    for (index, component) in components.iter().enumerate() {
        if component.is_empty() {
            // Only a single trailing separator is ordinary ("C:\x\").
            if index + 1 == components.len() && (trailing_separator || tail.is_empty()) {
                continue;
            }
            return None;
        }
        if !plain_component(component) {
            return None;
        }
        out.push('\\');
        out.push_str(component);
    }
    if out.len() == 2 {
        // `\\?\D:` / `\\?\D:\` -> `D:\` (never the drive-relative `D:`).
        out.push('\\');
    }
    // Ordinary paths are limited to MAX_PATH; longer ones need the prefix.
    if out.encode_utf16().count() >= 260 {
        return None;
    }
    Some(out)
}

/// A path component that means the same with and without `\\?\`.
fn plain_component(component: &str) -> bool {
    if component == "." || component == ".." || component.ends_with('.') || component.ends_with(' ') {
        return false;
    }
    if component.chars().any(|c| matches!(c, '<' | '>' | ':' | '"' | '/' | '|' | '?' | '*') || (c as u32) < 32) {
        return false;
    }
    let stem = component.split('.').next().unwrap_or("").trim_end().to_ascii_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$")
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && matches!(stem.as_bytes()[3], b'0'..=b'9' | 0xB9 | 0xB2 | 0xB3));
    !reserved
}

/// Constant-time equality for tokens (no early exit on the first mismatch).
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    let mut diff = (a.len() ^ b.len()) as u8 | ((a.len() ^ b.len()) >> 8) as u8;
    for index in 0..a.len().max(b.len()) {
        let x = a.get(index).copied().unwrap_or(0);
        let y = b.get(index).copied().unwrap_or(0);
        diff |= x ^ y;
    }
    diff == 0 && a.len() == b.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verbatim_windows_paths_are_simplified_for_backends() {
        let cases = [
            (r"\\?\C:\Users\me\proj", Some(r"C:\Users\me\proj")),
            (r"\\?\D:\", Some(r"D:\")),
            (r"\\?\D:", Some(r"D:\")),
            (r"\\?\c:\a\b\", Some(r"c:\a\b")),
            (r"\\?\C:\中文 目录\x.txt", Some(r"C:\中文 目录\x.txt")),
            (r"\\?\UNC\server\share\dir\f", Some(r"\\server\share\dir\f")),
            (r"\\?\UNC\server\share", Some(r"\\server\share")),
            (r"\\?\UNC\server\share\", Some(r"\\server\share")),
            // Not simplifiable: would change meaning or exceed MAX_PATH.
            (r"\\?\C:\a\..\b", None),
            (r"\\?\C:\a\.\b", None),
            (r"\\?\C:\a\NUL", None),
            (r"\\?\C:\a\com1.txt", None),
            (r"\\?\C:\a\trailing.", None),
            (r"\\?\C:\a\trailing ", None),
            (r"\\?\C:\a\\b", None),
            (r"\\?\C:\a/b", None),
            (r"\\?\Volume{0b5c1d3e-0000-0000-0000-100000000000}\x", None),
            (r"\\?\UNC\server", None),
            (r"\\?\C:x", None),
            // Already ordinary, or not a path at all: left alone.
            (r"C:\Users\me", None),
            (r"\\server\share", None),
            ("/home/me/proj", None),
            (r#"{"web_search":"disabled"}"#, None),
        ];
        for (input, expected) in cases {
            assert_eq!(simplify_verbatim_path(input).as_deref(), expected, "{input}");
        }
        let long = format!(r"\\?\C:\{}", "a".repeat(300));
        assert_eq!(simplify_verbatim_path(&long), None, "over MAX_PATH keeps the prefix");
        assert_eq!(backend_visible_path(std::path::Path::new(r"\\?\C:\w")), std::path::PathBuf::from(r"C:\w"));
        assert_eq!(backend_visible_path(std::path::Path::new("/w/x")), std::path::PathBuf::from("/w/x"));
    }

    #[test]
    fn bridge_answer_carries_env_and_cwd_as_one_json_line() {
        let mut endpoint = BridgeEndpoint::new_local(None).unwrap();
        assert_eq!(endpoint.handshake_answer(), b"OK {\"env\":{}}\n".to_vec());
        endpoint.child_cwd = Some(r"C:\Users\me\proj".into());
        endpoint.child_env = vec![
            ("CODEX_CONFIG".into(), r#"{"web_search":"disabled"}"#.into()),
            ("PCT".into(), "100%DONE%".into()),
            ("MULTI".into(), "line1\r\nline2".into()),
            ("OPENAI_API_KEY".into(), "sk-test".into()),
        ];
        let answer = endpoint.handshake_answer();
        assert!(answer.starts_with(b"OK {"));
        assert_eq!(answer.iter().filter(|byte| **byte == b'\n').count(), 1, "exactly one line");
        assert_eq!(answer.last(), Some(&b'\n'));
        let payload: serde_json::Value = serde_json::from_slice(&answer[3..answer.len() - 1]).unwrap();
        assert_eq!(payload["cwd"], r"C:\Users\me\proj");
        assert_eq!(payload["env"]["CODEX_CONFIG"], r#"{"web_search":"disabled"}"#);
        assert_eq!(payload["env"]["PCT"], "100%DONE%");
        assert_eq!(payload["env"]["MULTI"], "line1\r\nline2");
        assert_eq!(payload["env"]["OPENAI_API_KEY"], "sk-test");
        let debug = format!("{endpoint:?}");
        assert!(!debug.contains("sk-test") && !debug.contains(&endpoint.token), "no values or token in Debug");
    }

    #[test]
    fn external_backends_launch_pinned_acp_entry_points() {
        assert!(launch_spec(AgentBackend::Fox).is_none());
        let codex = launch_spec(AgentBackend::Codex).unwrap();
        assert!(codex.args.iter().any(|arg| arg == "@agentclientprotocol/codex-acp@2.2.2"));
        let pi = launch_spec(AgentBackend::Pi).unwrap();
        assert!(pi.args.iter().any(|arg| arg == "pi-acp@0.0.34"));
        let deepseek = launch_spec(AgentBackend::Deepseek).unwrap();
        assert_eq!(deepseek.args, ["--profile", "acp"]);
        assert!(deepseek.experimental);
    }

    #[test]
    fn dev_entry_prefers_override_then_managed_runtime_then_npx() {
        let windows = cfg!(windows);
        let root = std::env::temp_dir().join(format!("fox-dev-entry-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(managed_entry(&root, AgentBackend::Pi, windows), None);
        let bin = root.join("backends/pi/node_modules/.bin");
        std::fs::create_dir_all(&bin).unwrap();
        let entry = bin.join(if windows { "pi-acp.cmd" } else { "pi-acp" });
        std::fs::write(&entry, "").unwrap();
        assert_eq!(managed_entry(&root, AgentBackend::Pi, windows), Some(entry));
        assert_eq!(managed_entry(&root, AgentBackend::Codex, windows), None);
        assert_eq!(managed_entry(&root, AgentBackend::Fox, windows), None);
        let _ = std::fs::remove_dir_all(&root);
        // Without any override or runtime the tests fall back to npx (the
        // process environment is not modified here; skip when it is set).
        let clean = ["FOX_PI_ACP_PROGRAM", "FOX_AGENT_RUNTIME_ROOT", "FOX_RUNTIME_ROOT", "FOX_RUNTIME_VERSION"].iter().all(|name| std::env::var_os(name).is_none());
        if clean && !cfg!(windows) {
            let (spec, source) = dev_launch_spec(AgentBackend::Pi).unwrap();
            assert_eq!(source, DevEntrySource::Npx);
            assert!(spec.args.iter().any(|arg| arg == "pi-acp@0.0.34"));
            let (spec, source) = dev_launch_spec(AgentBackend::Deepseek).unwrap();
            assert_eq!(source, DevEntrySource::Npx);
            assert_eq!(spec.args, ["-y", "@deepseek-ai/dsh@0.2.1-rc.2", "--profile", "acp"]);
        }
    }

    #[test]
    fn child_environment_keeps_only_allowlist_and_named_credentials() {
        let spec = launch_spec(AgentBackend::Deepseek).unwrap();
        let parent = vec![
            ("PATH".to_string(), "/bin".to_string()),
            ("AWS_SECRET_ACCESS_KEY".to_string(), "x".to_string()),
            ("OPENAI_API_KEY".to_string(), "y".to_string()),
            ("DEEPSEEK_API_KEY".to_string(), "z".to_string()),
            ("NODE_OPTIONS".to_string(), "--require evil".to_string()),
        ];
        let env = child_environment(&spec, parent);
        let keys: Vec<_> = env.iter().map(|(key, _)| key.as_str()).collect();
        assert_eq!(keys, ["PATH", "DEEPSEEK_API_KEY"]);
    }

    #[test]
    fn process_environment_is_allowlist_plus_session_vars() {
        let mut spec = launch_spec(AgentBackend::Pi).unwrap();
        spec.session_env = vec![("PATH".into(), "/fox".into()), ("X_FOX".into(), "1".into())];
        let env = process_environment(&spec, vec![("PATH".into(), "/bin".into()), ("SECRET".into(), "s".into())]);
        assert_eq!(env, vec![("PATH".to_string(), "/fox".to_string()), ("X_FOX".to_string(), "1".to_string())]);
    }

    #[test]
    fn bridge_endpoint_is_random_and_never_prints_its_token() {
        let a = BridgeEndpoint::new_local(None).unwrap();
        let b = BridgeEndpoint::new_local(None).unwrap();
        assert_eq!(a.token.len(), 32);
        assert!(a.token.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a.token, b.token);
        assert_ne!(a.address, b.address);
        assert!(a.address.contains("fox-acp-"));
        assert!(!format!("{a:?}").contains(&a.token));
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"abcd"));
        assert!(!constant_time_eq(b"", b"a"));
    }
}
