//! Runtime behavior for `cargo ai profile`.
use clap::ArgMatches;
use serde_json::{json, Value};
use std::fs;
use std::io::{self, Read, Write};

use crate::config::adder::add_profile;
use crate::config::loader::{config_path, find_profile, load_config};
use crate::config::remover::remove_profile;
use crate::config::schema::{Profile, ProfileAuthMode};
use crate::credentials::store;
use crate::ui;

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

fn write_config(cfg: &crate::config::schema::Config) -> Result<(), String> {
    let path = config_path();
    let serialized = toml::to_string_pretty(cfg)
        .map_err(|error| format!("failed to serialize config: {error}"))?;
    fs::write(&path, serialized)
        .map_err(|error| format!("failed to write '{}': {error}", path.display()))
}

fn profile_remove_success_ui_response(name: &str) -> Value {
    json!({
        "ui": {
            "schema": "1.0",
            "kind": "success",
            "icon": "✓",
            "title": "Profile removed",
            "summary": format!("Profile `{name}` was removed."),
            "sections": [
                {
                    "type": "kv",
                    "title": "Available commands",
                    "title_style": "plain",
                    "layout": "aligned",
                    "items": [
                        {"label": "List profiles", "value": "`cargo ai profile list`"}
                    ]
                }
            ]
        }
    })
}

fn profile_add_success_ui_response(name: &str, auth_mode: ProfileAuthMode) -> Value {
    json!({
        "ui": {
            "schema": "1.0",
            "kind": "success",
            "icon": "✓",
            "title": "Profile saved",
            "summary": format!("Created profile `{name}`."),
            "sections": [
                {
                    "type": "kv",
                    "title": "Profile",
                    "title_style": "plain",
                    "layout": "aligned",
                    "items": [
                        {"label": "Name", "value": name},
                        {"label": "Auth mode", "value": auth_mode.as_str()}
                    ]
                },
                {
                    "type": "kv",
                    "title": "Available commands",
                    "title_style": "plain",
                    "layout": "aligned",
                    "items": [
                        {"label": "Show profile", "value": format!("`cargo ai profile show {name}`")}
                    ]
                }
            ]
        }
    })
}

fn profile_set_success_ui_response(
    name: &str,
    metadata_changes: &[&str],
    token_change: Option<&str>,
    auth_mode: ProfileAuthMode,
) -> Value {
    let mut sections = Vec::new();

    let mut change_items = Vec::new();
    if !metadata_changes.is_empty() {
        change_items.push(json!({
            "label": "Metadata",
            "value": metadata_changes.join(", ")
        }));
    }
    if let Some(token_change) = token_change {
        change_items.push(json!({
            "label": "Token",
            "value": token_change
        }));
    }
    if !change_items.is_empty() {
        sections.push(json!({
            "type": "kv",
            "title": "Changes",
            "title_style": "plain",
            "layout": "aligned",
            "items": change_items
        }));
    }

    if token_change.is_some() && auth_mode != ProfileAuthMode::ApiKey {
        sections.push(json!({
            "type": "kv",
            "title": "Guidance",
            "title_style": "plain",
            "layout": "aligned",
            "items": [
                {"label": "Auth mode", "value": auth_mode.as_str()},
                {"label": "Note", "value": "Set `--auth api_key` to use the stored token by default"}
            ]
        }));
    }

    sections.push(json!({
        "type": "kv",
        "title": "Available commands",
        "title_style": "plain",
        "layout": "aligned",
        "items": [
            {"label": "Show profile", "value": format!("`cargo ai profile show {name}`")}
        ]
    }));

    json!({
        "ui": {
            "schema": "1.0",
            "kind": "success",
            "icon": "✓",
            "title": "Profile updated",
            "summary": format!("Updated profile `{name}`."),
            "sections": sections
        }
    })
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
        eprintln!("Please provide --model (for example: gpt-5.2 or mistral).");
        return false;
    };

    let auth_mode = add_m
        .get_one::<String>("auth")
        .and_then(|raw_mode| parse_auth_mode(raw_mode))
        .unwrap_or(ProfileAuthMode::None);

    let new_profile = Profile {
        name: name.to_string(),
        server: server.to_string(),
        model: model.to_string(),
        url: add_m.get_one::<String>("url").cloned(),
        token: None,
        timeout_in_sec: 60,
        description: add_m.get_one::<String>("description").cloned(),
        auth_mode,
    };

    let set_as_default = add_m.get_flag("default");

    if let Err(error) = add_profile(new_profile, false, set_as_default) {
        eprintln!("Failed to add profile: {error}");
        false
    } else {
        ui::account_status::render_backend_ui(&profile_add_success_ui_response(name, auth_mode));
        true
    }
}

fn run_set(set_m: &ArgMatches) -> bool {
    let Some(name) = set_m.get_one::<String>("name") else {
        eprintln!("❌ Missing profile name.");
        return false;
    };

    let mut cfg = match load_config() {
        Some(cfg) => cfg,
        None => {
            eprintln!("❌ No config file found.");
            return false;
        }
    };

    let Some(profile) = cfg.profile.iter_mut().find(|profile| profile.name == *name) else {
        eprintln!("❌ Profile '{}' not found.", name);
        return false;
    };

    let mut metadata_changes: Vec<&str> = Vec::new();

    if let Some(server) = set_m.get_one::<String>("server") {
        profile.server = server.to_string();
        metadata_changes.push("server");
    }

    if let Some(model) = set_m.get_one::<String>("model") {
        profile.model = model.to_string();
        metadata_changes.push("model");
    }

    if let Some(raw_mode) = set_m.get_one::<String>("auth") {
        let Some(mode) = parse_auth_mode(raw_mode) else {
            eprintln!(
                "❌ Invalid auth mode '{}'. Use none|api_key|openai_account.",
                raw_mode
            );
            return false;
        };
        profile.auth_mode = mode;
        metadata_changes.push("auth");
    }

    if let Some(url) = set_m.get_one::<String>("url") {
        profile.url = Some(url.to_string());
        metadata_changes.push("url");
    } else if set_m.get_flag("clear_url") {
        profile.url = None;
        metadata_changes.push("url");
    }

    if let Some(description) = set_m.get_one::<String>("description") {
        profile.description = Some(description.to_string());
        metadata_changes.push("description");
    } else if set_m.get_flag("clear_description") {
        profile.description = None;
        metadata_changes.push("description");
    }

    if set_m.get_flag("default") {
        cfg.default_profile = Some(name.to_string());
        metadata_changes.push("default");
    }

    let mut token_change: Option<&str> = None;
    if set_m.get_flag("clear_token") {
        if let Err(error) = store::clear_profile_token(name) {
            eprintln!("❌ Failed to clear token for profile '{}': {error}", name);
            return false;
        }
        token_change = Some("cleared");
    } else if set_m.get_one::<String>("token").is_some()
        || set_m.get_flag("stdin")
        || set_m.get_one::<String>("env").is_some()
    {
        let token = match resolve_token_input(set_m) {
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
        token_change = Some("updated");
    }

    if !metadata_changes.is_empty() {
        if let Err(error) = write_config(&cfg) {
            eprintln!("❌ Failed to persist profile updates: {error}");
            return false;
        }
    }

    let auth_mode = cfg
        .profile
        .iter()
        .find(|profile| profile.name == *name)
        .map(|profile| profile.auth_mode)
        .unwrap_or(ProfileAuthMode::None);
    ui::account_status::render_backend_ui(&profile_set_success_ui_response(
        name,
        metadata_changes.as_slice(),
        token_change,
        auth_mode,
    ));

    true
}

fn run_remove(remove_m: &ArgMatches) -> bool {
    if let Some(name) = remove_m.get_one::<String>("name") {
        if !profile_exists(name) {
            eprintln!("❌ Profile '{}' not found.", name);
            return false;
        }

        let confirmed = match confirm(&format!(
            "Are you sure you want to remove profile '{name}'?"
        )) {
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

        if let Err(error) = remove_profile(name) {
            eprintln!("Failed to remove profile '{}': {error}", name);
            false
        } else {
            ui::account_status::render_backend_ui(&profile_remove_success_ui_response(name));
            true
        }
    } else {
        eprintln!(
            "❌ Please provide a profile name to remove. Example: cargo ai profile remove openai-prod"
        );
        false
    }
}

/// Executes profile list/show/add/set/remove operations.
pub fn run(sub_m: &ArgMatches) -> bool {
    if sub_m.subcommand_matches("list").is_some() {
        run_list()
    } else if let Some(show_m) = sub_m.subcommand_matches("show") {
        run_show(show_m)
    } else if let Some(add_m) = sub_m.subcommand_matches("add") {
        run_add(add_m)
    } else if let Some(set_m) = sub_m.subcommand_matches("set") {
        run_set(set_m)
    } else if let Some(remove_m) = sub_m.subcommand_matches("remove") {
        run_remove(remove_m)
    } else {
        eprintln!(
            "❌ No profile subcommand found. Try 'cargo ai profile list', 'cargo ai profile show <name>', 'cargo ai profile add ...', or 'cargo ai profile set ...'."
        );
        false
    }
}

#[cfg(test)]
mod tests {
    use super::{
        parse_auth_mode, profile_add_success_ui_response, profile_remove_success_ui_response,
        profile_set_success_ui_response,
    };
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

    #[test]
    fn profile_remove_success_includes_list_profiles_available_command() {
        let response = profile_remove_success_ui_response("openai-prod");
        assert_eq!(response["ui"]["title"].as_str(), Some("Profile removed"));
        assert_eq!(
            response["ui"]["sections"][0]["title"].as_str(),
            Some("Available commands")
        );
        assert_eq!(
            response["ui"]["sections"][0]["items"][0]["value"].as_str(),
            Some("`cargo ai profile list`")
        );
    }

    #[test]
    fn profile_add_success_uses_available_commands() {
        let response =
            profile_add_success_ui_response("my_open_ai", ProfileAuthMode::OpenaiAccount);

        assert_eq!(response["ui"]["title"].as_str(), Some("Profile saved"));
        assert_eq!(
            response["ui"]["sections"][1]["items"][0]["value"].as_str(),
            Some("`cargo ai profile show my_open_ai`")
        );
    }

    #[test]
    fn profile_set_success_includes_guidance_when_token_updates_non_api_key_mode() {
        let response = profile_set_success_ui_response(
            "my_open_ai",
            &["server", "model"],
            Some("updated"),
            ProfileAuthMode::OpenaiAccount,
        );

        assert_eq!(response["ui"]["title"].as_str(), Some("Profile updated"));
        assert_eq!(
            response["ui"]["sections"][0]["items"][0]["value"].as_str(),
            Some("server, model")
        );
        assert_eq!(
            response["ui"]["sections"][1]["title"].as_str(),
            Some("Guidance")
        );
        assert_eq!(
            response["ui"]["sections"][2]["items"][0]["value"].as_str(),
            Some("`cargo ai profile show my_open_ai`")
        );
    }
}
