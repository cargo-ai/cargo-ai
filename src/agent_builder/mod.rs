// Handles directory structure and initialization logic for new agent projects
pub mod project;

// Handles automatically building
pub mod build;

// Handles exporting after build
pub mod export;

// Handles cleaup after export
pub mod cleanup;

use std::env;
use std::path::PathBuf;

/// Resolve Cargo’s home directory ($CARGO_HOME or default ~/.cargo)
fn cargo_home() -> PathBuf {
    if let Ok(path) = env::var("CARGO_HOME") {
        PathBuf::from(path)
    } else {
        dirs::home_dir()
            .expect("could not find home directory")
            .join(".cargo")
    }
}

/// Root for CargoAI’s agents inside Cargo home
fn agents_workspace_root() -> PathBuf {
    cargo_home().join(".cargo-ai/agents")
}

/// Full path to a specific agent workspace
pub fn agent_workspace_path(agent_name: &str) -> PathBuf {
    agents_workspace_root().join(agent_name)
}