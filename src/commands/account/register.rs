//! Runtime behavior for `cargo ai account register`.
use clap::ArgMatches;

use crate::config::adder::set_account_email;
use crate::config::loader::load_config;
use crate::config::setup::{config_path, ensure_config_file_exists};
use crate::infra_api;
use crate::ui;

use std::io::{self, Write};

use super::helpers::{
    extract_status_account_email, fetch_status_for_register_guard, load_account_auth,
    INFRA_BASE_URL,
};

/// Registers an account email and persists the active email on success.
pub async fn run(reg_m: &ArgMatches) -> bool {
    let Some(email) = reg_m.get_one::<String>("email") else {
        eprintln!("❌ Missing email. Use `cargo ai account register <email>`.");
        return false;
    };

    if let Err(e) = ensure_config_file_exists() {
        eprintln!(
            "❌ Failed to initialize local config at '{}': {e}",
            config_path().display()
        );
        return false;
    }

    // Guard: skip register when local session is already valid for the requested email.
    if let Some(cfg) = load_config() {
        if let Some(acct) = cfg.account.as_ref() {
            if acct.email.as_deref().is_some() {
                if let Ok(auth) = load_account_auth() {
                    let status_response = fetch_status_for_register_guard(
                        auth.access_token.as_str(),
                        auth.refresh_token.as_deref(),
                    )
                    .await;

                    if let Some(active_email) = extract_status_account_email(&status_response) {
                        if active_email.eq_ignore_ascii_case(email) {
                            println!(
                                "✅ You are already signed in as '{}'. Registration is not needed.",
                                active_email
                            );
                            return true;
                        }
                    }
                }
            }

            // If an account email is already configured and differs, confirm before proceeding.
            if let Some(existing_email) = acct.email.as_ref() {
                if !existing_email.eq_ignore_ascii_case(email) {
                    println!(
                        "⚠️ Local account is currently configured as '{}'.",
                        existing_email
                    );
                    println!(
                        "Continuing will replace local account email and tokens on this machine."
                    );
                    println!(
                        "If you need the current local state, back up '{}'.",
                        config_path().display()
                    );
                    print!("Continue and switch to '{}'? [y/N]: ", email);
                    if let Err(e) = io::stdout().flush() {
                        eprintln!("⚠️ Failed to flush stdout: {e}");
                        return false;
                    }

                    let mut input = String::new();
                    if let Err(e) = io::stdin().read_line(&mut input) {
                        eprintln!("⚠️ Failed to read input: {e}");
                        return false;
                    }

                    let input = input.trim();
                    if !(input.eq_ignore_ascii_case("y") || input.eq_ignore_ascii_case("yes")) {
                        println!("Operation canceled.");
                        return true;
                    }
                }
            }
        }
    }

    match infra_api::account::register::register_email(INFRA_BASE_URL, email).await {
        Ok(json) => {
            if !ui::account_status::render_backend_ui(&json) {
                match serde_json::to_string_pretty(&json) {
                    Ok(pretty) => println!("{pretty}"),
                    Err(_) => println!("{json:?}"),
                }
            }

            // Persist the active account email locally only on successful registration.
            if json
                .get("status")
                .and_then(|s| s.as_str())
                .map(|s| s.eq_ignore_ascii_case("success"))
                .unwrap_or(false)
            {
                if let Err(e) = set_account_email(email.to_string(), true) {
                    eprintln!("⚠️ Failed to save account email to config: {e}");
                }
                true
            } else {
                false
            }
        }
        Err(e) => {
            eprintln!("❌ Request failed: {e}");
            false
        }
    }
}
