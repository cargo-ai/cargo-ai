//! This module handles agent scaffolding in the Cargo-AI agent builds.
//!
//! It creates the `.cargo-ai/agents/{agent_name}` folder and populates it
//! with the necessary config and build files.

use std::{fs, io::Error, path::PathBuf};

include!(concat!(env!("OUT_DIR"), "/templates_generated.rs"));

const AGENTS_WORKSPACE_DIRECTORY: &str = ".cargo-ai/agents";

/// Creates a new agent project directory and initializes required files.
pub fn create_new_agent_project(agent_name: &str, agentcfg: Option<&str>) -> Result<(), Error> {
    create_agent_workspace(agent_name)?;
    load_agent_workspace(agent_name, agentcfg)?;
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
fn load_agent_workspace(agent_name: &str, agentcfg: Option<&str>) -> Result<(), Error> {
    // let templates = templates();
    let base_path = agent_workspace_path(agent_name); 
    for (file_name, file_contents) in  templates() {
        let file_path = base_path.join(file_name);

        // Create parent directories if needed
        if let Some(parent) = file_path.parent() {
            fs::create_dir_all(parent)?;
        }
        
        // Handle custom .agentcfg file
        if *file_name == ".agentcfg" {
            if let Some(path) = agentcfg {
                let contents = fs::read_to_string(path)?;
                fs::write(file_path, contents)?;
                continue;
            }
        }

        let file_contents = file_contents
            .replace("cargo-ai", agent_name)
            .replace("cargo_ai", agent_name);

        // Replace cargo_ai with agent name for some 

        fs::write(file_path, file_contents)?;
    }
    Ok(())
}
