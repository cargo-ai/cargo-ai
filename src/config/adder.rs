use crate::config::schema::{Account, Config, Profile};
use crate::config::loader::{config_path, load_config};
use std::fs;
use std::io::{self, Write};

pub fn add_profile(new_profile: Profile, overwrite: bool, set_as_default: bool) -> Result<(), Box<dyn std::error::Error>> {
    let mut cfg = load_config().unwrap_or(Config { 
        profile: Vec::new(), 
        cargo_ai_token: None,
        default_profile: None,
        account: None,
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

pub fn set_account_email(email: String, overwrite: bool) -> Result<(), Box<dyn std::error::Error>> {
    let mut cfg = load_config().unwrap_or(Config {
        profile: Vec::new(),
        cargo_ai_token: None,
        default_profile: None,
        account: None,
    });

    let existing_email = cfg.account.as_ref().and_then(|a| a.email.clone());

    match existing_email {
        None => {
            cfg.account = Some(Account {
                email: Some(email.clone()),
                access_token: None,
                refresh_token: None,
                access_token_expires_in: None,
                access_token_issued_at: None,
            });
        }
        Some(ref old_email) if old_email == &email => {
            return Ok(());
        }
        Some(ref old_email) => {
            if overwrite {
                println!("Overwriting existing account email '{}'.", old_email);
                cfg.account = Some(Account {
                    email: Some(email.clone()),
                    access_token: None,
                    refresh_token: None,
                    access_token_expires_in: None,
                    access_token_issued_at: None,
                });
            } else {
                print!("Account email is already set to '{}'. Replace with '{}'? [y/N]: ", old_email, email);
                io::stdout().flush()?;
                let mut input = String::new();
                io::stdin().read_line(&mut input)?;
                if input.trim().to_lowercase() == "y" {
                    cfg.account = Some(Account {
                        email: Some(email.clone()),
                        access_token: None,
                        refresh_token: None,
                        access_token_expires_in: None,
                        access_token_issued_at: None,
                    });
                } else {
                    println!("Operation canceled.");
                    return Ok(());
                }
            }
        }
    }

    let serialized = toml::to_string_pretty(&cfg)?;
    fs::write(config_path(), serialized)?;
    Ok(())
}

pub fn set_account_tokens(
    access_token: String,
    refresh_token: String,
    access_token_expires_in: i32,
) -> Result<(), Box<dyn std::error::Error>> {
    use std::time::{SystemTime, UNIX_EPOCH};

    let mut cfg = load_config().unwrap_or(Config {
        profile: Vec::new(),
        cargo_ai_token: None,
        default_profile: None,
        account: None,
    });

    let issued_at = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as i64;

    if let Some(account) = cfg.account.as_mut() {
        account.access_token = Some(access_token);
        account.refresh_token = Some(refresh_token);
        account.access_token_expires_in = Some(access_token_expires_in);
        account.access_token_issued_at = Some(issued_at);
    } else {
        return Err(Box::<dyn std::error::Error>::from("No account configured. Run `cargo ai account register <email>` first."));
    }

    let serialized = toml::to_string_pretty(&cfg)?;
    fs::write(config_path(), serialized)?;
    Ok(())
}
