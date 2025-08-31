use std::{fs, io::Error, path::PathBuf, str::FromStr};

use crate::templates::*;

const AGENTS_WORKSPACE_DIRECTORY: &str = ".cargo-ai/agents";

pub fn create_new_agent_project(agent_name: &str) -> Result<(), Error> {

    create_agent_workspace(agent_name)?;
    load_agent_workspace(agent_name)?;

    Ok(())
}

fn workspace_director(agent_name: &str) -> PathBuf {
    let agent_workspace_directory = format!("{AGENTS_WORKSPACE_DIRECTORY}/{agent_name}");

    PathBuf::from_str(&agent_workspace_directory).expect("Unable to create agent path buffer.")
}

fn create_agent_workspace(agent_name: &str) -> Result<(), Error> {

    let agent_workspace_directory = workspace_director(agent_name);

    if !agent_workspace_directory.exists() {
        fs::create_dir_all(agent_workspace_directory)?;
    }
    
    Ok(()) 
}

fn load_agent_workspace(agent_name: &str) -> Result<(), Error> {

    let base_path = workspace_director(agent_name); 

    for (file_name, template) in [
        (BUILD_RS_NAME, BUILD_RS_TEMPLATE),
        (AGENTCFG_NAME, AGENTCFG_TEMPLATE),
        ] {
        let file_path = base_path.join(file_name);
        fs::write(file_path, template)?;
   }

    Ok(())
}