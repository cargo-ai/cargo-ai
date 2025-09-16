//! Handles cleanup of temporary agent build directories.

use std::fs;
use std::io;
use std::path::PathBuf;

/// Deletes the `.cargo-ai/agents/{agent_name}` workspace directory.
pub fn delete_agent_workspace(agent_name: &str) -> io::Result<()> {
    let path = PathBuf::from(format!(".cargo-ai/agents/{}", agent_name));
    if path.exists() {
        fs::remove_dir_all(&path)?;
        println!("🧹 Deleted agent workspace: {}", path.display());
    } else {
        println!("ℹ️ Agent workspace does not exist: {}", path.display());
    }
    Ok(())
}