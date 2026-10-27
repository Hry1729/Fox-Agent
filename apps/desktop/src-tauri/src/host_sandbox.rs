//! SRT boundary for the Fox Agent's own Host tools.
//!
//! External Agents (pi / Codex / DeepSeek) are deliberately NOT wrapped here:
//! they keep their native sandboxes, permission modes and approvals. This
//! module is the one place that wraps a Fox Agent `run_command` in Fox's
//! managed SRT runtime.
//!
//! The command itself (including every child it starts) executes as the
//! dedicated SRT sandbox account on Windows, or inside bwrap / seatbelt on
//! Unix. File tools do not spawn a process, so they cannot inherit this
//! boundary: their Host-side path checks in `tool_host.rs` remain the
//! authority, and the write target must still resolve inside the authorized
//! project root.
//!
//! No silent fallback: if the runtime, SRT package, preflight or policy is
//! unavailable, `begin` returns the structured refusal and no process starts.

use crate::process_jobs::{BackendAvailability, BackendCapabilityProof, BackendRefusal, ExecutionBackend, SpawnSpec};
use fox_agent_backend::external_launch::HostLaunchConfig;
use fox_agent_backend::sandbox::{
    canonical_managed_path, ensure_backend_home_dirs, preflight, read_srt_sandbox_sid, AuthorizedDir,
    HostOs, PrivateHome, SandboxDeclaration, SandboxError, SandboxErrorCode, SandboxPolicy, SrtLauncher,
};
use fox_agent_backend::winacl::RealWindowsAcl;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Source revision of the Host command boundary. Changing how a command is
/// wrapped must change this digest; the execution admission compares it with
/// the digest it recorded before approving the call.
pub const HOST_SANDBOX_VERSION: &str = "fox-host-srt-v1";

pub struct HostSandbox {
    launcher: SrtLauncher,
    config: HostLaunchConfig,
    srt_sid: String,
}

/// One prepared command. Dropping it removes the per-command settings/temp
/// directory and releases the Windows ancestor-ACE reference.
pub struct HostSandboxSession {
    pub spec: SpawnSpec,
    pub declaration: SandboxDeclaration,
    settings_dir: PathBuf,
    session_tmp: PathBuf,
    cleanup: Option<WindowsAncestorSession>,
}

struct WindowsAncestorSession {
    coordinator: Arc<fox_agent_backend::winancestor::AncestorAceCoordinator>,
    session_id: String,
}

impl HostSandbox {
    /// Discover the production Fox/SRT installation without changing it.
    pub fn discover() -> Result<Self, SandboxError> {
        #[cfg(test)]
        if std::env::var("FOX_TEST_REAL_SRT").ok().as_deref() != Some("1") {
            return Err(SandboxError::new(
                SandboxErrorCode::SandboxUnavailable,
                "real SRT execution is disabled in unit tests; set FOX_TEST_REAL_SRT=1 for the explicit real-process check",
            ));
        }
        let fox_data = fox_data_dir().ok_or_else(|| {
            SandboxError::new(SandboxErrorCode::SandboxUnavailable, "cannot determine the Fox data directory")
        })?;
        let config = HostLaunchConfig::from_env(&fox_data);
        let install = config.srt_install().ok_or_else(|| {
            SandboxError::new(
                SandboxErrorCode::SandboxDependencyMissing,
                format!("Node is not installed in {}", config.runtime_root.display()),
            )
            .remedy("安装 Fox 受管运行时（含 Node 与 SRT）")
        })?;
        let ready = preflight(&install, &fox_agent_backend::sandbox::RealProbe)?;
        let srt_sid = if config.os == HostOs::Windows {
            read_srt_sandbox_sid(&fox_agent_backend::sandbox::RealProbe).ok_or_else(|| {
                SandboxError::new(SandboxErrorCode::SandboxNotInstalled, "the Windows SRT sandbox account SID is missing")
                    .remedy("在设置中启用一次 Fox 沙箱")
            })?
        } else {
            String::new()
        };
        let launcher = SrtLauncher::new(ready, config.state_root.clone())
            .with_managed_runtime_root(config.runtime_root.clone())
            .with_srt_sandbox_sid(srt_sid.clone());
        Ok(Self { launcher, config, srt_sid })
    }

    /// Digest of the exact boundary this process would use.
    pub fn digest(&self) -> String {
        format!(
            "{HOST_SANDBOX_VERSION}:{}:{}:{}",
            self.launcher.ready.install.node.display(),
            self.launcher.ready.install.cli_js.display(),
            self.config.runtime_root.display()
        )
    }

    /// Prepare one command. `session_id` is supplied by the durable command
    /// job or a sync call and is never model-controlled.
    pub fn begin(
        &self,
        command: &str,
        cwd: &Path,
        project_root: &Path,
        session_id: &str,
    ) -> Result<HostSandboxSession, SandboxError> {
        let os = self.config.os;
        // Rust's canonicalize returns a verbatim `\\?\` path on Windows;
        // cmd.exe and srt-win need the ordinary drive form.
        let cwd = fox_agent_backend::sandbox::canonical_existing_path(cwd)?;
        let project_root = fox_agent_backend::sandbox::canonical_existing_path(project_root)?;
        let session_id = safe_session_id(session_id)?;
        let settings_dir = self.config.state_root.join("host-sandbox").join(&session_id);
        let tmp_root = self
            .launcher
            .session_tmp(&session_id)
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.config.state_root.join("host-tmp"));
        let session_tmp = tmp_root.join(&session_id);
        std::fs::create_dir_all(&settings_dir)
            .map_err(|error| io_error(format!("cannot create {}: {error}", settings_dir.display())))?;
        std::fs::create_dir_all(&session_tmp)
            .map_err(|error| io_error(format!("cannot create {}: {error}", session_tmp.display())))?;

        let (private_home, private_home_root) = private_home_for(&self.config, &project_root)?;
        ensure_backend_home_dirs(fox_agent_backend::port::AgentBackend::Fox, &private_home)
            .map_err(|error| io_error(format!("cannot create private HOME {}: {error}", private_home.display())))?;

        let mut policy = SandboxPolicy::new(
            vec![AuthorizedDir { path: project_root.clone(), writable: true }],
            PrivateHome { root: private_home_root, path: private_home },
            Vec::new(),
            Vec::new(),
            os,
        );
        policy.session_tmp = Some(session_tmp.clone());
        policy = self.launcher.effective_policy(&policy);
        policy.session_tmp = Some(session_tmp.clone());
        if let Some(tmp) = policy.session_tmp.clone() {
            policy.session_tmp = Some(canonical_managed_path(&tmp)?);
        }
        policy = policy.canonicalized()?;
        policy.validate(os)?;
        let plan = if os == HostOs::Windows { Some(self.launcher.windows_ace_plan(&policy)?) } else { None };
        let settings = serde_json::to_vec_pretty(&policy.to_srt_settings()).expect("settings serialize");
        let settings_path = settings_dir.join("srt-settings.json");
        write_private(&settings_path, &settings).map_err(|error| io_error(format!("cannot write {}: {error}", settings_path.display())))?;

        let script = if os == HostOs::Windows { session_tmp.join("command.cmd") } else { session_tmp.join("command.sh") };
        let body = command_script(command, &cwd, os)?;
        std::fs::write(&script, body).map_err(|error| io_error(format!("cannot write {}: {error}", script.display())))?;

        let mut args = vec![
            self.launcher.ready.install.cli_js.to_string_lossy().into_owned(),
            "--settings".to_owned(),
            settings_path.to_string_lossy().into_owned(),
        ];
        if os == HostOs::Windows {
            args.push("-c".to_owned());
            args.push(format!("cmd.exe /D /S /C call \"{}\"", script.display()));
        } else {
            args.push("--".to_owned());
            args.push("/bin/sh".to_owned());
            args.push(script.to_string_lossy().into_owned());
        }
        let spec = SpawnSpec {
            program: self.launcher.ready.install.node.to_string_lossy().into_owned(),
            args,
            cwd: cwd.to_string_lossy().into_owned(),
            env: crate::process_jobs::EnvMode::Inherit,
        };
        let declaration = SandboxDeclaration::declare(
            fox_agent_backend::sandbox::PermissionMode::Sandboxed,
            Some(&policy),
            os,
        );

        let cleanup = if os == HostOs::Windows {
            let paths = plan.map(|p| p.ancestors).unwrap_or_default();
            let sid = self.srt_sid.clone();
            let acl = Arc::new(RealWindowsAcl);
            let coordinator = Arc::new(
                fox_agent_backend::winancestor::AncestorAceCoordinator::new(
                    self.config.state_root.clone(),
                    acl.clone(),
                    acl,
                )
                .with_srt_acl_lock(fox_agent_backend::winancestor::srt_acl_lock_path()),
            );
            let mut transition = coordinator.transition()?;
            if let Err(error) = transition.begin_session(&session_id, &sid, &paths) {
                let _ = transition.end_session(&session_id);
                transition.finish();
                return Err(error);
            }
            transition.finish();
            Some(WindowsAncestorSession { coordinator, session_id: session_id.clone() })
        } else {
            None
        };

        Ok(HostSandboxSession {
            spec,
            declaration,
            settings_dir,
            session_tmp,
            cleanup,
        })
    }
}

impl Drop for HostSandboxSession {
    fn drop(&mut self) {
        if let Some(ancestor) = self.cleanup.take() {
            if let Ok(mut transition) = ancestor.coordinator.transition() {
                let _ = transition.end_session(&ancestor.session_id);
                transition.finish();
            }
        }
        let _ = std::fs::remove_dir_all(&self.settings_dir);
        let _ = std::fs::remove_dir_all(&self.session_tmp);
    }
}

/// The production capability proof. It reports the real SRT readiness and
/// digest; when SRT is missing the Host refuses to start a process instead of
/// running it unconfined.
pub struct SrtHostBackend;

impl BackendCapabilityProof for SrtHostBackend {
    fn availability(&self) -> BackendAvailability {
        #[cfg(test)]
        if std::env::var("FOX_TEST_REAL_SRT").ok().as_deref() != Some("1") {
            return BackendAvailability::Unavailable(BackendRefusal::NoVerifiedBackend);
        }
        match HostSandbox::discover() {
            Ok(sandbox) => BackendAvailability::Available {
                backend: ExecutionBackend::SrtHost,
                digest: sandbox.digest(),
            },
            Err(_) => BackendAvailability::Unavailable(BackendRefusal::NoVerifiedBackend),
        }
    }
}

/// Detailed availability for the tool catalog and the pre-dispatch check.
pub fn availability() -> Result<String, SandboxError> {
    let sandbox = HostSandbox::discover()?;
    Ok(sandbox.digest())
}

fn private_home_for(config: &HostLaunchConfig, project_root: &Path) -> Result<(PathBuf, PathBuf), SandboxError> {
    let key = project_key(project_root);
    if config.os == HostOs::Windows {
        let layout = fox_agent_backend::winruntime::WindowsPrivateLayout::for_current_user(&RealWindowsAcl)?;
        let known = fox_agent_backend::winruntime::known_srt_sids(&config.state_root)?;
        fox_agent_backend::winruntime::prepare_windows_private_root(
            &layout,
            &RealWindowsAcl,
            &RealWindowsAcl,
            &known,
        )?;
        let root = layout.homes_root();
        Ok((root.join("fox-host").join(&key), root))
    } else {
        let root = config.fox_data.join("host-homes");
        Ok((root.join(&key), root))
    }
}

fn project_key(project_root: &Path) -> String {
    use sha2::{Digest, Sha256};
    let mut hash = Sha256::new();
    hash.update(project_root.to_string_lossy().replace('\\', "/").to_ascii_lowercase().as_bytes());
    format!("{:x}", hash.finalize())[..32].to_owned()
}

fn command_script(command: &str, cwd: &Path, os: HostOs) -> Result<String, SandboxError> {
    let cwd = cwd.to_string_lossy();
    if cwd.contains(['"', '\r', '\n']) && os == HostOs::Windows {
        return Err(SandboxError::new(SandboxErrorCode::SandboxPolicyInvalid, "command working directory cannot be represented in a batch file"));
    }
    Ok(match os {
        HostOs::Windows => format!("@echo off\r\ncd /d \"{cwd}\"\r\n{command}\r\nexit /b %ERRORLEVEL%\r\n"),
        _ => format!("#!/bin/sh\ncd -- '{}'\n{}\n", cwd.replace('\'', "'\\''"), command),
    })
}

fn safe_session_id(session_id: &str) -> Result<String, SandboxError> {
    let cleaned: String = session_id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .take(96)
        .collect();
    if cleaned.is_empty() {
        return Err(SandboxError::new(SandboxErrorCode::SandboxPolicyInvalid, "empty command session id"));
    }
    Ok(cleaned)
}

fn fox_data_dir() -> Option<PathBuf> {
    if let Some(value) = std::env::var_os("FOX_DATA_DIR").filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(value));
    }
    if let Some(value) = std::env::var_os("LOCALAPPDATA").filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(value).join("Fox"));
    }
    std::env::var_os("HOME").filter(|value| !value.is_empty()).map(|value| PathBuf::from(value).join(".fox"))
}

fn io_error(message: String) -> SandboxError {
    SandboxError::new(SandboxErrorCode::SandboxUnavailable, message)
}

fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(path)?;
        file.write_all(bytes)
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, bytes)
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    fn real_srt_enabled() -> bool {
        std::env::var("FOX_TEST_REAL_SRT").ok().as_deref() == Some("1")
    }

    #[test]
    fn unit_tests_do_not_silently_fall_back_to_the_host() {
        if real_srt_enabled() {
            return;
        }
        let error = match HostSandbox::discover() {
            Ok(_) => panic!("unit tests must not use the real SRT runtime"),
            Err(error) => error,
        };
        assert_eq!(error.code, SandboxErrorCode::SandboxUnavailable);
        assert!(error.message.contains("FOX_TEST_REAL_SRT"));
    }

    /// Explicit real-process check. It creates a throw-away project and lets
    /// SRT add its normal per-session ACLs there; it does not touch the SRT
    /// account, the installed runtime or any user profile ACL.
    #[cfg(windows)]
    #[test]
    fn real_srt_host_command_runs_as_the_sandbox_account_and_blocks_parent_write() {
        if !real_srt_enabled() {
            eprintln!("skipped: set FOX_TEST_REAL_SRT=1 to run the real SRT process check");
            return;
        }
        let root = std::env::temp_dir().join(format!("fox-host-srt-real-{}", std::process::id()));
        let outside_name = format!("fox-host-srt-outside-{}.txt", std::process::id());
        let outside = root.parent().unwrap().join(&outside_name);
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_file(&outside);
        std::fs::create_dir_all(&root).unwrap();
        let sandbox = HostSandbox::discover().expect("the installed Fox SRT runtime");
        let command = format!("whoami > whoami.txt & echo outside > \"..\\{outside_name}\"");
        let wrapped = sandbox
            .begin(&command, &root, &root, "real-srt-check")
            .expect("the Host command policy");
        let mut command = std::process::Command::new(&wrapped.spec.program);
        command.args(&wrapped.spec.args).current_dir(&wrapped.spec.cwd);
        let output = command.output().expect("spawn srt");
        let status = output.status;
        let stderr = String::from_utf8_lossy(&output.stderr);
        drop(wrapped);
        assert!(status.success(), "srt exited {status:?}: {stderr}");
        let who = std::fs::read_to_string(root.join("whoami.txt")).expect("whoami output");
        assert!(who.to_ascii_lowercase().contains("srt-sandbox"), "not the sandbox account: {who:?}");
        assert!(!outside.exists(), "a write outside the authorized project must not reach the Host: {}", outside.display());
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_file(&outside);
    }

    /// The declaration is generated from the actual policy, and the only
    /// writable roots are the project and Fox's private session state.
    #[test]
    fn declaration_cannot_widen_a_command_outside_the_project() {
        let root = std::env::temp_dir().join(format!("fox-host-policy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let policy = SandboxPolicy::new(
            vec![AuthorizedDir { path: root.clone(), writable: true }],
            PrivateHome { root: root.join("homes"), path: root.join("homes").join("fox") },
            vec![],
            vec![],
            HostOs::Windows,
        );
        let declaration = SandboxDeclaration::declare(fox_agent_backend::sandbox::PermissionMode::Sandboxed, Some(&policy), HostOs::Windows);
        assert_eq!(declaration.mechanism.as_deref(), Some("srt"));
        assert!(declaration.writable_roots.iter().any(|path| Path::new(path) == root));
        assert!(!declaration.writable_roots.iter().any(|path| path.to_ascii_lowercase().ends_with("windows")));
        let _ = std::fs::remove_dir_all(&root);
    }
}
