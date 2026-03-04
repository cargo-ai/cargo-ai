//! Shared helpers for account command modules.
use crate::config::adder::set_account_tokens;
use crate::config::loader::load_config;
use crate::config::setup::config_path;
use crate::credentials::store;
use crate::infra_api;

/// Canonical Cargo-AI API base URL used by account command flows.
pub const INFRA_BASE_URL: &str = "https://api.cargo-ai.org";

/// In-memory account auth tokens loaded from local credential storage.
#[derive(Debug, Clone)]
pub struct AccountAuth {
    pub access_token: String,
    pub refresh_token: Option<String>,
}

/// Failure modes when attempting to refresh an expired access token.
#[derive(Debug)]
pub enum RefreshAccessError {
    MissingRefreshToken,
    RequestFailed(String),
    MissingRefreshedToken(serde_json::Value),
}

/// Loads the account access and refresh tokens from local credential storage with
/// user-facing validation errors.
pub fn load_account_auth() -> Result<AccountAuth, String> {
    let cfg = load_config().ok_or_else(|| {
        format!(
            "❌ No local config file found at '{}'. Run `cargo ai account register <email>` on this machine, or copy your config from another machine.",
            config_path().display()
        )
    })?;

    let acct = cfg.account.as_ref().ok_or_else(|| {
        "❌ No account found in config. You must confirm your account first.".to_string()
    })?;

    if let Some(account_tokens) = store::load_account_tokens()
        .map_err(|error| format!("❌ Failed to load account credentials: {error}"))?
    {
        return Ok(AccountAuth {
            access_token: account_tokens.access_token,
            refresh_token: account_tokens.refresh_token,
        });
    }

    // Backward-compatible fallback for legacy configs when migration has not run yet.
    let access_token = acct.access_token.as_ref().cloned().ok_or_else(|| {
        "❌ No access token found in credentials store or legacy config. Run `cargo ai account confirm <code>` first."
            .to_string()
    })?;

    Ok(AccountAuth {
        access_token,
        refresh_token: acct.refresh_token.clone(),
    })
}

/// Refreshes an expired access token by calling account status with refresh
/// token support and returns the retry token + optional expiry.
pub async fn refresh_access_token_for_retry(
    access_token: &str,
    refresh_token: Option<&str>,
) -> Result<(String, Option<i32>), RefreshAccessError> {
    let rt = refresh_token.ok_or(RefreshAccessError::MissingRefreshToken)?;

    let refresh_response =
        infra_api::account::status::fetch_status(INFRA_BASE_URL, access_token, Some(rt))
            .await
            .map_err(|e| RefreshAccessError::RequestFailed(format!("{e:?}")))?;

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

    match refreshed_access_token {
        Some(token) => Ok((token, refreshed_expires_in)),
        None => Err(RefreshAccessError::MissingRefreshedToken(refresh_response)),
    }
}

/// Persists refreshed access token metadata when expiry is provided.
pub fn persist_refreshed_access_token(
    refreshed_access_token: &str,
    refresh_token: &str,
    refreshed_expires_in: Option<i32>,
) {
    if let Some(expires_in) = refreshed_expires_in {
        if let Err(e) = set_account_tokens(
            refreshed_access_token.to_string(),
            refresh_token.to_string(),
            expires_in,
        ) {
            eprintln!("⚠️ Failed to update account tokens in credential store: {e}");
        }
    }
}

/// Applies `--limit` output truncation to successful agents-list responses.
pub fn apply_agents_list_display_limit(
    response: &mut serde_json::Value,
    display_limit: Option<usize>,
) -> Option<(usize, usize)> {
    let limit = display_limit?;
    let response_type = response.get("type").and_then(|v| v.as_str());
    if response_type != Some("account_agents_list_succeeded") {
        return None;
    }

    let agents = response.get_mut("agents").and_then(|v| v.as_array_mut())?;
    let total = agents.len();
    if total <= limit {
        return None;
    }

    agents.truncate(limit);
    let shown = agents.len();

    if let Some(ui) = response.get_mut("ui") {
        if let Some(summary) = ui.get_mut("summary") {
            *summary = serde_json::json!(format!("Showing {shown} of {total} agents."));
        }

        if let Some(sections) = ui.get_mut("sections").and_then(|v| v.as_array_mut()) {
            for section in sections.iter_mut() {
                let is_list_section = section
                    .get("type")
                    .and_then(|v| v.as_str())
                    .map(|v| v == "list")
                    .unwrap_or(false);
                let is_kv_section = section
                    .get("type")
                    .and_then(|v| v.as_str())
                    .map(|v| v == "kv")
                    .unwrap_or(false);

                if is_list_section {
                    if let Some(items) = section.get_mut("items").and_then(|v| v.as_array_mut()) {
                        items.truncate(limit);
                    }
                }

                if is_kv_section {
                    if let Some(items) = section.get_mut("items").and_then(|v| v.as_array_mut()) {
                        for item in items.iter_mut() {
                            let is_count = item
                                .get("label")
                                .and_then(|v| v.as_str())
                                .map(|label| label.eq_ignore_ascii_case("count"))
                                .unwrap_or(false);

                            if is_count {
                                item["value"] = serde_json::json!(shown);
                            }
                        }
                    }
                }
            }

            sections.push(serde_json::json!({
                "type": "notice",
                "message": format!(
                    "Showing {shown} of {total} agents. Use --limit <N> or --all to adjust output."
                )
            }));
        }
    }

    Some((shown, total))
}

/// Fetches status for register-guard checks and retries once with refresh token
/// when the initial access token is expired.
pub async fn fetch_status_for_register_guard(
    access_token: &str,
    refresh_token: Option<&str>,
) -> serde_json::Value {
    let first_response =
        match infra_api::account::status::fetch_status(INFRA_BASE_URL, access_token, None).await {
            Ok(v) => v,
            Err(e) => {
                eprintln!("⚠️ Could not validate local session before register: {e:?}");
                return serde_json::Value::Null;
            }
        };

    let is_expired_error = first_response
        .get("type")
        .and_then(|v| v.as_str())
        .map(|t| t == "access_token_expired")
        .unwrap_or(false);

    if !is_expired_error {
        return first_response;
    }

    let rt = match refresh_token {
        Some(rt) => rt,
        None => return first_response,
    };

    match infra_api::account::status::fetch_status(INFRA_BASE_URL, access_token, Some(rt)).await {
        Ok(v) => v,
        Err(e) => {
            eprintln!("⚠️ Could not refresh local session before register: {e:?}");
            serde_json::Value::Null
        }
    }
}

/// Extracts the account email from a successful status response payload.
pub fn extract_status_account_email(status_response: &serde_json::Value) -> Option<String> {
    let is_success = status_response
        .get("status")
        .and_then(|v| v.as_str())
        .map(|s| s.eq_ignore_ascii_case("success"))
        .unwrap_or(false);

    if !is_success {
        return None;
    }

    status_response
        .get("account")
        .and_then(|v| v.get("email"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::{apply_agents_list_display_limit, extract_status_account_email};
    use serde_json::json;

    fn sample_agents_list_response() -> serde_json::Value {
        json!({
            "status": "success",
            "type": "account_agents_list_succeeded",
            "agents": [
                { "name": "agent-1" },
                { "name": "agent-2" },
                { "name": "agent-3" }
            ],
            "ui": {
                "summary": "Showing all agents.",
                "sections": [
                    {
                        "type": "list",
                        "items": [
                            { "name": "agent-1" },
                            { "name": "agent-2" },
                            { "name": "agent-3" }
                        ]
                    },
                    {
                        "type": "kv",
                        "items": [
                            { "label": "count", "value": 3 },
                            { "label": "owner", "value": "demo" }
                        ]
                    }
                ]
            }
        })
    }

    #[test]
    fn applies_display_limit_and_keeps_response_shape_consistent() {
        let mut response = sample_agents_list_response();
        let result = apply_agents_list_display_limit(&mut response, Some(2));

        assert_eq!(result, Some((2, 3)));
        assert_eq!(
            response
                .get("agents")
                .and_then(|v| v.as_array())
                .map(Vec::len),
            Some(2)
        );
        assert_eq!(
            response
                .get("ui")
                .and_then(|v| v.get("summary"))
                .and_then(|v| v.as_str()),
            Some("Showing 2 of 3 agents.")
        );

        let sections = response
            .get("ui")
            .and_then(|v| v.get("sections"))
            .and_then(|v| v.as_array())
            .expect("ui.sections should be present");

        let list_len = sections
            .iter()
            .find(|section| section.get("type").and_then(|v| v.as_str()) == Some("list"))
            .and_then(|section| section.get("items"))
            .and_then(|items| items.as_array())
            .map(Vec::len);
        assert_eq!(list_len, Some(2));

        let kv_count_value = sections
            .iter()
            .find(|section| section.get("type").and_then(|v| v.as_str()) == Some("kv"))
            .and_then(|section| section.get("items"))
            .and_then(|items| items.as_array())
            .and_then(|items| {
                items.iter().find(|item| {
                    item.get("label")
                        .and_then(|v| v.as_str())
                        .map(|label| label.eq_ignore_ascii_case("count"))
                        .unwrap_or(false)
                })
            })
            .and_then(|item| item.get("value"))
            .and_then(|v| v.as_i64());
        assert_eq!(kv_count_value, Some(2));

        let has_notice = sections.iter().any(|section| {
            section.get("type").and_then(|v| v.as_str()) == Some("notice")
                && section
                    .get("message")
                    .and_then(|v| v.as_str())
                    .map(|m| m.contains("Showing 2 of 3 agents"))
                    .unwrap_or(false)
        });
        assert!(has_notice);
    }

    #[test]
    fn does_not_apply_limit_for_non_success_agents_list_shape() {
        let mut response = json!({
            "status": "error",
            "type": "account_agents_list_failed",
            "agents": [{ "name": "agent-1" }]
        });
        let original = response.clone();
        let result = apply_agents_list_display_limit(&mut response, Some(1));

        assert_eq!(result, None);
        assert_eq!(response, original);
    }

    #[test]
    fn extracts_status_email_on_success_shape() {
        let response = json!({
            "status": "success",
            "account": {
                "email": "jp@example.com"
            }
        });

        assert_eq!(
            extract_status_account_email(&response),
            Some("jp@example.com".to_string())
        );
    }

    #[test]
    fn returns_none_for_non_success_status_shape() {
        let response = json!({
            "status": "error",
            "account": {
                "email": "jp@example.com"
            }
        });

        assert_eq!(extract_status_account_email(&response), None);
    }
}
