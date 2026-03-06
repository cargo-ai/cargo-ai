//! Handles compiling an agent workspace into an executable.

use std::path::Path;
use std::process::{Command, Stdio};

/// Builds the agent project at the given path (e.g. `.cargo-ai/agents/my_agent`)
pub fn build_agent_project(
    agent_name: &str,
    explicit_target_triple: Option<&str>,
) -> Result<(), std::io::Error> {
    let project_path = super::agent_workspace_path(agent_name);
    run_cargo_compile_in_path(&project_path, "build", explicit_target_triple)
}

/// Runs `cargo check` for the agent project at the given path.
pub fn check_agent_project(
    agent_name: &str,
    explicit_target_triple: Option<&str>,
) -> Result<(), std::io::Error> {
    let project_path = super::agent_workspace_path(agent_name);
    run_cargo_compile_in_path(&project_path, "check", explicit_target_triple)
}

/// Builds an arbitrary workspace path with `cargo build`.
pub(crate) fn build_workspace(
    project_path: &Path,
    explicit_target_triple: Option<&str>,
) -> Result<(), std::io::Error> {
    run_cargo_compile_in_path(project_path, "build", explicit_target_triple)
}

fn cargo_compile_args(command: &str, explicit_target_triple: Option<&str>) -> Vec<String> {
    let mut args = vec![command.to_string()];
    if let Some(target_triple) = super::resolved_target_triple(explicit_target_triple) {
        args.push("--target".to_string());
        args.push(target_triple);
    }
    args
}

fn run_cargo_compile_in_path(
    project_path: &Path,
    command: &str,
    explicit_target_triple: Option<&str>,
) -> Result<(), std::io::Error> {
    if !project_path.exists() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "Agent path does not exist",
        ));
    }

    let args = cargo_compile_args(command, explicit_target_triple);
    let mut cargo_command = Command::new("cargo");
    cargo_command
        .args(&args)
        .current_dir(project_path)
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());

    let status = cargo_command.status()?;

    if !status.success() {
        let target_detail = super::resolved_target_triple(explicit_target_triple)
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

#[cfg(test)]
mod tests {
    use super::cargo_compile_args;

    #[test]
    fn cargo_compile_args_omit_target_when_not_requested() {
        assert_eq!(cargo_compile_args("build", None), vec!["build".to_string()]);
    }

    #[test]
    fn cargo_compile_args_include_explicit_target() {
        assert_eq!(
            cargo_compile_args("check", Some("x86_64-pc-windows-msvc")),
            vec![
                "check".to_string(),
                "--target".to_string(),
                "x86_64-pc-windows-msvc".to_string()
            ]
        );
    }
}
