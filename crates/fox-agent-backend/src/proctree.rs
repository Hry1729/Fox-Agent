//! Confirming that a backend process tree has really ended.
//!
//! A revocation stop may only lead to a rebuild without full access once the
//! old full-access process tree is gone (docs/sandbox.md section 8.2). "Gone" is
//! judged on the whole tree, never on the parent's exit alone:
//!
//! * Linux / macOS: the backend runs in its own process group (`process_group(0)`
//!   at spawn). The group counts as ended when `kill(-pgid, 0)` fails with
//!   `ESRCH`, or when every remaining member is a zombie (it has exited and
//!   only waits to be reaped by its new parent; it runs no code). Descendants
//!   that left the group (pi's bash tool spawns with `detached: true`, i.e. a
//!   group of its own) are tracked by a snapshot of the parent links taken
//!   before the stop ([`snapshot_tree`]), identified by pid + start time so a
//!   reused pid is not mistaken for the old process; they are killed by pid
//!   ([`kill_escaped`]) and must be gone too. A descendant that was started
//!   and left the group between the last snapshot and the kill cannot be seen
//!   by this method (documented limit).
//! * Windows: see `acp::win_job` — `QueryInformationJobObject`
//!   (`JobObjectBasicAccountingInformation`) must report `ActiveProcesses == 0`.
//!
//! The process table is read from `/proc` on Linux and from `ps` on other
//! Unix systems. When it cannot be read the result is never `Confirmed`.

#[cfg(unix)]
use crate::port::StopConfirmation;
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// How long the stop waits for the tree to disappear after SIGKILL /
/// `TerminateJobObject` before reporting `FailedOrUnconfirmed`.
pub const STOP_CONFIRM_TIMEOUT: Duration = Duration::from_secs(3);
#[cfg_attr(not(unix), allow(dead_code))]
const POLL: Duration = Duration::from_millis(25);

/// One process as seen at snapshot time. `start` is the kernel start time
/// (`/proc/<pid>/stat` field 22 on Linux, `ps -o lstart` elsewhere): a pid
/// with a different start time is a different process.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProcIdentity {
    pub pid: u32,
    pub pgid: u32,
    pub start: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(not(unix), allow(dead_code))]
struct ProcRow {
    pid: u32,
    ppid: u32,
    pgid: u32,
    zombie: bool,
    start: String,
}

/// `/proc/<pid>/stat`: the command name sits in parentheses and may contain
/// spaces, so fields are counted after the last `)`.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn parse_proc_stat(pid: u32, stat: &str) -> Option<ProcRow> {
    let rest = &stat[stat.rfind(')')? + 1..];
    let fields: Vec<&str> = rest.split_whitespace().collect();
    let state = *fields.first()?;
    Some(ProcRow {
        pid,
        zombie: state == "Z" || state == "X",
        ppid: fields.get(1)?.parse().ok()?,
        pgid: fields.get(2)?.parse().ok()?,
        start: (*fields.get(19)?).to_owned(),
    })
}

/// `ps -A -o pid=,ppid=,pgid=,stat=,lstart=` output (lstart has spaces and
/// comes last).
#[allow(dead_code)]
fn parse_ps(text: &str) -> Vec<ProcRow> {
    text.lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let pid = parts.next()?.parse().ok()?;
            let ppid = parts.next()?.parse().ok()?;
            let pgid = parts.next()?.parse().ok()?;
            let stat = parts.next()?;
            let start = parts.collect::<Vec<_>>().join(" ");
            Some(ProcRow { pid, ppid, pgid, zombie: stat.starts_with('Z'), start })
        })
        .collect()
}

#[cfg(target_os = "linux")]
fn table() -> Option<Vec<ProcRow>> {
    let mut rows = Vec::new();
    for entry in std::fs::read_dir("/proc").ok()?.flatten() {
        let Some(pid) = entry.file_name().to_str().and_then(|name| name.parse::<u32>().ok()) else { continue };
        // A process that exits between readdir and read is simply absent.
        if let Ok(stat) = std::fs::read_to_string(entry.path().join("stat")) {
            rows.extend(parse_proc_stat(pid, &stat));
        }
    }
    Some(rows)
}

#[cfg(all(unix, not(target_os = "linux")))]
fn table() -> Option<Vec<ProcRow>> {
    let output = std::process::Command::new("ps")
        .args(["-A", "-o", "pid=,ppid=,pgid=,stat=,lstart="])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    output.status.success().then(|| parse_ps(&String::from_utf8_lossy(&output.stdout)))
}

#[allow(dead_code)]
fn descendants_in(rows: &[ProcRow], root: u32) -> Vec<ProcIdentity> {
    let mut found = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut frontier = vec![root];
    if let Some(row) = rows.iter().find(|row| row.pid == root && !row.zombie) {
        seen.insert(row.pid);
        found.push(ProcIdentity { pid: row.pid, pgid: row.pgid, start: row.start.clone() });
    }
    while let Some(parent) = frontier.pop() {
        for row in rows.iter().filter(|row| row.ppid == parent && row.pid != parent) {
            if seen.insert(row.pid) {
                frontier.push(row.pid);
                if !row.zombie {
                    found.push(ProcIdentity { pid: row.pid, pgid: row.pgid, start: row.start.clone() });
                }
            }
        }
    }
    found
}

/// `root` and every live descendant, by parent links. Empty when the process
/// table cannot be read (the confirmation then reports that).
#[cfg(unix)]
pub fn snapshot_tree(root: u32) -> Vec<ProcIdentity> {
    table().map(|rows| descendants_in(&rows, root)).unwrap_or_default()
}

/// Merge snapshots, keeping one entry per (pid, start).
pub fn merge(into: &mut Vec<ProcIdentity>, more: Vec<ProcIdentity>) {
    for identity in more {
        if !into.iter().any(|known| known.pid == identity.pid && known.start == identity.start) {
            into.push(identity);
        }
    }
}

/// Send `signal` to the process group `pgid`. `Ok(false)`: the group no
/// longer exists (`ESRCH`).
#[cfg(unix)]
pub fn signal_group(pgid: u32, signal: i32) -> Result<bool, String> {
    let Ok(pgid_i) = i32::try_from(pgid) else { return Err(format!("invalid process group {pgid}")) };
    if pgid_i <= 1 {
        return Err(format!("refusing to signal process group {pgid}"));
    }
    // SAFETY: plain syscall with integer arguments.
    if unsafe { libc::kill(-pgid_i, signal) } == 0 {
        return Ok(true);
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) {
        Ok(false)
    } else {
        Err(format!("kill(-{pgid}, {signal}) failed: {error}"))
    }
}

/// Live (non-zombie) members of `pgid`; empty when the group is gone or holds
/// only zombies. `Err` when this cannot be decided.
#[cfg(unix)]
fn live_group_members(pgid: u32) -> Result<Vec<u32>, String> {
    match signal_group(pgid, 0) {
        Ok(false) => return Ok(Vec::new()),
        Ok(true) => {}
        Err(error) => return Err(error),
    }
    let rows = table().ok_or_else(|| format!("process group {pgid} still exists and the process table is unreadable"))?;
    let members: Vec<&ProcRow> = rows.iter().filter(|row| row.pgid == pgid).collect();
    let live: Vec<u32> = members.iter().filter(|row| !row.zombie).map(|row| row.pid).collect();
    if live.is_empty() && members.is_empty() {
        // kill(-pgid, 0) saw a member the table did not (it exited in
        // between, or the table hides it). Decide on the next poll.
        return Err(format!("process group {pgid} still answers kill(-pgid, 0) but no member is visible"));
    }
    Ok(live)
}

#[cfg(unix)]
fn still_running<'a>(tracked: &'a [ProcIdentity], rows: &[ProcRow]) -> Vec<&'a ProcIdentity> {
    tracked
        .iter()
        .filter(|identity| rows.iter().any(|row| row.pid == identity.pid && row.start == identity.start && !row.zombie))
        .collect()
}

/// SIGKILL every tracked descendant that is still running outside `pgid`
/// (same pid and start time as recorded). Returns notes for the report.
#[cfg(unix)]
pub fn kill_escaped(pgid: u32, tracked: &[ProcIdentity]) -> Vec<String> {
    let Some(rows) = table() else {
        return if tracked.is_empty() { Vec::new() } else { vec!["process table unreadable; escaped descendants not killed".into()] };
    };
    let mut notes = Vec::new();
    for identity in still_running(tracked, &rows) {
        let in_group = rows.iter().any(|row| row.pid == identity.pid && row.pgid == pgid);
        if in_group {
            continue;
        }
        let Ok(pid) = i32::try_from(identity.pid) else { continue };
        // SAFETY: plain syscall with integer arguments.
        let rc = unsafe { libc::kill(pid, libc::SIGKILL) };
        notes.push(if rc == 0 {
            format!("killed descendant pid {} that had left process group {pgid}", identity.pid)
        } else {
            format!("could not kill descendant pid {}: {}", identity.pid, std::io::Error::last_os_error())
        });
    }
    notes
}

/// Wait up to `timeout` for the process group and every tracked descendant
/// to be gone. Never `Confirmed` when something could not be checked.
#[cfg(unix)]
pub fn confirm_tree_gone(pgid: u32, tracked: &[ProcIdentity], timeout: Duration) -> StopConfirmation {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let mut reasons = Vec::new();
        match live_group_members(pgid) {
            Ok(live) if live.is_empty() => {}
            Ok(live) => reasons.push(format!("process group {pgid} still has running members {live:?}")),
            Err(error) => reasons.push(error),
        }
        if !tracked.is_empty() {
            match table() {
                Some(rows) => {
                    let left: Vec<u32> = still_running(tracked, &rows).iter().map(|identity| identity.pid).collect();
                    if !left.is_empty() {
                        reasons.push(format!("descendant processes still running: {left:?}"));
                    }
                }
                None => reasons.push("process table unreadable; descendants not verified".into()),
            }
        }
        if reasons.is_empty() {
            return StopConfirmation::Confirmed;
        }
        if std::time::Instant::now() >= deadline {
            return StopConfirmation::FailedOrUnconfirmed { reasons };
        }
        std::thread::sleep(POLL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proc_stat_parsing_survives_spaces_and_parentheses_in_the_name() {
        let stat = "4242 (node (pi) x) S 4200 4100 4100 0 -1 4194560 1 0 0 0 0 0 0 0 20 0 1 0 987654 1000 10";
        let row = parse_proc_stat(4242, stat).unwrap();
        assert_eq!((row.pid, row.ppid, row.pgid, row.zombie, row.start.as_str()), (4242, 4200, 4100, false, "987654"));
        let zombie = parse_proc_stat(7, "7 (sh) Z 1 7 7 0 -1 0 0 0 0 0 0 0 0 0 20 0 1 0 55 0 0").unwrap();
        assert!(zombie.zombie);
    }

    #[test]
    fn ps_parsing_and_descendant_walk() {
        let rows = parse_ps(
            "  100     1   100 Ss   Mon Oct 19 10:00:00 2026\n  101   100   100 S    Mon Oct 19 10:00:01 2026\n  102   101   102 S    Mon Oct 19 10:00:02 2026\n  103   102   102 Z    Mon Oct 19 10:00:03 2026\n  200     1   200 S    Mon Oct 19 10:00:04 2026\n",
        );
        assert_eq!(rows.len(), 5);
        let tree = descendants_in(&rows, 100);
        let pids: Vec<u32> = tree.iter().map(|identity| identity.pid).collect();
        assert_eq!(pids, [100, 101, 102], "zombie 103 and unrelated 200 excluded");
        assert_eq!(tree[2].pgid, 102, "a detached descendant keeps its own group");
        let mut merged = tree.clone();
        merge(&mut merged, descendants_in(&rows, 101));
        assert_eq!(merged.len(), 3, "merge dedupes by pid + start");
    }

    /// The parent exits while its child (same group) keeps running: not
    /// confirmed. After the group is killed: confirmed.
    #[cfg(unix)]
    #[test]
    fn parent_exit_with_a_live_child_is_not_confirmed() {
        use std::os::unix::process::CommandExt;
        let mut command = std::process::Command::new("sh");
        command.args(["-c", "sleep 30 & exit 0"]).stdout(std::process::Stdio::null()).process_group(0);
        let mut child = command.spawn().expect("sh");
        let pgid = child.id();
        assert!(child.wait().unwrap().success(), "parent exited");
        let verdict = confirm_tree_gone(pgid, &[], Duration::from_millis(200));
        assert!(matches!(verdict, StopConfirmation::FailedOrUnconfirmed { .. }), "{verdict:?}");
        assert!(signal_group(pgid, libc::SIGKILL).unwrap());
        assert_eq!(confirm_tree_gone(pgid, &[], STOP_CONFIRM_TIMEOUT), StopConfirmation::Confirmed);
    }

    /// A descendant that moved to its own session/group survives the group
    /// kill; it is found through the snapshot, killed by pid, then confirmed.
    #[cfg(target_os = "linux")]
    #[test]
    fn escaped_descendant_blocks_confirmation_until_killed() {
        use std::os::unix::process::CommandExt;
        if std::process::Command::new("setsid").arg("--version").output().is_err() {
            eprintln!("SKIPPED escaped_descendant_blocks_confirmation_until_killed: setsid not installed");
            return;
        }
        let mut command = std::process::Command::new("sh");
        command
            .args(["-c", "setsid sleep 31 & sleep 30"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .process_group(0);
        let mut child = command.spawn().expect("sh");
        let pgid = child.id();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let tracked = loop {
            let tree = snapshot_tree(pgid);
            if tree.iter().any(|identity| identity.pgid != pgid) || std::time::Instant::now() > deadline {
                break tree;
            }
            std::thread::sleep(POLL);
        };
        let escaped: Vec<_> = tracked.iter().filter(|identity| identity.pgid != pgid).cloned().collect();
        assert_eq!(escaped.len(), 1, "setsid child recorded: {tracked:?}");
        signal_group(pgid, libc::SIGKILL).unwrap();
        let _ = child.wait();
        let verdict = confirm_tree_gone(pgid, &tracked, Duration::from_millis(200));
        assert!(
            matches!(&verdict, StopConfirmation::FailedOrUnconfirmed { reasons } if reasons.iter().any(|r| r.contains(&escaped[0].pid.to_string()))),
            "{verdict:?}"
        );
        let notes = kill_escaped(pgid, &tracked);
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert_eq!(confirm_tree_gone(pgid, &tracked, STOP_CONFIRM_TIMEOUT), StopConfirmation::Confirmed);
    }
}
