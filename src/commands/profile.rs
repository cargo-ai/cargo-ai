//! Runtime behavior for `cargo ai profile`.
use clap::ArgMatches;

use crate::config::adder::add_profile;
use crate::config::loader::{find_profile, load_config};
use crate::config::remover::remove_profile;
use crate::config::schema::Profile;

/// Executes profile list/show/add/remove operations.
pub fn run(sub_m: &ArgMatches) {
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
        } else {
            println!("No config file found.");
        }
    } else if let Some(add_m) = sub_m.subcommand_matches("add") {
        let name = add_m
            .get_one::<String>("name")
            .expect("Profile name is required");
        let server = add_m
            .get_one::<String>("server")
            .expect("Server is required");
        let model = add_m.get_one::<String>("model").expect("Model is required");
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
        println!("  Token: {}", token);
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
                    io::stdout().flush().unwrap();

                    let mut input = String::new();
                    io::stdin().read_line(&mut input).unwrap();

                    if input.trim().eq_ignore_ascii_case("y")
                        || input.trim().eq_ignore_ascii_case("yes")
                    {
                        if let Err(e) = remove_profile(name) {
                            eprintln!("Failed to remove profile '{}': {}", name, e);
                        }
                    } else {
                        println!("Operation canceled.");
                    }
                } else {
                    println!("Profile '{}' not found.", name);
                }
            } else {
                println!("No config file found.");
            }
        } else {
            println!("Please provide a profile name to remove. Example: cargo ai profile remove openai-prod");
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
                    println!(
                        "Token:   {}",
                        p.token.as_ref().map(|_| "***********").unwrap_or("(none)")
                    );
                    println!("Timeout: {}", p.timeout_in_sec);
                    if let Some(desc) = &p.description {
                        println!("Description: {}", desc);
                    }
                } else {
                    println!("Profile '{}' not found.", name);
                }
            } else {
                println!("No config file found.");
            }
        } else {
            println!("Please provide a profile name. Example: cargo ai profile show openai-prod");
        }
    } else {
        println!("No profile subcommand found. Try 'cargo ai profile list'.");
    }
}
