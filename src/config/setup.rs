use crate::config::schema::{default_secret_store_mode, Config};
use std::fs;
use std::io;
use std::path::PathBuf;

pub fn cargo_ai_root() -> PathBuf {
    crate::config::paths::cargo_ai_root()
}

pub fn config_path() -> PathBuf {
    cargo_ai_root().join("config.toml")
}

pub fn ensure_config_dir_exists() -> io::Result<()> {
    fs::create_dir_all(cargo_ai_root())
}

pub fn ensure_config_file_exists() -> Result<(), Box<dyn std::error::Error>> {
    ensure_config_dir_exists()?;

    let path = config_path();
    if !path.exists() {
        let cfg = Config {
            profile: Vec::new(),
            cargo_ai_token: None,
            default_profile: None,
            secret_store: Some(default_secret_store_mode()),
            account: None,
            openai_auth: None,
            web_resources: None,
            update_check: None,
            cargo_ai_metadata: None,
        };

        let serialized = toml::to_string_pretty(&cfg)?;
        fs::write(path, serialized)?;
    }

    Ok(())
}
