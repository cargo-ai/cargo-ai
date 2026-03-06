//! Handles compiling an agent workspace into an executable.

use super::build_target::BuildTarget;
use std::path::Path;
use std::process::{Command, Stdio};

/// Builds the agent project at the given path (e.g. `.cargo-ai/agents/my_agent`)
pub fn build_agent_project(
    agent_name: &str,
    build_target: &BuildTarget,
) -> Result<(), std::io::Error> {
    let project_path = super::agent_workspace_path(agent_name);
    run_cargo_compile_in_path(&project_path, "build", build_target)
}

/// Runs `cargo check` for the agent project at the given path.
pub fn check_agent_project(
    agent_name: &str,
    build_target: &BuildTarget,
) -> Result<(), std::io::Error> {
    let project_path = super::agent_workspace_path(agent_name);
    run_cargo_compile_in_path(&project_path, "check", build_target)
}

/// Builds an arbitrary workspace path with `cargo build`.
pub(crate) fn build_workspace(
    project_path: &Path,
    build_target: &BuildTarget,
) -> Result<(), std::io::Error> {
    run_cargo_compile_in_path(project_path, "build", build_target)
}

fn run_cargo_compile_in_path(
    project_path: &Path,
    command: &str,
    build_target: &BuildTarget,
) -> Result<(), std::io::Error> {
    if !project_path.exists() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "Agent path does not exist",
        ));
    }

    let mut cargo_command = Command::new("cargo");
    cargo_command
        .args(build_target.cargo_args(command))
        .current_dir(project_path)
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());

    let status = cargo_command.status()?;

    if !status.success() {
        let target_detail = build_target
            .cargo_target()
            .map(|target| {
                format!(
                    " for target '{target}'. See compiler output above for missing rustup target, linker, or SDK details."
                )
            })
            .unwrap_or_default();
        return Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("Cargo {command} failed{target_detail}"),
        ));
    }

    Ok(())
}
