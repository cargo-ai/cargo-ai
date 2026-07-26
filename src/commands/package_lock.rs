//! Cross-process serialization for installed package alias mutations.

use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::ErrorKind;
#[cfg(windows)]
use std::os::windows::fs::MetadataExt;
use std::path::Path;

#[cfg(windows)]
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;

/// Guard that holds an operating-system lock for one installed package alias.
/// Closing the file releases the lock automatically, including on process exit.
#[derive(Debug)]
pub(crate) struct PackageAliasLockGuard {
    file: File,
}

impl Drop for PackageAliasLockGuard {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

/// Acquires a non-blocking exclusive alias lock. Lock files remain in the lock
/// directory so crash recovery relies on the operating system, not timestamps.
pub(crate) fn try_acquire_package_alias_lock(
    lock_root: &Path,
    alias: &str,
) -> Result<PackageAliasLockGuard, String> {
    ensure_real_lock_directory(lock_root)?;
    let lock_path = lock_root.join(format!("{alias}.lock"));
    let file = open_real_lock_file(&lock_path)?;
    match file.try_lock() {
        Ok(()) => Ok(PackageAliasLockGuard { file }),
        Err(TryLockError::WouldBlock) => Err(format!(
            "Package alias `{alias}` is being changed by another Cargo AI process. Wait for that operation to finish and try again."
        )),
        Err(TryLockError::Error(error)) => Err(format!(
            "Failed to acquire package alias lock '{}': {}",
            lock_path.display(),
            error
        )),
    }
}

/// Acquires a non-blocking shared alias lease. Multiple package executions can
/// coexist, while install, update, rollback, and uninstall remain exclusive.
pub(crate) fn try_acquire_package_alias_read_lock(
    lock_root: &Path,
    alias: &str,
) -> Result<PackageAliasLockGuard, String> {
    ensure_real_lock_directory(lock_root)?;
    let lock_path = lock_root.join(format!("{alias}.lock"));
    let file = open_real_lock_file(&lock_path)?;
    match file.try_lock_shared() {
        Ok(()) => Ok(PackageAliasLockGuard { file }),
        Err(TryLockError::WouldBlock) => Err(format!(
            "Package alias `{alias}` is being changed by another Cargo AI process. Wait for that operation to finish and try again."
        )),
        Err(TryLockError::Error(error)) => Err(format!(
            "Failed to acquire package alias read lease '{}': {}",
            lock_path.display(),
            error
        )),
    }
}

fn open_real_lock_file(path: &Path) -> Result<File, String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata_is_link_like(&metadata) || !metadata.is_file() => {
            return Err(format!(
                "Package alias lock '{}' must be a real file and not a symbolic link or reparse point.",
                path.display()
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => {
            match OpenOptions::new()
                .create_new(true)
                .read(true)
                .write(true)
                .open(path)
            {
                Ok(file) => return Ok(file),
                Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
                Err(error) => {
                    return Err(format!(
                        "Failed to create package alias lock '{}': {}",
                        path.display(),
                        error
                    ));
                }
            }
        }
        Err(error) => {
            return Err(format!(
                "Failed to inspect package alias lock '{}': {}",
                path.display(),
                error
            ));
        }
    }

    let metadata = fs::symlink_metadata(path).map_err(|error| {
        format!(
            "Failed to inspect package alias lock '{}': {}",
            path.display(),
            error
        )
    })?;
    if metadata_is_link_like(&metadata) || !metadata.is_file() {
        return Err(format!(
            "Package alias lock '{}' must be a real file and not a symbolic link or reparse point.",
            path.display()
        ));
    }
    OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(|error| {
            format!(
                "Failed to open package alias lock '{}': {}",
                path.display(),
                error
            )
        })
}

fn ensure_real_lock_directory(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata_is_link_like(&metadata) || !metadata.is_dir() => {
            return Err(format!(
                "Package lock root '{}' must be a real directory and not a symbolic link or reparse point.",
                path.display()
            ));
        }
        Ok(_) => return Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => {
            return Err(format!(
                "Failed to inspect package lock root '{}': {}",
                path.display(),
                error
            ));
        }
    }

    fs::create_dir_all(path).map_err(|error| {
        format!(
            "Failed to create package lock root '{}': {}",
            path.display(),
            error
        )
    })?;
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        format!(
            "Failed to inspect package lock root '{}': {}",
            path.display(),
            error
        )
    })?;
    if metadata_is_link_like(&metadata) || !metadata.is_dir() {
        return Err(format!(
            "Package lock root '{}' must be a real directory and not a symbolic link or reparse point.",
            path.display()
        ));
    }
    Ok(())
}

#[cfg(windows)]
fn metadata_is_link_like(metadata: &fs::Metadata) -> bool {
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn metadata_is_link_like(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

#[cfg(test)]
mod tests {
    use super::{try_acquire_package_alias_lock, try_acquire_package_alias_read_lock};
    use std::fs;
    use std::process::Command;
    use std::thread;
    use std::time::Duration;
    use std::time::{SystemTime, UNIX_EPOCH};

    const SUBPROCESS_HELPER_ENV: &str = "CARGO_AI_PACKAGE_LOCK_SUBPROCESS_HELPER";
    const SUBPROCESS_ROOT_ENV: &str = "CARGO_AI_PACKAGE_LOCK_SUBPROCESS_ROOT";
    const SUBPROCESS_READY_ENV: &str = "CARGO_AI_PACKAGE_LOCK_SUBPROCESS_READY";
    const SUBPROCESS_RELEASE_ENV: &str = "CARGO_AI_PACKAGE_LOCK_SUBPROCESS_RELEASE";

    fn temp_lock_root(stem: &str) -> std::path::PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should move forward")
            .as_nanos();
        std::env::temp_dir().join(format!("cargo-ai-package-lock-{stem}-{unique}"))
    }

    #[test]
    fn same_alias_contends_until_guard_drops() {
        let root = temp_lock_root("contention");
        let guard = try_acquire_package_alias_lock(&root, "reports")
            .expect("first alias lock should be acquired");
        let error = try_acquire_package_alias_lock(&root, "reports")
            .err()
            .expect("second alias lock should contend");
        assert!(error.contains("another Cargo AI process"));

        drop(guard);
        let reacquired = try_acquire_package_alias_lock(&root, "reports")
            .expect("operating system should release the lock with the guard");
        drop(reacquired);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn different_aliases_do_not_contend() {
        let root = temp_lock_root("aliases");
        let first = try_acquire_package_alias_lock(&root, "reports")
            .expect("first alias lock should be acquired");
        let second = try_acquire_package_alias_lock(&root, "images")
            .expect("different alias should acquire independently");
        drop((first, second));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn shared_readers_coexist_and_block_exclusive_mutation() {
        let root = temp_lock_root("shared-readers");
        let first = try_acquire_package_alias_read_lock(&root, "reports")
            .expect("first reader should acquire");
        let second = try_acquire_package_alias_read_lock(&root, "reports")
            .expect("second reader should coexist");
        let error = try_acquire_package_alias_lock(&root, "reports")
            .expect_err("exclusive mutation should wait for active readers");
        assert!(error.contains("another Cargo AI process"));

        drop((first, second));
        let exclusive = try_acquire_package_alias_lock(&root, "reports")
            .expect("exclusive mutation should acquire after readers finish");
        let error = try_acquire_package_alias_read_lock(&root, "reports")
            .expect_err("new reader should wait for an exclusive mutation");
        assert!(error.contains("being changed"));
        drop(exclusive);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn subprocess_shared_lock_helper() {
        if std::env::var_os(SUBPROCESS_HELPER_ENV).is_none() {
            return;
        }
        let root = std::path::PathBuf::from(
            std::env::var_os(SUBPROCESS_ROOT_ENV).expect("helper lock root should be provided"),
        );
        let ready = std::path::PathBuf::from(
            std::env::var_os(SUBPROCESS_READY_ENV).expect("helper ready path should be provided"),
        );
        let release = std::path::PathBuf::from(
            std::env::var_os(SUBPROCESS_RELEASE_ENV)
                .expect("helper release path should be provided"),
        );
        let _lease = try_acquire_package_alias_read_lock(&root, "reports")
            .expect("helper reader should acquire");
        fs::write(&ready, "ready").expect("helper should signal readiness");
        for _ in 0..1_000 {
            if release.exists() {
                return;
            }
            thread::sleep(Duration::from_millis(5));
        }
        panic!("helper timed out waiting for release signal");
    }

    #[test]
    fn subprocess_reader_lease_blocks_mutation_until_execution_finishes() {
        let root = temp_lock_root("subprocess-reader");
        let ready = root.with_extension("ready");
        let release = root.with_extension("release");
        let parent_reader = try_acquire_package_alias_read_lock(&root, "reports")
            .expect("parent reader should acquire before the child starts");
        let test_binary = std::env::current_exe().expect("test binary path should resolve");
        let mut child = Command::new(test_binary)
            .arg("--exact")
            .arg("commands::package_lock::tests::subprocess_shared_lock_helper")
            .arg("--nocapture")
            .env(SUBPROCESS_HELPER_ENV, "1")
            .env(SUBPROCESS_ROOT_ENV, &root)
            .env(SUBPROCESS_READY_ENV, &ready)
            .env(SUBPROCESS_RELEASE_ENV, &release)
            .spawn()
            .expect("reader helper should start");

        let mut ready_observed = false;
        for _ in 0..1_000 {
            if ready.exists() {
                ready_observed = true;
                break;
            }
            thread::sleep(Duration::from_millis(5));
        }
        if !ready_observed {
            let _ = fs::write(&release, "release");
            let _ = child.wait();
            panic!("reader helper did not acquire its lease");
        }

        let error = try_acquire_package_alias_lock(&root, "reports")
            .expect_err("mutation must wait while parent and child readers coexist");
        assert!(error.contains("another Cargo AI process"));
        drop(parent_reader);
        let error = try_acquire_package_alias_lock(&root, "reports")
            .expect_err("the child reader must keep blocking mutation after the parent drops");
        assert!(error.contains("another Cargo AI process"));
        fs::write(&release, "release").expect("parent should release helper");
        assert!(child.wait().expect("helper should exit").success());
        let mutation = try_acquire_package_alias_lock(&root, "reports")
            .expect("mutation should acquire after the reader process exits");
        drop(mutation);

        let _ = fs::remove_file(ready);
        let _ = fs::remove_file(release);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn subprocess_reader_lease_is_released_when_process_is_killed() {
        let root = temp_lock_root("subprocess-reader-killed");
        let ready = root.with_extension("ready");
        let release = root.with_extension("release");
        let test_binary = std::env::current_exe().expect("test binary path should resolve");
        let mut child = Command::new(test_binary)
            .arg("--exact")
            .arg("commands::package_lock::tests::subprocess_shared_lock_helper")
            .arg("--nocapture")
            .env(SUBPROCESS_HELPER_ENV, "1")
            .env(SUBPROCESS_ROOT_ENV, &root)
            .env(SUBPROCESS_READY_ENV, &ready)
            .env(SUBPROCESS_RELEASE_ENV, &release)
            .spawn()
            .expect("reader helper should start");

        let mut ready_observed = false;
        for _ in 0..1_000 {
            if ready.exists() {
                ready_observed = true;
                break;
            }
            thread::sleep(Duration::from_millis(5));
        }
        if !ready_observed {
            let _ = child.kill();
            let _ = child.wait();
            panic!("reader helper did not acquire its lease");
        }

        let error = try_acquire_package_alias_lock(&root, "reports")
            .expect_err("mutation must wait while the child reader is alive");
        assert!(error.contains("another Cargo AI process"));
        child.kill().expect("reader helper should be terminated");
        let _ = child.wait().expect("terminated helper should be reaped");
        let mutation = try_acquire_package_alias_lock(&root, "reports")
            .expect("the operating system should release the lease when the process exits");
        drop(mutation);

        let _ = fs::remove_file(ready);
        let _ = fs::remove_file(release);
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn symbolic_link_lock_file_is_rejected() {
        use std::os::unix::fs::symlink;

        let root = temp_lock_root("symlink");
        fs::create_dir_all(&root).expect("lock root should exist");
        let target = root.join("target");
        fs::write(&target, "").expect("target should exist");
        symlink(&target, root.join("reports.lock")).expect("symlink should be created");
        let error = try_acquire_package_alias_lock(&root, "reports")
            .err()
            .expect("symbolic link lock should fail");
        assert!(error.contains("symbolic link or reparse point"));
        let _ = fs::remove_dir_all(root);
    }
}
