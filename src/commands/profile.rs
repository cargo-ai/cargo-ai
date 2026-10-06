//! Runtime behavior for `cargo ai profile`.
use super::secret_input;
use clap::ArgMatches;
use serde_json::{json, Value};
use std::fs;
use std::io::{self, Write};

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

fn thinking_from_args(args: &ArgMatches) -> Option<crate::providers::thinking::ThinkingSetting> {
    use crate::providers::thinking::ThinkingSetting;
    if let Some(value) = args.get_one::<String>("thinking_choice") {
        Some(ThinkingSetting::Choice {
            value: value.clone(),
        })
    } else if let Some(value) = args.get_one::<String>("thinking") {
        Some(ThinkingSetting::from_cli(value))
    } else if args.get_flag("thinking_provider_default") {
        Some(ThinkingSetting::ProviderDefault)
    } else {
        None
    }
}

fn thinking_label(setting: Option<&crate::providers::thinking::ThinkingSetting>) -> &str {
    use crate::providers::thinking::ThinkingSetting;
    match setting {
        Some(ThinkingSetting::Choice { value }) => value,
        Some(ThinkingSetting::ProviderDefault) => "provider default (explicit)",
        Some(ThinkingSetting::On) => "on",
        Some(ThinkingSetting::Off) => "off",
        None => "inherited/provider default",
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
        return secret_input::read_stdin(secret_input::PROFILE_TOKEN_LIMIT).map_err(str::to_owned);
    }

    if let Some(env_var) = set_m.get_one::<String>("env") {
        let value = std::env::var(env_var)
            .map_err(|_| "The selected token environment variable is unavailable.".to_string())?;
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Err("The selected token environment variable is empty.".to_string());
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
            "{:<20} {:<10} {:<20} {:<15} {:<30} {}",
            "Name", "Server", "Auth mode", "Model", "Thinking", "Default"
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
                "{:<20} {:<10} {:<20} {:<15} {:<30} {}",
                profile.name,
                profile.server,
                profile.auth_mode.as_str(),
                profile.model,
                thinking_label(profile.thinking.as_ref()),
                mark
            );
        }
        true
    } else {
        eprintln!("x No config file found.");
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
                println!("Thinking: {}", thinking_label(profile.thinking.as_ref()));
                println!("Auth:    {}", profile.auth_mode.as_str());
                let token_available = match store::load_profile_token(&profile.name) {
                    Ok(Some(_)) => true,
                    Ok(None) => profile.token.is_some(),
                    Err(error) => {
                        eprintln!("! Failed to load profile token from credential store: {error}");
                        profile.token.is_some()
                    }
                };
                println!(
                    "Token:   {}",
                    if token_available { "present" } else { "(none)" }
                );
                println!("Timeout: {}", profile.timeout_in_sec);
                println!(
                    "Temperature: {}",
                    profile
                        .temperature
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "provider default".to_string())
                );
                println!(
                    "Max output tokens: {}",
                    profile
                        .max_output_tokens
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "provider default".to_string())
                );
                if let Some(url) = &profile.url {
                    println!("URL:     {}", url);
                }
                if let Some(description) = &profile.description {
                    println!("Description: {}", description);
                }
                true
            } else {
                eprintln!("x Profile '{}' not found.", name);
                false
            }
        } else {
            eprintln!("x No config file found.");
            false
        }
    } else {
        eprintln!("x Please provide a profile name. Example: cargo ai profile show openai-prod");
        false
    }
}

fn profile_from_add(
    add_m: &ArgMatches,
    name: &str,
    server: &str,
    model: &str,
    auth_mode: ProfileAuthMode,
) -> Profile {
    Profile {
        name: name.to_string(),
        server: server.to_string(),
        model: model.to_string(),
        url: add_m.get_one::<String>("url").cloned(),
        token: None,
        timeout_in_sec: 60,
        max_output_tokens: add_m.get_one::<u32>("max_output_tokens").copied(),
        temperature: add_m.get_one::<f64>("temperature").copied(),
        thinking: thinking_from_args(add_m),
        description: add_m.get_one::<String>("description").cloned(),
        auth_mode,
    }
}

fn run_add(add_m: &ArgMatches) -> bool {
    let Some(name) = add_m.get_one::<String>("name") else {
        eprintln!("Please provide a profile name. Example: cargo ai profile add <name> ...");
        return false;
    };
    let Some(server) = add_m.get_one::<String>("server") else {
        eprintln!("Please provide --server (anthropic, gemini, mistral, ollama, openai, typesafe, or xai).");
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

    let new_profile = profile_from_add(add_m, name, server, model, auth_mode);

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
    run_set_with_writer(set_m, write_config)
}

struct SetOutcome {
    name: String,
    metadata_changes: Vec<&'static str>,
    token_change: Option<&'static str>,
    auth_mode: ProfileAuthMode,
    effects: Value,
    expected_profile: Value,
    expected_default: Option<String>,
    expected_token: Option<String>,
}
impl std::fmt::Debug for SetOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SetOutcome")
            .field("metadata_changes", &self.metadata_changes)
            .field("token_change", &self.token_change)
            .field("effects", &self.effects)
            .finish_non_exhaustive()
    }
}
#[derive(Debug)]
struct SetFailure {
    code: &'static str,
    legacy_message: String,
    effects: Value,
}
const STORE_TOKEN_FAILURE: &str = "x Failed to store the profile token. Check credential-store access and configuration; setup is incomplete.";
const CLEAR_TOKEN_FAILURE: &str =
    "x Failed to clear the profile token. Check credential-store access and configuration.";
impl SetFailure {
    fn new(code: &'static str, legacy_message: &str, effects: Value) -> Self {
        Self {
            code,
            legacy_message: legacy_message.to_owned(),
            effects,
        }
    }

    fn context_unavailable(reason: &str, operation: Option<&str>, effects: Value) -> Self {
        let message = match operation {
            Some(operation) => format!("{operation} {reason}"),
            None => reason.to_owned(),
        };
        Self::new("profile.context_unavailable", &message, effects)
    }
}
fn run_set_with_writer(
    set_m: &ArgMatches,
    persist_config: impl FnOnce(&crate::config::schema::Config) -> Result<(), String>,
) -> bool {
    match set_outcome(set_m, persist_config) {
        Ok(result) => {
            ui::account_status::render_backend_ui(&profile_set_success_ui_response(
                &result.name,
                &result.metadata_changes,
                result.token_change,
                result.auth_mode,
            ));
            true
        }
        Err(error) => {
            eprintln!("{}", error.legacy_message);
            false
        }
    }
}

fn set_outcome(
    set_m: &ArgMatches,
    persist_config: impl FnOnce(&crate::config::schema::Config) -> Result<(), String>,
) -> Result<SetOutcome, SetFailure> {
    let mut effects = json!({"local":"unapplied","remote":"unapplied","credential":"unapplied","metadata":"unapplied","credential_migration":"unapplied"});
    let Some(name) = set_m.get_one::<String>("name") else {
        return Err(SetFailure::new(
            "input.invalid",
            "x Missing profile name.",
            effects,
        ));
    };

    let token = if set_m.get_one::<String>("token").is_some()
        || set_m.get_flag("stdin")
        || set_m.get_one::<String>("env").is_some()
    {
        match resolve_token_input(set_m) {
            Ok(token) => Some(token),
            Err(error) => {
                return Err(SetFailure::new(
                    "input.invalid",
                    &format!("x Failed to read token input: {error}"),
                    effects,
                ));
            }
        }
    } else {
        None
    };
    let token_failure_message = if set_m.get_flag("clear_token") {
        Some(CLEAR_TOKEN_FAILURE)
    } else {
        token.as_ref().map(|_| STORE_TOKEN_FAILURE)
    };
    let _context_lock = crate::credentials::role_context::root()
        .and_then(|root| crate::credentials::role_context::lock_at(&root))
        .map_err(|_| {
            SetFailure::context_unavailable(
                "Profile context could not be locked safely; no profile changes were applied.",
                token_failure_message,
                effects.clone(),
            )
        })?;
    let preparation_config_before = fs::read(config_path()).ok();
    let managed_backup_path = config_path().with_extension("toml.bak");
    let preparation_backup_before = fs::symlink_metadata(&managed_backup_path).is_ok();
    let migration = crate::credentials::migration::run_legacy_credential_migration().map_err(|_| {
        effects["credential_migration"] = json!("unknown");
        SetFailure::new("credentials.prepare_failed", "x Could not prepare existing credentials. Check the selected home, config and credential-store access; profile setup is incomplete.", effects.clone())
    })?;
    if migration.changed()
        || preparation_config_before != fs::read(config_path()).ok()
        || preparation_backup_before != fs::symlink_metadata(&managed_backup_path).is_ok()
    {
        effects["credential_migration"] = json!("applied");
        effects["local"] = json!("applied");
    }

    let mut cfg = match load_config() {
        Some(cfg) => cfg,
        None => {
            return Err(SetFailure::new(
                "config.missing",
                "x No config file found.",
                effects,
            ));
        }
    };

    let Some(profile) = cfg.profile.iter_mut().find(|profile| profile.name == *name) else {
        return Err(SetFailure::new(
            "profile.not_found",
            &format!("x Profile '{}' not found.", name),
            effects,
        ));
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
            return Err(SetFailure::new(
                "input.invalid",
                &format!(
                    "x Invalid auth mode '{}'. Use none|api_key|openai_account.",
                    raw_mode
                ),
                effects,
            ));
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

    if let Some(temperature) = set_m.get_one::<f64>("temperature") {
        profile.temperature = Some(*temperature);
        metadata_changes.push("temperature");
    } else if set_m.get_flag("clear_temperature") {
        profile.temperature = None;
        metadata_changes.push("temperature");
    }
    if let Some(thinking) = thinking_from_args(set_m) {
        profile.thinking = Some(thinking);
        metadata_changes.push("thinking");
    } else if set_m.get_flag("clear_thinking") {
        profile.thinking = None;
        metadata_changes.push("thinking");
    }
    if let Some(max_output_tokens) = set_m.get_one::<u32>("max_output_tokens") {
        profile.max_output_tokens = Some(*max_output_tokens);
        metadata_changes.push("max_output_tokens");
    } else if set_m.get_flag("clear_max_output_tokens") {
        profile.max_output_tokens = None;
        metadata_changes.push("max_output_tokens");
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

    if !metadata_changes.is_empty() || token.is_some() || set_m.get_flag("clear_token") {
        crate::credentials::role_context::before_mutation(Some(name), false).map_err(|_| {
            SetFailure::context_unavailable(
                "Profile context could not be invalidated safely; no profile changes were applied.",
                token_failure_message,
                effects.clone(),
            )
        })?;
    }
    let mut token_change: Option<&str> = None;
    if set_m.get_flag("clear_token") {
        if store::clear_profile_token(name).is_err() {
            effects["credential"] = json!("unknown");
            effects["local"] = json!("unknown");
            return Err(SetFailure::new(
                "credentials.persist_failed",
                CLEAR_TOKEN_FAILURE,
                effects,
            ));
        }
        token_change = Some("cleared");
    } else if let Some(ref token) = token {
        if store::store_profile_token(name, token.as_str()).is_err() {
            effects["credential"] = json!("unknown");
            effects["local"] = json!("unknown");
            return Err(SetFailure::new(
                "credentials.persist_failed",
                STORE_TOKEN_FAILURE,
                effects,
            ));
        }
        token_change = Some("updated");
    }

    if token_change.is_some() {
        effects["credential"] = json!("applied");
        effects["local"] = json!("applied");
    }
    if !metadata_changes.is_empty() {
        if persist_config(&cfg).is_err() {
            effects["metadata"] = json!("unknown");
            effects["local"] = json!("unknown");
            return Err(SetFailure::new("persistence.failed", "x Failed to persist profile updates. A requested token change may already be saved; setup is incomplete. Check config write access before retrying.", effects));
        }
        effects["metadata"] = json!("applied");
        effects["local"] = json!("applied");
    }

    let auth_mode = cfg
        .profile
        .iter()
        .find(|profile| profile.name == *name)
        .map(|profile| profile.auth_mode)
        .unwrap_or(ProfileAuthMode::None);
    let expected_profile =
        serde_json::to_value(cfg.profile.iter().find(|p| p.name == *name).unwrap()).unwrap();
    Ok(SetOutcome {
        name: name.clone(),
        metadata_changes,
        token_change,
        auth_mode,
        effects,
        expected_profile,
        expected_default: cfg.default_profile,
        expected_token: token,
    })
}

fn run_remove(remove_m: &ArgMatches) -> bool {
    if let Some(name) = remove_m.get_one::<String>("name") {
        if !profile_exists(name) {
            eprintln!("x Profile '{}' not found.", name);
            return false;
        }

        let confirmed = match confirm(&format!(
            "Are you sure you want to remove profile '{name}'?"
        )) {
            Ok(confirmed) => confirmed,
            Err(error) => {
                eprintln!("x {error}");
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
            "x Please provide a profile name to remove. Example: cargo ai profile remove openai-prod"
        );
        false
    }
}

/// Executes profile list/show/add/set/remove operations.
pub fn run(sub_m: &ArgMatches) -> bool {
    if let Some(args) = sub_m.subcommand_matches("refresh-context") {
        match crate::credentials::role_context::refresh_profile_context(
            args.get_one::<String>("name").unwrap(),
        ) {
            Ok(reference) => {
                println!("{}", serde_json::to_string(&reference).unwrap());
                true
            }
            Err(_) => {
                eprintln!("Profile context refresh failed; inspect the selected configuration and credentials before retrying.");
                false
            }
        }
    } else if sub_m.subcommand_matches("list").is_some() {
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
            "x No profile subcommand found. Try 'cargo ai profile list', 'cargo ai profile show <name>', 'cargo ai profile add ...', or 'cargo ai profile set ...'."
        );
        false
    }
}

fn machine_config() -> Result<Option<crate::config::schema::Config>, super::machine::Failure> {
    use crate::config::loader::{load_config_strict, ConfigLoad};
    match load_config_strict().map_err(|_| {
        super::machine::Failure::new(
            "config.invalid",
            "The selected config could not be read safely.",
        )
    })? {
        ConfigLoad::Missing => Ok(None),
        ConfigLoad::Loaded(loaded) => toml::from_str(loaded.original_contents())
            .map(Some)
            .map_err(|_| {
                super::machine::Failure::new("config.invalid", "The selected config is invalid.")
            }),
    }
}
fn persisted_machine_config(
    effects: &Value,
) -> Result<crate::config::schema::Config, super::machine::Failure> {
    machine_config().ok().flatten().ok_or_else(|| {
        super::machine::Failure::new(
            "persistence.unverified",
            "The persisted profile state could not be verified.",
        )
        .with_data(json!({"effects":effects,"partial":true,"postconditions_verified":false}))
    })
}
fn profile_payload(profile: &Profile, default: Option<&str>, token_state: &str) -> Value {
    let redacted = profile.url.as_ref().is_some_and(|url| {
        reqwest::Url::parse(url).map_or(true, |url| {
            !url.username().is_empty()
                || url.password().is_some()
                || url.query().is_some()
                || url.fragment().is_some()
        })
    });
    let safe_url = profile.url.as_deref().filter(|_| !redacted);
    let origin = if redacted {
        profile
            .url
            .as_deref()
            .and_then(crate::providers::provider_url_origin)
    } else {
        None
    };
    json!({"name":profile.name,"server":profile.server,"model":profile.model,
        "auth_mode":profile.auth_mode.as_str(),"is_default":default == Some(profile.name.as_str()),
        "token_presence":token_state,"timeout_in_sec":profile.timeout_in_sec,
        "temperature":profile.temperature,"thinking":profile.thinking,"max_output_tokens":profile.max_output_tokens,
        "url_present":profile.url.is_some(),"url":safe_url,"url_redacted":redacted,"url_origin":origin,
        "description":profile.description})
}
fn profile_payload_with_token(profile: &Profile, default: Option<&str>) -> Value {
    let token = match store::load_profile_token(&profile.name) {
        Ok(Some(_)) => "present",
        Ok(None) if profile.token.is_some() => "present",
        Ok(None) => "absent",
        Err(_) if profile.token.is_some() => "present",
        Err(_) => "unknown",
    };
    profile_payload(profile, default, token)
}
fn machine_profile(name: &str) -> Result<Value, super::machine::Failure> {
    let cfg = machine_config()?.ok_or_else(|| {
        super::machine::Failure::new("config.missing", "No selected config exists.")
    })?;
    let profile = find_profile(&cfg, name).ok_or_else(|| {
        super::machine::Failure::new("profile.not_found", "The selected profile does not exist.")
    })?;
    Ok(profile_payload_with_token(
        profile,
        cfg.default_profile.as_deref(),
    ))
}

/// Typed profile operations retain the same metadata/credential operation seams.
pub(crate) fn machine_run(matches: &ArgMatches) -> Result<Value, super::machine::Failure> {
    use super::machine::Failure;
    let (command, args) = matches
        .subcommand()
        .ok_or_else(|| Failure::new("input.invalid", "A profile command is required."))?;
    if command == "list" {
        let cfg = machine_config()?
            .ok_or_else(|| Failure::new("config.missing", "No selected config exists."))?;
        let profiles: Vec<_> = cfg
            .profile
            .iter()
            .map(|profile| {
                profile_payload(profile, cfg.default_profile.as_deref(), "not_inspected")
            })
            .collect();
        return Ok(
            json!({"profiles":profiles,"default_profile":cfg.default_profile,"ordering":"configuration_order","complete":true,"next_cursor":null,
            "effects":{"local":"unapplied","remote":"unapplied"}}),
        );
    }
    let name = args
        .get_one::<String>("name")
        .ok_or_else(|| Failure::new("input.invalid", "A profile name is required."))?;
    if command == "refresh-context" {
        let reference = crate::credentials::role_context::refresh_profile_context(name).map_err(|_| Failure::new("profile.context_unavailable", "Profile context refresh could not be verified; inspect the selected configuration and credentials before retrying.").with_data(json!({"effects":{"local":"unknown","remote":"unapplied","context_metadata":"unknown"}})))?;
        return Ok(
            json!({"context":reference,"postconditions_verified":true,"effects":{"local":"applied","remote":"unapplied","context_metadata":"applied"}}),
        );
    }
    if command == "show" {
        return Ok(
            json!({"profile":machine_profile(name)?,"effects":{"local":"unapplied","remote":"unapplied"}}),
        );
    }
    // Consent and secret transport are checked before credential migration or persistence.
    if command == "remove" && !args.get_flag("yes") {
        return Err(Failure::new(
            "cli.interaction_required",
            "Profile removal requires explicit --yes consent.",
        )
        .with_data(json!({"effects":{"local":"unapplied","remote":"unapplied"}})));
    }
    if command == "set" && args.get_one::<String>("token").is_some() {
        return Err(Failure::new(
            "input.secret_source",
            "Machine mode requires a token through stdin or environment.",
        )
        .with_data(json!({"effects":{"local":"unapplied","remote":"unapplied"}})));
    }
    let cfg = machine_config()?;
    match command {
        "add" => {
            if cfg
                .as_ref()
                .is_some_and(|cfg| find_profile(cfg, name).is_some())
            {
                return Err(Failure::new("cli.interaction_required", "Replacing a profile requires interaction; use separate profile set operations.")
                    .with_data(json!({"effects":{"local":"unapplied","remote":"unapplied"}})));
            }
            let server = args
                .get_one::<String>("server")
                .ok_or_else(|| Failure::new("input.invalid", "A provider is required."))?;
            let model = args
                .get_one::<String>("model")
                .ok_or_else(|| Failure::new("input.invalid", "A model is required."))?;
            let auth = args
                .get_one::<String>("auth")
                .and_then(|v| parse_auth_mode(v))
                .unwrap_or(ProfileAuthMode::None);
            let profile = profile_from_add(args, name, server, model, auth);
            let expected = serde_json::to_value(&profile).unwrap();
            let managed_home = crate::config::paths::cargo_ai_root();
            let create_home = !managed_home.exists();
            if create_home {
                fs::create_dir_all(&managed_home).map_err(|_| Failure::new("persistence.failed", "The selected home could not be created.")
                    .with_data(json!({"effects":{"local":"unknown","remote":"unapplied","managed_home":"unknown","metadata":"unapplied"}})))?;
            }
            let home_effect = if create_home { "applied" } else { "unapplied" };
            crate::config::adder::add_profile_noninteractive(profile, args.get_flag("default"))
                .map_err(|_| Failure::new("persistence.failed", "Profile addition could not be persisted; inspect profiles before retrying.")
                    .with_data(json!({"effects":{"local":"unknown","remote":"unapplied","managed_home":home_effect,"metadata":"unknown"},"partial":create_home})))?;
            let effects = json!({"local":"applied","remote":"unapplied","metadata":"applied","credential":"unapplied","managed_home":home_effect});
            let persisted = persisted_machine_config(&effects)?;
            let verified = find_profile(&persisted, name)
                .is_some_and(|p| serde_json::to_value(p).ok().as_ref() == Some(&expected))
                && (!args.get_flag("default")
                    || persisted.default_profile.as_deref() == Some(name));
            let payload = find_profile(&persisted, name).map(|profile| {
                profile_payload_with_token(profile, persisted.default_profile.as_deref())
            });
            let data =
                json!({"profile":payload,"effects":effects,"postconditions_verified":verified});
            if !verified {
                return Err(Failure::new(
                    "persistence.unverified",
                    "The saved profile differs from the requested state.",
                )
                .with_data({
                    let mut data = data;
                    data["partial"] = json!(true);
                    data
                }));
            }
            Ok(data)
        }
        "set" => {
            let result = set_outcome(args, write_config).map_err(|error| Failure::new(error.code, "Profile update did not complete; inspect the profile and credential state before retrying.")
                .with_data(json!({"partial":error.effects.as_object().is_some_and(|effects| effects.values().any(|value| value == "applied")),"effects":error.effects})))?;
            let persisted = persisted_machine_config(&result.effects)?;
            if !find_profile(&persisted, name).is_some_and(|p| {
                serde_json::to_value(p).ok().as_ref() == Some(&result.expected_profile)
            }) || persisted.default_profile != result.expected_default
            {
                return Err(Failure::new(
                    "persistence.unverified",
                    "The updated profile differs from the requested state.",
                )
                .with_data(json!({"effects":result.effects,"partial":true,"postconditions_verified":false})));
            }
            let payload = profile_payload_with_token(
                find_profile(&persisted, name).unwrap(),
                persisted.default_profile.as_deref(),
            );
            if result.token_change.is_some() {
                let credential_matches = store::load_profile_token(name)
                    .is_ok_and(|saved| saved == result.expected_token);
                if !credential_matches {
                    return Err(Failure::new("persistence.unverified", "The profile credential state could not be verified.")
                        .with_data(json!({"effects":result.effects,"profile":payload,"partial":true,"postconditions_verified":false})));
                }
            }
            Ok(
                json!({"profile":payload,"changed_fields":result.metadata_changes,"token_change":result.token_change,
                "effects":result.effects,"postconditions_verified":true}),
            )
        }
        "remove" => {
            if !cfg
                .as_ref()
                .is_some_and(|cfg| find_profile(cfg, name).is_some())
            {
                return Err(Failure::new(
                    "profile.not_found",
                    "The selected profile does not exist.",
                ));
            }
            let cleanup = crate::config::remover::remove_profile_noninteractive(name).map_err(|_| Failure::new("persistence.failed", "Profile removal failed; inspect the profile before retrying.")
                .with_data(json!({"effects":{"local":"unknown","remote":"unapplied","metadata":"unknown","credential":"unapplied"}})))?;
            let effects = json!({"local":"applied","remote":"unapplied","metadata":"applied","credential":if cleanup {"applied"} else {"unknown"}});
            let persisted = persisted_machine_config(&effects)?;
            let removed = find_profile(&persisted, name).is_none();
            let data = json!({"removed":removed,"default_profile":persisted.default_profile,"effects":effects,"postconditions_verified":removed && cleanup});
            if !cleanup {
                return Err(Failure::new(
                    "operation.partial",
                    "The profile was removed but credential cleanup failed.",
                )
                .with_data({
                    let mut data = data;
                    data["partial"] = json!(true);
                    data
                }));
            }
            if !removed {
                return Err(Failure::new(
                    "persistence.unverified",
                    "Profile removal could not be verified.",
                )
                .with_data({
                    let mut data = data;
                    data["partial"] = json!(true);
                    data
                }));
            }
            Ok(data)
        }
        _ => Err(Failure::new(
            "contract.unsupported",
            "This profile command has no selected finite contract.",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        parse_auth_mode, profile_add_success_ui_response, profile_remove_success_ui_response,
        profile_set_success_ui_response,
    };
    use crate::config::schema::ProfileAuthMode;

    fn machine_args(words: &[&str]) -> clap::ArgMatches {
        {
            let all = std::iter::once("cargo-ai")
                .chain(words.iter().copied())
                .map(std::ffi::OsString::from)
                .collect();
            crate::args::parse_cli("cargo-ai", all)
                .unwrap()
                .subcommand_matches("profile")
                .unwrap()
                .clone()
        }
    }
    #[test]
    fn thinking_boolean_profile_round_trip_and_exact_choice_escape() {
        let _home = crate::commands::secret_input::test_support::Home::new();
        for (flag, value, expected) in [
            ("--thinking", "ON", serde_json::json!({"mode":"on"})),
            ("--thinking", "oFf", serde_json::json!({"mode":"off"})),
            (
                "--thinking-choice",
                "on",
                serde_json::json!({"mode":"choice","value":"on"}),
            ),
        ] {
            let changed =
                super::machine_run(&machine_args(&["profile", "set", "example", flag, value]))
                    .unwrap();
            assert_eq!(changed["profile"]["thinking"], expected);
            let shown = super::machine_run(&machine_args(&["profile", "show", "example"])).unwrap();
            assert_eq!(shown["profile"]["thinking"], expected);
            let listed = super::machine_run(&machine_args(&["profile", "list"])).unwrap();
            assert_eq!(listed["profiles"][0]["thinking"], expected);
            assert_eq!(changed["effects"]["remote"], "unapplied");
        }
    }

    #[test]
    fn thinking_profile_mutations_are_local_and_preserve_choice_on_connection_change() {
        let _home = crate::commands::secret_input::test_support::Home::new();
        let setting = serde_json::json!({"mode":"choice", "value":"max"});
        let result = super::machine_run(&machine_args(&[
            "profile",
            "set",
            "example",
            "--thinking",
            "max",
        ]))
        .unwrap();
        assert_eq!(result["profile"]["thinking"], setting);
        assert_eq!(result["effects"]["remote"], "unapplied");
        for (flag, value) in [
            ("--server", "ollama"),
            ("--model", "different"),
            ("--auth", "none"),
        ] {
            let result =
                super::machine_run(&machine_args(&["profile", "set", "example", flag, value]))
                    .unwrap();
            assert_eq!(result["profile"]["thinking"], setting);
        }
        let shown = super::machine_run(&machine_args(&["profile", "show", "example"])).unwrap();
        assert_eq!(shown["profile"]["thinking"], setting);
        let listed = super::machine_run(&machine_args(&["profile", "list"])).unwrap();
        assert_eq!(listed["profiles"][0]["thinking"], setting);
        let result = super::machine_run(&machine_args(&[
            "profile",
            "set",
            "example",
            "--thinking-provider-default",
        ]))
        .unwrap();
        assert_eq!(
            result["profile"]["thinking"],
            serde_json::json!({"mode":"provider_default"})
        );
        let result = super::machine_run(&machine_args(&[
            "profile",
            "set",
            "example",
            "--clear-thinking",
        ]))
        .unwrap();
        assert_eq!(result["profile"]["thinking"], serde_json::Value::Null);
        let result = super::machine_run(&machine_args(&[
            "profile", "add", "unset", "--server", "ollama", "--model", "manual",
        ]))
        .unwrap();
        assert_eq!(result["profile"]["thinking"], serde_json::Value::Null);
        assert_eq!(result["effects"]["remote"], "unapplied");
    }

    #[test]
    fn machine_contract_profile_consent_and_secret_transport_precede_effects() {
        let home = crate::commands::secret_input::test_support::Home::new();
        let before = std::fs::read(home.path.join("config.toml")).unwrap();
        for words in [
            vec!["profile", "remove", "example"],
            vec![
                "profile",
                "set",
                "example",
                "--token",
                "synthetic-private-secret",
            ],
        ] {
            let error = super::machine_run(&machine_args(&words)).unwrap_err();
            assert!(matches!(
                error.code,
                "cli.interaction_required" | "input.secret_source"
            ));
            assert_eq!(error.data["effects"]["local"], "unapplied");
            assert!(!format!("{error:?}").contains("synthetic-private-secret"));
            assert_eq!(
                std::fs::read(home.path.join("config.toml")).unwrap(),
                before
            );
        }
    }
    #[test]
    fn machine_contract_profile_read_update_add_remove_use_persisted_state() {
        let home = crate::commands::secret_input::test_support::Home::new();
        let mut config = std::fs::read_to_string(home.path.join("config.toml")).unwrap();
        config = config.replace("auth_mode = 'api_key'", "auth_mode = 'api_key'\nurl = 'https://private-user:private-pass@example.test/?api_key=private-key'");
        std::fs::write(home.path.join("config.toml"), config).unwrap();
        let shown = super::machine_run(&machine_args(&["profile", "show", "example"])).unwrap();
        assert_eq!(shown["profile"]["url_present"], true);
        assert!(!shown.to_string().contains("private-pass"));
        assert_eq!(shown["profile"]["url"], serde_json::Value::Null);
        assert_eq!(shown["profile"]["url_redacted"], true);
        assert_eq!(shown["profile"]["url_origin"], "https://example.test");
        let result = super::machine_run(&machine_args(&[
            "profile",
            "set",
            "example",
            "--model",
            "updated-model",
        ]))
        .unwrap();
        assert_eq!(result["profile"]["model"], "updated-model");
        assert_eq!(result["effects"]["metadata"], "applied");
        assert_eq!(result["postconditions_verified"], true);
        let result = super::machine_run(&machine_args(&[
            "profile", "add", "second", "--server", "ollama", "--model", "fixture",
        ]))
        .unwrap();
        assert_eq!(result["profile"]["name"], "second");
        let conflict = super::machine_run(&machine_args(&[
            "profile", "add", "second", "--server", "openai", "--model", "other",
        ]))
        .unwrap_err();
        assert_eq!(conflict.code, "cli.interaction_required");
        let result =
            super::machine_run(&machine_args(&["profile", "remove", "second", "--yes"])).unwrap();
        assert_eq!(result["removed"], true);
        let list = super::machine_run(&machine_args(&["profile", "list"])).unwrap();
        assert_eq!(list["profiles"].as_array().unwrap().len(), 1);
        assert_eq!(list["complete"], true);
    }
    #[test]
    fn machine_contract_profile_first_add_creates_only_selected_home() {
        let home = crate::commands::secret_input::test_support::Home::new();
        std::fs::remove_dir_all(&home.path).unwrap();
        let result = super::machine_run(&machine_args(&[
            "profile",
            "add",
            "first",
            "--server",
            "ollama",
            "--model",
            "fixture",
            "--url",
            "http://localhost:11434/path",
        ]))
        .unwrap();
        assert_eq!(result["effects"]["managed_home"], "applied");
        assert_eq!(result["profile"]["is_default"], true);
        assert_eq!(result["profile"]["url"], "http://localhost:11434/path");
        assert_eq!(result["profile"]["url_redacted"], false);
        assert!(home.path.join("config.toml").is_file());
    }
    #[test]
    fn machine_contract_profile_remove_exposes_partial_credential_cleanup() {
        let _home = crate::commands::secret_input::test_support::Home::new();
        let path = crate::credentials::store::credentials_path();
        std::fs::write(&path, "malformed private-secret credential document").unwrap();
        let error = super::machine_run(&machine_args(&["profile", "remove", "example", "--yes"]))
            .unwrap_err();
        assert_eq!(error.code, "operation.partial");
        assert_eq!(error.data["removed"], true);
        assert_eq!(error.data["effects"]["metadata"], "applied");
        assert_eq!(error.data["effects"]["credential"], "unknown");
        assert!(!format!("{error:?}").contains("private-secret"));
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "malformed private-secret credential document"
        );
    }
    #[test]
    fn machine_contract_profile_environment_token_verifies_exact_value_then_clear() {
        let _home = crate::commands::secret_input::test_support::Home::new();
        let variable = "CARGO_AI_MACHINE_CONTRACT_FIXTURE_TOKEN";
        let previous = std::env::var_os(variable);
        std::env::set_var(variable, "machine-private-token");
        let result = super::machine_run(&machine_args(&[
            "profile", "set", "example", "--env", variable,
        ]));
        match previous {
            Some(value) => std::env::set_var(variable, value),
            None => std::env::remove_var(variable),
        }
        let result = result.unwrap();
        assert_eq!(result["profile"]["token_presence"], "present");
        assert_eq!(result["postconditions_verified"], true);
        assert!(!result.to_string().contains("machine-private-token"));
        assert_eq!(
            crate::credentials::store::load_profile_token("example")
                .unwrap()
                .as_deref(),
            Some("machine-private-token")
        );
        let result = super::machine_run(&machine_args(&[
            "profile",
            "set",
            "example",
            "--clear-token",
        ]))
        .unwrap();
        assert_eq!(result["profile"]["token_presence"], "absent");
        assert_eq!(result["postconditions_verified"], true);
        assert!(crate::credentials::store::load_profile_token("example")
            .unwrap()
            .is_none());
    }
    #[test]
    fn machine_contract_profile_core_failure_retains_exact_boundary_effects() {
        let _home = crate::commands::secret_input::test_support::Home::new();
        let args = machine_args(&["profile", "set", "example", "--default"]);
        let error = super::set_outcome(args.subcommand_matches("set").unwrap(), |_| {
            Err("synthetic-private-secret".into())
        })
        .unwrap_err();
        assert_eq!(error.code, "persistence.failed");
        assert_eq!(error.effects["metadata"], "unknown");
        assert_eq!(error.effects["credential"], "unapplied");
        assert!(!error.legacy_message.contains("synthetic-private-secret"));
    }

    #[test]
    fn context_failure_preserves_token_operation_diagnostics_and_unapplied_effects() {
        let home = crate::commands::secret_input::test_support::Home::new();
        let path = crate::credentials::store::credentials_path();
        let malformed = "malformed private-secret credential document";
        std::fs::write(&path, malformed).unwrap();
        let config = std::fs::read(home.path.join("config.toml")).unwrap();
        for (operation, message) in [
            (
                vec!["--token", "synthetic-new-token"],
                super::STORE_TOKEN_FAILURE,
            ),
            (vec!["--clear-token"], super::CLEAR_TOKEN_FAILURE),
            (
                vec!["--default"],
                "Profile context could not be invalidated safely; no profile changes were applied.",
            ),
        ] {
            let mut words = vec!["profile", "set", "example"];
            words.extend(operation);
            let args = machine_args(&words);
            let error = super::set_outcome(args.subcommand_matches("set").unwrap(), |_| {
                panic!("context failure must precede metadata persistence")
            })
            .unwrap_err();
            assert_eq!(error.code, "profile.context_unavailable");
            assert!(error.legacy_message.starts_with(message));
            assert!(error
                .legacy_message
                .contains("Profile context could not be invalidated safely"));
            assert!(error
                .effects
                .as_object()
                .unwrap()
                .values()
                .all(|value| value == "unapplied"));
            assert!(!format!("{error:?}").contains("private-secret"));
            assert!(!format!("{error:?}").contains("synthetic-new-token"));
            assert_eq!(std::fs::read(&path).unwrap(), malformed.as_bytes());
            assert_eq!(
                std::fs::read(home.path.join("config.toml")).unwrap(),
                config
            );
        }
    }

    #[test]
    fn secret_input_profile_metadata_failure_preserves_credentials_and_fails() {
        let home = crate::commands::secret_input::test_support::Home::new();
        let token = "synthetic-profile-secret";
        crate::credentials::store::store_profile_token("example", token).unwrap();
        let before = std::fs::read(home.path.join("config.toml")).unwrap();
        let args = crate::args::parse_cli(
            "cargo-ai",
            ["cargo-ai", "profile", "set", "example", "--default"]
                .into_iter()
                .map(Into::into)
                .collect(),
        )
        .unwrap();
        let matches = args
            .subcommand_matches("profile")
            .unwrap()
            .subcommand_matches("set")
            .unwrap();
        assert!(!super::run_set_with_writer(matches, |_| Err(
            token.to_owned()
        )));
        assert_eq!(
            std::fs::read(home.path.join("config.toml")).unwrap(),
            before
        );
        assert!(
            crate::credentials::store::load_profile_token("example")
                .unwrap()
                .as_deref()
                == Some(token)
        );
    }

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

    #[test]
    fn machine_context_refresh_and_partial_profile_mutation_require_new_review() {
        let _home = crate::commands::secret_input::test_support::Home::new();
        crate::credentials::store::store_profile_token("example", "synthetic-context-original")
            .unwrap();
        let first =
            super::machine_run(&machine_args(&["profile", "refresh-context", "example"])).unwrap();
        assert_eq!(first["postconditions_verified"], true);
        let a: crate::credentials::role_context::ProfileContextRef =
            serde_json::from_value(first["context"].clone()).unwrap();
        assert!(crate::credentials::role_context::revalidate_profile_context(&a).is_ok());
        let failed = super::set_outcome(
            &machine_args(&[
                "profile",
                "set",
                "example",
                "--model",
                "new-model",
                "--token",
                "synthetic-context-new",
            ])
            .subcommand_matches("set")
            .unwrap(),
            |_| Err("injected config persistence failure".to_owned()),
        );
        assert!(failed.is_err());
        assert_eq!(
            crate::credentials::store::load_profile_token("example")
                .unwrap()
                .as_deref(),
            Some("synthetic-context-new")
        );
        assert!(crate::credentials::role_context::revalidate_profile_context(&a).is_err());
        let second =
            super::machine_run(&machine_args(&["profile", "refresh-context", "example"])).unwrap();
        let b: crate::credentials::role_context::ProfileContextRef =
            serde_json::from_value(second["context"].clone()).unwrap();
        assert_eq!(a.profile_uuid, b.profile_uuid);
        assert_ne!(a.connection_generation, b.connection_generation);
        assert!(crate::credentials::role_context::revalidate_profile_context(&b).is_ok());
        assert!(!second.to_string().contains("synthetic-context"));
        super::machine_run(&machine_args(&["profile", "remove", "example", "--yes"])).unwrap();
        super::machine_run(&machine_args(&[
            "profile", "add", "example", "--server", "ollama", "--model", "fixture",
        ]))
        .unwrap();
        let third =
            super::machine_run(&machine_args(&["profile", "refresh-context", "example"])).unwrap();
        assert_ne!(third["context"]["profile_uuid"], b.profile_uuid);
        assert!(crate::credentials::role_context::revalidate_profile_context(&b).is_err());
    }
}
