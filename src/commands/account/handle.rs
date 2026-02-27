use clap::ArgMatches;

use crate::config::adder::set_account_tokens;
use crate::config::loader::load_config;
use crate::config::setup::config_path;
use crate::infra_api;
use crate::ui;

use super::helpers::INFRA_BASE_URL;

pub async fn run(handle_m: &ArgMatches) {
    // Account handle: get current handle or set a new one.
    //
    // Behavior:
    // - `cargo ai account handle` => GET current handle
    // - `cargo ai account handle --set <HANDLE>` => SET handle

    // 1. Load config
    let cfg = match load_config() {
        Some(cfg) => cfg,
        None => {
            eprintln!(
                "❌ No local config file found at '{}'. Run `cargo ai account register <email>` on this machine, or copy your config from another machine.",
                config_path().display()
            );
            return;
        }
    };

    // 2. Extract account
    let acct = match cfg.account.as_ref() {
        Some(acct) => acct,
        None => {
            eprintln!("❌ No account found in config. You must confirm your account first.");
            return;
        }
    };

    // 3. Extract access token
    let access_token = match acct.access_token.as_ref() {
        Some(t) => t,
        None => {
            eprintln!("❌ No access token found in config. Run `cargo ai account confirm <code>` first.");
            return;
        }
    };
    let refresh_token = acct.refresh_token.as_ref();

    // 4. Route GET vs SET (first attempt with current access token)
    let access_token_owned = access_token.clone();
    let new_handle_opt = handle_m.get_one::<String>("set").map(|s| s.to_string());

    let mut response = if let Some(new_handle) = new_handle_opt.as_deref() {
        match infra_api::account::handle::set_handle(
            INFRA_BASE_URL,
            access_token_owned.as_str(),
            new_handle,
        )
        .await
        {
            Ok(r) => r,
            Err(e) => {
                eprintln!("❌ Request failed: {e:?}");
                return;
            }
        }
    } else {
        match infra_api::account::handle::fetch_handle(INFRA_BASE_URL, access_token_owned.as_str())
            .await
        {
            Ok(r) => r,
            Err(e) => {
                eprintln!("❌ Request failed: {e:?}");
                return;
            }
        }
    };

    // 5. If access token is expired, refresh via status and retry handle once.
    let is_expired_error = response
        .get("type")
        .and_then(|v| v.as_str())
        .map(|t| t == "access_token_expired")
        .unwrap_or(false);

    if is_expired_error {
        let rt = match refresh_token {
            Some(rt) => rt,
            None => {
                eprintln!("⚠️ Access token expired, and no refresh token exists in config. Run `cargo ai account status` or re-confirm account.");
                if !ui::account_status::render_backend_ui(&response) {
                    match serde_json::to_string_pretty(&response) {
                        Ok(pretty) => println!("{pretty}"),
                        Err(_) => println!("{response:?}"),
                    }
                }
                return;
            }
        };

        let refresh_response = match infra_api::account::status::fetch_status(
            INFRA_BASE_URL,
            access_token_owned.as_str(),
            Some(rt.as_str()),
        )
        .await
        {
            Ok(r) => r,
            Err(e) => {
                eprintln!("❌ Request failed while refreshing session: {e:?}");
                return;
            }
        };

        let refreshed_access_token = refresh_response
            .get("session")
            .and_then(|s| s.get("access_token"))
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());

        let refreshed_expires_in: Option<i32> = refresh_response
            .get("session")
            .and_then(|s| s.get("expires_in_seconds"))
            .and_then(|v| v.as_i64())
            .and_then(|n| i32::try_from(n).ok());

        let retry_access_token = match refreshed_access_token {
            Some(ref at) => {
                if let Some(expires_in) = refreshed_expires_in {
                    if let Err(e) = set_account_tokens(at.to_string(), rt.clone(), expires_in) {
                        eprintln!("⚠️ Failed to update account tokens in config: {e}");
                    }
                }
                at.clone()
            }
            None => {
                eprintln!("⚠️ Session refresh did not return a new access token. Cannot retry handle request.");
                if !ui::account_status::render_backend_ui(&refresh_response) {
                    match serde_json::to_string_pretty(&refresh_response) {
                        Ok(pretty) => println!("{pretty}"),
                        Err(_) => println!("{refresh_response:?}"),
                    }
                }
                return;
            }
        };

        response = if let Some(new_handle) = new_handle_opt.as_deref() {
            match infra_api::account::handle::set_handle(
                INFRA_BASE_URL,
                retry_access_token.as_str(),
                new_handle,
            )
            .await
            {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("❌ Request failed after session refresh: {e:?}");
                    return;
                }
            }
        } else {
            match infra_api::account::handle::fetch_handle(
                INFRA_BASE_URL,
                retry_access_token.as_str(),
            )
            .await
            {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("❌ Request failed after session refresh: {e:?}");
                    return;
                }
            }
        };
    }

    // 6. Render backend-provided UI when available, fallback to raw JSON.
    if !ui::account_status::render_backend_ui(&response) {
        match serde_json::to_string_pretty(&response) {
            Ok(pretty) => println!("{pretty}"),
            Err(_) => println!("{response:?}"),
        }
    }
}
