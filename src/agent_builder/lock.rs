//! Per-agent workspace locking for hatch/check execution.
//!
//! Locking is implemented with atomic lock-file creation under
//! `.cargo/.cargo-ai/locks/` keyed by agent name.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// Guard that keeps an agent lock held for the lifetime of the value.
pub struct AgentLockGuard {
    _file: File,
    path: PathBuf,
}

impl Drop for AgentLockGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// Acquires a non-blocking lock for a named agent.
///
/// Returns `WouldBlock` when another process currently holds the same lock.
pub fn try_acquire_agent_lock(agent_name: &str) -> io::Result<AgentLockGuard> {
    let lock_dir = super::locks_root();
    fs::create_dir_all(&lock_dir)?;

    let lock_path = lock_dir.join(format!("{agent_name}.lock"));
    let mut lock_file = match OpenOptions::new()
        .create_new(true)
        .read(true)
        .write(true)
        .open(&lock_path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                format!("lock already exists: {}", lock_path.display()),
            ));
        }
        Err(error) => return Err(error),
    };

    let pid = std::process::id();
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let _ = writeln!(lock_file, "pid={pid}\nacquired_unix_ts={ts}");

    Ok(AgentLockGuard {
        _file: lock_file,
        path: lock_path,
    })
}
