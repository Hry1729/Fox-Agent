//! Windows: ownership repair for files the sandbox created
//! (docs/sandbox.md section 7.8).
//!
//! Inside the sandbox the backend runs as the srt sandbox account, so every
//! file or folder it creates in an authorized folder or in its private HOME is
//! *owned* by that account's SID. After `srt windows-uninstall` deletes the
//! account those objects show an unknown SID as owner (orphans; the Windows
//! run found 7). Access is unaffected (the user's inherited ACEs still apply),
//! but the owner is wrong and owner-based quotas / `CREATOR OWNER` rights point
//! at a dead SID.
//!
//! [`repair_sandbox_ownership`] walks only the given roots (the authorized
//! folders and the private HOME root), never follows a reparse point
//! (symlink, junction, any other reparse tag), and changes the owner to the
//! current user only for objects whose owner is one of the srt sandbox SIDs
//! Fox has recorded (`srt-sids.json`, [`crate::winruntime::record_srt_sid`]).
//! Objects owned by any other SID, including other orphaned SIDs, are left
//! alone. It needs `WRITE_OWNER` on each object (the user has it through the
//! inherited full-control ACE of their own folders). Where the user has only
//! Modify (no `WRITE_OWNER`), the change is refused with Access denied; such
//! objects are reported as `needs_elevation`, not as failures, and
//! [`ownership_repair_list`] turns them into the input of the elevated
//! `-RepairOwnershipList` step of the enable / disable scripts (docs/sandbox.md
//! 7.8). It is an API, not an
//! automatic step: run it before uninstalling srt, after reinstalling it,
//! or on demand; never after every session.

use crate::winacl::{WindowsAclProbe, WindowsAclWriter};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "action", content = "detail")]
pub enum OwnershipAction {
    /// Owner was a recorded srt SID; now the current user.
    Repaired { previous_owner: String },
    /// Owner was a recorded srt SID; changing it failed (reason).
    Failed { previous_owner: String, reason: String },
    /// Owner was a recorded srt SID; the change was refused with Access
    /// denied (Win32 error 5). Not a failure: without elevation the user has
    /// no `WRITE_OWNER` on objects whose DACL gives them only Modify (the
    /// Windows run found one under a folder granting Authenticated Users
    /// Modify). Fixed by the elevated `-RepairOwnershipList` step of the
    /// enable / disable scripts with [`ownership_repair_list`].
    NeedsElevation { previous_owner: String, reason: String },
    /// A reparse point: neither changed nor descended into.
    SkippedReparsePoint,
    /// The owner could not be read (reason); left alone.
    OwnerUnreadable { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnershipItem {
    pub path: PathBuf,
    #[serde(flatten)]
    pub action: OwnershipAction,
}

/// Per-item report. Objects with any other owner are only counted.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnershipReport {
    pub items: Vec<OwnershipItem>,
    /// Objects examined (reparse points included, roots included).
    pub scanned: usize,
    /// Objects whose owner is neither a recorded srt SID nor unreadable
    /// (left untouched, other orphans included).
    pub untouched: usize,
    /// Roots that do not exist or cannot be listed (`path: reason`).
    pub errors: Vec<String>,
}

impl OwnershipReport {
    pub fn repaired(&self) -> usize {
        self.items.iter().filter(|item| matches!(item.action, OwnershipAction::Repaired { .. })).count()
    }

    pub fn failed(&self) -> usize {
        self.items.iter().filter(|item| matches!(item.action, OwnershipAction::Failed { .. })).count()
    }

    pub fn needs_elevation(&self) -> usize {
        self.items.iter().filter(|item| matches!(item.action, OwnershipAction::NeedsElevation { .. })).count()
    }
}

/// Whether a Win32 failure text is Access denied (error 5).
pub fn is_access_denied(reason: &str) -> bool {
    let lower = reason.to_ascii_lowercase();
    lower.contains("win32 error 5)") || lower.contains("win32 error 5,") || lower.ends_with("win32 error 5") || lower.contains("access is denied") || lower.contains("access denied")
}

/// Format version of the repair list ([`OwnershipRepairList`]).
pub const OWNERSHIP_REPAIR_LIST_VERSION: u32 = 1;

/// One object for the elevated repair: change the owner of `path` from
/// `expected_owner` (a recorded srt SID, re-read and checked by the script
/// before it changes anything) to `new_owner`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OwnershipRepairEntry {
    pub path: PathBuf,
    pub expected_owner: String,
    pub new_owner: String,
}

/// Input of `enable-sandbox.ps1` / `disable-sandbox.ps1 -RepairOwnershipList
/// <file>`: exactly the [`OwnershipAction::NeedsElevation`] items of a
/// non-elevated [`repair_sandbox_ownership`] run. The script refuses entries
/// whose current owner is not `expected_owner` or not in `srtSids`, never
/// follows a reparse point, and writes one result per entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OwnershipRepairList {
    pub version: u32,
    /// Always "fox-nonelevated-ownership-scan".
    pub source: String,
    /// The user the objects go back to (the non-elevated scanning user).
    pub user_sid: String,
    /// Every srt sandbox SID Fox has recorded (`srt-sids.json`).
    pub srt_sids: Vec<String>,
    pub items: Vec<OwnershipRepairEntry>,
}

pub const OWNERSHIP_REPAIR_SOURCE: &str = "fox-nonelevated-ownership-scan";

/// Build the repair list from a non-elevated scan. Only `NeedsElevation`
/// items whose previous owner is a recorded srt SID are listed; `user_sid`
/// and every SID must be string SIDs.
pub fn ownership_repair_list(report: &OwnershipReport, user_sid: &str, known_srt_sids: &[String]) -> Result<OwnershipRepairList, String> {
    if !crate::winacl::is_string_sid(user_sid) {
        return Err(format!("{user_sid:?} is not a SID"));
    }
    if let Some(bad) = known_srt_sids.iter().find(|sid| !crate::winacl::is_string_sid(sid)) {
        return Err(format!("{bad:?} is not a SID"));
    }
    let items = report
        .items
        .iter()
        .filter_map(|item| match &item.action {
            OwnershipAction::NeedsElevation { previous_owner, .. } if known_srt_sids.iter().any(|sid| sid.eq_ignore_ascii_case(previous_owner)) => {
                Some(OwnershipRepairEntry { path: item.path.clone(), expected_owner: previous_owner.clone(), new_owner: user_sid.to_owned() })
            }
            _ => None,
        })
        .collect();
    Ok(OwnershipRepairList {
        version: OWNERSHIP_REPAIR_LIST_VERSION,
        source: OWNERSHIP_REPAIR_SOURCE.to_owned(),
        user_sid: user_sid.to_owned(),
        srt_sids: known_srt_sids.to_vec(),
        items,
    })
}

/// Whether `metadata` (from `symlink_metadata`, i.e. not followed) is a
/// reparse point: any reparse tag on Windows, a symlink elsewhere.
fn is_reparse_point(metadata: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

/// Repair the owner of objects under `roots` whose owner is one of
/// `known_srt_sids` (see the module docs). Depth-first, roots included.
pub fn repair_sandbox_ownership(
    roots: &[PathBuf],
    known_srt_sids: &[String],
    probe: &dyn WindowsAclProbe,
    writer: &dyn WindowsAclWriter,
) -> OwnershipReport {
    let mut report = OwnershipReport::default();
    for root in roots {
        let metadata = match std::fs::symlink_metadata(root) {
            Ok(metadata) => metadata,
            Err(error) => {
                report.errors.push(format!("{}: {error}", root.display()));
                continue;
            }
        };
        visit(root, &metadata, known_srt_sids, probe, writer, &mut report);
    }
    report
}

fn visit(path: &Path, metadata: &std::fs::Metadata, known: &[String], probe: &dyn WindowsAclProbe, writer: &dyn WindowsAclWriter, report: &mut OwnershipReport) {
    report.scanned += 1;
    if is_reparse_point(metadata) {
        report.items.push(OwnershipItem { path: path.to_path_buf(), action: OwnershipAction::SkippedReparsePoint });
        return;
    }
    match probe.owner_sid(path) {
        Ok(owner) if known.iter().any(|sid| sid.eq_ignore_ascii_case(&owner)) => {
            let action = match writer.set_owner_to_current_user(path) {
                Ok(()) => OwnershipAction::Repaired { previous_owner: owner },
                Err(reason) if is_access_denied(&reason) => OwnershipAction::NeedsElevation { previous_owner: owner, reason },
                Err(reason) => OwnershipAction::Failed { previous_owner: owner, reason },
            };
            report.items.push(OwnershipItem { path: path.to_path_buf(), action });
        }
        Ok(_) => report.untouched += 1,
        Err(reason) => report.items.push(OwnershipItem { path: path.to_path_buf(), action: OwnershipAction::OwnerUnreadable { reason } }),
    }
    if !metadata.is_dir() {
        return;
    }
    let entries = match std::fs::read_dir(path) {
        Ok(entries) => entries,
        Err(error) => {
            report.errors.push(format!("{}: {error}", path.display()));
            return;
        }
    };
    let mut children: Vec<PathBuf> = entries.filter_map(Result::ok).map(|entry| entry.path()).collect();
    children.sort();
    for child in children {
        match std::fs::symlink_metadata(&child) {
            Ok(child_metadata) => visit(&child, &child_metadata, known, probe, writer, report),
            Err(error) => report.errors.push(format!("{}: {error}", child.display())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::winacl::BroadAccess;
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    const USER: &str = "S-1-5-21-1-2-3-1001";
    const SRT_OLD: &str = "S-1-5-21-1-2-3-1004";
    const SRT_NOW: &str = "S-1-5-21-1-2-3-1010";
    const STRANGER: &str = "S-1-5-21-9-9-9-1200";

    /// Owners by file name; `set_owner` records the change.
    struct Fake {
        owners: Mutex<BTreeMap<String, String>>,
        refuse: Vec<&'static str>,
    }

    fn name(path: &Path) -> String {
        path.file_name().unwrap().to_string_lossy().into_owned()
    }

    impl WindowsAclProbe for Fake {
        fn check_write_dac(&self, _: &Path) -> Result<(), String> {
            Ok(())
        }
        fn broad_access(&self, _: &Path) -> Result<BroadAccess, String> {
            Ok(BroadAccess::default())
        }
        fn owner_sid(&self, path: &Path) -> Result<String, String> {
            self.owners.lock().unwrap().get(&name(path)).cloned().ok_or_else(|| "no owner recorded".into())
        }
    }

    impl WindowsAclWriter for Fake {
        fn has_exact_allow_ace(&self, _: &Path, _: &str, _: u32) -> Result<bool, String> {
            unreachable!()
        }
        fn add_exact_allow_ace(&self, _: &Path, _: &str, _: u32) -> Result<(), String> {
            unreachable!()
        }
        fn remove_exact_allow_ace(&self, _: &Path, _: &str, _: u32) -> Result<usize, String> {
            unreachable!()
        }
        fn set_owner_to_current_user(&self, path: &Path) -> Result<(), String> {
            if self.refuse.contains(&name(path).as_str()) {
                return Err(format!("cannot take ownership of {} (WRITE_OWNER refused?, Win32 error 5)", path.display()));
            }
            if name(path) == "busy.txt" {
                return Err("cannot take ownership (Win32 error 32)".into());
            }
            self.owners.lock().unwrap().insert(name(path), USER.into());
            Ok(())
        }
        fn create_private_dir(&self, _: &Path, _: &str) -> Result<bool, String> {
            unreachable!()
        }
    }

    #[test]
    fn only_recorded_srt_owners_are_repaired_and_links_are_not_followed() {
        let root = std::env::temp_dir().join(format!("fox-owner-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let proj = root.join("proj");
        std::fs::create_dir_all(proj.join("build")).unwrap();
        std::fs::write(proj.join("by-sandbox.txt"), "x").unwrap();
        std::fs::write(proj.join("build").join("out.bin"), "x").unwrap();
        std::fs::write(proj.join("by-user.txt"), "x").unwrap();
        std::fs::write(proj.join("by-stranger.txt"), "x").unwrap();
        std::fs::write(proj.join("locked.txt"), "x").unwrap();
        std::fs::write(proj.join("busy.txt"), "x").unwrap();
        let outside = root.join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("victim.txt"), "x").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, proj.join("link")).unwrap();
        let owners = BTreeMap::from([
            ("proj".to_owned(), USER.to_owned()),
            ("build".to_owned(), SRT_OLD.to_owned()),
            ("out.bin".to_owned(), SRT_NOW.to_owned()),
            ("by-sandbox.txt".to_owned(), SRT_NOW.to_owned()),
            ("by-user.txt".to_owned(), USER.to_owned()),
            ("by-stranger.txt".to_owned(), STRANGER.to_owned()),
            ("locked.txt".to_owned(), SRT_NOW.to_owned()),
            ("busy.txt".to_owned(), SRT_NOW.to_owned()),
            ("victim.txt".to_owned(), SRT_NOW.to_owned()),
            ("link".to_owned(), SRT_NOW.to_owned()),
        ]);
        let fake = Fake { owners: Mutex::new(owners), refuse: vec!["locked.txt"] };
        let known = [SRT_OLD.to_owned(), SRT_NOW.to_owned()];
        let report = repair_sandbox_ownership(&[proj.clone(), root.join("missing")], &known, &fake, &fake);
        let by_name: BTreeMap<String, &OwnershipAction> = report.items.iter().map(|item| (name(&item.path), &item.action)).collect();
        assert_eq!(by_name["build"], &OwnershipAction::Repaired { previous_owner: SRT_OLD.into() }, "older recorded SIDs too");
        assert_eq!(by_name["out.bin"], &OwnershipAction::Repaired { previous_owner: SRT_NOW.into() });
        assert_eq!(by_name["by-sandbox.txt"], &OwnershipAction::Repaired { previous_owner: SRT_NOW.into() });
        assert!(matches!(by_name["locked.txt"], OwnershipAction::NeedsElevation { .. }), "Access denied is not a failure");
        assert!(matches!(by_name["busy.txt"], OwnershipAction::Failed { .. }));
        assert!(!by_name.contains_key("by-stranger.txt"), "an unrecorded (orphan) SID is not touched");
        assert!(!by_name.contains_key("by-user.txt"));
        assert!(!by_name.contains_key("victim.txt"), "never reached through the link");
        #[cfg(unix)]
        assert_eq!(by_name["link"], &OwnershipAction::SkippedReparsePoint);
        assert_eq!(fake.owners.lock().unwrap()["victim.txt"], SRT_NOW);
        assert_eq!(fake.owners.lock().unwrap()["by-stranger.txt"], STRANGER);
        assert_eq!(report.repaired(), 3);
        assert_eq!(report.failed(), 1);
        assert_eq!(report.needs_elevation(), 1);
        let list = ownership_repair_list(&report, USER, &known).unwrap();
        assert_eq!(list.items.len(), 1);
        assert_eq!(list.items[0].path, proj.join("locked.txt"));
        assert_eq!(list.items[0].expected_owner, SRT_NOW);
        assert_eq!(list.items[0].new_owner, USER);
        let json = serde_json::to_value(&list).unwrap();
        assert_eq!(json["source"], OWNERSHIP_REPAIR_SOURCE);
        assert_eq!(json["userSid"], USER);
        assert_eq!(json["items"][0]["expectedOwner"], SRT_NOW);
        assert!(ownership_repair_list(&report, "not-a-sid", &known).is_err());
        // A list built with only the older SID recorded leaves the item out.
        assert!(ownership_repair_list(&report, USER, &[SRT_OLD.to_owned()]).unwrap().items.is_empty());
        assert_eq!(report.untouched, 3, "proj, by-user, by-stranger");
        assert_eq!(report.errors.len(), 1, "missing root reported");
        let json = serde_json::to_value(&report.items[0]).unwrap();
        assert!(json.get("action").is_some() && json.get("path").is_some());
        let _ = std::fs::remove_dir_all(&root);
    }
}
