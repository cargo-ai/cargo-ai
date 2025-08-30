use std::{fs, io, path::Path};

const AGENT_WORKSPACE_DIR: &str = ".cargo-ai/agents";

pub fn create_new_agent_project(agent_name: &str) -> Result<(), io::Error> {

    create_agent_workspace()?;

    Ok(())
}

fn create_agent_workspace() -> Result<(), io::Error> {

    let agent_workspace_directory = Path::new(AGENT_WORKSPACE_DIR);
    if !agent_workspace_directory.exists() {
        fs::create_dir_all(agent_workspace_directory)?;
    }
    
    Ok(()) 
}