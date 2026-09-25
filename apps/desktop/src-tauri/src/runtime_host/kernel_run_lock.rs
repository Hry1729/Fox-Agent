//! An OS-owned lease: process death releases it, while a slow/live owner never
//! loses ownership due to a guessed heartbeat timeout. Never unlink lock files.
use sha2::{Digest, Sha256};
use std::{
    fs::{File, OpenOptions},
    path::Path,
};

pub(super) const KERNEL_RUN_ALREADY_OWNED: &str = "kernel.run_already_owned";

/// Test-only, opt-in observation of who holds the Run's OS lease.
///
/// The lease is an advisory file lock, so a conflict says nothing about *which*
/// path holds it. This registry records the acquiring thread's name (fixture
/// threads and blocking-pool wake tasks are named distinctly) plus releases,
/// gated behind `FOX_RUN_LOCK_TRACE` so the ordinary suite pays one cached
/// atomic load and allocates nothing.
#[cfg(test)]
pub(crate) mod lock_trace {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Mutex, OnceLock};
    use std::time::Instant;

    const MAX_ENTRIES: usize = 4096;

    fn enabled() -> bool {
        static ENABLED: OnceLock<AtomicBool> = OnceLock::new();
        ENABLED
            .get_or_init(|| AtomicBool::new(std::env::var_os("FOX_RUN_LOCK_TRACE").is_some()))
            .load(Ordering::Relaxed)
    }

    fn store() -> &'static Mutex<Vec<String>> {
        static STORE: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
        STORE.get_or_init(|| Mutex::new(Vec::new()))
    }

    /// Thread name plus a monotonic offset: two contended paths can be told
    /// apart without guessing from a bare error string.
    pub(crate) fn who() -> String {
        static ORIGIN: OnceLock<Instant> = OnceLock::new();
        let at = ORIGIN.get_or_init(Instant::now).elapsed().as_millis();
        let name = std::thread::current()
            .name()
            .unwrap_or("<unnamed>")
            .to_owned();
        format!("{name}@{at}ms")
    }

    pub(crate) fn record(event: String) {
        if !enabled() {
            return;
        }
        if let Ok(mut entries) = store().lock() {
            if entries.len() >= MAX_ENTRIES {
                entries.drain(..MAX_ENTRIES / 2);
            }
            entries.push(event);
        }
    }

    pub(crate) fn acquired(run_id: &str) {
        record(format!("acquired run={run_id} by={}", who()));
    }

    /// Names the most recent acquirer: that is the path whose release has not
    /// happened yet when a later attempt conflicts.
    pub(crate) fn conflict(run_id: &str) {
        let last = store()
            .lock()
            .ok()
            .and_then(|entries| {
                entries
                    .iter()
                    .rev()
                    .find(|entry| entry.starts_with("acquired "))
                    .cloned()
            })
            .unwrap_or_else(|| "<none recorded>".to_owned());
        record(format!("CONFLICT run={run_id} by={} last_holder={last}", who()));
    }

    pub(crate) fn released(run_id: &str) {
        record(format!("released run={run_id} by={}", who()));
    }

    /// Every event so far, drained by a bounded probe run so it can print.
    pub(crate) fn take() -> Vec<String> {
        store()
            .lock()
            .map(|mut entries| std::mem::take(&mut *entries))
            .unwrap_or_default()
    }
}

pub(super) struct KernelRunLock {
    _file: File,
    #[cfg(test)]
    run_id: String,
}

impl KernelRunLock {
    pub(super) fn acquire(sessions_dir: &Path, run_id: &str) -> Result<Self, String> {
        if run_id.trim().is_empty() || run_id.len() > 512 {
            return Err("invalid Kernel Run lock identity".into());
        }
        let directory = sessions_dir.join("kernel-run-locks");
        std::fs::create_dir_all(&directory)
            .map_err(|error| format!("Kernel lock directory: {error}"))?;
        // Never use a run id as a path component.
        let filename = format!("{}.lock", hex::encode(Sha256::digest(run_id.as_bytes())));
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(directory.join(filename))
            .map_err(|error| format!("Kernel lock file: {error}"))?;
        #[cfg(test)]
        lock_trace::record(format!("attempt run={run_id} by={}", lock_trace::who()));
        file.try_lock().map_err(|error| match error {
            std::fs::TryLockError::WouldBlock => {
                #[cfg(test)]
                lock_trace::conflict(run_id);
                KERNEL_RUN_ALREADY_OWNED.to_owned()
            }
            other => format!("Kernel Run cannot be locked: {other}"),
        })?;
        #[cfg(test)]
        lock_trace::acquired(run_id);
        Ok(Self {
            _file: file,
            #[cfg(test)]
            run_id: run_id.to_owned(),
        })
    }
}

impl Drop for KernelRunLock {
    fn drop(&mut self) {
        // Test-only observation; the real release is the `_file` handle's close.
        #[cfg(test)]
        lock_trace::released(&self.run_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ownership_is_exclusive_and_released_on_drop() {
        let directory =
            std::env::temp_dir().join(format!("fox-kernel-lock-{}", uuid::Uuid::new_v4()));
        let first = KernelRunLock::acquire(&directory, "run/../../not-a-path").unwrap();
        assert_eq!(KernelRunLock::acquire(&directory, "run/../../not-a-path")
            .err().as_deref(), Some(KERNEL_RUN_ALREADY_OWNED));
        let independent = KernelRunLock::acquire(&directory, "other-run").unwrap();
        drop(first);
        let recovered = KernelRunLock::acquire(&directory, "run/../../not-a-path").unwrap();
        assert!(KernelRunLock::acquire(&directory, "run/../../not-a-path").is_err());
        drop(recovered);
        drop(independent);
        assert_eq!(
            std::fs::read_dir(directory.join("kernel-run-locks"))
                .unwrap()
                .count(),
            2
        );
    }
}
