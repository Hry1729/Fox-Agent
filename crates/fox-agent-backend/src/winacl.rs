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

/// Well-known SIDs used by the private-directory checks.
pub const SID_LOCAL_SYSTEM: &str = "S-1-5-18";
pub const SID_BUILTIN_ADMINISTRATORS: &str = "S-1-5-32-544";
pub const SID_TRUSTED_INSTALLER: &str = "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464";

/// One ACE of a DACL as read by [`WindowsAclProbe::dacl_entries`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AceEntry {
    /// String SID (`S-1-5-...`).
    pub sid: String,
    /// `true` for ACCESS_ALLOWED, `false` for ACCESS_DENIED. Other ACE types
    /// are not listed.
    pub allow: bool,
    pub mask: u32,
    /// Raw `AceFlags` (inheritance, `INHERITED_ACE` = 0x10).
    pub flags: u8,
}

/// `INHERITED_ACE` in `AceFlags`.
pub const ACE_FLAG_INHERITED: u8 = 0x10;

/// Access bits counted as "write class" when judging whether a broad group may
/// change a directory: data, append / add-subdirectory, EA, attributes,
/// delete, delete-child, `WRITE_DAC`, `WRITE_OWNER`, generic write / all.
pub const WRITE_CLASS_MASK: u32 = 0x0000_0002 // FILE_WRITE_DATA / FILE_ADD_FILE
    | 0x0000_0004 // FILE_APPEND_DATA / FILE_ADD_SUBDIRECTORY
    | 0x0000_0010 // FILE_WRITE_EA
    | 0x0000_0040 // FILE_DELETE_CHILD
    | 0x0000_0100 // FILE_WRITE_ATTRIBUTES
    | 0x0001_0000 // DELETE
    | 0x0004_0000 // WRITE_DAC
    | 0x0008_0000 // WRITE_OWNER
    | 0x4000_0000 // GENERIC_WRITE
    | 0x1000_0000; // GENERIC_ALL

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
    /// `write` covers every bit of [`WRITE_CLASS_MASK`].
    fn broad_access(&self, path: &Path) -> Result<BroadAccess, String>;
    /// Owner SID of `path` (not following a final reparse point is not
    /// guaranteed; callers skip reparse points themselves). Fails closed by
    /// default.
    fn owner_sid(&self, _path: &Path) -> Result<String, String> {
        Err("owner query not supported by this probe".into())
    }
    /// Allow / deny ACEs of the DACL of `path`, in order. Fails closed by
    /// default.
    fn dacl_entries(&self, _path: &Path) -> Result<Vec<AceEntry>, String> {
        Err("DACL query not supported by this probe".into())
    }
    /// String SID of the user of the current process token. Fails closed by
    /// default.
    fn current_user_sid(&self) -> Result<String, String> {
        Err("token query not supported by this probe".into())
    }
}

/// ACL changes Fox itself makes on Windows (ancestor metadata ACEs,
/// ownership repair, private directories). Off Windows every call fails; the
/// logic that decides what to change is tested against fakes.
pub trait WindowsAclWriter: Send + Sync {
    /// Whether `path` carries an explicit (not inherited, no inheritance
    /// flags) ACCESS_ALLOWED ACE for `sid` with exactly `mask`.
    fn has_exact_allow_ace(&self, path: &Path, sid: &str, mask: u32) -> Result<bool, String>;
    /// Add that ACE (no inheritance flags) after the explicit deny ACEs. No-op
    /// when it is already there. Written with `SetFileSecurityW`, which does
    /// not re-propagate inheritance into the subtree (the ACE is not
    /// inheritable, so nothing below changes).
    fn add_exact_allow_ace(&self, path: &Path, sid: &str, mask: u32) -> Result<(), String>;
    /// Remove every ACE that matches SID, mask and "no flags" exactly; returns
    /// how many were removed. Every other ACE is kept as it was.
    fn remove_exact_allow_ace(&self, path: &Path, sid: &str, mask: u32) -> Result<usize, String>;
    /// Make the current user the owner of `path`
    /// (`SetNamedSecurityInfoW(OWNER_SECURITY_INFORMATION)`; needs
    /// `WRITE_OWNER`).
    fn set_owner_to_current_user(&self, path: &Path) -> Result<(), String>;
    /// Create directory `path` with a protected DACL granting full control to
    /// `user_sid`, SYSTEM and Administrators only, owner `user_sid`.
    /// `Ok(false)` when it already existed (nothing changed).
    fn create_private_dir(&self, path: &Path, user_sid: &str) -> Result<bool, String>;
}

/// Whether `text` looks like a string SID (`S-1-<authority>-<sub>...`).
pub fn is_string_sid(text: &str) -> bool {
    let mut parts = text.split('-');
    parts.next() == Some("S")
        && parts.next() == Some("1")
        && {
            let rest: Vec<&str> = parts.collect();
            rest.len() >= 2 && rest.iter().all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()))
        }
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

    fn owner_sid(&self, path: &Path) -> Result<String, String> {
        imp::owner_sid(path)
    }

    fn dacl_entries(&self, path: &Path) -> Result<Vec<AceEntry>, String> {
        imp::dacl_entries(path)
    }

    fn current_user_sid(&self) -> Result<String, String> {
        imp::current_user_sid()
    }
}

impl WindowsAclWriter for RealWindowsAcl {
    fn has_exact_allow_ace(&self, path: &Path, sid: &str, mask: u32) -> Result<bool, String> {
        imp::has_exact_allow_ace(path, sid, mask)
    }

    fn add_exact_allow_ace(&self, path: &Path, sid: &str, mask: u32) -> Result<(), String> {
        imp::edit_exact_allow_ace(path, sid, mask, true).map(|_| ())
    }

    fn remove_exact_allow_ace(&self, path: &Path, sid: &str, mask: u32) -> Result<usize, String> {
        imp::edit_exact_allow_ace(path, sid, mask, false)
    }

    fn set_owner_to_current_user(&self, path: &Path) -> Result<(), String> {
        imp::set_owner_to_current_user(path)
    }

    fn create_private_dir(&self, path: &Path, user_sid: &str) -> Result<bool, String> {
        if !is_string_sid(user_sid) {
            return Err(format!("{user_sid:?} is not a SID"));
        }
        imp::create_private_dir(path, user_sid)
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
    /// Paths whose DACL could not be read (`path: reason`). Same content as
    /// [`Self::audit_errors`], kept for older readers.
    pub errors: Vec<String>,
    /// Every path looked at with its result: exposed, not exposed, or read
    /// failed. A read failure is neither exposed nor safe.
    #[serde(default)]
    pub checked: Vec<CheckedPath>,
    /// Paths that could not be checked (DACL unreadable, drive root not
    /// listable). Any entry makes the audit incomplete: it is not a
    /// successful audit, whatever `exposed` says.
    #[serde(default)]
    pub audit_errors: Vec<AuditError>,
}

/// Result for one audited path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditResult {
    Exposed,
    NotExposed,
    ReadFailed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckedPath {
    pub path: PathBuf,
    pub result: AuditResult,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditError {
    pub path: PathBuf,
    pub reason: String,
}

/// Overall state of an exposure audit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExposureAuditStatus {
    /// Off Windows: nothing scanned.
    NotSupported,
    /// Every path was read.
    Complete,
    /// At least one path could not be read ([`ExposureReport::audit_errors`]).
    Incomplete,
}

impl ExposureReport {
    pub fn status(&self) -> ExposureAuditStatus {
        if !self.supported {
            ExposureAuditStatus::NotSupported
        } else if self.audit_errors.is_empty() {
            ExposureAuditStatus::Complete
        } else {
            ExposureAuditStatus::Incomplete
        }
    }

    /// True only for a supported audit that read every path.
    pub fn is_complete(&self) -> bool {
        self.status() == ExposureAuditStatus::Complete
    }

    fn read_failed(&mut self, path: PathBuf, reason: String) {
        self.errors.push(format!("{}: {reason}", path.display()));
        self.checked.push(CheckedPath { path: path.clone(), result: AuditResult::ReadFailed });
        self.audit_errors.push(AuditError { path, reason });
    }
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
            Err(error) => report.read_failed(root.clone(), format!("cannot list: {error}")),
        }
        for path in candidates {
            match probe.broad_access(&path) {
                Ok(access) if access.any() => {
                    report.checked.push(CheckedPath { path: path.clone(), result: AuditResult::Exposed });
                    report.exposed.push(ExposedPath { path, access });
                }
                Ok(_) => report.checked.push(CheckedPath { path, result: AuditResult::NotExposed }),
                Err(error) => report.read_failed(path, error),
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
    const ACCESS_DENIED_ACE_TYPE: u8 = 1;
    /// `SECURITY_DESCRIPTOR_REVISION` (winnt.h; not exported under the enabled windows-sys features).
    const SECURITY_DESCRIPTOR_REVISION: u32 = 1;

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
                let writes = mask & (super::WRITE_CLASS_MASK | FILE_WRITE_DATA | FILE_APPEND_DATA | GENERIC_WRITE | GENERIC_ALL) != 0;
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

    // ---- SIDs, owner, DACL entries -------------------------------------

    /// A SID from `ConvertStringSidToSidW` (freed with `LocalFree`).
    struct LocalSid(*mut core::ffi::c_void);

    impl LocalSid {
        fn parse(text: &str) -> Result<Self, String> {
            use windows_sys::Win32::Security::Authorization::ConvertStringSidToSidW;
            let wide_text: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
            let mut sid: *mut core::ffi::c_void = std::ptr::null_mut();
            if unsafe { ConvertStringSidToSidW(wide_text.as_ptr(), &mut sid) } == 0 || sid.is_null() {
                return Err(format!("{text:?} is not a valid SID (Win32 error {})", unsafe { GetLastError() }));
            }
            Ok(Self(sid))
        }
    }

    impl Drop for LocalSid {
        fn drop(&mut self) {
            unsafe { LocalFree(self.0 as _) };
        }
    }

    fn sid_to_string(sid: *mut core::ffi::c_void) -> Result<String, String> {
        use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
        let mut text: *mut u16 = std::ptr::null_mut();
        if unsafe { ConvertSidToStringSidW(sid, &mut text) } == 0 || text.is_null() {
            return Err(format!("ConvertSidToStringSidW failed (Win32 error {})", unsafe { GetLastError() }));
        }
        let mut len = 0usize;
        while unsafe { *text.add(len) } != 0 {
            len += 1;
        }
        let value = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(text, len) });
        unsafe { LocalFree(text as _) };
        Ok(value)
    }

    /// The TOKEN_USER buffer of the current process (u64-aligned).
    fn token_user() -> Result<Vec<u64>, String> {
        use windows_sys::Win32::Security::{GetTokenInformation, TokenUser, TOKEN_QUERY};
        use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
        let mut token = std::ptr::null_mut();
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
            return Err(format!("OpenProcessToken failed (Win32 error {})", unsafe { GetLastError() }));
        }
        let mut needed = 0u32;
        unsafe { GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut needed) };
        let mut buffer = vec![0u64; (needed as usize).div_ceil(8).max(1)];
        let ok = unsafe { GetTokenInformation(token, TokenUser, buffer.as_mut_ptr() as *mut _, (buffer.len() * 8) as u32, &mut needed) };
        let error = unsafe { GetLastError() };
        unsafe { CloseHandle(token) };
        if ok == 0 {
            return Err(format!("GetTokenInformation(TokenUser) failed (Win32 error {error})"));
        }
        Ok(buffer)
    }

    pub fn current_user_sid() -> Result<String, String> {
        use windows_sys::Win32::Security::TOKEN_USER;
        let buffer = token_user()?;
        let user = unsafe { &*(buffer.as_ptr() as *const TOKEN_USER) };
        sid_to_string(user.User.Sid)
    }

    pub fn owner_sid(path: &Path) -> Result<String, String> {
        use windows_sys::Win32::Security::OWNER_SECURITY_INFORMATION;
        let name = wide(path);
        let mut owner: *mut core::ffi::c_void = std::ptr::null_mut();
        let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
        let status = unsafe {
            GetNamedSecurityInfoW(
                name.as_ptr(),
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION,
                &mut owner,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut descriptor,
            )
        };
        if status != 0 {
            return Err(format!("cannot read the owner of {} (Win32 error {status})", path.display()));
        }
        let result = if owner.is_null() { Err("no owner".to_owned()) } else { sid_to_string(owner) };
        unsafe { LocalFree(descriptor as _) };
        result
    }

    /// Iterate the ACEs of `dacl`: (type, flags, mask, sid pointer, ace pointer, size).
    fn for_each_ace(dacl: *mut ACL, mut visit: impl FnMut(u8, u8, u32, *mut core::ffi::c_void, *mut core::ffi::c_void, u16)) -> Result<(), String> {
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
                return Err(format!("GetAce({index}) failed (Win32 error {})", unsafe { GetLastError() }));
            }
            let header = unsafe { &*(ace as *const ACE_HEADER) };
            // ACCESS_ALLOWED_ACE and ACCESS_DENIED_ACE share the layout
            // header / Mask / SidStart.
            let (mask, sid) = if header.AceType == ACCESS_ALLOWED_ACE_TYPE || header.AceType == ACCESS_DENIED_ACE_TYPE {
                let allowed = unsafe { &*(ace as *const ACCESS_ALLOWED_ACE) };
                let sid = unsafe { (ace as *mut u8).add(std::mem::offset_of!(ACCESS_ALLOWED_ACE, SidStart)) } as *mut core::ffi::c_void;
                (allowed.Mask, sid)
            } else {
                (0, std::ptr::null_mut())
            };
            visit(header.AceType, header.AceFlags, mask, sid, ace, header.AceSize);
        }
        Ok(())
    }

    pub fn dacl_entries(path: &Path) -> Result<Vec<super::AceEntry>, String> {
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
            return Err(format!("cannot read the DACL of {} (Win32 error {status})", path.display()));
        }
        let result = if dacl.is_null() {
            Err("NULL DACL (everyone has full access)".to_owned())
        } else {
            let mut entries = Vec::new();
            let mut failure = None;
            let walked = for_each_ace(dacl, |kind, flags, mask, sid, _, _| {
                if sid.is_null() || failure.is_some() {
                    return;
                }
                match sid_to_string(sid) {
                    Ok(sid) => entries.push(super::AceEntry { sid, allow: kind == ACCESS_ALLOWED_ACE_TYPE, mask, flags }),
                    Err(error) => failure = Some(error),
                }
            });
            match (walked, failure) {
                (Err(error), _) | (Ok(()), Some(error)) => Err(error),
                (Ok(()), None) => Ok(entries),
            }
        };
        unsafe { LocalFree(descriptor as _) };
        result
    }

    // ---- exact ACE edits (ancestor metadata ACEs) -----------------------

    /// Self-relative security descriptor of `path` with its DACL
    /// (`GetFileSecurityW`), u64-aligned.
    fn file_dacl_descriptor(path: &Path) -> Result<Vec<u64>, String> {
        use windows_sys::Win32::Security::GetFileSecurityW;
        let name = wide(path);
        let mut needed = 0u32;
        unsafe { GetFileSecurityW(name.as_ptr(), DACL_SECURITY_INFORMATION, std::ptr::null_mut(), 0, &mut needed) };
        if needed == 0 {
            return Err(format!("GetFileSecurityW({}) failed (Win32 error {})", path.display(), unsafe { GetLastError() }));
        }
        let mut buffer = vec![0u64; (needed as usize).div_ceil(8)];
        if unsafe { GetFileSecurityW(name.as_ptr(), DACL_SECURITY_INFORMATION, buffer.as_mut_ptr() as *mut _, (buffer.len() * 8) as u32, &mut needed) } == 0 {
            return Err(format!("GetFileSecurityW({}) failed (Win32 error {})", path.display(), unsafe { GetLastError() }));
        }
        Ok(buffer)
    }

    fn descriptor_dacl(descriptor: &mut [u64]) -> Result<(*mut ACL, u16), String> {
        use windows_sys::Win32::Security::{GetSecurityDescriptorControl, GetSecurityDescriptorDacl};
        let psd = descriptor.as_mut_ptr() as *mut core::ffi::c_void;
        let mut present = 0;
        let mut defaulted = 0;
        let mut dacl: *mut ACL = std::ptr::null_mut();
        if unsafe { GetSecurityDescriptorDacl(psd, &mut present, &mut dacl, &mut defaulted) } == 0 {
            return Err(format!("GetSecurityDescriptorDacl failed (Win32 error {})", unsafe { GetLastError() }));
        }
        if present == 0 || dacl.is_null() {
            return Err("the object has no (or a NULL) DACL; refusing to edit it".into());
        }
        let mut control = 0u16;
        let mut revision = 0u32;
        if unsafe { GetSecurityDescriptorControl(psd, &mut control, &mut revision) } == 0 {
            return Err(format!("GetSecurityDescriptorControl failed (Win32 error {})", unsafe { GetLastError() }));
        }
        Ok((dacl, control))
    }

    fn is_exact(kind: u8, flags: u8, mask: u32, ace_sid: *mut core::ffi::c_void, want_sid: *mut core::ffi::c_void, want_mask: u32) -> bool {
        use windows_sys::Win32::Security::EqualSid;
        kind == ACCESS_ALLOWED_ACE_TYPE && flags == 0 && mask == want_mask && !ace_sid.is_null() && unsafe { EqualSid(ace_sid, want_sid) } != 0
    }

    pub fn has_exact_allow_ace(path: &Path, sid: &str, mask: u32) -> Result<bool, String> {
        let want = LocalSid::parse(sid)?;
        let mut descriptor = file_dacl_descriptor(path)?;
        let (dacl, _) = descriptor_dacl(&mut descriptor)?;
        let mut found = false;
        for_each_ace(dacl, |kind, flags, ace_mask, ace_sid, _, _| {
            found |= is_exact(kind, flags, ace_mask, ace_sid, want.0, mask);
        })?;
        Ok(found)
    }

    /// `add = true`: insert the exact ACE after the leading explicit deny
    /// ACEs (canonical order) unless present. `add = false`: drop every exact
    /// match. Returns the number of ACEs added / removed. The DACL is written
    /// back with `SetFileSecurityW`, keeping the `SE_DACL_PROTECTED` and
    /// `SE_DACL_AUTO_INHERITED` bits and every other ACE (inherited ones
    /// included) unchanged.
    pub fn edit_exact_allow_ace(path: &Path, sid: &str, mask: u32, add: bool) -> Result<usize, String> {
        use windows_sys::Win32::Security::{
            AddAccessAllowedAceEx, AddAce, GetLengthSid, InitializeAcl, InitializeSecurityDescriptor, SetFileSecurityW,
            SetSecurityDescriptorControl, SetSecurityDescriptorDacl, SECURITY_DESCRIPTOR, SE_DACL_AUTO_INHERITED, SE_DACL_PROTECTED,
        };
        let want = LocalSid::parse(sid)?;
        let mut descriptor = file_dacl_descriptor(path)?;
        let (old, control) = descriptor_dacl(&mut descriptor)?;
        let mut aces: Vec<(u8, u8, bool, *mut core::ffi::c_void, u16)> = Vec::new();
        for_each_ace(old, |kind, flags, ace_mask, ace_sid, ace, size| {
            aces.push((kind, flags, is_exact(kind, flags, ace_mask, ace_sid, want.0, mask), ace, size));
        })?;
        let matches = aces.iter().filter(|ace| ace.2).count();
        if add && matches > 0 {
            return Ok(0);
        }
        if !add && matches == 0 {
            return Ok(0);
        }
        let revision = u32::from(unsafe { (*old).AclRevision }).max(windows_sys::Win32::Security::ACL_REVISION);
        let old_size = u32::from(unsafe { (*old).AclSize });
        let extra = if add { std::mem::size_of::<ACCESS_ALLOWED_ACE>() as u32 + unsafe { GetLengthSid(want.0) } } else { 0 };
        let size = (old_size + extra + 3) & !3;
        let mut acl_buffer = vec![0u32; (size as usize) / 4];
        let new = acl_buffer.as_mut_ptr() as *mut ACL;
        if unsafe { InitializeAcl(new, size, revision) } == 0 {
            return Err(format!("InitializeAcl failed (Win32 error {})", unsafe { GetLastError() }));
        }
        let insert_at = aces.iter().position(|ace| !(ace.0 == ACCESS_DENIED_ACE_TYPE && ace.1 & super::ACE_FLAG_INHERITED == 0)).unwrap_or(aces.len());
        let mut changed = 0usize;
        for (index, (_, _, exact, ace, ace_size)) in aces.iter().enumerate() {
            if add && index == insert_at {
                if unsafe { AddAccessAllowedAceEx(new, revision, 0, mask, want.0) } == 0 {
                    return Err(format!("AddAccessAllowedAceEx failed (Win32 error {})", unsafe { GetLastError() }));
                }
                changed += 1;
            }
            if !add && *exact {
                changed += 1;
                continue;
            }
            if unsafe { AddAce(new, revision, u32::MAX, *ace, u32::from(*ace_size)) } == 0 {
                return Err(format!("AddAce failed (Win32 error {})", unsafe { GetLastError() }));
            }
        }
        if add && insert_at == aces.len() {
            if unsafe { AddAccessAllowedAceEx(new, revision, 0, mask, want.0) } == 0 {
                return Err(format!("AddAccessAllowedAceEx failed (Win32 error {})", unsafe { GetLastError() }));
            }
            changed += 1;
        }
        let mut sd: SECURITY_DESCRIPTOR = unsafe { std::mem::zeroed() };
        let psd = &mut sd as *mut SECURITY_DESCRIPTOR as *mut core::ffi::c_void;
        if unsafe { InitializeSecurityDescriptor(psd, SECURITY_DESCRIPTOR_REVISION) } == 0
            || unsafe { SetSecurityDescriptorDacl(psd, 1, new, 0) } == 0
        {
            return Err(format!("building the security descriptor failed (Win32 error {})", unsafe { GetLastError() }));
        }
        let keep = SE_DACL_PROTECTED | SE_DACL_AUTO_INHERITED;
        if unsafe { SetSecurityDescriptorControl(psd, keep, control & keep) } == 0 {
            return Err(format!("SetSecurityDescriptorControl failed (Win32 error {})", unsafe { GetLastError() }));
        }
        let name = wide(path);
        if unsafe { SetFileSecurityW(name.as_ptr(), DACL_SECURITY_INFORMATION, psd) } == 0 {
            return Err(format!("SetFileSecurityW({}) failed (Win32 error {})", path.display(), unsafe { GetLastError() }));
        }
        Ok(changed)
    }

    pub fn set_owner_to_current_user(path: &Path) -> Result<(), String> {
        use windows_sys::Win32::Security::Authorization::SetNamedSecurityInfoW;
        use windows_sys::Win32::Security::{OWNER_SECURITY_INFORMATION, TOKEN_USER};
        let buffer = token_user()?;
        let user = unsafe { &*(buffer.as_ptr() as *const TOKEN_USER) };
        let name = wide(path);
        let status = unsafe {
            SetNamedSecurityInfoW(
                name.as_ptr(),
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION,
                user.User.Sid,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
            )
        };
        if status != 0 {
            return Err(format!("cannot take ownership of {} (WRITE_OWNER refused?, Win32 error {status})", path.display()));
        }
        Ok(())
    }

    pub fn create_private_dir(path: &Path, user_sid: &str) -> Result<bool, String> {
        use windows_sys::Win32::Foundation::ERROR_ALREADY_EXISTS;
        use windows_sys::Win32::Security::Authorization::{ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1};
        use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
        use windows_sys::Win32::Storage::FileSystem::CreateDirectoryW;
        let sddl = format!("O:{user_sid}D:P(A;OICI;FA;;;{user_sid})(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)");
        let sddl_wide: Vec<u16> = sddl.encode_utf16().chain(std::iter::once(0)).collect();
        let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
        if unsafe { ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl_wide.as_ptr(), SDDL_REVISION_1, &mut descriptor, std::ptr::null_mut()) } == 0 {
            return Err(format!("cannot build the private directory descriptor (Win32 error {})", unsafe { GetLastError() }));
        }
        let attributes = SECURITY_ATTRIBUTES { nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32, lpSecurityDescriptor: descriptor, bInheritHandle: 0 };
        let name = wide(path);
        let ok = unsafe { CreateDirectoryW(name.as_ptr(), &attributes) };
        let error = unsafe { GetLastError() };
        unsafe { LocalFree(descriptor as _) };
        if ok != 0 {
            return Ok(true);
        }
        if error == ERROR_ALREADY_EXISTS {
            return Ok(false);
        }
        Err(format!("cannot create {} (Win32 error {error})", path.display()))
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

    pub fn owner_sid(_path: &Path) -> Result<String, String> {
        Err(NOT_WINDOWS.into())
    }

    pub fn dacl_entries(_path: &Path) -> Result<Vec<super::AceEntry>, String> {
        Err(NOT_WINDOWS.into())
    }

    pub fn current_user_sid() -> Result<String, String> {
        Err(NOT_WINDOWS.into())
    }

    pub fn has_exact_allow_ace(_path: &Path, _sid: &str, _mask: u32) -> Result<bool, String> {
        Err(NOT_WINDOWS.into())
    }

    pub fn edit_exact_allow_ace(_path: &Path, _sid: &str, _mask: u32, _add: bool) -> Result<usize, String> {
        Err(NOT_WINDOWS.into())
    }

    pub fn set_owner_to_current_user(_path: &Path) -> Result<(), String> {
        Err(NOT_WINDOWS.into())
    }

    pub fn create_private_dir(_path: &Path, _user_sid: &str) -> Result<bool, String> {
        Err(NOT_WINDOWS.into())
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
                "locked" => return Err("cannot read the DACL (Win32 error 5)".into()),
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
        assert!(report.audit_errors.is_empty());
        assert_eq!(report.status(), if cfg!(windows) { ExposureAuditStatus::Complete } else { ExposureAuditStatus::NotSupported });
        assert!(root.join("private").is_dir(), "report only");
        // A DACL that cannot be read: listed, and the audit is incomplete.
        std::fs::create_dir_all(root.join("locked")).unwrap();
        let mut report = audit_with(&Fake, &[root.clone()]);
        report.supported = true;
        assert_eq!(report.audit_errors.len(), 1);
        assert!(report.audit_errors[0].path.ends_with("locked"));
        assert_eq!(report.status(), ExposureAuditStatus::Incomplete);
        let result = |name: &str| report.checked.iter().find(|c| c.path.ends_with(name)).unwrap().result;
        assert_eq!(result("locked"), AuditResult::ReadFailed);
        assert_eq!(result("private"), AuditResult::NotExposed);
        assert_eq!(result("shared"), AuditResult::Exposed);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(not(windows))]
    #[test]
    fn real_queries_fail_closed_off_windows() {
        assert!(RealWindowsAcl.check_write_dac(Path::new("/")).is_err());
        assert!(RealWindowsAcl.broad_access(Path::new("/")).is_err());
        assert!(!audit_broad_exposure().supported);
        assert!(RealWindowsAcl.current_user_sid().is_err());
        assert!(RealWindowsAcl.add_exact_allow_ace(Path::new("/"), "S-1-5-18", 0x80).is_err());
        assert!(RealWindowsAcl.remove_exact_allow_ace(Path::new("/"), "S-1-5-18", 0x80).is_err());
        assert!(RealWindowsAcl.set_owner_to_current_user(Path::new("/")).is_err());
    }

    #[test]
    fn string_sids_are_recognised() {
        assert!(is_string_sid("S-1-5-21-1004336348-1177238915-682003330-1001"));
        assert!(is_string_sid(SID_LOCAL_SYSTEM) && is_string_sid(SID_BUILTIN_ADMINISTRATORS));
        for bad in ["", "S-1", "S-1-5", "S-1-5-21-x", "s-1-5-18", "S-1-5-18\\..", "S-1-5--18"] {
            assert!(!is_string_sid(bad), "{bad:?}");
        }
    }
}
