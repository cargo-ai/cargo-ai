//! Runtime behavior for `cargo ai account confirm`.
use clap::ArgMatches;

use crate::config::adder::set_account_tokens;
use crate::config::loader::load_config;
use crate::config::setup::config_path;
use crate::infra_api;
use crate::ui;

use super::helpers::INFRA_BASE_URL;

/// Confirms a registration code and persists returned tokens on success.
pub async fn run(conf_m: &ArgMatches) -> bool {
    let Some(code) = conf_m.get_one::<String>("code") else {
        eprintln!("❌ Missing confirmation code. Use `cargo ai account confirm <code>`.");
        return false;
    };

    let cfg = match load_config() {
        Some(cfg) => cfg,
        None => {
            eprintln!(
                "❌ No local config file found at '{}'. Run `cargo ai account register <email>` on this machine, or copy your config from another machine.",
                config_path().display()
            );
            return false;
        }
    };

    // Load the configured account email (set during registration)
    let email = match cfg.account.and_then(|acct| acct.email) {
        Some(e) => e,
        None => {
            eprintln!("❌ No account email found in config. Run `cargo ai account register <email>` first.");
            return false;
        }
    };

    match infra_api::account::confirm::confirm_email(INFRA_BASE_URL, &email, code).await {
        Ok(json) => {
            if !ui::account_status::render_backend_ui(&json) {
                match serde_json::to_string_pretty(&json) {
                    Ok(pretty) => println!("{pretty}"),
                    Err(_) => println!("{json:?}"),
                }
            }

            // Persist tokens locally only on successful confirmation.
            if json
                .get("status")
                .and_then(|s| s.as_str())
                .map(|s| s.eq_ignore_ascii_case("success"))
                .unwrap_or(false)
            {
                let creds = json.get("credentials");

                let access_token = creds
                    .and_then(|c| c.get("access_token"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());

                let refresh_token = creds
                    .and_then(|c| c.get("refresh_token"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());

                let expires_in = creds
                    .and_then(|c| c.get("expires_in"))
                    .and_then(|v| v.as_i64())
                    .and_then(|n| i32::try_from(n).ok());

                match (access_token, refresh_token, expires_in) {
                    (Some(at), Some(rt), Some(ex)) => {
                        if let Err(e) = set_account_tokens(at, rt, ex) {
                            eprintln!("⚠️ Failed to save account tokens to config: {e}");
                        }
                    }
                    _ => {
                        eprintln!("⚠️ Confirmation succeeded, but expected credentials were missing from the response.");
                    }
                }
            }
            json
                .get("status")
                .and_then(|s| s.as_str())
                .map(|s| s.eq_ignore_ascii_case("success"))
                .unwrap_or(false)
        }
        Err(e) => {
            eprintln!("❌ Request failed: {e:?}");
            false
        }
    }
}
