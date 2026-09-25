//! An OS-owned lease: process death releases it, while a slow/live owner never
//! loses ownership due to a guessed heartbeat timeout. Never unlink lock files.
use sha2::{Digest, Sha256};
use std::{
    fs::{File, OpenOptions},
    path::Path,
};

pub(super) const KERNEL_RUN_ALREADY_OWNED: &str = "kernel.run_already_owned";

pub(super) struct KernelRunLock {
    _file: File,
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
        file.try_lock().map_err(|error| match error {
            std::fs::TryLockError::WouldBlock => KERNEL_RUN_ALREADY_OWNED.to_owned(),
            other => format!("Kernel Run cannot be locked: {other}"),
        })?;
        Ok(Self { _file: file })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// Bounded handoff window for the assertions below. This test must finish
    /// even when a lease is unexpectedly still held, so it never blocks forever.
    const HANDOFF_TIMEOUT: Duration = Duration::from_secs(5);

    #[test]
    fn ownership_is_exclusive_and_released_on_drop() {
        let directory =
            std::env::temp_dir().join(format!("fox-kernel-lock-{}", uuid::Uuid::new_v4()));
        let first = KernelRunLock::acquire(&directory, "run/../../not-a-path").unwrap();
        assert_eq!(KernelRunLock::acquire(&directory, "run/../../not-a-path")
            .err().as_deref(), Some(KERNEL_RUN_ALREADY_OWNED));
        let independent = KernelRunLock::acquire(&directory, "other-run").unwrap();
        drop(first);
        let deadline = Instant::now() + HANDOFF_TIMEOUT;
        let recovered = loop {
            match KernelRunLock::acquire(&directory, "run/../../not-a-path") {
                Ok(lock) => break lock,
                // Only a real ownership conflict may be retried; the released
                // handle's close is asynchronous enough to need this window.
                Err(error) if error == KERNEL_RUN_ALREADY_OWNED && Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(error) => panic!("lease was not released within {HANDOFF_TIMEOUT:?}: {error}"),
            }
        };
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
