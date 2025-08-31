//! This module handles agent scaffolding in the Cargo-AI agent builds.
//!
//! It creates the `.cargo-ai/agents/{agent_name}` folder and populates it
//! with the necessary config and build files.

use std::{fs, io::Error, path::PathBuf};

use crate::templates::*;

const AGENTS_WORKSPACE_DIRECTORY: &str = ".cargo-ai/agents";

/// Creates a new agent project directory and initializes required files.
pub fn create_new_agent_project(agent_name: &str) -> Result<(), Error> {
    create_agent_workspace(agent_name)?;
    load_agent_workspace(agent_name)?;
    Ok(())
}

/// Returns the full path to the workspace directory for the given agent.
fn agent_workspace_path(agent_name: &str) -> PathBuf {
    PathBuf::from(format!("{AGENTS_WORKSPACE_DIRECTORY}/{agent_name}"))
}

/// Creates the agent-specific directory under `.cargo-ai/agents/{agent_name}`.
fn create_agent_workspace(agent_name: &str) -> Result<(), Error> {
    let agent_workspace_directory = agent_workspace_path(agent_name);
    if !agent_workspace_directory.exists() {
        fs::create_dir_all(agent_workspace_directory)?;
    }
    Ok(()) 
}

/// Writes template files (`build.rs`, `.agentcfg`) to the agent workspace.
fn load_agent_workspace(agent_name: &str) -> Result<(), Error> {
    let base_path = agent_workspace_path(agent_name); 
    for (file_name, template) in  TEMPLATES {
        let file_path = base_path.join(file_name);
        fs::write(file_path, template)?;
    }
    Ok(())
}