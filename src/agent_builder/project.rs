//! This module handles agent scaffolding in the Cargo-AI agent builds.
//!
//! It creates the `.cargo-ai/agents/{agent_name}` folder and populates it
//! with the necessary config and build files.

use std::{fs, env};
use std::io::{Error, ErrorKind};

include!(concat!(env!("OUT_DIR"), "/.generated_templates.rs"));

/// Creates a new agent project directory and initializes required files.
pub fn create_new_agent_project(agent_name: &str, agentcfg: Option<&str>) -> Result<(), Error> {
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
fn load_agent_workspace(agent_name: &str, agentcfg: Option<&str>) -> Result<(), Error> {
    let base_path = super::agent_workspace_path(agent_name); 
    for (file_name, file_contents) in  TEMPLATES {
        let file_path = base_path.join(file_name);

        // Create parent directories if needed
        if let Some(parent) = file_path.parent() {
            fs::create_dir_all(parent)?;
        }
        
        // Handle custom .agentcfg file
        if file_name == ".agentcfg" {
            if let Some(path) = agentcfg {
                let contents = config_contents(path)?;
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

fn config_contents(path: &str) -> Result<String, std::io::Error> {
    if path.contains('.') {
        // Local file path
        fs::read_to_string(path)
    } else {
        // Fetch from Cargo-AI registry
        fetch_from_registry(path)
    }
}

fn fetch_from_registry(name: &str) -> Result<String, std::io::Error> {
    let url = "https://api.cargo-ai.org/public";
    let client = reqwest::blocking::Client::new();

    let body = serde_json::json!({ "request": name });

    let resp = client
        .post(url)
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .map_err(|e| Error::new(ErrorKind::Other, format!("network error: {e}")))?;

    if !resp.status().is_success() {
        return Err(Error::new(
            ErrorKind::Other,
            format!("HTTP {} for {url}", resp.status()),
        ));
    }

    resp.text().map_err(|e| Error::new(ErrorKind::Other, e.to_string()))
}