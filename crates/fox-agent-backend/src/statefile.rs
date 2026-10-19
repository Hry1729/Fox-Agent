//! Small JSON state files under Fox's state directory (`%LOCALAPPDATA%\Fox\state`
//! on Windows), shared by several Fox processes: read / write under an
//! exclusive lock on a sibling `.lock` file (`std::fs::File::lock`:
//! `LockFileEx` over the whole range on Windows, `flock` on Unix; two handles
//! conflict even inside one process), written atomically (temp file +
//! rename). A missing file reads as the default value; a file that exists
//! but does not parse is an error (callers fail closed rather than start from
//! an empty ledger and forget what they must undo).

use serde::de::DeserializeOwned;
use serde::Serialize;
use std::fs::File;
use std::path::{Path, PathBuf};

/// Exclusive lock held for as long as the value lives (released on drop, and
/// by the OS when the process dies).
#[derive(Debug)]
pub struct StateLock {
    file: File,
    path: PathBuf,
}

impl StateLock {
    /// Block until the lock file `path` is held exclusively. Creates the
    /// parent directory and the file when missing.
    pub fn acquire(path: &Path) -> std::io::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).open(path)?;
        file.lock()?;
        Ok(Self { file, path: path.to_path_buf() })
    }

    /// Try once without blocking; `Ok(None)` when another holder has it.
    pub fn try_acquire(path: &Path) -> std::io::Result<Option<Self>> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).open(path)?;
        match file.try_lock() {
            Ok(()) => Ok(Some(Self { file, path: path.to_path_buf() })),
            Err(std::fs::TryLockError::WouldBlock) => Ok(None),
            Err(std::fs::TryLockError::Error(error)) => Err(error),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for StateLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

/// `<file>.lock` next to a state file.
pub fn lock_path_for(file: &Path) -> PathBuf {
    let mut name = file.file_name().map(|name| name.to_os_string()).unwrap_or_default();
    name.push(".lock");
    file.with_file_name(name)
}

/// Read a JSON state file; missing = `T::default()`, unparsable = error.
pub fn read_json<T: DeserializeOwned + Default>(path: &Path) -> Result<T, String> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|error| format!("{} is corrupt: {error}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(T::default()),
        Err(error) => Err(format!("cannot read {}: {error}", path.display())),
    }
}

/// Write a JSON state file atomically (temp file in the same directory, then
/// rename over the old one).
pub fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let parent = path.parent().ok_or_else(|| format!("{} has no parent directory", path.display()))?;
    std::fs::create_dir_all(parent).map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    let bytes = serde_json::to_vec_pretty(value).map_err(|error| format!("cannot serialize {}: {error}", path.display()))?;
    let mut name = path.file_name().map(|name| name.to_os_string()).unwrap_or_default();
    name.push(format!(".tmp-{}", std::process::id()));
    let temp = path.with_file_name(name);
    std::fs::write(&temp, &bytes).map_err(|error| format!("cannot write {}: {error}", temp.display()))?;
    std::fs::rename(&temp, path).map_err(|error| {
        let _ = std::fs::remove_file(&temp);
        format!("cannot replace {}: {error}", path.display())
    })
}

/// Seconds since the Unix epoch (0 if the clock is before it).
pub fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|elapsed| elapsed.as_secs()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_reads_default_corrupt_fails_and_writes_are_atomic() {
        let dir = std::env::temp_dir().join(format!("fox-statefile-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let file = dir.join("state").join("x.json");
        let value: Vec<String> = read_json(&file).unwrap();
        assert!(value.is_empty());
        write_json(&file, &vec!["a".to_owned()]).unwrap();
        assert_eq!(read_json::<Vec<String>>(&file).unwrap(), ["a"]);
        std::fs::write(&file, "{not json").unwrap();
        assert!(read_json::<Vec<String>>(&file).unwrap_err().contains("corrupt"));
        assert_eq!(lock_path_for(&file), dir.join("state").join("x.json.lock"));
        let held = StateLock::acquire(&lock_path_for(&file)).unwrap();
        assert!(StateLock::try_acquire(&lock_path_for(&file)).unwrap().is_none(), "second handle conflicts, even in-process");
        drop(held);
        assert!(StateLock::try_acquire(&lock_path_for(&file)).unwrap().is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
