//! Handles cleanup of temporary agent build directories.

use std::fs;
use std::io;

/// Deletes the `.cargo-ai/agents/{agent_name}` workspace directory.
pub fn delete_agent_workspace(agent_name: &str) -> io::Result<()> {
    let project_path = super::agent_workspace_path(agent_name);
    if project_path.exists() {
        fs::remove_dir_all(&project_path)?;
        println!("🧹 Deleted agent workspace: {}", project_path.display());
    } else {
        println!("ℹ️ Agent workspace does not exist: {}", project_path.display());
    }
    Ok(())
}