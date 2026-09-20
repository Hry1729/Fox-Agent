use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io;
use std::process::Command;

#[cfg(windows)]
const ALLOWED_PARENT_ENV: &[&str] = &[
    "PATH", "PATHEXT", "SystemRoot", "WINDIR", "COMSPEC", "TEMP", "TMP",
    "USERPROFILE", "HOMEDRIVE", "HOMEPATH", "LOCALAPPDATA", "APPDATA",
    "PROGRAMDATA", "PROGRAMFILES", "PROGRAMFILES(X86)", "PROGRAMW6432",
    "USERNAME", "USERDOMAIN", "NUMBER_OF_PROCESSORS", "PROCESSOR_ARCHITECTURE",
    "OS", "LANG", "LC_ALL", "LC_CTYPE", "TERM", "CARGO_HOME", "RUSTUP_HOME",
    "VCPKG_ROOT", "VSINSTALLDIR", "VCINSTALLDIR", "WindowsSdkDir", "INCLUDE",
    "LIB", "LIBPATH",
];

#[cfg(not(windows))]
const ALLOWED_PARENT_ENV: &[&str] = &[
    "PATH", "HOME", "USER", "LOGNAME", "SHELL", "TMPDIR", "TEMP", "TMP",
    "LANG", "LC_ALL", "LC_CTYPE", "TERM", "XDG_CACHE_HOME", "XDG_CONFIG_HOME",
    "CARGO_HOME", "RUSTUP_HOME", "LD_LIBRARY_PATH", "DYLD_LIBRARY_PATH",
    "PKG_CONFIG_PATH",
];

fn canonical_allowed_name(name: &str) -> Option<&'static str> {
    ALLOWED_PARENT_ENV
        .iter()
        .copied()
        .find(|allowed| allowed.eq_ignore_ascii_case(name))
}

pub fn inherited_environment() -> Vec<(OsString, OsString)> {
    let parent: BTreeMap<String, (OsString, OsString)> = std::env::vars_os()
        .map(|(key, value)| (key.to_string_lossy().to_ascii_uppercase(), (key, value)))
        .collect();
    ALLOWED_PARENT_ENV
        .iter()
        .filter_map(|name| parent.get(&name.to_ascii_uppercase()).cloned())
        .collect()
}

pub fn validate_explicit_environment(entries: &[(String, String)]) -> io::Result<()> {
    let mut seen = BTreeMap::<String, ()>::new();
    for (key, value) in entries {
        if key.is_empty()
            || key.contains('=')
            || key.contains('\0')
            || value.contains('\0')
            || !key.bytes().all(|byte| byte == b'_' || byte.is_ascii_alphanumeric())
        {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid child environment entry"));
        }
        let folded = key.to_ascii_uppercase();
        if seen.insert(folded.clone(), ()).is_some() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "duplicate child environment key"));
        }
        let sensitive = ["TOKEN", "SECRET", "PASSWORD", "PASSWD", "API_KEY", "APIKEY", "CREDENTIAL", "COOKIE", "AUTH"];
        if sensitive.iter().any(|marker| folded.contains(marker)) {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, "sensitive child environment keys are not permitted"));
        }
        // Explicit values may add ordinary application configuration. Names
        // inherited from the parent are canonicalized to avoid Windows aliases.
        let _ = canonical_allowed_name(key);
    }
    Ok(())
}

pub fn apply_inherited(command: &mut Command) {
    command.env_clear();
    command.envs(inherited_environment());
}

pub fn apply_explicit(command: &mut Command, entries: &[(String, String)]) -> io::Result<()> {
    validate_explicit_environment(entries)?;
    command.env_clear();
    command.envs(entries.iter().map(|(key, value)| (key, value)));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parent_environment_excludes_secret_shaped_canaries() {
        std::env::set_var("FOX_TEST_SECRET_TOKEN", "must-not-cross");
        let environment = inherited_environment();
        std::env::remove_var("FOX_TEST_SECRET_TOKEN");
        assert!(!environment.iter().any(|(key, _)| key.to_string_lossy().eq_ignore_ascii_case("FOX_TEST_SECRET_TOKEN")));
        assert!(environment.iter().any(|(key, _)| key.to_string_lossy().eq_ignore_ascii_case("PATH")));
    }

    #[test]
    fn explicit_environment_rejects_secrets_duplicates_and_invalid_names() {
        assert!(validate_explicit_environment(&[("API_TOKEN".into(), "x".into())]).is_err());
        assert!(validate_explicit_environment(&[("A".into(), "1".into()), ("a".into(), "2".into())]).is_err());
        assert!(validate_explicit_environment(&[("A=B".into(), "1".into())]).is_err());
        assert!(validate_explicit_environment(&[("MODE".into(), "safe".into())]).is_ok());
    }
}
