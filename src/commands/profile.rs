//! Runtime behavior for `cargo ai profile`.
use clap::ArgMatches;
use std::io::{self, Read, Write};

use crate::config::adder::add_profile;
use crate::config::loader::{find_profile, load_config};
use crate::config::remover::remove_profile;
use crate::config::schema::{default_profile_auth_mode, Profile, ProfileAuthMode};
use crate::config::settings as config_settings;
use crate::credentials::store;

fn parse_auth_mode(raw: &str) -> Option<ProfileAuthMode> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "none" => Some(ProfileAuthMode::None),
        "api_key" => Some(ProfileAuthMode::ApiKey),
        "openai_account" => Some(ProfileAuthMode::OpenaiAccount),
        _ => None,
    }
}

fn profile_exists(name: &str) -> bool {
    load_config()
        .map(|cfg| cfg.profile.iter().any(|profile| profile.name == name))
        .unwrap_or(false)
}

fn confirm(message: &str) -> Result<bool, String> {
    print!("{message} [y/N]: ");
    io::stdout()
        .flush()
        .map_err(|error| format!("failed to flush stdout: {error}"))?;
    let mut input = String::new();
    io::stdin()
        .read_line(&mut input)
        .map_err(|error| format!("failed to read confirmation input: {error}"))?;
    Ok(matches!(
        input.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

fn resolve_token_input(set_m: &ArgMatches) -> Result<String, String> {
    if let Some(token) = set_m.get_one::<String>("token") {
        let trimmed = token.trim();
        if trimmed.is_empty() {
            return Err("`--token` cannot be empty.".to_string());
        }
        return Ok(trimmed.to_string());
    }

    if set_m.get_flag("stdin") {
        let mut input = String::new();
        io::stdin()
            .read_to_string(&mut input)
            .map_err(|error| format!("failed reading token from stdin: {error}"))?;
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return Err("no token content was received from stdin.".to_string());
        }
        return Ok(trimmed.to_string());
    }

    if let Some(env_var) = set_m.get_one::<String>("env") {
        let value = std::env::var(env_var)
            .map_err(|_| format!("environment variable '{env_var}' is not set"))?;
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Err(format!("environment variable '{env_var}' is empty"));
        }
        return Ok(trimmed.to_string());
    }

    Err("no token source provided".to_string())
}

fn run_list() -> bool {
    if let Some(cfg) = load_config() {
        println!("Configured profiles:");
        println!(
            "{:<20} {:<10} {:<20} {:<15} {}",
            "Name", "Server", "Auth mode", "Model", "Default"
        );
        println!("{:-<90}", "");

        let default_name = cfg.default_profile.clone();

        for profile in cfg.profile {
            let is_default = default_name
                .as_ref()
                .map(|default_profile| default_profile == &profile.name)
                .unwrap_or(false);
            let mark = if is_default { "✓" } else { "" };

            println!(
                "{:<20} {:<10} {:<20} {:<15} {}",
                profile.name,
                profile.server,
                profile.auth_mode.as_str(),
                profile.model,
                mark
            );
        }
        true
    } else {
        eprintln!("❌ No config file found.");
        false
    }
}

fn run_show(show_m: &ArgMatches) -> bool {
    if let Some(name) = show_m.get_one::<String>("name") {
        if let Some(cfg) = load_config() {
            if let Some(profile) = find_profile(&cfg, name) {
                println!("Profile: {}", profile.name);
                let is_default = cfg
                    .default_profile
                    .as_ref()
                    .map(|default_profile| default_profile == &profile.name)
                    .unwrap_or(false);
                println!("Default: {}", if is_default { "Yes" } else { "No" });
                println!("Server:  {}", profile.server);
                println!("Model:   {}", profile.model);
                println!("Auth:    {}", profile.auth_mode.as_str());
                let token_available = match store::load_profile_token(&profile.name) {
                    Ok(Some(_)) => true,
                    Ok(None) => profile.token.is_some(),
                    Err(error) => {
                        eprintln!("⚠️ Failed to load profile token from credential store: {error}");
                        profile.token.is_some()
                    }
                };
                println!(
                    "Token:   {}",
                    if token_available { "present" } else { "(none)" }
                );
                println!("Timeout: {}", profile.timeout_in_sec);
                if let Some(url) = &profile.url {
                    println!("URL:     {}", url);
                }
                if let Some(description) = &profile.description {
                    println!("Description: {}", description);
                }
                true
            } else {
                eprintln!("❌ Profile '{}' not found.", name);
                false
            }
        } else {
            eprintln!("❌ No config file found.");
            false
        }
    } else {
        eprintln!("❌ Please provide a profile name. Example: cargo ai profile show openai-prod");
        false
    }
}

fn run_add(add_m: &ArgMatches) -> bool {
    let Some(name) = add_m.get_one::<String>("name") else {
        eprintln!("Please provide a profile name. Example: cargo ai profile add <name> ...");
        return false;
    };
    let Some(server) = add_m.get_one::<String>("server") else {
        eprintln!("Please provide --server (for example: openai or ollama).");
        return false;
    };
    let Some(model) = add_m.get_one::<String>("model") else {
        eprintln!("Please provide --model (for example: gpt-4o or mistral).");
        return false;
    };

    let url = add_m
        .get_one::<String>("url")
        .map(String::as_str)
        .unwrap_or("(none)");
    let description = add_m
        .get_one::<String>("description")
        .map(String::as_str)
        .unwrap_or("(none)");
    let auth_mode = if let Some(raw_mode) = add_m.get_one::<String>("auth") {
        let Some(mode) = parse_auth_mode(raw_mode) else {
            eprintln!(
                "❌ Invalid auth mode '{}'. Use none|api_key|openai_account.",
                raw_mode
            );
            return false;
        };
        mode
    } else {
        default_profile_auth_mode()
    };

    let new_profile = Profile {
        name: name.to_string(),
        server: server.to_string(),
        model: model.to_string(),
        url: if url == "(none)" {
            None
        } else {
            Some(url.to_string())
        },
        token: None,
        timeout_in_sec: 60,
        description: if description == "(none)" {
            None
        } else {
            Some(description.to_string())
        },
        auth_mode,
    };

    let set_as_default = add_m.get_flag("default");

    if let Err(error) = add_profile(new_profile, false, set_as_default) {
        eprintln!("Failed to add profile: {error}");
        false
    } else {
        println!(
            "✅ Profile '{}' saved. Auth mode: '{}'.",
            name,
            auth_mode.as_str()
        );
        true
    }
}

fn run_remove(remove_m: &ArgMatches) -> bool {
    if let Some(name) = remove_m.get_one::<String>("name") {
        if let Some(cfg) = load_config() {
            if cfg.profile.iter().any(|profile| profile.name == *name) {
                print!(
                    "Are you sure you want to remove profile '{}'? [y/N]: ",
                    name
                );
                if let Err(error) = io::stdout().flush() {
                    eprintln!("Failed to flush stdout: {error}");
                    return false;
                }

                let mut input = String::new();
                if let Err(error) = io::stdin().read_line(&mut input) {
                    eprintln!("Failed to read input: {error}");
                    return false;
                }

                if input.trim().eq_ignore_ascii_case("y")
                    || input.trim().eq_ignore_ascii_case("yes")
                {
                    if let Err(error) = remove_profile(name) {
                        eprintln!("Failed to remove profile '{}': {error}", name);
                        return false;
                    }
                    true
                } else {
                    println!("Operation canceled.");
                    true
                }
            } else {
                eprintln!("❌ Profile '{}' not found.", name);
                false
            }
        } else {
            eprintln!("❌ No config file found.");
            false
        }
    } else {
        eprintln!("❌ Please provide a profile name to remove. Example: cargo ai profile remove openai-prod");
        false
    }
}

fn run_auth_set(auth_set_m: &ArgMatches) -> bool {
    let Some(name) = auth_set_m.get_one::<String>("name") else {
        eprintln!("❌ Missing profile name.");
        return false;
    };
    let Some(raw_mode) = auth_set_m.get_one::<String>("mode") else {
        eprintln!("❌ Missing auth mode.");
        return false;
    };
    let Some(mode) = parse_auth_mode(raw_mode) else {
        eprintln!(
            "❌ Invalid auth mode '{}'. Use none|api_key|openai_account.",
            raw_mode
        );
        return false;
    };

    if let Err(error) = config_settings::set_profile_auth_mode(name, mode) {
        eprintln!("❌ Failed to set auth mode for profile '{}': {error}", name);
        return false;
    }

    println!(
        "✅ Profile '{}' auth mode set to '{}'.",
        name,
        mode.as_str()
    );
    true
}

fn run_auth_status(auth_status_m: &ArgMatches) -> bool {
    let cfg = match load_config() {
        Some(cfg) => cfg,
        None => {
            eprintln!("❌ No config file found.");
            return false;
        }
    };

    if let Some(name) = auth_status_m.get_one::<String>("name") {
        if let Some(profile) = cfg.profile.iter().find(|profile| profile.name == *name) {
            println!(
                "Profile '{}' auth mode: {}",
                profile.name,
                profile.auth_mode.as_str()
            );
            true
        } else {
            eprintln!("❌ Profile '{}' not found.", name);
            false
        }
    } else {
        println!("Profile auth modes:");
        println!("{:<20} {:<10} {}", "Name", "Server", "Auth mode");
        println!("{:-<55}", "");
        for profile in cfg.profile {
            println!(
                "{:<20} {:<10} {}",
                profile.name,
                profile.server,
                profile.auth_mode.as_str()
            );
        }
        true
    }
}

fn run_token_set(token_set_m: &ArgMatches) -> bool {
    let Some(name) = token_set_m.get_one::<String>("name") else {
        eprintln!("❌ Missing profile name.");
        return false;
    };

    if !profile_exists(name) {
        eprintln!("❌ Profile '{}' not found.", name);
        return false;
    }

    let token = match resolve_token_input(token_set_m) {
        Ok(token) => token,
        Err(error) => {
            eprintln!("❌ Failed to read token input: {error}");
            return false;
        }
    };

    if let Err(error) = store::store_profile_token(name, token.as_str()) {
        eprintln!("❌ Failed to store token for profile '{}': {error}", name);
        return false;
    }

    println!("✅ Stored API token for profile '{}'.", name);

    if let Some(cfg) = load_config() {
        if let Some(profile) = cfg.profile.iter().find(|profile| profile.name == *name) {
            if profile.auth_mode != ProfileAuthMode::ApiKey {
                println!(
                    "ℹ️ Profile '{}' auth mode is '{}'. Set it to '{}' when you want to use this token by default:",
                    name,
                    profile.auth_mode.as_str(),
                    ProfileAuthMode::ApiKey.as_str()
                );
                println!(
                    "   cargo ai profile auth set {} {}",
                    name,
                    ProfileAuthMode::ApiKey.as_str()
                );
            }
        }
    }

    true
}

fn run_token_clear(token_clear_m: &ArgMatches) -> bool {
    let Some(name) = token_clear_m.get_one::<String>("name") else {
        eprintln!("❌ Missing profile name.");
        return false;
    };

    if !profile_exists(name) {
        eprintln!("❌ Profile '{}' not found.", name);
        return false;
    }

    if !token_clear_m.get_flag("yes") {
        let confirmed = match confirm(&format!("Clear API token for profile '{name}'?")) {
            Ok(confirmed) => confirmed,
            Err(error) => {
                eprintln!("❌ {error}");
                return false;
            }
        };
        if !confirmed {
            println!("Operation canceled.");
            return true;
        }
    }

    if let Err(error) = store::clear_profile_token(name) {
        eprintln!("❌ Failed to clear token for profile '{}': {error}", name);
        return false;
    }

    println!("✅ Cleared API token for profile '{}'.", name);
    true
}

fn run_token_status(token_status_m: &ArgMatches) -> bool {
    let Some(name) = token_status_m.get_one::<String>("name") else {
        eprintln!("❌ Missing profile name.");
        return false;
    };

    if !profile_exists(name) {
        eprintln!("❌ Profile '{}' not found.", name);
        return false;
    }

    let token_present = match store::load_profile_token(name) {
        Ok(Some(token)) => !token.trim().is_empty(),
        Ok(None) => false,
        Err(error) => {
            eprintln!(
                "❌ Failed to inspect token status for profile '{}': {error}",
                name
            );
            return false;
        }
    };

    println!(
        "Profile '{}' token status: {}",
        name,
        if token_present { "present" } else { "missing" }
    );
    true
}

/// Executes profile list/show/add/remove operations.
pub fn run(sub_m: &ArgMatches) -> bool {
    if sub_m.subcommand_matches("list").is_some() {
        run_list()
    } else if let Some(show_m) = sub_m.subcommand_matches("show") {
        run_show(show_m)
    } else if let Some(add_m) = sub_m.subcommand_matches("add") {
        run_add(add_m)
    } else if let Some(remove_m) = sub_m.subcommand_matches("remove") {
        run_remove(remove_m)
    } else if let Some(auth_m) = sub_m.subcommand_matches("auth") {
        if let Some(auth_set_m) = auth_m.subcommand_matches("set") {
            run_auth_set(auth_set_m)
        } else if let Some(auth_status_m) = auth_m.subcommand_matches("status") {
            run_auth_status(auth_status_m)
        } else {
            eprintln!(
                "❌ No profile auth subcommand found. Try 'cargo ai profile auth set <name> <none|api_key|openai_account>' or 'cargo ai profile auth status [name]'."
            );
            false
        }
    } else if let Some(token_m) = sub_m.subcommand_matches("token") {
        if let Some(token_set_m) = token_m.subcommand_matches("set") {
            run_token_set(token_set_m)
        } else if let Some(token_clear_m) = token_m.subcommand_matches("clear") {
            run_token_clear(token_clear_m)
        } else if let Some(token_status_m) = token_m.subcommand_matches("status") {
            run_token_status(token_status_m)
        } else {
            eprintln!(
                "❌ No profile token subcommand found. Try 'cargo ai profile token set|clear|status ...'."
            );
            false
        }
    } else {
        eprintln!(
            "❌ No profile subcommand found. Try 'cargo ai profile list', 'cargo ai profile auth ...', or 'cargo ai profile token ...'."
        );
        false
    }
}

#[cfg(test)]
mod tests {
    use super::parse_auth_mode;
    use crate::config::schema::ProfileAuthMode;

    #[test]
    fn parse_auth_mode_supports_all_modes() {
        assert_eq!(parse_auth_mode("none"), Some(ProfileAuthMode::None));
        assert_eq!(parse_auth_mode("api_key"), Some(ProfileAuthMode::ApiKey));
        assert_eq!(
            parse_auth_mode("openai_account"),
            Some(ProfileAuthMode::OpenaiAccount)
        );
        assert_eq!(parse_auth_mode("wat"), None);
    }
}
