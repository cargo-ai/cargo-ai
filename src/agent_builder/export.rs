//! Handles exporting the compiled agent binary to the current working directory.

use std::fs;
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};

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

fn compiled_binary_path(
    project_path: &Path,
    agent_name: &str,
    explicit_target_triple: Option<&str>,
) -> PathBuf {
    let mut source_path = project_path.join("target");
    if let Some(target_triple) = super::resolved_target_triple(explicit_target_triple) {
        source_path = source_path.join(target_triple);
    }
    source_path = source_path.join("debug").join(agent_name);
    if super::target_uses_windows_exe(explicit_target_triple) {
        source_path.set_extension("exe");
    }
    source_path
}

fn exported_binary_path(
    dest_dir: &Path,
    agent_name: &str,
    explicit_target_triple: Option<&str>,
) -> PathBuf {
    let mut dest_path = dest_dir.join(agent_name);
    if super::target_uses_windows_exe(explicit_target_triple) {
        dest_path.set_extension("exe");
    }
    dest_path
}

/// Copies the built binary from `.cargo-ai/agents/{agent}/target/...`
/// into the directory where the `cargo-ai` command was invoked.
pub fn export_binary(
    agent_name: &str,
    force_overwrite: bool,
    explicit_target_triple: Option<&str>,
) -> io::Result<()> {
    let project_path = super::agent_workspace_path(agent_name);
    let source_path = compiled_binary_path(&project_path, agent_name, explicit_target_triple);
    let dest_dir = std::env::current_dir()?;
    let dest_path = exported_binary_path(&dest_dir, agent_name, explicit_target_triple);

    export_binary_to_path(&source_path, &dest_path, force_overwrite)
}

#[cfg(test)]
mod tests {
    use super::{compiled_binary_path, export_binary_to_path, exported_binary_path};
    use std::fs;
    use std::io::ErrorKind;
    use std::path::{Path, PathBuf};
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

    #[test]
    fn target_specific_source_path_uses_nested_target_directory() {
        let project_path = PathBuf::from("/tmp/demo-agent");
        let compiled = compiled_binary_path(
            &project_path,
            "weather_agent",
            Some("x86_64-unknown-linux-gnu"),
        );

        assert_eq!(
            compiled,
            project_path
                .join("target")
                .join("x86_64-unknown-linux-gnu")
                .join("debug")
                .join("weather_agent")
        );
    }

    #[test]
    fn windows_target_export_paths_use_exe_extension() {
        let project_path = PathBuf::from("/tmp/demo-agent");
        let source = compiled_binary_path(
            &project_path,
            "weather_agent",
            Some("x86_64-pc-windows-msvc"),
        );
        let dest = exported_binary_path(
            Path::new("/tmp/out"),
            "weather_agent",
            Some("x86_64-pc-windows-msvc"),
        );

        assert_eq!(
            source,
            project_path
                .join("target")
                .join("x86_64-pc-windows-msvc")
                .join("debug")
                .join("weather_agent.exe")
        );
        assert_eq!(dest, Path::new("/tmp/out").join("weather_agent.exe"));
    }
}
