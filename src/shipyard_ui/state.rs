use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct PersistedShipyardUiState {
    execution_view_visible: Option<bool>,
    execution_panel_ratio: Option<f32>,
    execution_panel_height: Option<f32>,
}

pub fn load_execution_view_visible() -> Option<bool> {
    let path = state_path()?;
    let contents = fs::read_to_string(path).ok()?;
    let state: PersistedShipyardUiState = serde_json::from_str(&contents).ok()?;

    if let Some(is_visible) = state.execution_view_visible {
        return Some(is_visible);
    }

    // Backward-compat fallback from older ratio/height settings.
    if let Some(ratio) = state.execution_panel_ratio {
        if ratio.is_finite() && ratio > 0.0 {
            return Some(true);
        }
    }

    if let Some(height) = state.execution_panel_height {
        if height.is_finite() && height > 0.0 {
            return Some(true);
        }
    }

    None
}

pub fn save_execution_view_visible(is_visible: bool) -> Result<(), String> {
    let path = state_path().ok_or_else(|| "config directory unavailable".to_string())?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }

    let state = PersistedShipyardUiState {
        execution_view_visible: Some(is_visible),
        execution_panel_ratio: None,
        execution_panel_height: None,
    };

    let json = serde_json::to_string_pretty(&state).map_err(|error| error.to_string())?;
    fs::write(path, json).map_err(|error| error.to_string())
}

fn state_path() -> Option<PathBuf> {
    let config_dir = dirs::config_dir()?;
    Some(config_dir.join("cargo-ai").join("shipyard_ui_state.json"))
}
