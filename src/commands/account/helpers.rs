//! Shared helpers for account command modules.
use crate::infra_api;

/// Canonical Cargo-AI API base URL used by account command flows.
pub const INFRA_BASE_URL: &str = "https://api.cargo-ai.org";

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
