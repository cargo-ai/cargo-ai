use crate::agent_builder::build;
use crate::agent_builder::build_target::BuildTarget;
use crate::agent_builder::project;
use std::fs;
use std::path::Path;

const TEMPLATE_SEED_AGENT_NAME: &str = "template_seed_agent";
const TEMPLATE_SEED_AGENTCFG: &str = include_str!("../../../templates/.agentcfg");

pub(super) fn prepare_warmed_template_workspace(
    path: &Path,
    build_target: &BuildTarget,
) -> Result<(), String> {
    if path.exists() {
        fs::remove_dir_all(path).map_err(|error| {
            format!(
                "Failed to reset incomplete template workspace '{}': {error}",
                path.display()
            )
        })?;
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            format!(
                "Failed to create template cache directory '{}': {error}",
                parent.display()
            )
        })?;
    }

    let creation_result = (|| -> Result<(), String> {
        project::create_template_project(path, TEMPLATE_SEED_AGENT_NAME, TEMPLATE_SEED_AGENTCFG)
            .map_err(|error| {
                format!(
                    "Failed to create warmed template workspace '{}': {error}",
                    path.display()
                )
            })?;
        build::build_workspace(path, build_target).map_err(|error| {
            format!(
                "Failed to warm template workspace '{}': {error}",
                path.display()
            )
        })?;
        Ok(())
    })();

    if let Err(error) = creation_result {
        let _ = fs::remove_dir_all(path);
        return Err(error);
    }

    if !template_workspace_ready(path) {
        let _ = fs::remove_dir_all(path);
        return Err(format!(
            "Warmed template workspace '{}' is incomplete after build.",
            path.display()
        ));
    }

    Ok(())
}

pub(super) fn template_workspace_ready(path: &Path) -> bool {
    path.join("Cargo.toml").is_file()
        && path.join("build.rs").is_file()
        && path.join(".agentcfg").is_file()
        && path.join("src").join("main.rs").is_file()
        && path.join("target").is_dir()
}
