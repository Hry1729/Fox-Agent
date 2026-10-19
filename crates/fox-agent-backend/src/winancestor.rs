//! Windows: ancestor metadata ACEs for the srt sandbox account
//! (docs/sandbox.md section 7.6).
//!
//! **Why.** srt-win grants the `srt-sandbox` account access to an authorized
//! folder with an inheritable ACE on that folder only. Opening the folder by
//! its full path works (the account has the bypass-traverse privilege), but
//! Node (module resolution, `fs.realpath`) `lstat`s every ancestor, and an
//! ancestor such as `C:\Users\<u>` or `C:\Users\<u>\Documents` admits only the
//! user, SYSTEM and Administrators: `EPERM` (measured on Node 24, Windows 11).
//! srt 0.0.80 `acl grant` knows only `read` (READ+EXECUTE) and `write`, always
//! with `(OI)(CI)` inheritance, so it cannot express "may read the attributes
//! of this one directory". Fox adds that itself: on each ancestor between the
//! traversal base (the profile root, inclusive; or the per-user private root
//! under `%ProgramData%\Fox\sandbox`) and the srt target, exclusive, one
//! `ACCESS_ALLOWED` ACE for the sandbox SID with exactly
//! [`ANCESTOR_ACE_MASK`] (`FILE_READ_ATTRIBUTES | SYNCHRONIZE`) and no
//! inheritance flags. It reveals neither names nor contents.
//!
//! **Conflict with srt** (srt-win `state_db.rs` `recompose_at` /
//! `acl.rs` `apply_sandbox_aces`, commit 626a5967): whenever srt converges a
//! path it drops *every* explicit ACE of the sandbox SID on that path (allow
//! and deny) and writes its own set. So:
//!
//! 1. ancestors that srt itself touches in this session (grant / deny targets
//!    and each deny's parent) or that lie inside a grant target are dropped
//!    from Fox's set: srt already gives READ+EXECUTE there (a superset);
//! 2. Fox adds its ACEs before srt starts;
//! 3. a global reference ledger (`win-ancestor-aces.json` in Fox's state
//!    directory, under a file lock) records path, SID and the sessions that
//!    need each ACE; an ACE is removed only when its last session ends;
//! 4. another session's srt may still recompose one of these paths (it
//!    authorized it directly) and wipe Fox's ACE: every srt start and stop in
//!    Fox runs inside one [`SrtTransition`] (global lock), and each transition
//!    ends with a reconcile that re-adds whatever is missing;
//! 5. crash recovery: at Fox start, [`recover_ancestor_aces`] removes every
//!    ACE whose sessions are all gone and re-adds the rest.
//!
//! Fox only ever removes ACEs that match SID, mask and "no flags" exactly; it
//! never touches any other ACE. The ledger logic is separate from the ACL
//! calls ([`crate::winacl::WindowsAclWriter`]) so it is tested on any host.

use crate::sandbox::{lexical_parent, normalized, within, HostOs, SandboxError, SandboxErrorCode, WindowsAcePlan};
use crate::statefile::{lock_path_for, read_json, write_json, StateLock};
use crate::winacl::{is_string_sid, WindowsAclProbe, WindowsAclWriter};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// `FILE_READ_ATTRIBUTES | SYNCHRONIZE`: enough for `lstat` / `realpath`
/// of the directory itself, nothing else.
pub const ANCESTOR_ACE_MASK: u32 = 0x0000_0080 | 0x0010_0000;

/// Ledger file name inside Fox's state directory.
pub const ANCESTOR_LEDGER_FILE: &str = "win-ancestor-aces.json";

/// Where the sandbox account cannot `lstat` on its own.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AncestorBases {
    /// The user's profile root (`%USERPROFILE%`). Its stock DACL admits only
    /// the user, SYSTEM and Administrators; its parent `C:\Users` is readable
    /// by Users. The profile root itself gets an ACE.
    pub profile_root: Option<PathBuf>,
    /// `%ProgramData%\Fox\sandbox`: the enable script lets Users stat it, but
    /// not the per-user directories below it. The base itself gets no ACE.
    pub private_base: Option<PathBuf>,
}

impl AncestorBases {
    /// `%USERPROFILE%` and `%ProgramData%\Fox\sandbox` of this process (empty
    /// off Windows).
    pub fn from_env() -> Self {
        if !cfg!(windows) {
            return Self::default();
        }
        Self {
            profile_root: std::env::var_os("USERPROFILE").map(PathBuf::from),
            private_base: Some(crate::winruntime::windows_private_base()),
        }
    }
}

fn same(a: &Path, b: &Path) -> bool {
    normalized(a, HostOs::Windows) == normalized(b, HostOs::Windows)
}

fn is_volume_root_path(path: &Path) -> bool {
    crate::sandbox::is_volume_root(path)
}

/// Ancestors of `target` the sandbox account needs [`ANCESTOR_ACE_MASK`] on,
/// from the top down, `target` excluded:
///
/// * under the profile root: the profile root and every directory below it;
/// * under the private base: every directory below the base (not the base);
/// * elsewhere: every ancestor below the volume root whose DACL gives no
///   broad group read access ([`WindowsAclProbe::broad_access`]); an
///   unreadable DACL counts as "no access" (one more ACE rather than a
///   sandbox that cannot stat its folder).
pub fn ancestor_chain(target: &Path, bases: &AncestorBases, probe: &dyn WindowsAclProbe) -> Vec<PathBuf> {
    let os = HostOs::Windows;
    let mut chain = Vec::new();
    let mut current = lexical_parent(target);
    if let Some(profile) = bases.profile_root.as_deref().filter(|profile| within(target, profile, os)) {
        while let Some(dir) = current {
            if !within(&dir, profile, os) {
                break;
            }
            current = lexical_parent(&dir);
            let top = same(&dir, profile);
            chain.push(dir);
            if top {
                break;
            }
        }
    } else if let Some(base) = bases.private_base.as_deref().filter(|base| within(target, base, os)) {
        while let Some(dir) = current {
            if same(&dir, base) || !within(&dir, base, os) {
                break;
            }
            current = lexical_parent(&dir);
            chain.push(dir);
        }
    } else {
        while let Some(dir) = current {
            if is_volume_root_path(&dir) {
                break;
            }
            current = lexical_parent(&dir);
            let broad_read = probe.broad_access(&dir).map(|access| access.read).unwrap_or(false);
            if !broad_read {
                chain.push(dir);
            }
        }
    }
    chain.reverse();
    chain
}

/// Fox's ancestor ACE set for one session: the chains of every srt grant
/// target, minus every path srt itself touches in this session (grant and
/// deny targets, deny parents) and anything inside a grant target (inherits
/// srt's READ+EXECUTE). Deduplicated, top-down.
pub fn plan_ancestor_aces(plan: &WindowsAcePlan, bases: &AncestorBases, probe: &dyn WindowsAclProbe) -> Vec<PathBuf> {
    let os = HostOs::Windows;
    let grants: Vec<&PathBuf> = plan.grant_read.iter().chain(&plan.grant_write).collect();
    let mut srt_touched: Vec<PathBuf> = grants.iter().map(|path| (*path).clone()).chain(plan.deny.iter().cloned()).collect();
    srt_touched.extend(plan.deny.iter().filter_map(|deny| lexical_parent(deny)));
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for target in &grants {
        for ancestor in ancestor_chain(target, bases, probe) {
            if srt_touched.iter().any(|touched| same(&ancestor, touched)) || grants.iter().any(|grant| within(&ancestor, grant, os)) {
                continue;
            }
            if seen.insert(normalized(&ancestor, os)) {
                out.push(ancestor);
            }
        }
    }
    out.sort_by_key(|path| (normalized(path, os).matches('\\').count(), normalized(path, os)));
    out
}

// ---------------------------------------------------------------------------
// Ledger (pure)
// ---------------------------------------------------------------------------

/// One ACE Fox placed: where, for which sandbox SID, and the sessions that
/// still need it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AncestorLedgerEntry {
    pub path: PathBuf,
    pub sid: String,
    pub sessions: BTreeSet<String>,
}

/// The reference ledger (`win-ancestor-aces.json`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AncestorLedger {
    #[serde(default)]
    pub entries: Vec<AncestorLedgerEntry>,
}

impl AncestorLedger {
    fn position(&self, path: &Path, sid: &str) -> Option<usize> {
        self.entries.iter().position(|entry| entry.sid.eq_ignore_ascii_case(sid) && same(&entry.path, path))
    }

    /// Add `session` as a holder of each `(path, sid)`. Returns the paths whose
    /// reference count went from 0 to 1.
    pub fn acquire(&mut self, session: &str, sid: &str, paths: &[PathBuf]) -> Vec<PathBuf> {
        let mut fresh = Vec::new();
        for path in paths {
            match self.position(path, sid) {
                Some(index) => {
                    self.entries[index].sessions.insert(session.to_owned());
                }
                None => {
                    self.entries.push(AncestorLedgerEntry {
                        path: path.clone(),
                        sid: sid.to_owned(),
                        sessions: BTreeSet::from([session.to_owned()]),
                    });
                    fresh.push(path.clone());
                }
            }
        }
        fresh
    }

    /// Drop `session` everywhere. Returns the entries whose count reached 0
    /// (they are removed from the ledger; their ACE must go).
    pub fn release(&mut self, session: &str) -> Vec<AncestorLedgerEntry> {
        for entry in &mut self.entries {
            entry.sessions.remove(session);
        }
        self.take_unreferenced()
    }

    /// Keep only the sessions in `live`. Returns the entries left with none.
    pub fn retain_sessions(&mut self, live: &BTreeSet<String>) -> Vec<AncestorLedgerEntry> {
        for entry in &mut self.entries {
            entry.sessions.retain(|session| live.contains(session));
        }
        self.take_unreferenced()
    }

    fn take_unreferenced(&mut self) -> Vec<AncestorLedgerEntry> {
        let (gone, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut self.entries).into_iter().partition(|entry| entry.sessions.is_empty());
        self.entries = kept;
        gone
    }

    /// Sessions currently holding any entry.
    pub fn sessions(&self) -> BTreeSet<String> {
        self.entries.iter().flat_map(|entry| entry.sessions.iter().cloned()).collect()
    }

    pub fn holders(&self, path: &Path, sid: &str) -> Option<&BTreeSet<String>> {
        self.position(path, sid).map(|index| &self.entries[index].sessions)
    }
}

/// What one ledger operation did on disk.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AncestorReport {
    /// ACEs written because a session needed them and they were missing.
    pub added: Vec<PathBuf>,
    /// ACEs re-written by a reconcile (srt had wiped them).
    pub restored: Vec<PathBuf>,
    /// ACEs removed because no live session needs them any more.
    pub removed: Vec<PathBuf>,
    /// `path: reason` for every failed ACL call.
    pub errors: Vec<String>,
}

impl AncestorReport {
    fn merge(&mut self, other: AncestorReport) {
        self.added.extend(other.added);
        self.restored.extend(other.restored);
        self.removed.extend(other.removed);
        self.errors.extend(other.errors);
    }
}

// ---------------------------------------------------------------------------
// Coordinator (ledger + ACL calls + global srt transition lock)
// ---------------------------------------------------------------------------

/// Owns the ledger file and the ACL calls. One per Fox state directory.
#[derive(Clone)]
pub struct AncestorAceCoordinator {
    state_root: PathBuf,
    writer: Arc<dyn WindowsAclWriter>,
    probe: Arc<dyn WindowsAclProbe>,
    /// srt-win's own per-user lock (`%LOCALAPPDATA%\sandbox-runtime\.acl-init.lock`,
    /// byte 0 held with `LockFileEx` around every `srt-win acl` operation).
    /// Fox holds it (whole-file lock, which overlaps byte 0) only while it
    /// edits DACLs, so its read-modify-write of a DACL never interleaves with
    /// an srt recompose, Fox's or another program's. `None` = not used.
    srt_acl_lock: Option<PathBuf>,
}

impl std::fmt::Debug for AncestorAceCoordinator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AncestorAceCoordinator").field("state_root", &self.state_root).field("srt_acl_lock", &self.srt_acl_lock).finish()
    }
}

/// srt-win's per-user ACL lock file (`state_db.rs` `InitMutex`).
pub fn srt_acl_lock_path() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").filter(|value| !value.is_empty()).map(|base| PathBuf::from(base).join("sandbox-runtime").join(".acl-init.lock"))
}

fn ancestor_error(code: SandboxErrorCode, message: impl Into<String>) -> SandboxError {
    SandboxError::new(code, message)
}

impl AncestorAceCoordinator {
    /// `state_root` is Fox's state directory (`%LOCALAPPDATA%\Fox\state`).
    pub fn new(state_root: PathBuf, writer: Arc<dyn WindowsAclWriter>, probe: Arc<dyn WindowsAclProbe>) -> Self {
        Self { state_root, writer, probe, srt_acl_lock: None }
    }

    /// Also hold srt-win's ACL lock while editing (production: [`srt_acl_lock_path`]).
    pub fn with_srt_acl_lock(mut self, path: Option<PathBuf>) -> Self {
        self.srt_acl_lock = path;
        self
    }

    pub fn ledger_path(&self) -> PathBuf {
        self.state_root.join(ANCESTOR_LEDGER_FILE)
    }

    /// Enter the global srt transition: blocks until no other srt start /
    /// stop of this user's Fox (any process) is in progress. Every srt start
    /// and stop must happen inside one; end it with [`SrtTransition::finish`]
    /// (reconciles, then unlocks).
    pub fn transition(&self) -> Result<SrtTransition<'_>, SandboxError> {
        let ledger_path = self.ledger_path();
        let lock = StateLock::acquire(&lock_path_for(&ledger_path))
            .map_err(|error| ancestor_error(SandboxErrorCode::SandboxUnavailable, format!("cannot lock {}: {error}", ledger_path.display())))?;
        let ledger: AncestorLedger = read_json(&ledger_path).map_err(|error| {
            ancestor_error(SandboxErrorCode::SandboxUnavailable, format!("ancestor ACE ledger unreadable ({error}); refusing to start sandboxes until it is repaired"))
        })?;
        Ok(SrtTransition { coordinator: self, _lock: lock, ledger, ledger_path })
    }

    fn with_srt_lock<T>(&self, f: impl FnOnce() -> T) -> T {
        let _held = self.srt_acl_lock.as_deref().and_then(|path| StateLock::acquire(path).ok());
        f()
    }
}

/// The global srt start / stop critical section (see the module docs).
pub struct SrtTransition<'a> {
    coordinator: &'a AncestorAceCoordinator,
    _lock: StateLock,
    ledger: AncestorLedger,
    ledger_path: PathBuf,
}

impl SrtTransition<'_> {
    pub fn ledger(&self) -> &AncestorLedger {
        &self.ledger
    }

    fn save(&self) -> Result<(), SandboxError> {
        write_json(&self.ledger_path, &self.ledger).map_err(|error| ancestor_error(SandboxErrorCode::SandboxUnavailable, error))
    }

    /// Before srt starts for `session_id`: record the session as a holder of
    /// each ancestor ACE (record first), then write the missing ones.
    ///
    /// Fail-closed: an invalid sandbox SID, a path without `WRITE_DAC` for the
    /// current user (`sandbox_path_not_owned`, e.g. no `WRITE_DAC` on the
    /// profile root) or a failed write refuses the start, and this session's
    /// references are rolled back (ACEs only it needed are removed again).
    pub fn begin_session(&mut self, session_id: &str, sandbox_sid: &str, paths: &[PathBuf]) -> Result<AncestorReport, SandboxError> {
        if !is_string_sid(sandbox_sid) {
            return Err(ancestor_error(
                SandboxErrorCode::SandboxNotInstalled,
                format!("sandbox account SID {sandbox_sid:?} is not valid; the Windows sandbox setup is incomplete"),
            )
            .remedy("需在设置中启用沙箱（一次管理员授权）"));
        }
        let mut refused = Vec::new();
        for path in paths {
            if let Err(reason) = self.coordinator.probe.check_write_dac(path) {
                refused.push(format!("{}: {reason}", path.display()));
            }
        }
        if !refused.is_empty() {
            let mut error = ancestor_error(
                SandboxErrorCode::SandboxPathNotOwned,
                format!(
                    "the sandbox account must be able to read the attributes of every folder above an authorized folder (up to the user profile root), but the current user may not change permissions on: {}",
                    refused.join("; ")
                ),
            )
            .remedy("把授权文件夹放在你有完全控制权限的位置（例如你自己的用户目录内）；若用户配置文件根目录的权限被改过，需要恢复你对它的“修改权限”");
            for path in paths {
                if refused.iter().any(|line| line.starts_with(&path.display().to_string())) {
                    error = error.missing(path.display().to_string());
                }
            }
            return Err(error);
        }
        self.ledger.acquire(session_id, sandbox_sid, paths);
        self.save()?;
        let mut report = AncestorReport::default();
        let mut failed = Vec::new();
        self.coordinator.with_srt_lock(|| {
            for path in paths {
                match self.coordinator.writer.has_exact_allow_ace(path, sandbox_sid, ANCESTOR_ACE_MASK) {
                    Ok(true) => {}
                    Ok(false) | Err(_) => match self.coordinator.writer.add_exact_allow_ace(path, sandbox_sid, ANCESTOR_ACE_MASK) {
                        Ok(()) => report.added.push(path.clone()),
                        Err(reason) => failed.push(format!("{}: {reason}", path.display())),
                    },
                }
            }
        });
        if !failed.is_empty() {
            let rollback = self.end_session(session_id).unwrap_or_default();
            report.merge(rollback);
            return Err(ancestor_error(
                SandboxErrorCode::SandboxPathNotOwned,
                format!("cannot add the sandbox account's attribute-read ACE on: {}", failed.join("; ")),
            ));
        }
        Ok(report)
    }

    /// After a session's srt stopped: drop its references and remove the ACEs
    /// no other session needs.
    pub fn end_session(&mut self, session_id: &str) -> Result<AncestorReport, SandboxError> {
        let gone = self.ledger.release(session_id);
        self.save()?;
        Ok(self.remove_entries(&gone))
    }

    fn remove_entries(&self, gone: &[AncestorLedgerEntry]) -> AncestorReport {
        let mut report = AncestorReport::default();
        self.coordinator.with_srt_lock(|| {
            for entry in gone {
                match self.coordinator.writer.remove_exact_allow_ace(&entry.path, &entry.sid, ANCESTOR_ACE_MASK) {
                    Ok(_) => report.removed.push(entry.path.clone()),
                    // The folder is gone: so is the ACE.
                    Err(_) if !entry.path.exists() => report.removed.push(entry.path.clone()),
                    Err(reason) => report.errors.push(format!("{}: {reason}", entry.path.display())),
                }
            }
        });
        report
    }

    /// Re-add every ledger ACE that is missing on disk (srt recomposed the
    /// path for another session and wiped it).
    pub fn reconcile(&mut self) -> AncestorReport {
        let mut report = AncestorReport::default();
        self.coordinator.with_srt_lock(|| {
            for entry in &self.ledger.entries {
                match self.coordinator.writer.has_exact_allow_ace(&entry.path, &entry.sid, ANCESTOR_ACE_MASK) {
                    Ok(true) => {}
                    Ok(false) => match self.coordinator.writer.add_exact_allow_ace(&entry.path, &entry.sid, ANCESTOR_ACE_MASK) {
                        Ok(()) => report.restored.push(entry.path.clone()),
                        Err(reason) => report.errors.push(format!("{}: {reason}", entry.path.display())),
                    },
                    Err(reason) => report.errors.push(format!("{}: {reason}", entry.path.display())),
                }
            }
        });
        report
    }

    /// Crash recovery: keep only `live_sessions`, remove the ACEs left
    /// without a session, re-add the rest.
    pub fn recover(&mut self, live_sessions: &BTreeSet<String>) -> Result<AncestorReport, SandboxError> {
        let gone = self.ledger.retain_sessions(live_sessions);
        self.save()?;
        let mut report = self.remove_entries(&gone);
        report.merge(self.reconcile());
        Ok(report)
    }

    /// Reconcile and leave the transition (releases the global lock).
    pub fn finish(mut self) -> AncestorReport {
        self.reconcile()
    }
}

/// Fox start (and on demand): remove every ancestor ACE whose sessions are
/// not in `live_sessions` (at start: none are live, so all of them) and
/// re-add the ACEs live sessions still need. Only Fox's own exact ACEs are
/// touched.
pub fn recover_ancestor_aces(coordinator: &AncestorAceCoordinator, live_sessions: &BTreeSet<String>) -> Result<AncestorReport, SandboxError> {
    let mut transition = coordinator.transition()?;
    let report = transition.recover(live_sessions)?;
    drop(transition);
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::winacl::BroadAccess;
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    const SID: &str = "S-1-5-21-1111111111-2222222222-3333333333-1004";

    fn p(text: &str) -> PathBuf {
        PathBuf::from(text)
    }

    /// Fake ACLs: broad read on listed prefixes; `WRITE_DAC` everywhere except
    /// listed prefixes; an in-memory set of (path, sid, mask) ACEs.
    #[derive(Default)]
    struct Fake {
        broad_read: Vec<&'static str>,
        no_write_dac: Vec<&'static str>,
        fail_add: Vec<&'static str>,
        aces: Mutex<BTreeMap<String, BTreeSet<(String, u32)>>>,
        calls: Mutex<Vec<String>>,
    }

    impl Fake {
        fn key(path: &Path) -> String {
            normalized(path, HostOs::Windows)
        }
        fn has(&self, path: &str) -> bool {
            self.aces.lock().unwrap().get(&Self::key(Path::new(path))).is_some_and(|set| set.contains(&(SID.to_owned(), ANCESTOR_ACE_MASK)))
        }
        /// srt recompose: drops every ACE of the SID on that path.
        fn srt_recompose(&self, path: &str) {
            if let Some(set) = self.aces.lock().unwrap().get_mut(&Self::key(Path::new(path))) {
                set.retain(|(sid, _)| sid != SID);
            }
        }
        /// An unrelated ACE (different mask) on the path.
        fn other_ace(&self, path: &str, mask: u32) {
            self.aces.lock().unwrap().entry(Self::key(Path::new(path))).or_default().insert((SID.to_owned(), mask));
        }
        fn count(&self) -> usize {
            self.aces.lock().unwrap().values().map(|set| set.iter().filter(|(_, mask)| *mask == ANCESTOR_ACE_MASK).count()).sum()
        }
    }

    impl WindowsAclProbe for Fake {
        fn check_write_dac(&self, path: &Path) -> Result<(), String> {
            if self.no_write_dac.iter().any(|prefix| within(path, Path::new(prefix), HostOs::Windows)) {
                Err("WRITE_DAC refused, Win32 error 5".into())
            } else {
                Ok(())
            }
        }
        fn broad_access(&self, path: &Path) -> Result<BroadAccess, String> {
            let read = self.broad_read.iter().any(|prefix| same(path, Path::new(prefix)));
            Ok(BroadAccess { read, write: false, groups: if read { vec!["Users".into()] } else { vec![] } })
        }
    }

    impl WindowsAclWriter for Fake {
        fn has_exact_allow_ace(&self, path: &Path, sid: &str, mask: u32) -> Result<bool, String> {
            Ok(self.aces.lock().unwrap().get(&Self::key(path)).is_some_and(|set| set.contains(&(sid.to_owned(), mask))))
        }
        fn add_exact_allow_ace(&self, path: &Path, sid: &str, mask: u32) -> Result<(), String> {
            self.calls.lock().unwrap().push(format!("add {}", path.display()));
            if self.fail_add.iter().any(|prefix| same(path, Path::new(prefix))) {
                return Err("Access is denied".into());
            }
            self.aces.lock().unwrap().entry(Self::key(path)).or_default().insert((sid.to_owned(), mask));
            Ok(())
        }
        fn remove_exact_allow_ace(&self, path: &Path, sid: &str, mask: u32) -> Result<usize, String> {
            self.calls.lock().unwrap().push(format!("remove {}", path.display()));
            let removed = self.aces.lock().unwrap().get_mut(&Self::key(path)).map(|set| usize::from(set.remove(&(sid.to_owned(), mask)))).unwrap_or(0);
            Ok(removed)
        }
        fn set_owner_to_current_user(&self, _path: &Path) -> Result<(), String> {
            unreachable!()
        }
        fn create_private_dir(&self, _path: &Path, _user_sid: &str) -> Result<bool, String> {
            unreachable!()
        }
    }

    fn bases() -> AncestorBases {
        AncestorBases { profile_root: Some(p(r"C:\Users\u")), private_base: Some(p(r"C:\ProgramData\Fox\sandbox")) }
    }

    fn plan_for(grant_write: &[&str], grant_read: &[&str], deny: &[&str]) -> WindowsAcePlan {
        WindowsAcePlan {
            grant_read: grant_read.iter().map(|path| p(path)).collect(),
            grant_write: grant_write.iter().map(|path| p(path)).collect(),
            deny: deny.iter().map(|path| p(path)).collect(),
            ..Default::default()
        }
    }

    fn texts(paths: &[PathBuf]) -> Vec<String> {
        paths.iter().map(|path| path.display().to_string()).collect()
    }

    #[test]
    fn ancestors_run_from_the_profile_root_or_the_private_root_and_exclude_the_target() {
        let fake = Fake::default();
        let chain = ancestor_chain(Path::new(r"C:\Users\u\Documents\中文 空格\proj"), &bases(), &fake);
        assert_eq!(texts(&chain), [r"C:\Users\u", r"C:\Users\u\Documents", r"C:\Users\u\Documents\中文 空格"]);
        // Case-insensitive containment.
        let chain = ancestor_chain(Path::new(r"c:\users\U\proj"), &bases(), &fake);
        assert_eq!(texts(&chain), [r"c:\users\U"]);
        // The profile root itself as target: nothing above it is needed.
        assert!(ancestor_chain(Path::new(r"C:\Users\u"), &bases(), &fake).is_empty());
        // Private root: below the base only.
        let sid_root = format!(r"C:\ProgramData\Fox\sandbox\{SID}");
        let home = format!(r"{sid_root}\homes\3f2a6c1e-8b4d-4e0a-9c55-0d1e2f3a4b5c\pi");
        let chain = ancestor_chain(Path::new(&home), &bases(), &fake);
        assert_eq!(texts(&chain), [sid_root.clone(), format!(r"{sid_root}\homes"), format!(r"{sid_root}\homes\3f2a6c1e-8b4d-4e0a-9c55-0d1e2f3a4b5c")]);
        // Elsewhere: only ancestors no broad group can read, never the drive root.
        let fake = Fake { broad_read: vec![r"D:\work"], ..Default::default() };
        let chain = ancestor_chain(Path::new(r"D:\work\private\proj"), &bases(), &fake);
        assert_eq!(texts(&chain), [r"D:\work\private"]);
    }

    #[test]
    fn srt_targets_and_paths_inside_grants_are_left_to_srt() {
        let fake = Fake::default();
        // Two authorized folders, one inside the other's parent chain; a deny
        // inside a project (srt stamps it and its parent).
        let plan = plan_for(
            &[r"C:\Users\u\Documents\a\proj", r"C:\Users\u\Documents\a"],
            &[r"C:\Users\u\Desktop\ref"],
            &[r"C:\Users\u\Documents\a\proj\secrets\key.txt"],
        );
        let ancestors = plan_ancestor_aces(&plan, &bases(), &fake);
        assert_eq!(texts(&ancestors), [r"C:\Users\u", r"C:\Users\u\Desktop", r"C:\Users\u\Documents"]);
        // C:\Users\u\Documents\a is an srt grant target (READ+EXECUTE superset): no Fox ACE.
        assert!(!ancestors.iter().any(|path| same(path, Path::new(r"C:\Users\u\Documents\a"))));
        // A deny parent above a grant is srt's too.
        let plan = plan_for(&[r"C:\Users\u\x\y\proj"], &[], &[r"C:\Users\u\x\y\other.txt"]);
        let ancestors = plan_ancestor_aces(&plan, &bases(), &fake);
        assert_eq!(texts(&ancestors), [r"C:\Users\u", r"C:\Users\u\x"], "C:\\Users\\u\\x\\y is the deny's parent (srt deny_fdc)");
    }

    #[test]
    fn ledger_counts_references_per_path_and_sid() {
        let mut ledger = AncestorLedger::default();
        let a = [p(r"C:\Users\u"), p(r"C:\Users\u\Documents")];
        assert_eq!(ledger.acquire("s1", SID, &a), a.to_vec());
        assert!(ledger.acquire("s2", SID, &[p(r"c:\users\U")]).is_empty(), "same path, other spelling: one entry");
        assert_eq!(ledger.holders(Path::new(r"C:\Users\u"), SID).unwrap().len(), 2);
        let gone = ledger.release("s1");
        assert_eq!(gone.len(), 1);
        assert_eq!(gone[0].path, p(r"C:\Users\u\Documents"));
        assert!(ledger.release("s1").is_empty(), "releasing twice is harmless");
        assert_eq!(ledger.release("s2").len(), 1);
        assert!(ledger.entries.is_empty());
        // Another SID (srt reinstalled) is another entry.
        ledger.acquire("s3", SID, &a[..1]);
        ledger.acquire("s3", "S-1-5-21-1-2-3-1005", &a[..1]);
        assert_eq!(ledger.entries.len(), 2);
        assert_eq!(ledger.retain_sessions(&BTreeSet::new()).len(), 2);
    }

    fn make(fake: Arc<Fake>, name: &str) -> (AncestorAceCoordinator, PathBuf) {
        let dir = std::env::temp_dir().join(format!("fox-ancestor-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        (AncestorAceCoordinator::new(dir.clone(), fake.clone(), fake), dir)
    }

    #[test]
    fn sessions_add_before_start_and_remove_only_at_zero() {
        let fake = Arc::new(Fake::default());
        let (coordinator, dir) = make(fake.clone(), "refcount");
        let docs = [p(r"C:\Users\u"), p(r"C:\Users\u\Documents")];
        let mut transition = coordinator.transition().unwrap();
        let report = transition.begin_session("s1", SID, &docs).unwrap();
        assert_eq!(report.added, docs.to_vec());
        transition.finish();
        let mut transition = coordinator.transition().unwrap();
        assert!(transition.begin_session("s2", SID, &docs[..1]).unwrap().added.is_empty(), "already there");
        transition.finish();
        // The ledger persisted across transitions.
        let saved: AncestorLedger = read_json(&coordinator.ledger_path()).unwrap();
        assert_eq!(saved.sessions(), BTreeSet::from(["s1".to_owned(), "s2".to_owned()]));
        let mut transition = coordinator.transition().unwrap();
        let report = transition.end_session("s1").unwrap();
        assert_eq!(report.removed, [p(r"C:\Users\u\Documents")]);
        assert!(fake.has(r"C:\Users\u") && !fake.has(r"C:\Users\u\Documents"));
        let report = transition.end_session("s2").unwrap();
        assert_eq!(report.removed, [p(r"C:\Users\u")]);
        assert_eq!(fake.count(), 0);
        transition.finish();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reconcile_restores_what_another_sessions_srt_wiped_and_never_touches_other_aces() {
        let fake = Arc::new(Fake::default());
        let (coordinator, dir) = make(fake.clone(), "reconcile");
        fake.other_ace(r"C:\Users\u\Documents", 0x1200a9); // srt's own READ+EXECUTE for another session
        let mut transition = coordinator.transition().unwrap();
        transition.begin_session("s1", SID, &[p(r"C:\Users\u"), p(r"C:\Users\u\Documents")]).unwrap();
        // Session 2 authorized Documents itself: its srt recompose wipes every
        // ACE of the SID there, Fox's included.
        fake.srt_recompose(r"C:\Users\u\Documents");
        assert!(!fake.has(r"C:\Users\u\Documents"));
        let report = transition.finish();
        assert_eq!(report.restored, [p(r"C:\Users\u\Documents")]);
        assert!(fake.has(r"C:\Users\u\Documents"));
        // Removal is exact: srt's (OI)(CI) ACE with another mask survives.
        let mut transition = coordinator.transition().unwrap();
        fake.other_ace(r"C:\Users\u\Documents", 0x1200a9);
        transition.end_session("s1").unwrap();
        assert!(fake.aces.lock().unwrap()[&Fake::key(Path::new(r"C:\Users\u\Documents"))].contains(&(SID.to_owned(), 0x1200a9)));
        assert_eq!(fake.count(), 0);
        transition.finish();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn crash_recovery_removes_aces_of_dead_sessions_and_keeps_live_ones() {
        let fake = Arc::new(Fake::default());
        let (coordinator, dir) = make(fake.clone(), "recover");
        let mut transition = coordinator.transition().unwrap();
        transition.begin_session("dead", SID, &[p(r"C:\Users\u"), p(r"C:\Users\u\a")]).unwrap();
        transition.begin_session("live", SID, &[p(r"C:\Users\u"), p(r"C:\Users\u\b")]).unwrap();
        drop(transition); // Host killed: no end_session, lock released by drop / process death.
        fake.srt_recompose(r"C:\Users\u\b");
        let report = recover_ancestor_aces(&coordinator, &BTreeSet::from(["live".to_owned()])).unwrap();
        assert_eq!(report.removed, [p(r"C:\Users\u\a")]);
        assert_eq!(report.restored, [p(r"C:\Users\u\b")]);
        assert!(fake.has(r"C:\Users\u") && fake.has(r"C:\Users\u\b") && !fake.has(r"C:\Users\u\a"));
        // Fox start: nothing is live.
        let report = recover_ancestor_aces(&coordinator, &BTreeSet::new()).unwrap();
        assert_eq!(report.removed.len(), 2);
        assert_eq!(fake.count(), 0);
        assert!(read_json::<AncestorLedger>(&coordinator.ledger_path()).unwrap().entries.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn begin_fails_closed_without_write_dac_or_with_a_bad_sid_and_rolls_back() {
        let fake = Arc::new(Fake { no_write_dac: vec![r"C:\Users\u"], ..Default::default() });
        let (coordinator, dir) = make(fake.clone(), "failclosed");
        let mut transition = coordinator.transition().unwrap();
        let error = transition.begin_session("s1", SID, &[p(r"C:\Users\u")]).unwrap_err();
        assert_eq!(error.code, SandboxErrorCode::SandboxPathNotOwned);
        assert_eq!(error.missing, [r"C:\Users\u"]);
        assert!(error.message.contains("profile root"), "{}", error.message);
        assert!(transition.ledger().entries.is_empty(), "nothing recorded");
        let error = transition.begin_session("s1", "srt-sandbox", &[p(r"C:\x\y")]).unwrap_err();
        assert_eq!(error.code, SandboxErrorCode::SandboxNotInstalled);
        drop(transition);

        // A write that fails half-way: the ACEs this session added are removed again.
        let fake = Arc::new(Fake { fail_add: vec![r"C:\Users\u\b"], ..Default::default() });
        let (coordinator, dir2) = make(fake.clone(), "rollback");
        let mut transition = coordinator.transition().unwrap();
        let error = transition.begin_session("s1", SID, &[p(r"C:\Users\u"), p(r"C:\Users\u\b")]).unwrap_err();
        assert_eq!(error.code, SandboxErrorCode::SandboxPathNotOwned);
        assert_eq!(fake.count(), 0, "rolled back");
        assert!(transition.ledger().entries.is_empty());
        transition.finish();
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&dir2);
    }

    #[test]
    fn transitions_are_serialized_across_handles() {
        let fake = Arc::new(Fake::default());
        let (coordinator, dir) = make(fake, "serial");
        let first = coordinator.transition().unwrap();
        assert!(StateLock::try_acquire(&lock_path_for(&coordinator.ledger_path())).unwrap().is_none(), "a second srt start waits");
        first.finish();
        assert!(StateLock::try_acquire(&lock_path_for(&coordinator.ledger_path())).unwrap().is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
