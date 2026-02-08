use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};

/// Contents written to the lock file.
#[derive(Serialize, Deserialize)]
struct LockInfo {
    pid: u32,
    started_at: String,
}

/// A session-exclusive lock backed by `.rloop/.lock`.
///
/// Only one rloop session may run against a repo at a time. The lock is
/// acquired at session start and released on drop.
#[derive(Debug)]
pub struct SessionLock {
    path: PathBuf,
}

impl SessionLock {
    /// Acquire the session lock for the given `.rloop/` directory.
    ///
    /// If a lock file already exists, checks whether the owning process is
    /// still alive. Stale locks (dead PID) are removed and re-acquired.
    ///
    /// # Errors
    /// Returns an error if another rloop process is actively holding the lock.
    pub fn acquire(rloop_dir: impl AsRef<Path>) -> anyhow::Result<Self> {
        let path = rloop_dir.as_ref().join(".lock");

        // Loop to handle stale lock removal + retry
        loop {
            let info = LockInfo {
                pid: std::process::id(),
                started_at: chrono::Utc::now().to_rfc3339(),
            };

            // Atomically create the lock file. This fails if the file already exists.
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(mut file) => {
                    // Success — we acquired the lock
                    let json =
                        serde_json::to_string(&info).context("failed to serialize lock info")?;
                    file.write_all(json.as_bytes())
                        .with_context(|| format!("failed to write lock file {}", path.display()))?;
                    return Ok(Self { path });
                }
                Err(e) if e.kind() == ErrorKind::AlreadyExists => {
                    // Lock file exists — check if it's stale
                    if let Ok(contents) = fs::read_to_string(&path) {
                        if let Ok(lock_info) = serde_json::from_str::<LockInfo>(&contents) {
                            if is_process_alive(lock_info.pid) {
                                bail!(
                                    "another rloop session is running (pid {}, started {}). \
                                     If this is stale, remove {}",
                                    lock_info.pid,
                                    lock_info.started_at,
                                    path.display()
                                );
                            }
                            // Stale lock — owning process is dead
                            eprintln!(
                                "warning: removing stale lock from pid {} (started {})",
                                lock_info.pid, lock_info.started_at
                            );
                        } else {
                            eprintln!("warning: removing malformed lock file");
                        }
                        // Remove the stale/malformed lock and retry
                        fs::remove_file(&path).with_context(|| {
                            format!("failed to remove stale lock {}", path.display())
                        })?;
                        // Loop back to retry acquisition
                    } else {
                        // Can't read — try to remove
                        eprintln!("warning: removing unreadable lock file");
                        if let Err(e) = fs::remove_file(&path) {
                            eprintln!("warning: failed to remove unreadable lock file: {e}");
                        }
                        // Loop back to retry acquisition
                    }
                }
                Err(e) => {
                    // Other error (permissions, etc.)
                    return Err(e)
                        .with_context(|| format!("failed to create lock file {}", path.display()));
                }
            }
        }
    }

    /// Release the lock by removing the lock file.
    ///
    /// Only removes the file if our PID still owns it (guards against
    /// accidental removal of a lock held by another process).
    ///
    /// # Errors
    /// Returns an error if the lock file cannot be removed.
    pub fn release(&self) -> anyhow::Result<()> {
        if !self.path.exists() {
            return Ok(());
        }

        // Only remove if we still own it
        if let Ok(contents) = fs::read_to_string(&self.path)
            && let Ok(info) = serde_json::from_str::<LockInfo>(&contents)
            && info.pid != std::process::id()
        {
            return Ok(());
        }

        fs::remove_file(&self.path)
            .with_context(|| format!("failed to remove lock file {}", self.path.display()))?;
        Ok(())
    }

    /// Path to the lock file (test-only).
    #[cfg(test)]
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for SessionLock {
    fn drop(&mut self) {
        if let Err(e) = self.release() {
            eprintln!("warning: failed to release session lock: {e}");
        }
    }
}

/// Check whether a process with the given PID is still running.
fn is_process_alive(pid: u32) -> bool {
    // On Unix, `kill(pid, 0)` checks existence without sending a signal.
    // Returns 0 if the process exists (and we can signal it), or -1 with
    // ESRCH if it doesn't exist. EPERM means it exists but we can't signal it.
    #[cfg(unix)]
    {
        // SAFETY: signal 0 does not affect the target process.
        let ret = unsafe { libc::kill(pid.cast_signed(), 0) };
        if ret == 0 {
            return true;
        }
        // EPERM means the process exists but belongs to another user
        std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }

    #[cfg(not(unix))]
    {
        // Conservative: assume alive if we can't check
        let _ = pid;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acquire_and_release() {
        let dir = tempfile::tempdir().unwrap();
        let lock = SessionLock::acquire(dir.path()).unwrap();
        assert!(lock.path().exists());
        lock.release().unwrap();
        assert!(!lock.path().exists());
    }

    #[test]
    fn acquire_creates_lock_file_with_pid() {
        let dir = tempfile::tempdir().unwrap();
        let lock = SessionLock::acquire(dir.path()).unwrap();
        let contents = fs::read_to_string(lock.path()).unwrap();
        let info: LockInfo = serde_json::from_str(&contents).unwrap();
        assert_eq!(info.pid, std::process::id());
        assert!(!info.started_at.is_empty());
    }

    #[test]
    fn double_acquire_fails() {
        let dir = tempfile::tempdir().unwrap();
        let _lock = SessionLock::acquire(dir.path()).unwrap();
        let result = SessionLock::acquire(dir.path());
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("another rloop session is running"));
    }

    #[test]
    fn stale_lock_is_recovered() {
        let dir = tempfile::tempdir().unwrap();
        let lock_path = dir.path().join(".lock");

        // Write a lock file with a PID that doesn't exist
        let info = LockInfo {
            pid: 999_999_999,
            started_at: "2024-01-01T00:00:00Z".into(),
        };
        fs::write(&lock_path, serde_json::to_string(&info).unwrap()).unwrap();

        // Should succeed — the PID is dead
        let lock = SessionLock::acquire(dir.path()).unwrap();
        let contents = fs::read_to_string(lock.path()).unwrap();
        let new_info: LockInfo = serde_json::from_str(&contents).unwrap();
        assert_eq!(new_info.pid, std::process::id());
    }

    #[test]
    fn malformed_lock_is_recovered() {
        let dir = tempfile::tempdir().unwrap();
        let lock_path = dir.path().join(".lock");

        fs::write(&lock_path, "not valid json").unwrap();

        // Should succeed — malformed locks are treated as stale
        let lock = SessionLock::acquire(dir.path()).unwrap();
        assert!(lock.path().exists());
    }

    #[test]
    fn drop_releases_lock() {
        let dir = tempfile::tempdir().unwrap();
        let lock_path = dir.path().join(".lock");

        {
            let _lock = SessionLock::acquire(dir.path()).unwrap();
            assert!(lock_path.exists());
        }
        // After drop, lock file should be gone
        assert!(!lock_path.exists());
    }

    #[test]
    fn release_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let lock = SessionLock::acquire(dir.path()).unwrap();
        lock.release().unwrap();
        lock.release().unwrap(); // second call is a no-op
    }

    #[test]
    fn release_does_not_remove_other_pid_lock() {
        let dir = tempfile::tempdir().unwrap();
        let lock = SessionLock::acquire(dir.path()).unwrap();

        // Overwrite lock with a different PID
        let other_info = LockInfo {
            pid: std::process::id() + 1,
            started_at: "2024-01-01T00:00:00Z".into(),
        };
        fs::write(lock.path(), serde_json::to_string(&other_info).unwrap()).unwrap();

        // Release should NOT remove the file since we don't own it
        lock.release().unwrap();
        assert!(lock.path().exists());
    }

    #[test]
    fn is_process_alive_self() {
        assert!(is_process_alive(std::process::id()));
    }

    #[test]
    fn is_process_alive_dead_pid() {
        // PID 999_999_999 almost certainly doesn't exist
        assert!(!is_process_alive(999_999_999));
    }

    #[test]
    fn acquire_after_release_succeeds() {
        let dir = tempfile::tempdir().unwrap();
        let lock = SessionLock::acquire(dir.path()).unwrap();
        lock.release().unwrap();

        // Should be able to acquire again
        let lock2 = SessionLock::acquire(dir.path()).unwrap();
        assert!(lock2.path().exists());
    }
}
