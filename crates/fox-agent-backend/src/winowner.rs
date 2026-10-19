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
//! inherited full-control ACE of their own folders). It is an API, not an
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
                return Err("WRITE_OWNER refused, Win32 error 5".into());
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
        assert!(matches!(by_name["locked.txt"], OwnershipAction::Failed { .. }));
        assert!(!by_name.contains_key("by-stranger.txt"), "an unrecorded (orphan) SID is not touched");
        assert!(!by_name.contains_key("by-user.txt"));
        assert!(!by_name.contains_key("victim.txt"), "never reached through the link");
        #[cfg(unix)]
        assert_eq!(by_name["link"], &OwnershipAction::SkippedReparsePoint);
        assert_eq!(fake.owners.lock().unwrap()["victim.txt"], SRT_NOW);
        assert_eq!(fake.owners.lock().unwrap()["by-stranger.txt"], STRANGER);
        assert_eq!(report.repaired(), 3);
        assert_eq!(report.failed(), 1);
        assert_eq!(report.untouched, 3, "proj, by-user, by-stranger");
        assert_eq!(report.errors.len(), 1, "missing root reported");
        let json = serde_json::to_value(&report.items[0]).unwrap();
        assert!(json.get("action").is_some() && json.get("path").is_some());
        let _ = std::fs::remove_dir_all(&root);
    }
}
