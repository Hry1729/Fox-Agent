//! Windows ACL queries for the srt sandbox (docs/sandbox.md section 7).
//!
//! srt 0.0.80 on Windows runs the backend as the dedicated local account
//! `srt-sandbox`. That account has no rights on the calling user's own files
//! (a profile under `C:\Users\<user>` grants only the user, SYSTEM and
//! Administrators), so srt reaches the paths it should read or write by adding
//! explicit ACEs for the sandbox SID: `srt-win acl grant` (allowRead /
//! allowWrite) and `srt-win acl stamp` (denyRead / denyWrite, plus a
//! `FILE_DELETE_CHILD` deny on the parent). Both end in
//! `SetNamedSecurityInfoW(DACL)` run as the **calling user, not elevated**
//! (vendor/srt-win-src/src/acl.rs `write_file_dacl`), which needs `WRITE_DAC`
//! on the object. A normal user has `WRITE_DAC` only where they own the object
//! (owner rights) or were granted it explicitly; not on `C:\Users`,
//! `C:\Program Files\nodejs`, a drive root and the like. That is the
//! `WIN32_ERROR=0x00000005` the Windows run hit, and why srt then rolled back
//! the whole batch.
//!
//! What this module offers:
//!
//! * [`WindowsAclProbe`]: the two questions the launcher asks before it hands
//!   paths to srt: may the current user change this DACL (`WRITE_DAC`)? and
//!   does this DACL open the path to everyone (Users / Authenticated Users /
//!   Everyone / INTERACTIVE)? The real implementation is [`RealWindowsAcl`];
//!   tests fake it.
//! * [`audit_broad_exposure`]: report-only scan of the fixed drives' roots and
//!   their first-level directories for DACLs that let every local account
//!   (the sandbox account included) read or write. Nothing is modified.
//!
//! Off Windows the real queries answer `Err` (fail closed) and the audit
//! reports `supported = false`.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Broad groups a DACL opens a path to. Every local account, the srt sandbox
/// account included, is a member of all four (the sandbox logon is
/// interactive, `CreateProcessWithLogonW` + `LOGON_WITH_PROFILE`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BroadAccess {
    /// Some broad group may read data (or list the directory).
    pub read: bool,
    /// Some broad group may write data (or add files).
    pub write: bool,
    /// The groups found, e.g. `["Authenticated Users", "Users"]`.
    pub groups: Vec<String>,
}

impl BroadAccess {
    pub fn any(&self) -> bool {
        self.read || self.write
    }
}

/// ACL questions the launcher asks on Windows. Faked in tests.
pub trait WindowsAclProbe: Send + Sync {
    /// `Ok` when the current user may change the DACL of `path` (`WRITE_DAC`),
    /// the right srt-win needs to add its ACE there. A path that does not exist
    /// yet is answered for its nearest existing ancestor (srt creates
    /// placeholders for missing deny targets as the same user).
    fn check_write_dac(&self, path: &Path) -> Result<(), String>;
    /// What the DACL of `path` (or its nearest existing ancestor) grants the
    /// broad groups. Allow ACEs only (deny ACEs are not subtracted, so this can
    /// over-report, never under-report); inherit-only ACEs are skipped.
    fn broad_access(&self, path: &Path) -> Result<BroadAccess, String>;
}

/// The real system (Windows API). Off Windows every query fails.
#[derive(Debug, Clone, Copy, Default)]
pub struct RealWindowsAcl;

impl WindowsAclProbe for RealWindowsAcl {
    fn check_write_dac(&self, path: &Path) -> Result<(), String> {
        imp::check_write_dac(&nearest_existing(path))
    }

    fn broad_access(&self, path: &Path) -> Result<BroadAccess, String> {
        imp::broad_access(&nearest_existing(path))
    }
}

fn nearest_existing(path: &Path) -> PathBuf {
    let mut current = path.to_path_buf();
    loop {
        if std::fs::symlink_metadata(&current).is_ok() {
            return current;
        }
        match current.parent() {
            Some(parent) if parent != current => current = parent.to_path_buf(),
            _ => return path.to_path_buf(),
        }
    }
}

/// One finding of [`audit_broad_exposure`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExposedPath {
    pub path: PathBuf,
    pub access: BroadAccess,
}

/// Result of [`audit_broad_exposure`]. Report only: nothing was changed.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExposureReport {
    /// False off Windows (nothing was scanned).
    pub supported: bool,
    /// Fixed drive roots scanned (`C:\`, `D:\` ...).
    pub drives: Vec<PathBuf>,
    /// Roots and first-level directories whose DACL lets every local account
    /// read or write. The sandbox account can reach these without any srt
    /// grant (subject to deeper DACLs, which are not scanned).
    pub exposed: Vec<ExposedPath>,
    /// Paths whose DACL could not be read (`path: reason`).
    pub errors: Vec<String>,
}

/// Directories under a drive root that Windows itself guards or that are
/// expected to be broadly readable system locations; listed in the report
/// like any other, but the UI may group them as "system".
pub const SYSTEM_FIRST_LEVEL: &[&str] =
    &["Windows", "Program Files", "Program Files (x86)", "ProgramData", "Users", "PerfLogs", "$Recycle.Bin", "System Volume Information"];

/// Scan every fixed drive root and its first-level directories and report the
/// ones whose DACL opens them to Users / Authenticated Users / Everyone /
/// INTERACTIVE (read or write). For the UI warning that a sandboxed backend on
/// Windows may read (and, where write is reported, change) these places.
/// Never modifies anything. Windows only; elsewhere `supported = false`.
pub fn audit_broad_exposure() -> ExposureReport {
    audit_with(&RealWindowsAcl, &imp::fixed_drive_roots())
}

/// [`audit_broad_exposure`] over given roots and probe (testable anywhere).
pub fn audit_with(probe: &dyn WindowsAclProbe, roots: &[PathBuf]) -> ExposureReport {
    let mut report = ExposureReport { supported: cfg!(windows), drives: roots.to_vec(), ..Default::default() };
    for root in roots {
        let mut candidates = vec![root.clone()];
        match std::fs::read_dir(root) {
            Ok(entries) => {
                let mut dirs: Vec<PathBuf> = entries
                    .filter_map(Result::ok)
                    .filter(|entry| entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false))
                    .map(|entry| entry.path())
                    .collect();
                dirs.sort();
                candidates.extend(dirs);
            }
            Err(error) => report.errors.push(format!("{}: {error}", root.display())),
        }
        for path in candidates {
            match probe.broad_access(&path) {
                Ok(access) if access.any() => report.exposed.push(ExposedPath { path, access }),
                Ok(_) => {}
                Err(error) => report.errors.push(format!("{}: {error}", path.display())),
            }
        }
    }
    report
}

#[cfg(windows)]
mod imp {
    use super::BroadAccess;
    use std::os::windows::ffi::OsStrExt;
    use std::path::{Path, PathBuf};
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, LocalFree, GENERIC_ALL, GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Security::Authorization::{GetNamedSecurityInfoW, SE_FILE_OBJECT};
    use windows_sys::Win32::Security::{
        AclSizeInformation, GetAce, GetAclInformation, IsWellKnownSid, WinAuthenticatedUserSid, WinBuiltinUsersSid,
        WinInteractiveSid, WinWorldSid, ACCESS_ALLOWED_ACE, ACE_HEADER, ACL, ACL_SIZE_INFORMATION,
        DACL_SECURITY_INFORMATION, INHERIT_ONLY_ACE, PSECURITY_DESCRIPTOR, WELL_KNOWN_SID_TYPE,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, GetDriveTypeW, GetLogicalDrives, FILE_APPEND_DATA, FILE_FLAG_BACKUP_SEMANTICS, FILE_READ_DATA,
        FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_WRITE_DATA, OPEN_EXISTING, READ_CONTROL, WRITE_DAC,
    };

    const DRIVE_FIXED: u32 = 3;
    const ACCESS_ALLOWED_ACE_TYPE: u8 = 0;

    fn wide(path: &Path) -> Vec<u16> {
        path.as_os_str().encode_wide().chain(std::iter::once(0)).collect()
    }

    /// Open the object asking for `WRITE_DAC | READ_CONTROL`, which is what
    /// `SetNamedSecurityInfoW(DACL)` opens it with. Success means the DACL (or
    /// ownership) gives the current token that right. `FILE_FLAG_BACKUP_SEMANTICS`
    /// is needed to open a directory; it bypasses checks only when the backup
    /// privilege is enabled, which a non-elevated token does not have.
    pub fn check_write_dac(path: &Path) -> Result<(), String> {
        let name = wide(path);
        let handle = unsafe {
            CreateFileW(
                name.as_ptr(),
                WRITE_DAC | READ_CONTROL,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS,
                std::ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE || handle.is_null() {
            let code = unsafe { GetLastError() };
            return Err(format!("the current user cannot change permissions here (WRITE_DAC refused, Win32 error {code})"));
        }
        unsafe { CloseHandle(handle) };
        Ok(())
    }

    fn broad_group(sid: *mut core::ffi::c_void) -> Option<&'static str> {
        let known: [(WELL_KNOWN_SID_TYPE, &str); 4] = [
            (WinWorldSid, "Everyone"),
            (WinAuthenticatedUserSid, "Authenticated Users"),
            (WinBuiltinUsersSid, "Users"),
            (WinInteractiveSid, "INTERACTIVE"),
        ];
        known.iter().find(|(kind, _)| unsafe { IsWellKnownSid(sid, *kind) } != 0).map(|(_, name)| *name)
    }

    pub fn broad_access(path: &Path) -> Result<BroadAccess, String> {
        let name = wide(path);
        let mut dacl: *mut ACL = std::ptr::null_mut();
        let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
        let status = unsafe {
            GetNamedSecurityInfoW(
                name.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut dacl,
                std::ptr::null_mut(),
                &mut descriptor,
            )
        };
        if status != 0 {
            return Err(format!("cannot read the DACL (Win32 error {status})"));
        }
        let result = (|| {
            let mut access = BroadAccess::default();
            if dacl.is_null() {
                // A NULL DACL grants everyone full access.
                access.read = true;
                access.write = true;
                access.groups.push("Everyone (NULL DACL)".into());
                return Ok(access);
            }
            let mut info = ACL_SIZE_INFORMATION { AceCount: 0, AclBytesInUse: 0, AclBytesFree: 0 };
            let ok = unsafe {
                GetAclInformation(dacl, &mut info as *mut _ as *mut core::ffi::c_void, std::mem::size_of::<ACL_SIZE_INFORMATION>() as u32, AclSizeInformation)
            };
            if ok == 0 {
                return Err(format!("GetAclInformation failed (Win32 error {})", unsafe { GetLastError() }));
            }
            for index in 0..info.AceCount {
                let mut ace: *mut core::ffi::c_void = std::ptr::null_mut();
                if unsafe { GetAce(dacl, index, &mut ace) } == 0 || ace.is_null() {
                    continue;
                }
                let header = unsafe { &*(ace as *const ACE_HEADER) };
                if header.AceType != ACCESS_ALLOWED_ACE_TYPE || u32::from(header.AceFlags) & INHERIT_ONLY_ACE != 0 {
                    continue;
                }
                let allowed = unsafe { &*(ace as *const ACCESS_ALLOWED_ACE) };
                let sid = unsafe { (ace as *mut u8).add(std::mem::offset_of!(ACCESS_ALLOWED_ACE, SidStart)) } as *mut core::ffi::c_void;
                let Some(group) = broad_group(sid) else { continue };
                let mask = allowed.Mask;
                let reads = mask & (FILE_READ_DATA | GENERIC_READ | GENERIC_ALL) != 0;
                let writes = mask & (FILE_WRITE_DATA | FILE_APPEND_DATA | GENERIC_WRITE | GENERIC_ALL) != 0;
                if reads || writes {
                    access.read |= reads;
                    access.write |= writes;
                    if !access.groups.iter().any(|known| known == group) {
                        access.groups.push(group.to_owned());
                    }
                }
            }
            Ok(access)
        })();
        unsafe { LocalFree(descriptor as _) };
        result
    }

    pub fn fixed_drive_roots() -> Vec<PathBuf> {
        let mask = unsafe { GetLogicalDrives() };
        (0..26u32)
            .filter(|bit| mask & (1 << bit) != 0)
            .map(|bit| PathBuf::from(format!("{}:\\", (b'A' + bit as u8) as char)))
            .filter(|root| unsafe { GetDriveTypeW(wide(root).as_ptr()) } == DRIVE_FIXED)
            .collect()
    }
}

#[cfg(not(windows))]
mod imp {
    use super::BroadAccess;
    use std::path::{Path, PathBuf};

    const NOT_WINDOWS: &str = "Windows ACL queries are only available on Windows";

    pub fn check_write_dac(_path: &Path) -> Result<(), String> {
        Err(NOT_WINDOWS.into())
    }

    pub fn broad_access(_path: &Path) -> Result<BroadAccess, String> {
        Err(NOT_WINDOWS.into())
    }

    pub fn fixed_drive_roots() -> Vec<PathBuf> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake;
    impl WindowsAclProbe for Fake {
        fn check_write_dac(&self, _path: &Path) -> Result<(), String> {
            Ok(())
        }
        fn broad_access(&self, path: &Path) -> Result<BroadAccess, String> {
            let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
            Ok(match name.as_str() {
                "shared" => BroadAccess { read: true, write: true, groups: vec!["Authenticated Users".into()] },
                "readonly" => BroadAccess { read: true, write: false, groups: vec!["Users".into()] },
                _ => BroadAccess::default(),
            })
        }
    }

    #[test]
    fn audit_reports_broad_dirs_and_changes_nothing() {
        let root = std::env::temp_dir().join(format!("fox-winacl-audit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for dir in ["shared", "readonly", "private"] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
        }
        let report = audit_with(&Fake, &[root.clone()]);
        let exposed: Vec<_> = report.exposed.iter().map(|item| item.path.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(exposed, ["readonly", "shared"]);
        assert!(report.exposed.iter().find(|item| item.path.ends_with("shared")).unwrap().access.write);
        assert!(report.errors.is_empty());
        assert!(root.join("private").is_dir(), "report only");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(not(windows))]
    #[test]
    fn real_queries_fail_closed_off_windows() {
        assert!(RealWindowsAcl.check_write_dac(Path::new("/")).is_err());
        assert!(RealWindowsAcl.broad_access(Path::new("/")).is_err());
        assert!(!audit_broad_exposure().supported);
    }
}
