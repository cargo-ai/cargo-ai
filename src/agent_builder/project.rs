//! This module handles agent scaffolding in the Cargo-AI agent builds.
//!
//! It creates the `.cargo-ai/agents/{agent_name}` folder and populates it
//! with the necessary config and build files.

use std::{fs, env};
use std::io::{Error};

include!(concat!(env!("OUT_DIR"), "/.generated_templates.rs"));

/// Creates a new agent project directory and initializes required files.
pub fn create_new_agent_project(agent_name: &str, agentcfg: Result<String, Error>) -> Result<(), Error> {
    create_agent_workspace(agent_name)?;
    load_agent_workspace(agent_name, agentcfg)?;
    Ok(())
}

/// Creates the agent-specific directory under `.cargo-ai/agents/{agent_name}`.
fn create_agent_workspace(agent_name: &str) -> Result<(), Error> {
    let agent_workspace_directory = super::agent_workspace_path(agent_name);
    if !agent_workspace_directory.exists() {
        fs::create_dir_all(agent_workspace_directory)?;
    }
    Ok(()) 
}

/// Writes template files (`build.rs`, `.agentcfg`) to the agent workspace.
fn load_agent_workspace(agent_name: &str, agentcfg: Result<String, Error>) -> Result<(), Error> {
    let base_path = super::agent_workspace_path(agent_name); 
    for (file_name, file_contents) in  TEMPLATES {
        let file_path = base_path.join(file_name);

        // Create parent directories if needed
        if let Some(parent) = file_path.parent() {
            fs::create_dir_all(parent)?;
        }
        
        // Handle custom .agentcfg file
        if file_name == ".agentcfg" {
            if let Ok(ref contents) = agentcfg {
                fs::write(file_path, contents)?;
                continue;
            }
        }

        // Skip replacements for loader.rs so config paths remain shared
        if file_name.ends_with("loader.rs") {
            fs::write(file_path, file_contents)?;
            continue;
        }

        let file_contents = file_contents
            .replace("cargo-ai", agent_name)
            .replace("cargo_ai", agent_name);

        // Replace cargo_ai with agent name for some 

        fs::write(file_path, file_contents)?;
    }
    Ok(())
}