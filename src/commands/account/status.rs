//! Runtime behavior for `cargo ai account status`.
use crate::config::loader::load_config;
use crate::config::setup::config_path;
use crate::infra_api;
use crate::ui;

use super::helpers::{load_account_auth, persist_refreshed_access_token, INFRA_BASE_URL};

/// Queries account/session status and persists refreshed access tokens.
pub async fn run() -> bool {
    execute(&mut super::machine::Report::new(false)).await
}
pub(crate) async fn machine_run() -> Result<serde_json::Value, crate::commands::machine::Failure> {
    let mut report = super::machine::Report::new(true);
    execute(&mut report).await;
    report.result
}
async fn execute(report: &mut super::machine::Report) -> bool {
    execute_at(report, INFRA_BASE_URL).await
}
async fn execute_at(report: &mut super::machine::Report, base_url: &str) -> bool {
    report.prerequisites();
    // Account status: check and optionally refresh tokens, print status.
    //
    // Behavior:
    // - Prefer using ONLY the access token.
    // - If local timestamps indicate the token is expired (or near expiry), include refresh_token.
    // - If the server still reports `access_token_expired`, retry once with refresh_token.
    //
    // NOTE: We avoid refreshing unless needed for security reasons.

    // 1. Load config
    let cfg = match load_config() {
        Some(cfg) => cfg,
        None => {
            eprintln!(
                "x No local config file found at '{}'. Run `cargo ai account register <email>` on this machine, or copy your config from another machine.",
                config_path().display()
            );
            return false;
        }
    };

    // 2. Extract account
    let acct = match cfg.account.as_ref() {
        Some(acct) => acct,
        None => {
            eprintln!("x No account found in config. You must confirm your account first.");
            return false;
        }
    };

    // 3. Extract token metadata from config and secret tokens from credential store.
    let auth = match load_account_auth() {
        Ok(auth) => auth,
        Err(error) => {
            eprintln!("{}", ui::account_status::normalize_leading_glyph(&error));
            return false;
        }
    };

    if auth.refresh_token.is_none() {
        eprintln!("! No refresh token found in credential store. Status will work only while the access token remains valid.");
    }

    // Compute token expiration using consistent integer types.
    //
    // access_token_issued_at: unix timestamp (seconds)
    // access_token_expires_in: duration in seconds
    //
    // We use a small safety buffer so we refresh slightly *before* expiry when needed.
    const EXPIRY_SAFETY_BUFFER_SEC: i64 = 30;

    let issued_at = acct.access_token_issued_at.unwrap_or(0); // i64 unix timestamp
    let expires_in_i64 = acct.access_token_expires_in.map(|n| n as i64).unwrap_or(0);

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    // If we don't have timestamps yet, do NOT pre-emptively refresh; let the server decide.
    let have_local_expiry = issued_at > 0 && expires_in_i64 > 0;
    let token_expired_or_near = if have_local_expiry {
        (issued_at + expires_in_i64 - EXPIRY_SAFETY_BUFFER_SEC) <= now
    } else {
        false
    };

    // 4. First attempt: access token only (unless local expiry suggests refresh)
    // This keeps refresh traffic low and makes token rotation explicit on demand.
    // NOTE: avoid an async closure here to keep lifetimes simple.
    let mut used_refresh = false;

    // Own the tokens so any futures we create don't borrow locals with tricky lifetimes.
    report.protect(&auth.access_token);
    if let Some(secret) = auth.refresh_token.as_deref() {
        report.protect(secret);
    }
    let access_token_owned = auth.access_token;
    let refresh_token_owned: Option<String> = auth.refresh_token;

    let first_refresh_token_opt: Option<&str> = if token_expired_or_near {
        used_refresh = refresh_token_owned.is_some();
        refresh_token_owned.as_deref()
    } else {
        None
    };

    report.transmitting(false);
    let mut response = match infra_api::account::status::fetch_status(
        base_url,
        access_token_owned.as_str(),
        first_refresh_token_opt,
    )
    .await
    {
        Ok(r) => r,
        Err(e) => {
            eprintln!("x Request failed: {e:?}");
            return false;
        }
    };

    // 5. Retry once with refresh token if the server reports expired and we didn't already refresh.
    let is_expired_error = response
        .get("status")
        .and_then(|v| v.as_str())
        .map(|s| s.eq_ignore_ascii_case("error"))
        .unwrap_or(false)
        && response
            .get("type")
            .and_then(|v| v.as_str())
            .map(|t| t == "access_token_expired")
            .unwrap_or(false);

    if is_expired_error && !used_refresh {
        if let Some(rt) = refresh_token_owned.as_deref() {
            match infra_api::account::status::fetch_status(
                base_url,
                access_token_owned.as_str(),
                Some(rt),
            )
            .await
            {
                Ok(r) => response = r,
                Err(e) => {
                    eprintln!("x Request failed: {e:?}");
                    return false;
                }
            }
        }
    }

    if let Some(secret) = response["session"]["access_token"].as_str() {
        report.protect(secret);
    }
    if report.active {
        if let Err(error) = super::machine::response_ok(&response, "account_status_succeeded") {
            report.result = Err(error);
            return false;
        }
        if response["account"]["email"]
            .as_str()
            .is_none_or(|email| email.is_empty())
        {
            report.fail(
                "response.invalid",
                "The service returned no usable account identity.",
                serde_json::json!({"session_persistence":"not_attempted"}),
            );
            return false;
        }
    }
    // 6. Render backend-provided UI when available, fallback to raw JSON.
    if !ui::account_status::render_account_status_ui(&response) {
        match serde_json::to_string_pretty(&response) {
            Ok(pretty) => println!("{pretty}"),
            Err(_) => println!("{response:?}"),
        }
    }

    // 7. Persist refreshed access token if present in response.
    //
    // Infra contract: when refresh occurred (and return_refreshed_access_token=true), response includes:
    //   session: { refreshed: true, access_token: "...", expires_in_seconds: 123 }
    if let Some(session) = response.get("session") {
        let new_access_token = session
            .get("access_token")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty());

        let new_expires_in_seconds: Option<i32> = session
            .get("expires_in_seconds")
            .and_then(|v| v.as_i64())
            .and_then(|n| i32::try_from(n).ok());

        if report.active
            && new_access_token.is_some_and(|token| token != access_token_owned)
            && new_expires_in_seconds.is_none()
        {
            report.fail("operation.partial", "Status was retrieved but refreshed session expiry metadata was missing or invalid.", serde_json::json!({"session_persistence":"failed","remote_session_refresh":"applied"}));
            return false;
        }
        if let (Some(at), Some(expires_in)) = (new_access_token, new_expires_in_seconds) {
            // We only update if the access token actually changed.
            if at != access_token_owned {
                let rt = match refresh_token_owned.as_deref() {
                    Some(rt) => rt,
                    None => {
                        // Shouldn't happen in the refresh scenario, but don't clobber anything.
                        eprintln!("! Refreshed access token returned, but no refresh token exists in credential store to persist alongside it.");
                        report.fail("operation.partial", "Status was retrieved but refreshed credentials could not be persisted without the existing refresh token.", serde_json::json!({"session_persistence":"failed","remote_session_refresh":"applied"}));
                        return false;
                    }
                };

                if report.active {
                    report.session_persistence =
                        match super::helpers::persist_selected_refreshed_access_token(
                            at,
                            rt,
                            Some(expires_in),
                        ) {
                            Ok(()) => "persisted",
                            Err(_) => "failed",
                        };
                } else {
                    persist_refreshed_access_token(at, rt, Some(expires_in));
                }
            }
        }
    }

    if report.active {
        report.result = super::machine::response_ok(&response, "account_status_succeeded")
            .and_then(|()| {
                if !response["account"].is_object() {
                    return Err(crate::commands::machine::Failure::new("response.invalid", "Account identity was missing from the status response."));
                }
                Ok(serde_json::json!({
                    "account":super::machine::fields(&response["account"], &["email", "handle", "status"]),
                    "plan":super::machine::fields(&response["plan"], &["code", "name"]),
                    "session":super::machine::fields(&response["session"], &["refreshed", "expires_in_seconds"]),
                    "session_persistence":report.session_persistence,
                    "readiness":"authenticated"
                }))
            });
        if let Ok(data) = &report.result {
            report.accepted(data.clone());
        }
        if report.session_persistence == "failed" {
            let data = match &report.result {
                Ok(value) => value.clone(),
                Err(_) => serde_json::json!({"session_persistence":"failed"}),
            };
            report.fail(
                "operation.partial",
                "Status was retrieved but refreshed credentials could not be persisted.",
                data,
            );
        }
    }
    response
        .get("status")
        .and_then(|v| v.as_str())
        .map(|s| s.eq_ignore_ascii_case("success"))
        .unwrap_or(false)
}

#[cfg(test)]
mod machine_tests {
    use super::*;
    use serde_json::json;
    #[tokio::test]
    async fn machine_account_status_projects_identity_and_persists_refresh_without_exposing_tokens()
    {
        let _home = crate::commands::secret_input::test_support::Home::new();
        crate::config::adder::set_account_tokens(
            "old-access".into(),
            "private-refresh".into(),
            3600,
        )
        .unwrap();
        let mut server = mockito::Server::new_async().await;
        let request=server.mock("POST","/account")
            .match_body(mockito::Matcher::PartialJson(json!({"action":"status","credentials":{"access_token":"old-access"}})))
            .with_status(200).with_body(json!({"status":"success","type":"account_status_succeeded",
                "account":{"email":"owner@example.test","handle":"fixture","internal_id":"private-id"},
                "session":{"refreshed":true,"access_token":"new-private-access","expires_in_seconds":3600},
                "plan":{"code":"fixture","name":"Fixture","description":"private-description"},
                "ui":{"title":"private-ui"},"message":"private-message"}).to_string())
            .expect(1).create_async().await;
        let mut report = super::super::machine::Report::new(true);
        assert!(execute_at(&mut report, &server.url()).await);
        let data = report.result.unwrap();
        assert_eq!(data["account"]["handle"], "fixture");
        assert_eq!(data["session_persistence"], "persisted");
        let serialized = data.to_string();
        for secret in [
            "new-private-access",
            "private-refresh",
            "private-id",
            "private-ui",
            "private-message",
            "private-description",
        ] {
            assert!(!serialized.contains(secret));
        }
        assert_eq!(
            crate::credentials::store::load_account_tokens()
                .unwrap()
                .unwrap()
                .access_token,
            "new-private-access"
        );
        request.assert_async().await;
    }
}
