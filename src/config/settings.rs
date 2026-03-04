use crate::config::loader::{config_path, load_config};
use crate::config::schema::{default_secret_store_mode, Config, SecretStoreMode};
use std::fs;

fn default_config() -> Config {
    Config {
        profile: Vec::new(),
        cargo_ai_token: None,
        default_profile: None,
        secret_store: Some(default_secret_store_mode()),
        account: None,
        web_resources: None,
        update_check: None,
        version_baseline: None,
    }
}

pub fn set_secret_store_mode(mode: SecretStoreMode) -> Result<(), String> {
    let mut cfg = load_config().unwrap_or_else(default_config);
    cfg.secret_store = Some(mode);

    let path = config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            format!(
                "failed to create config directory '{}': {error}",
                parent.display()
            )
        })?;
    }

    let serialized = toml::to_string_pretty(&cfg)
        .map_err(|error| format!("failed to serialize config: {error}"))?;
    fs::write(&path, serialized)
        .map_err(|error| format!("failed to write '{}': {error}", path.display()))
}
