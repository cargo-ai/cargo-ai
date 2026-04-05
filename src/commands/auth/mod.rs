//! Runtime behavior for `cargo ai auth`.
use clap::ArgMatches;
use serde::Serialize;
use serde_json::{json, Value};
use std::io::{self, Write};
use std::process::Command;

use crate::config::loader::load_config;
use crate::config::schema::{
    default_profile_auth_mode, default_secret_store_mode, ProfileAuthMode,
};
use crate::config::settings as config_settings;
use crate::credentials::{openai_oauth, store};
use crate::ui;

#[derive(Debug, Serialize)]
struct AuthStatusJson {
    provider: &'static str,
    session_state: String,
    auth_mode_effective: String,
    has_refresh_token: bool,
    access_token_expires_at_unix: Option<i64>,
    secret_store_mode: String,
}

fn now_unix_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
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

fn validate_profile_target(profile_name: &str) -> Result<(), String> {
    let cfg = load_config().ok_or_else(|| {
        "No config file found. Create a profile first with `cargo ai profile add`.".to_string()
    })?;

    let profile = cfg
        .profile
        .iter()
        .find(|candidate| candidate.name == profile_name)
        .ok_or_else(|| format!("Profile '{profile_name}' not found."))?;

    if !profile.server.eq_ignore_ascii_case("openai") {
        return Err(format!(
            "Profile '{profile_name}' uses server '{}'. OpenAI login can only target profiles with `--server openai`.",
            profile.server
        ));
    }

    Ok(())
}

fn effective_auth_mode_for_status() -> String {
    let Some(cfg) = load_config() else {
        return default_profile_auth_mode().as_str().to_string();
    };

    if let Some(default_profile) = cfg.default_profile.as_deref() {
        if let Some(profile) = cfg.profile.iter().find(|profile| {
            profile.name == default_profile && profile.server.eq_ignore_ascii_case("openai")
        }) {
            return profile.auth_mode.as_str().to_string();
        }
    }

    let mut seen_modes: Vec<&'static str> = Vec::new();
    for profile in cfg
        .profile
        .iter()
        .filter(|profile| profile.server.eq_ignore_ascii_case("openai"))
    {
        let mode = profile.auth_mode.as_str();
        if !seen_modes.contains(&mode) {
            seen_modes.push(mode);
        }
    }

    match seen_modes.len() {
        0 => default_profile_auth_mode().as_str().to_string(),
        1 => seen_modes[0].to_string(),
        _ => "mixed".to_string(),
    }
}

fn local_session_state() -> Result<AuthStatusJson, String> {
    let session = openai_oauth::load_codex_session()?;
    let now = now_unix_seconds();
    let locally_disabled = openai_oauth::openai_account_locally_disabled();

    let access_token_expires_at_unix = if locally_disabled {
        None
    } else {
        session
            .as_ref()
            .and_then(|session| session.access_token_expires_at_unix)
    };

    let session_state = match session.as_ref() {
        _ if locally_disabled => "logged_out_local".to_string(),
        None => "logged_out".to_string(),
        Some(session)
            if openai_oauth::access_token_expired_or_near(
                session.access_token_expires_at_unix,
                now,
            ) =>
        {
            "expiring".to_string()
        }
        Some(session) if session.access_token_expires_at_unix.is_some() => "active".to_string(),
        Some(_) => "active_unknown_expiry".to_string(),
    };

    let has_refresh_token = if locally_disabled {
        false
    } else {
        session
            .as_ref()
            .and_then(|session| session.refresh_token.as_ref())
            .is_some()
    };

    let configured_store_mode = store::configured_secret_store_mode()
        .unwrap_or(default_secret_store_mode())
        .as_str()
        .to_string();

    Ok(AuthStatusJson {
        provider: "openai",
        session_state,
        auth_mode_effective: effective_auth_mode_for_status(),
        has_refresh_token,
        access_token_expires_at_unix,
        secret_store_mode: configured_store_mode,
    })
}

fn auth_status_summary(status: &AuthStatusJson) -> (&'static str, &'static str, String) {
    match status.session_state.as_str() {
        "active" => ("success", "✓", "OpenAI session active.".to_string()),
        "active_unknown_expiry" => (
            "success",
            "✓",
            "OpenAI session active, but expiry is unknown.".to_string(),
        ),
        "expiring" => (
            "info",
            "!",
            "OpenAI session is expiring or needs attention soon.".to_string(),
        ),
        "logged_out_local" => (
            "info",
            "!",
            "Cargo AI is logged out locally; Codex may still be signed in.".to_string(),
        ),
        "logged_out" => ("info", "!", "OpenAI session is logged out.".to_string()),
        other => ("info", "!", format!("OpenAI session state is '{other}'.")),
    }
}

fn auth_status_ui_response(status: &AuthStatusJson) -> Value {
    let (kind, icon, summary) = auth_status_summary(status);
    let mut sections = vec![
        json!({
            "type": "kv",
            "title": "Context",
            "title_style": "plain",
            "layout": "aligned",
            "items": [
                {"label": "Provider", "value": status.provider},
                {"label": "Session", "value": status.session_state},
                {"label": "Auth mode", "value": status.auth_mode_effective}
            ]
        }),
        json!({
            "type": "kv",
            "title": "Session",
            "title_style": "plain",
            "layout": "aligned",
            "items": [
                {
                    "label": "Refresh token",
                    "value": if status.has_refresh_token { "Yes" } else { "No" }
                },
                {
                    "label": "Access token expiry (unix)",
                    "value": status
                        .access_token_expires_at_unix
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "(unknown)".to_string())
                }
            ]
        }),
        json!({
            "type": "kv",
            "title": "Storage",
            "title_style": "plain",
            "layout": "aligned",
            "items": [
                {"label": "Secret store", "value": status.secret_store_mode}
            ]
        }),
    ];

    if matches!(
        status.session_state.as_str(),
        "logged_out" | "logged_out_local" | "expiring"
    ) {
        sections.push(json!({
            "type": "kv",
            "title": "Next steps",
            "title_style": "plain",
            "layout": "aligned",
            "items": [
                {"label": "Login", "value": "cargo ai auth login openai"}
            ]
        }));
    }

    json!({
        "ui": {
            "schema": "1.0",
            "kind": kind,
            "icon": icon,
            "title": "Auth status",
            "summary": summary,
            "sections": sections
        }
    })
}

fn run_codex_login() -> Result<(), String> {
    let status = Command::new("codex")
        .arg("login")
        .status()
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                "`codex` CLI was not found in PATH. Install Codex and run `codex login`, then retry `cargo ai auth login openai`.".to_string()
            } else {
                format!("failed to run `codex login`: {error}")
            }
        })?;

    if status.success() {
        Ok(())
    } else {
        Err(format!("`codex login` exited with status {status}"))
    }
}

fn run_codex_logout() -> Result<(), String> {
    let status = Command::new("codex")
        .arg("logout")
        .status()
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                "`codex` CLI was not found in PATH. Install Codex to use `cargo ai auth logout`."
                    .to_string()
            } else {
                format!("failed to run `codex logout`: {error}")
            }
        })?;

    if status.success() {
        Ok(())
    } else {
        Err(format!("`codex logout` exited with status {status}"))
    }
}

async fn run_login_openai(login_openai_m: &ArgMatches) -> bool {
    let profile_name = login_openai_m
        .get_one::<String>("profile")
        .map(String::as_str);
    let set_default = login_openai_m.get_flag("set_default");

    if let Some(profile_name) = profile_name {
        if let Err(error) = validate_profile_target(profile_name) {
            eprintln!("❌ {error}");
            return false;
        }
    }

    println!("Starting OpenAI browser login via Codex...");
    if let Err(error) = run_codex_login() {
        eprintln!("❌ {error}");
        return false;
    }

    match openai_oauth::load_codex_session() {
        Ok(Some(_)) => {}
        Ok(None) => {
            eprintln!(
                "❌ Codex login completed, but no local auth session was found. Verify Codex is configured and run `codex login` again."
            );
            return false;
        }
        Err(error) => {
            eprintln!("❌ Failed to read Codex auth session: {error}");
            return false;
        }
    }

    // Clean up any duplicated OpenAI session material from prior Cargo AI builds.
    openai_oauth::clear_legacy_openai_session_tokens();
    if let Err(error) = config_settings::set_openai_auth_locally_disabled(false) {
        eprintln!("❌ Login succeeded, but failed to clear local logout state: {error}");
        return false;
    }

    if let Some(profile_name) = profile_name {
        if let Err(error) =
            config_settings::set_profile_auth_mode(profile_name, ProfileAuthMode::OpenaiAccount)
        {
            eprintln!("❌ Login succeeded, but failed to update profile auth mode: {error}");
            return false;
        }

        if set_default {
            if let Err(error) = config_settings::set_default_profile(profile_name) {
                eprintln!("❌ Login succeeded, but failed to set default profile: {error}");
                return false;
            }
        }
    }

    println!("✅ OpenAI login complete (Codex session detected).");
    if let Some(profile_name) = profile_name {
        println!(
            "Profile '{profile_name}' auth mode set to '{}'.",
            ProfileAuthMode::OpenaiAccount.as_str()
        );
        if set_default {
            println!("Profile '{profile_name}' set as default.");
        }
    }

    true
}

async fn run_status(status_m: &ArgMatches) -> bool {
    let status = match local_session_state() {
        Ok(status) => status,
        Err(error) => {
            eprintln!("❌ {error}");
            return false;
        }
    };

    if status_m.get_flag("json") {
        match serde_json::to_string_pretty(&status) {
            Ok(serialized) => println!("{serialized}"),
            Err(error) => {
                eprintln!("❌ Failed to serialize auth status JSON: {error}");
                return false;
            }
        }
    } else {
        ui::account_status::render_backend_ui(&auth_status_ui_response(&status));
    }

    true
}

#[cfg(test)]
mod tests {
    use super::{auth_status_summary, auth_status_ui_response, AuthStatusJson};

    #[test]
    fn auth_status_logged_out_local_has_login_next_step() {
        let response = auth_status_ui_response(&AuthStatusJson {
            provider: "openai",
            session_state: "logged_out_local".to_string(),
            auth_mode_effective: "openai_account".to_string(),
            has_refresh_token: false,
            access_token_expires_at_unix: None,
            secret_store_mode: "keychain".to_string(),
        });

        assert_eq!(response["ui"]["title"].as_str(), Some("Auth status"));
        assert_eq!(response["ui"]["icon"].as_str(), Some("!"));
        assert_eq!(
            response["ui"]["sections"][3]["items"][0]["value"].as_str(),
            Some("cargo ai auth login openai")
        );
    }

    #[test]
    fn auth_status_active_uses_success_summary() {
        let (_, icon, summary) = auth_status_summary(&AuthStatusJson {
            provider: "openai",
            session_state: "active".to_string(),
            auth_mode_effective: "api_key".to_string(),
            has_refresh_token: true,
            access_token_expires_at_unix: Some(123),
            secret_store_mode: "file".to_string(),
        });

        assert_eq!(icon, "✓");
        assert_eq!(summary, "OpenAI session active.");
    }
}

async fn run_logout(logout_m: &ArgMatches) -> bool {
    let global = logout_m.get_flag("global") || logout_m.get_flag("revoke");
    let yes = logout_m.get_flag("yes");

    if !yes {
        let prompt = if global {
            "Log out of OpenAI for Cargo AI and also log out Codex globally?"
        } else {
            "Log out of OpenAI for Cargo AI only? (Codex stays signed in)"
        };
        let confirmed = match confirm(prompt) {
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

    if global {
        if let Err(error) = run_codex_logout() {
            eprintln!("❌ {error}");
            return false;
        }
    }

    if let Err(error) = openai_oauth::clear_local_session() {
        eprintln!("❌ Failed to clear local OpenAI metadata: {error}");
        return false;
    }
    if let Err(error) = config_settings::set_openai_auth_locally_disabled(true) {
        eprintln!("❌ Failed to persist local Cargo AI logout state: {error}");
        return false;
    }

    if global {
        println!("✅ OpenAI logged out globally via Codex and logged out for Cargo AI.");
    } else {
        println!("✅ OpenAI logged out for Cargo AI only. Codex session remains signed in.");
    }
    true
}

/// Routes `cargo ai auth ...` subcommands to runtime handlers.
pub async fn run(sub_m: &ArgMatches) -> bool {
    if let Some(login_m) = sub_m.subcommand_matches("login") {
        if let Some(login_openai_m) = login_m.subcommand_matches("openai") {
            run_login_openai(login_openai_m).await
        } else {
            eprintln!("No auth login provider found. Try 'cargo ai auth login openai'.");
            false
        }
    } else if let Some(status_m) = sub_m.subcommand_matches("status") {
        run_status(status_m).await
    } else if let Some(logout_m) = sub_m.subcommand_matches("logout") {
        run_logout(logout_m).await
    } else {
        eprintln!(
            "No auth subcommand found. Try 'cargo ai auth login openai', 'cargo ai auth status [--json]', or 'cargo ai auth logout [--global] [--yes]'."
        );
        false
    }
}
