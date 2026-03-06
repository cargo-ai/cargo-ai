//! Handles compiling an agent workspace into an executable.

use std::path::Path;
use std::process::{Command, Stdio};

/// Builds the agent project at the given path (e.g. `.cargo-ai/agents/my_agent`)
pub fn build_agent_project(agent_name: &str) -> Result<(), std::io::Error> {
    let project_path = super::agent_workspace_path(agent_name);
    run_cargo_compile_in_path(&project_path, "build")
}

/// Runs `cargo check` for the agent project at the given path.
pub fn check_agent_project(agent_name: &str) -> Result<(), std::io::Error> {
    let project_path = super::agent_workspace_path(agent_name);
    run_cargo_compile_in_path(&project_path, "check")
}

/// Builds an arbitrary workspace path with `cargo build`.
pub(crate) fn build_workspace(project_path: &Path) -> Result<(), std::io::Error> {
    run_cargo_compile_in_path(project_path, "build")
}

fn run_cargo_compile_in_path(project_path: &Path, command: &str) -> Result<(), std::io::Error> {
    if !project_path.exists() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "Agent path does not exist",
        ));
    }

    let status = Command::new("cargo")
        .arg(command)
        .current_dir(&project_path)
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()?;

    if !status.success() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("Cargo {command} failed"),
        ));
    }

    Ok(())
}
