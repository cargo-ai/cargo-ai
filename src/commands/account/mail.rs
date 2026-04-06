//! Runtime behavior for `cargo ai account mail`.
use clap::ArgMatches;

use crate::infra_api;
use crate::ui;

use super::helpers::{
    load_account_auth, persist_refreshed_access_token, refresh_access_token_for_retry,
    RefreshAccessError, INFRA_BASE_URL,
};

/// Routes account mail operations to `test` and `prefs` handlers.
pub async fn run(mail_m: &ArgMatches) -> bool {
    if let Some(test_m) = mail_m.subcommand_matches("test") {
        run_test(test_m).await
    } else if let Some(prefs_m) = mail_m.subcommand_matches("prefs") {
        run_prefs(prefs_m).await
    } else {
        eprintln!(
            "No mail subcommand found. Try 'cargo ai account mail test' or 'cargo ai account mail prefs [--disable-all|--enable-all]'."
        );
        false
    }
}

/// Sends a test email using the configured account session.
async fn run_test(test_m: &ArgMatches) -> bool {
    const DEFAULT_TEST_MAIL_SUBJECT: &str = "Cargo-AI deliverability test";
    const DEFAULT_TEST_MAIL_TEXT: &str = "This is a setup test email from Cargo-AI.";

    let subject = test_m
        .get_one::<String>("subject")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| DEFAULT_TEST_MAIL_SUBJECT.to_string());

    let text = test_m
        .get_one::<String>("text")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| DEFAULT_TEST_MAIL_TEXT.to_string());

    let auth = match load_account_auth() {
        Ok(auth) => auth,
        Err(message) => {
            eprintln!("{}", ui::account_status::normalize_leading_glyph(&message));
            return false;
        }
    };
    let access_token_owned = auth.access_token;
    let refresh_token = auth.refresh_token;

    // 4. First attempt with current access token.
    let mut response = match infra_api::account::send_mail::send_test_mail(
        INFRA_BASE_URL,
        access_token_owned.as_str(),
        subject.as_str(),
        text.as_str(),
    )
    .await
    {
        Ok(r) => r,
        Err(e) => {
            eprintln!("x Request failed: {e:?}");
            return false;
        }
    };

    // 5. If access token is expired, refresh via status and retry once.
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
                eprintln!("! Session refresh did not return a new access token. Cannot retry send-mail request.");
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

                response = match infra_api::account::send_mail::send_test_mail(
                    INFRA_BASE_URL,
                    retry_access_token.as_str(),
                    subject.as_str(),
                    text.as_str(),
                )
                .await
                {
                    Ok(r) => r,
                    Err(e) => {
                        eprintln!("x Request failed after session refresh: {e:?}");
                        return false;
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

/// Gets or updates account-wide mail preference flags.
async fn run_prefs(prefs_m: &ArgMatches) -> bool {
    let set_all_emails_enabled = if prefs_m.get_flag("disable_all") {
        Some(false)
    } else if prefs_m.get_flag("enable_all") {
        Some(true)
    } else {
        None
    };

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
    let mut response = if let Some(all_emails_enabled) = set_all_emails_enabled {
        match infra_api::account::mail_preferences::set_all_emails_enabled(
            INFRA_BASE_URL,
            access_token_owned.as_str(),
            all_emails_enabled,
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
        match infra_api::account::mail_preferences::fetch_preferences(
            INFRA_BASE_URL,
            access_token_owned.as_str(),
        )
        .await
        {
            Ok(r) => r,
            Err(e) => {
                eprintln!("x Request failed: {e:?}");
                return false;
            }
        }
    };

    // 5. If access token is expired, refresh via status and retry once.
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
                eprintln!("! Session refresh did not return a new access token. Cannot retry mail-preferences request.");
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

                response = if let Some(all_emails_enabled) = set_all_emails_enabled {
                    match infra_api::account::mail_preferences::set_all_emails_enabled(
                        INFRA_BASE_URL,
                        retry_access_token.as_str(),
                        all_emails_enabled,
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
                    match infra_api::account::mail_preferences::fetch_preferences(
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
