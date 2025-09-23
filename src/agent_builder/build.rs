//! Handles compiling an agent workspace into an executable.

use std::process::{Command, Stdio};
use std::path::Path;

/// Builds the agent project at the given path (e.g. `.cargo-ai/agents/my_agent`)
pub fn build_agent_project(agent_name: &str) -> Result<(), std::io::Error> {
    let project_path = super::agent_workspace_path(agent_name);
    if !Path::new(&project_path).exists() {
        return Err(std::io::Error::new(std::io::ErrorKind::NotFound, "Agent path does not exist"));
    }

    let status = Command::new("cargo")
        .arg("build")
        .current_dir(&project_path)
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()?;

    if !status.success() {
        return Err(std::io::Error::new(std::io::ErrorKind::Other, "Cargo build failed"));
    }

    Ok(())
}