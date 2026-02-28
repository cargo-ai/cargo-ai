//! Handles exporting the compiled agent binary to the current working directory.

use std::fs;
use std::io::{self, ErrorKind};

/// Copies the built binary from `.cargo-ai/agents/{agent}/target/debug/{agent}`
/// into the directory where the `cargo-ai` command was invoked.
pub fn export_binary(agent_name: &str, force_overwrite: bool) -> io::Result<()> {

    let project_path = super::agent_workspace_path(agent_name);

    #[allow(unused_mut)]
    let mut source_path = project_path
        .join("target")
        .join("debug") 
        .join(agent_name);

    #[cfg(windows)]
    source_path.set_extension("exe");

    if !source_path.exists() {
        return Err(io::Error::new(ErrorKind::NotFound, format!("Expected binary not found at {:?}", source_path)));
    }

    let dest_dir = std::env::current_dir()?;

    #[allow(unused_mut)]
    let mut dest_path = dest_dir.join(agent_name);
    
    #[cfg(windows)]
    dest_path.set_extension("exe");

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
    
    fs::copy(&source_path, &dest_path)?;

    println!("✅ Exported binary to: {:?}", dest_path);
    Ok(())
}
