use serde_json::{json, Value};

fn build_fetch_preferences_body(access_token: &str) -> Value {
    json!({
        "action": "mail_preferences",
        "credentials": {
            "access_token": access_token
        }
    })
}

fn build_set_preferences_body(access_token: &str, all_emails_enabled: bool) -> Value {
    json!({
        "action": "mail_preferences",
        "credentials": {
            "access_token": access_token
        },
        "mail_preferences": {
            "set": {
                "all_emails_enabled": all_emails_enabled
            }
        }
    })
}

/// Fetch current account mail preferences using an access token.
///
/// POST /account
/// {
///   "action": "mail_preferences",
///   "credentials": {
///     "access_token": "<access_token>"
///   }
/// }
///
/// Returns the raw JSON response from the infra API (success or failure).
pub async fn fetch_preferences(base_url: &str, access_token: &str) -> Result<Value, reqwest::Error> {
    let url = format!("{}/account", base_url.trim_end_matches('/'));
    let body = build_fetch_preferences_body(access_token);

    let client = reqwest::Client::new();
    let resp = client.post(url).json(&body).send().await?;

    // Always attempt to return the JSON body even for non-2xx responses,
    // so the CLI can surface infra error details directly.
    match resp.json::<Value>().await {
        Ok(v) => Ok(v),
        Err(e) => Err(e),
    }
}

/// Set account-level mail preference using an access token.
///
/// POST /account
/// {
///   "action": "mail_preferences",
///   "credentials": {
///     "access_token": "<access_token>"
///   },
///   "mail_preferences": {
///     "set": {
///       "all_emails_enabled": <bool>
///     }
///   }
/// }
///
/// Returns the raw JSON response from the infra API (success or failure).
pub async fn set_all_emails_enabled(
    base_url: &str,
    access_token: &str,
    all_emails_enabled: bool,
) -> Result<Value, reqwest::Error> {
    let url = format!("{}/account", base_url.trim_end_matches('/'));
    let body = build_set_preferences_body(access_token, all_emails_enabled);

    let client = reqwest::Client::new();
    let resp = client.post(url).json(&body).send().await?;

    // Always attempt to return the JSON body even for non-2xx responses,
    // so the CLI can surface infra error details directly.
    match resp.json::<Value>().await {
        Ok(v) => Ok(v),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::{build_fetch_preferences_body, build_set_preferences_body};
    use serde_json::json;

    #[test]
    fn build_fetch_preferences_body_matches_contract() {
        let body = build_fetch_preferences_body("access-token-123");

        assert_eq!(
            body,
            json!({
                "action": "mail_preferences",
                "credentials": {
                    "access_token": "access-token-123"
                }
            })
        );
    }

    #[test]
    fn build_set_preferences_body_matches_contract_for_disable() {
        let body = build_set_preferences_body("access-token-123", false);

        assert_eq!(
            body,
            json!({
                "action": "mail_preferences",
                "credentials": {
                    "access_token": "access-token-123"
                },
                "mail_preferences": {
                    "set": {
                        "all_emails_enabled": false
                    }
                }
            })
        );
    }

    #[test]
    fn build_set_preferences_body_matches_contract_for_enable() {
        let body = build_set_preferences_body("access-token-123", true);

        assert_eq!(
            body,
            json!({
                "action": "mail_preferences",
                "credentials": {
                    "access_token": "access-token-123"
                },
                "mail_preferences": {
                    "set": {
                        "all_emails_enabled": true
                    }
                }
            })
        );
    }
}
