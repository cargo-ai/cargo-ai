//! Handles exporting the compiled agent binary to the current working directory.

use std::fs;
use std::path::PathBuf;
use std::io::{self, ErrorKind};

/// Copies the built binary from `.cargo-ai/agents/{agent}/target/debug/{agent}`
/// into the directory where the `cargo-ai` command was invoked.
pub fn export_binary(agent_name: &str) -> io::Result<()> {
    let source_path = PathBuf::from(format!(".cargo-ai/agents/{}/target/debug/{}", agent_name, agent_name));
    if !source_path.exists() {
        return Err(io::Error::new(ErrorKind::NotFound, format!("Expected binary not found at {:?}", source_path)));
    }

    let dest_dir = std::env::current_dir()?;
    let dest_path = dest_dir.join(agent_name);

    fs::copy(&source_path, &dest_path)?;

    println!("✅ Exported binary to: {:?}", dest_path);
    Ok(())
}