use crate::config::loader::{config_path, load_config};
use crate::credentials::store;
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
            if let Err(error) = store::clear_profile_token(name) {
                eprintln!(
                    "⚠️ Profile removed from config, but token cleanup failed for '{}': {}",
                    name, error
                );
            }

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
