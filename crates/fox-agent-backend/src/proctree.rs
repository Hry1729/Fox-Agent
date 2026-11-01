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
    // console-policy: Unix only; there are no console windows to hide.
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

/// 同一个进程：pid 与启动时间都相同。启动时间不同是 pid 复用，不算同一个。
fn same_process(known: &ProcIdentity, now: &ProcIdentity) -> bool {
    known.pid == now.pid && known.start == now.start
}

/// 当前快照里不在基线里的进程（pid 与启动时间都要比对）。
///
/// 基线成员留下；同 pid 不同启动时间要清扫。空基线表示「成功读到了一份空快照」，
/// 当前进程全部都是目标。查询失败的「无基线」不是空切片，调用方必须另走 fail-closed，
/// 不能把失败当成空基线来猜。
pub fn sweep_targets(baseline: &[ProcIdentity], current: &[ProcIdentity]) -> Vec<ProcIdentity> {
    current.iter().filter(|now| !baseline.iter().any(|known| same_process(known, now))).cloned().collect()
}

/// Windows Job 的 pid 列表版：只比 pid。空基线时当前 pid 全部都是目标。
pub fn sweep_pid_targets(baseline: &[u32], current: &[u32]) -> Vec<u32> {
    let mut targets: Vec<u32> = current.iter().copied().filter(|pid| !baseline.contains(pid)).collect();
    targets.sort_unstable();
    targets.dedup();
    targets
}

/// One row of a whole-system process table (Windows Toolhelp snapshot).
/// `created` is the creation time (FILETIME, 100 ns since 1601); `None` when
/// the process could not be opened for `PROCESS_QUERY_LIMITED_INFORMATION`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemProcess {
    pub pid: u32,
    pub parent: u32,
    pub name: String,
    pub created: Option<u64>,
}

/// What the out-of-Job pass found (13.35).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OutsideDescendants {
    /// Not in the Job, descended (by parent links) from a Job member, created
    /// at or after the turn baseline: started during this turn and escaped.
    pub targets: Vec<SystemProcess>,
    /// Same ancestry, but the creation time could not be read (another user's
    /// process, e.g. a sandbox account): cannot be attributed to this turn and
    /// almost certainly cannot be terminated either. Reported, never guessed.
    pub unverifiable: Vec<SystemProcess>,
}

/// Processes that left the backend's Job but still descend from one of its
/// members, created during this turn.
///
/// A Job only holds processes created by its members through the normal
/// creation path. A process started on a member's behalf by a service
/// (Secondary Logon / `CreateProcessWithLogonW`, as Codex's elevated Windows
/// sandbox does for `codex-command-runner`) is outside the Job; when Windows
/// records the member as its parent it is found here by walking parent links.
///
/// Every link is checked against pid reuse: a child must not be older than the
/// parent it names. `since` excludes anything that already existed before the
/// prompt (a session-level helper started in an earlier turn is kept).
pub fn outside_job_descendants(members: &[u32], table: &[SystemProcess], since: u64) -> OutsideDescendants {
    use std::collections::HashMap;
    let by_pid: HashMap<u32, &SystemProcess> = table.iter().map(|row| (row.pid, row)).collect();
    let mut found = OutsideDescendants::default();
    for row in table {
        if members.contains(&row.pid) || row.pid == 0 {
            continue;
        }
        // Walk up at most 32 links until a Job member, a missing parent, a
        // reused pid or a cycle ends the chain.
        let mut child = row;
        let mut linked = false;
        for _ in 0..32 {
            if child.parent == 0 || child.parent == child.pid {
                break;
            }
            let Some(parent) = by_pid.get(&child.parent) else { break };
            if let (Some(child_created), Some(parent_created)) = (child.created, parent.created) {
                if child_created < parent_created {
                    break; // the named parent is a newer process with a reused pid
                }
            }
            if members.contains(&parent.pid) {
                linked = true;
                break;
            }
            child = parent;
        }
        if !linked {
            continue;
        }
        match row.created {
            Some(created) if created >= since => found.targets.push(row.clone()),
            Some(_) => {}
            None => found.unverifiable.push(row.clone()),
        }
    }
    found.targets.sort_by_key(|row| row.pid);
    found.unverifiable.sort_by_key(|row| row.pid);
    found
}

/// Windows: the current time as FILETIME (100 ns since 1601), comparable with
/// process creation times.
#[cfg(windows)]
pub fn filetime_now() -> u64 {
    let mut now = windows_sys::Win32::Foundation::FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
    unsafe { windows_sys::Win32::System::SystemInformation::GetSystemTimeAsFileTime(&mut now) };
    (u64::from(now.dwHighDateTime) << 32) | u64::from(now.dwLowDateTime)
}

/// Windows: creation time of `pid` (FILETIME), `None` when it cannot be opened.
#[cfg(windows)]
pub fn process_created(pid: u32) -> Option<u64> {
    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME};
    use windows_sys::Win32::System::Threading::{GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        return None;
    }
    let zero = || FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
    let (mut created, mut exited, mut kernel, mut user) = (zero(), zero(), zero(), zero());
    let ok = unsafe { GetProcessTimes(handle, &mut created, &mut exited, &mut kernel, &mut user) };
    unsafe { CloseHandle(handle) };
    (ok != 0).then(|| (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime))
}

/// Windows: the whole process table with creation times (Toolhelp snapshot).
#[cfg(windows)]
pub fn system_process_table() -> Result<Vec<SystemProcess>, String> {
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS};
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Err(format!("CreateToolhelp32Snapshot(processes) 失败 (Win32 {})", unsafe { GetLastError() }));
    }
    let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
    let mut rows = Vec::new();
    let mut ok = unsafe { Process32FirstW(snapshot, &mut entry) } != 0;
    while ok {
        let len = entry.szExeFile.iter().position(|unit| *unit == 0).unwrap_or(entry.szExeFile.len());
        rows.push(SystemProcess {
            pid: entry.th32ProcessID,
            parent: entry.th32ParentProcessID,
            name: String::from_utf16_lossy(&entry.szExeFile[..len]),
            created: None,
        });
        ok = unsafe { Process32NextW(snapshot, &mut entry) } != 0;
    }
    unsafe { CloseHandle(snapshot) };
    for row in &mut rows {
        row.created = process_created(row.pid);
    }
    Ok(rows)
}

/// Windows: terminate `pid` only if it is still the process created at
/// `created` (pid reuse guard). `Ok(false)`: already gone or reused.
#[cfg(windows)]
pub fn terminate_verified(pid: u32, created: u64, wait: Duration) -> Result<bool, String> {
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_INVALID_PARAMETER, FILETIME, WAIT_OBJECT_0};
    use windows_sys::Win32::System::Threading::{GetProcessTimes, OpenProcess, TerminateProcess, WaitForSingleObject, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE};
    let handle = unsafe { OpenProcess(PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE, 0, pid) };
    if handle.is_null() {
        let error = unsafe { GetLastError() };
        if error == ERROR_INVALID_PARAMETER {
            return Ok(false);
        }
        return Err(format!("OpenProcess({pid}) 失败 (Win32 {error})，Job 外的本轮进程无法结束"));
    }
    struct Close(windows_sys::Win32::Foundation::HANDLE);
    impl Drop for Close {
        fn drop(&mut self) {
            unsafe { CloseHandle(self.0) };
        }
    }
    let _close = Close(handle);
    let zero = || FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
    let (mut c, mut e, mut k, mut u) = (zero(), zero(), zero(), zero());
    if unsafe { GetProcessTimes(handle, &mut c, &mut e, &mut k, &mut u) } == 0
        || ((u64::from(c.dwHighDateTime) << 32) | u64::from(c.dwLowDateTime)) != created
    {
        return Ok(false);
    }
    if unsafe { TerminateProcess(handle, 1) } == 0 {
        return Err(format!("TerminateProcess({pid}) 失败 (Win32 {})", unsafe { GetLastError() }));
    }
    let millis = u32::try_from(wait.as_millis()).unwrap_or(u32::MAX);
    if unsafe { WaitForSingleObject(handle, millis) } == WAIT_OBJECT_0 {
        Ok(true)
    } else {
        Err(format!("pid {pid} 在 {} ms 内没有退出", wait.as_millis()))
    }
}

/// 进程已经结束：不存在，或（Linux）已经是僵尸。
///
/// 取消清扫杀掉的工具子进程，父进程是后端自己，后端不 `wait` 时会变成僵尸。
/// 僵尸不再执行代码，测试里视为已经结束。
#[cfg(unix)]
pub fn process_ended(pid: u32) -> bool {
    let Ok(pid_i) = i32::try_from(pid) else { return false };
    if pid_i <= 0 {
        return false;
    }
    // SAFETY: 只查询进程是否存在，不发终止信号。
    let rc = unsafe { libc::kill(pid_i, 0) };
    if rc != 0 {
        return std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH);
    }
    #[cfg(target_os = "linux")]
    {
        return linux_stat_is_zombie(pid);
    }
    #[cfg(not(target_os = "linux"))]
    {
        match table() {
            Some(rows) => match rows.iter().find(|row| row.pid == pid) {
                Some(row) => row.zombie,
                // kill 还看得到，但 ps 没有这一行：不当成已结束。
                None => false,
            },
            None => false,
        }
    }
}

/// `/proc/<pid>/stat` 状态为 Z 或 X 时，进程已经退出，只等回收。
#[cfg(target_os = "linux")]
fn linux_stat_is_zombie(pid: u32) -> bool {
    let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
        // kill(pid, 0) 刚刚成功，随即读不到 stat：进程正在退出。
        return true;
    };
    stat.rfind(')')
        .and_then(|index| stat[index + 1..].split_whitespace().next())
        .is_some_and(|state| state == "Z" || state == "X")
}

/// Windows：`OpenProcess` + `GetExitCodeProcess` / `WaitForSingleObject`。
/// 打不开视为已不存在。`STILL_ACTIVE` 且等待立即超时则仍在运行。
#[cfg(windows)]
pub fn process_ended(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, STILL_ACTIVE, WAIT_OBJECT_0};
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, WaitForSingleObject, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
    };
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE, 0, pid) };
    if handle.is_null() {
        return true;
    }
    let mut code = 0u32;
    let got = unsafe { GetExitCodeProcess(handle, &mut code) };
    if got != 0 && code != STILL_ACTIVE as u32 {
        unsafe { CloseHandle(handle) };
        return true;
    }
    let waited = unsafe { WaitForSingleObject(handle, 0) };
    unsafe { CloseHandle(handle) };
    waited == WAIT_OBJECT_0
}

/// 对差集里的进程发 SIGKILL。已经不在（ESRCH）、已是僵尸、或 pid 已被复用的，不记失败。
///
/// 离开进程组的（`detached`）也按快照里的 pid 杀，不向进程组发信号，避免误伤后端。
/// 进程表读不到时一条都不杀，全部记入失败（fail-closed）。
#[cfg(unix)]
pub(crate) fn kill_sweep_targets(targets: &[ProcIdentity]) -> (Vec<u32>, Vec<String>) {
    let Some(rows) = table() else {
        return (Vec::new(), targets.iter().map(|identity| format!("进程表不可读，未向 pid {} 发 SIGKILL", identity.pid)).collect());
    };
    let mut killed = Vec::new();
    let mut failed = Vec::new();
    for identity in targets {
        let same = rows.iter().find(|row| row.pid == identity.pid && row.start == identity.start);
        match same {
            None => continue,
            Some(row) if row.zombie => continue,
            Some(_) => {}
        }
        let Ok(pid) = i32::try_from(identity.pid) else {
            failed.push(format!("pid {} 无法作为信号目标", identity.pid));
            continue;
        };
        if pid <= 1 {
            failed.push(format!("拒绝向 pid {pid} 发 SIGKILL"));
            continue;
        }
        // SAFETY: 参数是整数 pid 与 SIGKILL。
        let rc = unsafe { libc::kill(pid, libc::SIGKILL) };
        if rc == 0 {
            killed.push(identity.pid);
            continue;
        }
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ESRCH) {
            continue;
        }
        failed.push(format!("SIGKILL pid {} 失败: {error}", identity.pid));
    }
    (killed, failed)
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
    use super::{outside_job_descendants, SystemProcess};

    fn row(pid: u32, parent: u32, created: Option<u64>) -> SystemProcess {
        SystemProcess { pid, parent, name: format!("p{pid}"), created }
    }

    #[test]
    fn outside_job_descendants_follow_parent_links_from_job_members_created_this_turn() {
        // 10 = backend root (Job), 11 = codex app-server (Job).
        // 20 = runner outside the Job, parent 11, created this turn; 21 = pwsh, 22 = PING.
        // 30 = older helper outside the Job (before the baseline): kept.
        // 40 = unrelated process; 50 = parent pid reused by a newer process.
        let table = vec![
            row(10, 1, Some(100)), row(11, 10, Some(110)),
            row(20, 11, Some(500)), row(21, 20, Some(510)), row(22, 21, Some(520)),
            row(30, 11, Some(200)),
            row(40, 1, Some(600)),
            row(50, 11, Some(50)),
            row(60, 11, None),
        ];
        let found = outside_job_descendants(&[10, 11], &table, 400);
        assert_eq!(found.targets.iter().map(|row| row.pid).collect::<Vec<_>>(), vec![20, 21, 22]);
        assert_eq!(found.unverifiable.iter().map(|row| row.pid).collect::<Vec<_>>(), vec![60]);
    }

    #[test]
    fn outside_job_descendants_stop_at_reused_pids_and_cycles() {
        // 70 names 11 as parent but is older than 11: pid reuse, not a descendant.
        let table = vec![row(11, 10, Some(300)), row(70, 11, Some(250)), row(80, 81, Some(900)), row(81, 80, Some(900))];
        let found = outside_job_descendants(&[11], &table, 0);
        assert!(found.targets.is_empty(), "{found:?}");
    }

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

    #[test]
    fn sweep_targets_kills_reused_pid_keeps_baseline_and_empty_baseline_is_everyone() {
        let baseline = vec![
            ProcIdentity { pid: 10, pgid: 10, start: "100".into() },
            ProcIdentity { pid: 11, pgid: 10, start: "101".into() },
        ];
        let current = vec![
            ProcIdentity { pid: 10, pgid: 10, start: "100".into() },
            // 同 pid、启动时间变了：pid 被复用，要杀。
            ProcIdentity { pid: 11, pgid: 99, start: "999".into() },
            // 新进程，pgid 不同也要杀（detached 离开了进程组）。
            ProcIdentity { pid: 12, pgid: 12, start: "102".into() },
            // 启动时间相同则 pgid 差异不构成新进程。
            ProcIdentity { pid: 10, pgid: 77, start: "100".into() },
        ];
        let targets = sweep_targets(&baseline, &current);
        let keys: Vec<(u32, &str)> = targets.iter().map(|identity| (identity.pid, identity.start.as_str())).collect();
        assert_eq!(keys, vec![(11, "999"), (12, "102")]);
        assert!(sweep_targets(&baseline, &baseline).is_empty(), "基线成员不杀");
        // 空切片是「读成功但没有进程」，不是查询失败的无基线。
        assert_eq!(sweep_targets(&[], &current).len(), current.len(), "空基线：当前进程全部都是目标");
        assert!(sweep_targets(&baseline, &[]).is_empty());
    }

    #[test]
    fn sweep_pid_targets_drops_baseline_pids_and_dedupes() {
        assert_eq!(sweep_pid_targets(&[1, 2], &[2, 3, 3, 4]), vec![3, 4]);
        assert!(sweep_pid_targets(&[7], &[7]).is_empty(), "基线 pid 不杀");
        assert_eq!(sweep_pid_targets(&[], &[4, 4, 5]), vec![4, 5], "空基线：当前 pid 全部都是目标");
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn the_current_process_has_not_ended() {
        assert!(!process_ended(std::process::id()));
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
