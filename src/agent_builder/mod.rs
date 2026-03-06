// Handles directory structure and initialization logic for new agent projects
pub mod project;

// Handles automatically building
pub mod build;

// Handles exporting after build
pub mod export;

// Handles cleaup after export
pub mod cleanup;

// Handles per-agent workspace locking during hatch/check runs
pub mod lock;

// Handles warmed template cache resolution/building
pub mod template_cache;

use std::env;
use std::path::PathBuf;

/// Resolve Cargo’s home directory ($CARGO_HOME or default ~/.cargo)
pub(crate) fn cargo_home() -> PathBuf {
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
    cargo_ai_root().join("agents")
}

/// Root for CargoAI internal files inside Cargo home
pub(crate) fn cargo_ai_root() -> PathBuf {
    cargo_home().join(".cargo-ai")
}

/// Root for warmed template workspaces inside CargoAI home.
pub(crate) fn templates_workspace_root() -> PathBuf {
    cargo_ai_root().join("templates")
}

/// Root for lock files that coordinate hatch/check runs.
fn locks_root() -> PathBuf {
    cargo_ai_root().join("locks")
}

/// Full path to a specific agent workspace
pub fn agent_workspace_path(agent_name: &str) -> PathBuf {
    agents_workspace_root().join(agent_name)
}

/// Explicit build target requested for spawned cargo builds, if any.
pub(crate) fn configured_target_triple() -> Option<String> {
    env::var("CARGO_BUILD_TARGET")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// Effective target triple when an explicit CLI target is provided, or when
/// inherited from the environment.
pub(crate) fn resolved_target_triple(explicit_target_triple: Option<&str>) -> Option<String> {
    explicit_target_triple
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(configured_target_triple)
}

/// Target triple key used for template cache partitioning.
pub(crate) fn requested_target_triple(explicit_target_triple: Option<&str>) -> String {
    resolved_target_triple(explicit_target_triple)
        .unwrap_or_else(crate::cargo_ai_metadata::current_build_target)
}

/// Whether the resolved target should produce a Windows executable extension.
pub(crate) fn target_uses_windows_exe(explicit_target_triple: Option<&str>) -> bool {
    resolved_target_triple(explicit_target_triple)
        .map(|target| target.contains("windows"))
        .unwrap_or(cfg!(windows))
}
