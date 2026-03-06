//! Handles exporting the compiled agent binary to the current working directory.

use super::build_target::BuildTarget;
use std::fs;
use std::io::{self, ErrorKind};
use std::path::Path;

fn export_binary_to_path(
    source_path: &Path,
    dest_path: &Path,
    force_overwrite: bool,
) -> io::Result<()> {
    if !source_path.exists() {
        return Err(io::Error::new(
            ErrorKind::NotFound,
            format!("Expected binary not found at {:?}", source_path),
        ));
    }

    if dest_path.exists() {
        if !force_overwrite {
            return Err(io::Error::new(
                ErrorKind::AlreadyExists,
                format!(
                    "Output already exists at {:?}. Re-run with --force to overwrite.",
                    dest_path
                ),
            ));
        }
        println!("ℹ️ Existing binary at {:?} will be overwritten.", dest_path);
    }

    fs::copy(source_path, dest_path)?;
    println!("✅ Exported binary to: {:?}", dest_path);
    Ok(())
}

/// Copies the built binary from `.cargo-ai/agents/{agent}/target/...`
/// into the directory where the `cargo-ai` command was invoked.
pub fn export_binary(
    agent_name: &str,
    force_overwrite: bool,
    build_target: &BuildTarget,
) -> io::Result<()> {
    let project_path = super::agent_workspace_path(agent_name);
    let source_path = build_target.compiled_binary_path(&project_path, agent_name);
    let dest_dir = std::env::current_dir()?;
    let dest_path = build_target.exported_binary_path(&dest_dir, agent_name);

    export_binary_to_path(&source_path, &dest_path, force_overwrite)
}

#[cfg(test)]
mod tests {
    use super::export_binary_to_path;
    use std::fs;
    use std::io::ErrorKind;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static TEMP_DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_test_dir() -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after epoch")
            .as_nanos();
        let sequence = TEMP_DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("cargo-ai-export-test-{nanos}-{sequence}"));
        fs::create_dir_all(&path).expect("test directory should be creatable");
        path
    }

    #[test]
    fn fails_when_destination_exists_without_force() {
        let dir = temp_test_dir();
        let source = dir.join("source-bin");
        let dest = dir.join("dest-bin");
        fs::write(&source, b"source").expect("source should be writable");
        fs::write(&dest, b"existing").expect("dest should be writable");

        let err = export_binary_to_path(&source, &dest, false).expect_err("should fail");
        assert_eq!(err.kind(), ErrorKind::AlreadyExists);
        assert!(err.to_string().contains("--force"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn overwrites_when_force_enabled() {
        let dir = temp_test_dir();
        let source = dir.join("source-bin");
        let dest = dir.join("dest-bin");
        fs::write(&source, b"source").expect("source should be writable");
        fs::write(&dest, b"existing").expect("dest should be writable");

        export_binary_to_path(&source, &dest, true).expect("force overwrite should succeed");
        let copied = fs::read(&dest).expect("dest should be readable");
        assert_eq!(copied, b"source");

        let _ = fs::remove_dir_all(&dir);
    }
}
