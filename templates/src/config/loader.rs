use std::fs;
use std::path::PathBuf;
use crate::config::schema::{Config, Profile};

pub fn config_path() -> PathBuf {
    dirs::home_dir()
        .expect("could not find home directory")
        .join(".cargo/.cargo-ai/config.toml")
}

pub fn load_config() -> Option<Config> {
    let path = config_path();
    if !path.exists() {
        return None;
    }

    let contents = fs::read_to_string(path).ok()?;
    toml::from_str::<Config>(&contents).ok()
}

pub fn find_profile<'a>(config: &'a Config, name: &str) -> Option<&'a Profile> {
    config.profile.iter().find(|p| p.name == name)
}