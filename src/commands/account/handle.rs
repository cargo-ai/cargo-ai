//! Runtime behavior for `cargo ai account handle`.
use clap::ArgMatches;

use crate::infra_api;
use crate::ui;

use super::helpers::{
    load_account_auth, persist_refreshed_access_token, refresh_access_token_for_retry,
    RefreshAccessError, INFRA_BASE_URL,
};

/// Fetches or updates the account handle, including token-refresh retry logic.
pub async fn run(handle_m: &ArgMatches) -> bool {
    // Account handle: get current handle or set a new one.
    //
    // Behavior:
    // - `cargo ai account handle` => GET current handle
    // - `cargo ai account handle --set <HANDLE>` => SET handle

    let auth = match load_account_auth() {
        Ok(auth) => auth,
        Err(message) => {
            eprintln!("{}", ui::account_status::normalize_leading_glyph(&message));
            return false;
        }
    };
    let access_token_owned = auth.access_token;
    let refresh_token = auth.refresh_token;

    // 4. Route GET vs SET (first attempt with current access token)
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
                eprintln!("x Request failed: {e:?}");
                return false;
            }
        }
    } else {
        match infra_api::account::handle::fetch_handle(INFRA_BASE_URL, access_token_owned.as_str())
            .await
        {
            Ok(r) => r,
            Err(e) => {
                eprintln!("x Request failed: {e:?}");
                return false;
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
        match refresh_access_token_for_retry(access_token_owned.as_str(), refresh_token.as_deref())
            .await
        {
            Err(RefreshAccessError::MissingRefreshToken) => {
                eprintln!("! Access token expired, and no refresh token exists in credential store. Run `cargo ai account status` or re-confirm account.");
                if !ui::account_status::render_backend_ui(&response) {
                    match serde_json::to_string_pretty(&response) {
                        Ok(pretty) => println!("{pretty}"),
                        Err(_) => println!("{response:?}"),
                    }
                }
                return false;
            }
            Err(RefreshAccessError::RequestFailed(error)) => {
                eprintln!("x Request failed while refreshing session: {error}");
                return false;
            }
            Err(RefreshAccessError::MissingRefreshedToken(refresh_response)) => {
                eprintln!("! Session refresh did not return a new access token. Cannot retry handle request.");
                if !ui::account_status::render_backend_ui(&refresh_response) {
                    match serde_json::to_string_pretty(&refresh_response) {
                        Ok(pretty) => println!("{pretty}"),
                        Err(_) => println!("{refresh_response:?}"),
                    }
                }
                return false;
            }
            Ok((retry_access_token, refreshed_expires_in)) => {
                if let Some(rt) = refresh_token.as_deref() {
                    persist_refreshed_access_token(
                        retry_access_token.as_str(),
                        rt,
                        refreshed_expires_in,
                    );
                }

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
                            eprintln!("x Request failed after session refresh: {e:?}");
                            return false;
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
                            eprintln!("x Request failed after session refresh: {e:?}");
                            return false;
                        }
                    }
                };
            }
        }
    }

    // 6. Render backend-provided UI when available, fallback to raw JSON.
    if !ui::account_status::render_backend_ui(&response) {
        match serde_json::to_string_pretty(&response) {
            Ok(pretty) => println!("{pretty}"),
            Err(_) => println!("{response:?}"),
        }
    }

    response
        .get("status")
        .and_then(|v| v.as_str())
        .map(|s| s.eq_ignore_ascii_case("success"))
        .unwrap_or(false)
}
