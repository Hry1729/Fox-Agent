//! Process identity for durable job ownership.
//!
//! A background job records which process owns it. Telling "my process" apart
//! from "a process that had this pid before the Host restarted" needs more than
//! the pid: the operating system reuses pids. The start marker below is captured
//! once per process and compared on every later read.

use std::sync::OnceLock;

static PROCESS_START_MARKER: OnceLock<Option<i64>> = OnceLock::new();

/// Unknown/access-denied ownership stays untouched. Another live Fox process
/// sharing the database is not an orphan merely because its PID differs.
pub(crate) fn owner_is_gone(pid: Option<i64>, marker: Option<i64>) -> bool {
    let Some(pid) = pid.and_then(|id| u32::try_from(id).ok()).filter(|id| *id > 0) else { return true; };
    if pid == std::process::id() { return marker != process_start_marker(); }
    #[cfg(target_os = "windows")]
    unsafe {
        #[repr(C)] struct FileTime { low: u32, high: u32 }
        #[link(name = "kernel32")]
        extern "system" {
            fn OpenProcess(access: u32, inherit: i32, pid: u32) -> isize;
            fn CloseHandle(handle: isize) -> i32;
            fn GetLastError() -> u32;
            fn GetExitCodeProcess(handle: isize, code: *mut u32) -> i32;
            fn GetProcessTimes(handle: isize, creation: *mut FileTime, exit: *mut FileTime, kernel: *mut FileTime, user: *mut FileTime) -> i32;
        }
        let handle = OpenProcess(0x1000, 0, pid);
        if handle == 0 { return matches!(GetLastError(), 87 | 1168); }
        let mut exit_code = 259;
        let stopped = GetExitCodeProcess(handle, &mut exit_code) != 0 && exit_code != 259;
        let mut creation: FileTime = std::mem::zeroed();
        let mut exit: FileTime = std::mem::zeroed();
        let mut kernel: FileTime = std::mem::zeroed();
        let mut user: FileTime = std::mem::zeroed();
        let known = GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user) != 0;
        CloseHandle(handle);
        let start = ((((creation.high as u64) << 32) | creation.low as u64) / 10_000) as i64;
        return stopped || (known && marker.is_some_and(|expected| expected != start));
    }
    #[cfg(not(target_os = "windows"))]
    { matches!(std::fs::metadata(format!("/proc/{pid}")), Err(error) if error.kind() == std::io::ErrorKind::NotFound) }
}

#[cfg(test)]
mod owner_tests {
    #[test]
    fn current_owner_is_preserved_and_reused_pid_is_detected() {
        let pid = Some(i64::from(std::process::id()));
        assert!(!super::owner_is_gone(pid, super::process_start_marker()));
        assert!(super::owner_is_gone(pid, Some(1)));
        assert!(super::owner_is_gone(Some(i64::MAX), Some(1)));
    }
}

/// A stable identifier for *this* process instance, or `None` when it cannot be
/// determined. `None` must never be treated as "this process" by a caller that
/// is deciding whether it may adopt an existing job.
pub(crate) fn process_start_marker() -> Option<i64> {
    *PROCESS_START_MARKER.get_or_init(|| {
        // Prefer the real process creation time so a pid reused after a restart
        // is a different marker. Reading it is best-effort; when it fails, fall
        // back to the wall clock captured at first use, which is still distinct
        // per process instance (it is never shared between two runs).
        process_creation_time_ms().or_else(|| Some(crate::database::now_ms()))
    })
}

#[cfg(target_os = "windows")]
fn process_creation_time_ms() -> Option<i64> {
    use std::mem::zeroed;
    #[repr(C)]
    struct FileTime {
        low: u32,
        high: u32,
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentProcess() -> isize;
        fn GetProcessTimes(
            process: isize,
            creation: *mut FileTime,
            exit: *mut FileTime,
            kernel: *mut FileTime,
            user: *mut FileTime,
        ) -> i32;
    }
    unsafe {
        let mut creation: FileTime = zeroed();
        let mut exit: FileTime = zeroed();
        let mut kernel: FileTime = zeroed();
        let mut user: FileTime = zeroed();
        if GetProcessTimes(
            GetCurrentProcess(),
            &mut creation,
            &mut exit,
            &mut kernel,
            &mut user,
        ) == 0
        {
            return None;
        }
        // FILETIME counts 100-ns intervals since 1601; milliseconds are enough
        // to distinguish two process instances on one machine.
        let ticks = ((creation.high as u64) << 32) | creation.low as u64;
        (ticks > 0).then(|| (ticks / 10_000) as i64)
    }
}

#[cfg(not(target_os = "windows"))]
fn process_creation_time_ms() -> Option<i64> {
    None
}
