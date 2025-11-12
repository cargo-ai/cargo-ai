use crate::config::loader::{load_config, config_path};
use std::fs;

pub fn remove_profile(name: &str) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(mut cfg) = load_config() {
        let before_count = cfg.profile.len();

        // If the removed profile is the current default, clear it
        let mut removed_default = false;
        if cfg.default_profile.as_deref() == Some(name) {
            cfg.default_profile = None;
            removed_default = true;
        }

        cfg.profile.retain(|p| p.name != name);

        if cfg.profile.len() == before_count {
            println!("Profile '{}' not found.", name);
        } else {
            let serialized = toml::to_string_pretty(&cfg)?;
            fs::write(config_path(), serialized)?;

            if removed_default {
                println!(
                    "Profile '{}' removed successfully (was default). Default profile cleared — this may affect agent behavior.",
                    name
                );
            } else {
                println!("Profile '{}' removed successfully.", name);
            }
        }
    } else {
        println!("No config file found.");
    }
    Ok(())
}