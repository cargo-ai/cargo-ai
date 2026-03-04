//! Runtime behavior for `cargo ai profile`.
use clap::ArgMatches;

use crate::config::adder::add_profile;
use crate::config::loader::{find_profile, load_config};
use crate::config::remover::remove_profile;
use crate::config::schema::Profile;
use crate::credentials::store;

/// Executes profile list/show/add/remove operations.
pub fn run(sub_m: &ArgMatches) -> bool {
    if let Some(_) = sub_m.subcommand_matches("list") {
        if let Some(cfg) = load_config() {
            println!("Configured profiles:");
            println!(
                "{:<20} {:<10} {:<15} {}",
                "Name", "Server", "Model", "Default"
            );
            println!("{:-<65}", "");

            let default_name = cfg.default_profile.clone();

            for profile in cfg.profile {
                let is_default = default_name
                    .as_ref()
                    .map(|d| d == &profile.name)
                    .unwrap_or(false);
                let mark = if is_default { "✓" } else { "" };

                println!(
                    "{:<20} {:<10} {:<15} {}",
                    profile.name, profile.server, profile.model, mark
                );
            }
            true
        } else {
            eprintln!("❌ No config file found.");
            false
        }
    } else if let Some(add_m) = sub_m.subcommand_matches("add") {
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
        let token = add_m
            .get_one::<String>("token")
            .map(String::as_str)
            .unwrap_or("(none)");
        let description = add_m
            .get_one::<String>("description")
            .map(String::as_str)
            .unwrap_or("(none)");

        println!("Adding profile:");
        println!("  Name: {}", name);
        println!("  Server: {}", server);
        println!("  Model: {}", model);
        println!("  URL: {}", url);
        println!(
            "  Token: {}",
            if token == "(none)" {
                "(none)"
            } else {
                "***********"
            }
        );
        println!("  Description: {}", description);

        let new_profile = Profile {
            name: name.to_string(),
            server: server.to_string(),
            model: model.to_string(),
            url: if url == "(none)" {
                None
            } else {
                Some(url.to_string())
            },
            token: if token == "(none)" {
                None
            } else {
                Some(token.to_string())
            },
            timeout_in_sec: 60, // default for now
            description: if description == "(none)" {
                None
            } else {
                Some(description.to_string())
            },
        };

        let set_as_default = add_m.get_flag("default");

        if let Err(e) = add_profile(new_profile, false, set_as_default) {
            eprintln!("Failed to add profile: {}", e);
            false
        } else {
            true
        }
    } else if let Some(remove_m) = sub_m.subcommand_matches("remove") {
        if let Some(name) = remove_m.get_one::<String>("name") {
            if let Some(cfg) = load_config() {
                if cfg.profile.iter().any(|p| p.name == *name) {
                    use std::io::{self, Write};
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
                        if let Err(e) = remove_profile(name) {
                            eprintln!("Failed to remove profile '{}': {}", name, e);
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
            eprintln!(
                "❌ Please provide a profile name to remove. Example: cargo ai profile remove openai-prod"
            );
            false
        }
    } else if let Some(show_m) = sub_m.subcommand_matches("show") {
        if let Some(name) = show_m.get_one::<String>("name") {
            if let Some(cfg) = load_config() {
                if let Some(p) = find_profile(&cfg, name) {
                    println!("Profile: {}", p.name);
                    let is_default = cfg
                        .default_profile
                        .as_ref()
                        .map(|d| d == &p.name)
                        .unwrap_or(false);
                    if is_default {
                        println!("Default: Yes");
                    } else {
                        println!("Default: No");
                    }
                    println!("Server:  {}", p.server);
                    println!("Model:   {}", p.model);
                    let token_available = match store::load_profile_token(&p.name) {
                        Ok(Some(_)) => true,
                        Ok(None) => p.token.is_some(),
                        Err(error) => {
                            eprintln!(
                                "⚠️ Failed to load profile token from credential store: {error}"
                            );
                            p.token.is_some()
                        }
                    };
                    println!(
                        "Token:   {}",
                        if token_available {
                            "***********"
                        } else {
                            "(none)"
                        }
                    );
                    println!("Timeout: {}", p.timeout_in_sec);
                    if let Some(desc) = &p.description {
                        println!("Description: {}", desc);
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
            eprintln!(
                "❌ Please provide a profile name. Example: cargo ai profile show openai-prod"
            );
            false
        }
    } else {
        eprintln!("❌ No profile subcommand found. Try 'cargo ai profile list'.");
        false
    }
}
