use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::shipyard_ui::config;

#[derive(Serialize, Deserialize)]
struct PersistedShipyardUiState {
    execution_panel_ratio: Option<f32>,
    execution_panel_height: Option<f32>,
}

pub fn load_execution_panel_ratio() -> Option<f32> {
    let path = state_path()?;
    let contents = fs::read_to_string(path).ok()?;
    let state: PersistedShipyardUiState = serde_json::from_str(&contents).ok()?;

    if let Some(ratio) = state.execution_panel_ratio {
        if ratio.is_finite() {
            return Some(config::clamp_execution_panel_ratio(ratio));
        }
    }

    // Backward-compat fallback for earlier persisted height-only state.
    if let Some(height) = state.execution_panel_height {
        if height.is_finite() && height > 0.0 {
            return Some(config::clamp_execution_panel_ratio(
                height / config::WINDOW_INITIAL_HEIGHT,
            ));
        }
    }

    None
}

pub fn save_execution_panel_ratio(ratio: f32) -> Result<(), String> {
    let path = state_path().ok_or_else(|| "config directory unavailable".to_string())?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }

    let state = PersistedShipyardUiState {
        execution_panel_ratio: Some(config::clamp_execution_panel_ratio(ratio)),
        execution_panel_height: None,
    };

    let json = serde_json::to_string_pretty(&state).map_err(|error| error.to_string())?;
    fs::write(path, json).map_err(|error| error.to_string())
}

fn state_path() -> Option<PathBuf> {
    let config_dir = dirs::config_dir()?;
    Some(config_dir.join("cargo-ai").join("shipyard_ui_state.json"))
}
