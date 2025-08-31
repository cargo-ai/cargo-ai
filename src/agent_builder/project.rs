use std::{fs, io::Error, path::PathBuf, str::FromStr};

use crate::templates::*;

const AGENTS_WORKSPACE_DIRECTORY: &str = ".cargo-ai/agents";

pub fn create_new_agent_project(agent_name: &str) -> Result<(), Error> {

    create_agent_workspace(agent_name)?;
    load_agent_workspace(agent_name)?;

    Ok(())
}

fn get_agent_workspace_director_path(agent_name: &str) -> PathBuf {
    let agent_workspace_directory = format!("{AGENTS_WORKSPACE_DIRECTORY}/{agent_name}");

    PathBuf::from_str(&agent_workspace_directory).expect("Unable to create agent path buffer.")
}

fn create_agent_workspace(agent_name: &str) -> Result<(), Error> {

    let agent_workspace_directory = get_agent_workspace_director_path(agent_name);

    if !agent_workspace_directory.exists() {
        fs::create_dir_all(agent_workspace_directory)?;
    }
    
    Ok(()) 
}

fn load_agent_workspace(agent_name: &str) -> Result<(), Error> {

   let agent_workspace_directory_path = get_agent_workspace_director_path(agent_name); 

   let mut build_rs_path = agent_workspace_directory_path.clone();
   build_rs_path.push(BUILD_RS_NAME);
   std::fs::write(build_rs_path, BUILD_RS_TEMPLATE)?;

   let mut agentcfg_path = agent_workspace_directory_path.clone();
    agentcfg_path.push(AGENTCFG_NAME);
    std::fs::write(agentcfg_path, AGENTCFG_TEMPLATE)?;

    Ok(())
}