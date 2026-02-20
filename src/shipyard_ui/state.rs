use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct PersistedShipyardUiState {
    execution_panel_height: f32,
}

pub fn load_execution_panel_height() -> Option<f32> {
    let path = state_path()?;
    let contents = fs::read_to_string(path).ok()?;
    let state: PersistedShipyardUiState = serde_json::from_str(&contents).ok()?;
    if state.execution_panel_height.is_finite() && state.execution_panel_height > 0.0 {
        Some(state.execution_panel_height)
    } else {
        None
    }
}

pub fn save_execution_panel_height(height: f32) -> Result<(), String> {
    let path = state_path().ok_or_else(|| "config directory unavailable".to_string())?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }

    let state = PersistedShipyardUiState {
        execution_panel_height: height,
    };
    let json = serde_json::to_string_pretty(&state).map_err(|error| error.to_string())?;
    fs::write(path, json).map_err(|error| error.to_string())
}

fn state_path() -> Option<PathBuf> {
    let config_dir = dirs::config_dir()?;
    Some(config_dir.join("cargo-ai").join("shipyard_ui_state.json"))
}
