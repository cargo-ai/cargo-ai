

use crate::config::loader::{load_config, config_path};
use std::fs;

pub fn remove_profile(name: &str) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(mut cfg) = load_config() {
        let before_count = cfg.profile.len();
        cfg.profile.retain(|p| p.name != name);

        if cfg.profile.len() == before_count {
            println!("Profile '{}' not found.", name);
        } else {
            let serialized = toml::to_string_pretty(&cfg)?;
            fs::write(config_path(), serialized)?;
            println!("Profile '{}' removed successfully.", name);
        }
    } else {
        println!("No config file found.");
    }
    Ok(())
}