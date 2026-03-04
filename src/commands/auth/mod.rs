//! Runtime behavior for `cargo ai auth`.
use clap::ArgMatches;
use serde::Serialize;
use serde_json::Value;
use std::io::{self, Write};
use std::process::Command;
use std::time::Duration;

use crate::config::loader::load_config;
use crate::config::schema::{
    default_profile_auth_mode, default_secret_store_mode, ProfileAuthMode,
};
use crate::config::settings as config_settings;
use crate::credentials::{openai_oauth, store};
use crate::infra_api;

const DEFAULT_POLL_INTERVAL_SECONDS: u64 = 2;
const MAX_LOGIN_POLL_ATTEMPTS: usize = 180;

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum PollLoginState {
    Pending,
    Succeeded,
    Failed,
    Unknown,
}

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

fn parse_poll_login_state(status: &str) -> PollLoginState {
    let normalized = status.trim().to_ascii_lowercase();
    if normalized.contains("pending") || normalized.contains("wait") {
        PollLoginState::Pending
    } else if normalized.contains("success")
        || normalized.contains("succeed")
        || normalized.contains("complete")
        || normalized.contains("authorized")
    {
        PollLoginState::Succeeded
    } else if normalized.contains("error")
        || normalized.contains("fail")
        || normalized.contains("deny")
        || normalized.contains("cancel")
        || normalized.contains("expire")
    {
        PollLoginState::Failed
    } else {
        PollLoginState::Unknown
    }
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

fn open_browser(url: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = Command::new("open");
        command.arg(url);
        command
    };

    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = Command::new("cmd");
        command.args(["/C", "start", "", url]);
        command
    };

    #[cfg(all(unix, not(target_os = "macos")))]
    let mut command = {
        let mut command = Command::new("xdg-open");
        command.arg(url);
        command
    };

    let status = command
        .status()
        .map_err(|error| format!("failed to launch browser command: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "browser launch command exited with status {status}"
        ))
    }
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
    let tokens = store::load_openai_oauth_tokens().map_err(|error| {
        format!("failed to load OpenAI OAuth session from secret store: {error}")
    })?;

    let metadata = openai_oauth::load_metadata();
    let now = now_unix_seconds();
    let access_token_expires_at_unix = openai_oauth::expires_at_unix(metadata);

    let session_state = if tokens.is_none() {
        "logged_out".to_string()
    } else if openai_oauth::token_expired_or_near(metadata, now) {
        "expiring".to_string()
    } else if access_token_expires_at_unix.is_some() {
        "active".to_string()
    } else {
        "active_unknown_expiry".to_string()
    };

    let has_refresh_token = tokens
        .as_ref()
        .and_then(|tokens| tokens.refresh_token.as_ref())
        .is_some();
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

    println!("Starting OpenAI browser login...");
    let start_response =
        match infra_api::auth::openai::start_login(openai_oauth::OPENAI_INFRA_BASE_URL).await {
            Ok(response) => response,
            Err(error) => {
                eprintln!("❌ Failed to initialize OpenAI login: {error}");
                return false;
            }
        };

    println!("OpenAI login URL:");
    println!("{}", start_response.login_url);
    match open_browser(start_response.login_url.as_str()) {
        Ok(()) => println!("Opened your browser. Complete login there to continue."),
        Err(error) => {
            eprintln!("⚠️ Could not open a browser automatically: {error}");
            eprintln!("Use the URL above to complete login manually.");
        }
    }

    let poll_interval = start_response
        .poll_interval_seconds
        .unwrap_or(DEFAULT_POLL_INTERVAL_SECONDS)
        .max(1);

    for _attempt in 0..MAX_LOGIN_POLL_ATTEMPTS {
        tokio::time::sleep(Duration::from_secs(poll_interval)).await;

        let poll_response = match infra_api::auth::openai::poll_login(
            openai_oauth::OPENAI_INFRA_BASE_URL,
            start_response.login_id.as_str(),
        )
        .await
        {
            Ok(response) => response,
            Err(error) => {
                eprintln!("❌ Failed while polling OpenAI login status: {error}");
                return false;
            }
        };

        if let Some(credentials) = poll_response.credentials {
            let access_token = credentials.access_token.trim().to_string();
            if access_token.is_empty() {
                eprintln!("❌ OpenAI login succeeded but returned an empty access token.");
                return false;
            }

            let refresh_token = credentials
                .refresh_token
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string);

            if let Err(error) =
                store::store_openai_oauth_tokens(access_token.as_str(), refresh_token.as_deref())
            {
                eprintln!("❌ Failed to persist OpenAI session credentials: {error}");
                return false;
            }

            let issued_at = if credentials.expires_in.is_some() {
                Some(now_unix_seconds())
            } else {
                None
            };
            if let Err(error) =
                config_settings::set_openai_auth_metadata(credentials.expires_in, issued_at)
            {
                eprintln!("❌ Failed to persist OpenAI session metadata: {error}");
                return false;
            }

            if let Some(profile_name) = profile_name {
                if let Err(error) = config_settings::set_profile_auth_mode(
                    profile_name,
                    ProfileAuthMode::OpenaiAccount,
                ) {
                    eprintln!(
                        "❌ Login succeeded, but failed to update profile auth mode: {error}"
                    );
                    return false;
                }

                if set_default {
                    if let Err(error) = config_settings::set_default_profile(profile_name) {
                        eprintln!("❌ Login succeeded, but failed to set default profile: {error}");
                        return false;
                    }
                }
            }

            println!("✅ OpenAI login complete.");
            if let Some(profile_name) = profile_name {
                println!(
                    "Profile '{profile_name}' auth mode set to '{}'.",
                    ProfileAuthMode::OpenaiAccount.as_str()
                );
                if set_default {
                    println!("Profile '{profile_name}' set as default.");
                }
            }
            return true;
        }

        if let Some(status) = poll_response.status.as_deref() {
            match parse_poll_login_state(status) {
                PollLoginState::Pending => continue,
                PollLoginState::Succeeded => {
                    eprintln!("❌ OpenAI login reported success but returned no credentials.");
                    return false;
                }
                PollLoginState::Failed => {
                    let message = poll_response
                        .message
                        .unwrap_or_else(|| "OpenAI login failed.".to_string());
                    eprintln!("❌ {message}");
                    return false;
                }
                PollLoginState::Unknown => {
                    let _ = status;
                    continue;
                }
            }
        }
    }

    eprintln!("❌ Timed out waiting for OpenAI login to complete.");
    false
}

fn render_status_text(status: &AuthStatusJson) {
    println!("Provider: {}", status.provider);
    println!("Session state: {}", status.session_state);
    println!("Effective auth mode: {}", status.auth_mode_effective);
    println!(
        "Refresh token present: {}",
        if status.has_refresh_token {
            "yes"
        } else {
            "no"
        }
    );
    println!(
        "Access token expires at (unix): {}",
        status
            .access_token_expires_at_unix
            .map(|value| value.to_string())
            .unwrap_or_else(|| "(unknown)".to_string())
    );
    println!("Secret-store mode: {}", status.secret_store_mode);
}

fn value_indicates_success(payload: &Value) -> bool {
    payload
        .get("status")
        .and_then(Value::as_str)
        .map(|status| status.eq_ignore_ascii_case("success"))
        .unwrap_or(false)
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
        render_status_text(&status);
    }

    true
}

async fn run_logout(logout_m: &ArgMatches) -> bool {
    let revoke = logout_m.get_flag("revoke");
    let yes = logout_m.get_flag("yes");

    if !yes {
        let prompt = if revoke {
            "Log out of OpenAI and request remote revoke?"
        } else {
            "Log out of OpenAI locally?"
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

    let local_tokens = match store::load_openai_oauth_tokens() {
        Ok(tokens) => tokens,
        Err(error) => {
            eprintln!("❌ Failed to load OpenAI session from secret store: {error}");
            return false;
        }
    };

    let mut revoke_warning: Option<String> = None;
    if revoke {
        if let Some(tokens) = local_tokens.as_ref() {
            match infra_api::auth::openai::logout(
                openai_oauth::OPENAI_INFRA_BASE_URL,
                tokens.access_token.as_str(),
                tokens.refresh_token.as_deref(),
                true,
            )
            .await
            {
                Ok(response) => {
                    if !value_indicates_success(&response) {
                        let detail = response
                            .get("message")
                            .and_then(Value::as_str)
                            .unwrap_or("remote revoke returned a non-success status");
                        revoke_warning = Some(format!(
                            "Remote revoke did not complete successfully: {detail}"
                        ));
                    }
                }
                Err(error) => {
                    revoke_warning = Some(format!("Remote revoke failed: {error}"));
                }
            }
        } else {
            revoke_warning = Some(
                "No local OpenAI session was found, so remote revoke was not attempted."
                    .to_string(),
            );
        }
    }

    if let Err(error) = openai_oauth::clear_local_session() {
        eprintln!("❌ Failed to clear local OpenAI session: {error}");
        return false;
    }

    println!("✅ Local OpenAI session cleared.");
    if let Some(warning) = revoke_warning {
        eprintln!("⚠️ {warning}");
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
            "No auth subcommand found. Try 'cargo ai auth login openai', 'cargo ai auth status [--json]', or 'cargo ai auth logout [--revoke] [--yes]'."
        );
        false
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_poll_login_state, PollLoginState};

    #[test]
    fn parse_poll_login_state_maps_known_variants() {
        assert_eq!(parse_poll_login_state("pending"), PollLoginState::Pending);
        assert_eq!(parse_poll_login_state("success"), PollLoginState::Succeeded);
        assert_eq!(parse_poll_login_state("failed"), PollLoginState::Failed);
        assert_eq!(parse_poll_login_state("wat"), PollLoginState::Unknown);
    }
}
