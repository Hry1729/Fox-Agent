//! Process launch specs for the external backends (plan 6.1, 6.7). Every
//! external backend is reached over ACP v1 through an existing entry point;
//! Fox does not implement per-vendor protocols.

use crate::port::AgentBackend;

/// Parent environment variables a backend process may inherit. Mirrors the
/// Host's `command_environment` allowlist; everything else (cloud keys,
/// tokens, NODE_OPTIONS, proxies) is dropped unless named per backend in
/// [`LaunchSpec::credential_env`].
#[cfg(windows)]
pub const ENV_ALLOWLIST: &[&str] = &[
    "PATH", "PATHEXT", "SystemRoot", "WINDIR", "COMSPEC", "TEMP", "TMP",
    "USERPROFILE", "HOMEDRIVE", "HOMEPATH", "LOCALAPPDATA", "APPDATA",
    "PROGRAMDATA", "PROGRAMFILES", "PROGRAMFILES(X86)", "PROGRAMW6432",
    "USERNAME", "USERDOMAIN", "NUMBER_OF_PROCESSORS", "PROCESSOR_ARCHITECTURE",
    "OS", "LANG", "LC_ALL", "LC_CTYPE", "TERM",
];

#[cfg(not(windows))]
pub const ENV_ALLOWLIST: &[&str] = &[
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

/// `None` for FoxBackend, which runs in-process through the existing Kernel.
pub fn launch_spec(backend: AgentBackend) -> Option<LaunchSpec> {
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
    let allowed = |key: &str| {
        ENV_ALLOWLIST
            .iter()
            .chain(spec.credential_env.iter())
            .any(|name| if cfg!(windows) { name.eq_ignore_ascii_case(key) } else { *name == key })
    };
    parent.into_iter().filter(|(key, _)| allowed(key)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
