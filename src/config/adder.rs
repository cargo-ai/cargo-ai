use crate::config::schema::{Config, Profile};
use crate::config::loader::{load_config, config_path};
use std::fs;
use std::io::{self, Write};

pub fn add_profile(new_profile: Profile, overwrite: bool, set_as_default: bool) -> Result<(), Box<dyn std::error::Error>> {
    let mut cfg = load_config().unwrap_or(Config { 
        profile: Vec::new(), 
        cargo_ai_token: None,
        default_profile: None,
    });

    if let Some(existing) = cfg.profile.iter().position(|p| p.name == new_profile.name) {
        if overwrite {
            println!("Overwriting existing profile '{}'.", new_profile.name);
            cfg.profile[existing] = new_profile;
        } else {
            print!("Profile '{}' already exists. Replace? [y/N]: ", cfg.profile[existing].name);
            io::stdout().flush()?;
            let mut input = String::new();
            io::stdin().read_line(&mut input)?;
            if input.trim().to_lowercase() == "y" {
                cfg.profile[existing] = new_profile;
                println!("Profile '{}' updated.", cfg.profile[existing].name);
            } else {
                println!("Operation canceled.");
                return Ok(());
            }
        }
    } else {
        cfg.profile.push(new_profile);
        println!("Added new profile '{}'.", cfg.profile.last().unwrap().name);
    }

    // Handle setting or overwriting the default profile
    if set_as_default {
        let profile_name = cfg.profile.last().unwrap().name.clone();
        cfg.default_profile = Some(profile_name.clone());
        println!("Profile '{}' set as default.", profile_name);
    } else if cfg.default_profile.is_none() {
        let profile_name = cfg.profile.last().unwrap().name.clone();
        cfg.default_profile = Some(profile_name.clone());
        println!("Profile '{}' set as default (first profile added).", profile_name);
    }

    let serialized = toml::to_string_pretty(&cfg)?;
    fs::write(config_path(), serialized)?;
    Ok(())
}