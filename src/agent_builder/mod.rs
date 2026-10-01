// Handles directory structure and initialization logic for new agent projects
#[cfg(feature = "developer-tools")]
pub mod project;

// Handles automatically building
#[cfg(feature = "developer-tools")]
pub mod build;

// Handles exporting after build
#[cfg(feature = "developer-tools")]
pub mod export;

// Handles cleaup after export
#[cfg(feature = "developer-tools")]
pub mod cleanup;

// Handles build-target resolution and target-specific output behavior
pub mod build_target;

// Handles per-agent workspace locking during hatch/check runs
#[cfg(feature = "developer-tools")]
pub mod lock;

// Handles warmed template cache resolution/building
#[cfg(feature = "developer-tools")]
pub mod template_cache;
#[cfg(feature = "developer-tools")]
use std::path::PathBuf;

/// Root for CargoAI’s agents inside Cargo home
#[cfg(feature = "developer-tools")]
fn agents_workspace_root() -> PathBuf {
    cargo_ai_root().join("agents")
}

/// Root for CargoAI internal files inside Cargo home
#[cfg(feature = "developer-tools")]
pub(crate) fn cargo_ai_root() -> PathBuf {
    crate::config::paths::cargo_ai_root()
}

/// Root for warmed template workspaces inside CargoAI home.
#[cfg(feature = "developer-tools")]
pub(crate) fn templates_workspace_root() -> PathBuf {
    cargo_ai_root().join("templates")
}

/// Root for lock files that coordinate hatch/check runs.
#[cfg(feature = "developer-tools")]
fn locks_root() -> PathBuf {
    cargo_ai_root().join("locks")
}

/// Full path to a specific agent workspace
#[cfg(feature = "developer-tools")]
pub fn agent_workspace_path(agent_name: &str) -> PathBuf {
    agents_workspace_root().join(agent_name)
}
