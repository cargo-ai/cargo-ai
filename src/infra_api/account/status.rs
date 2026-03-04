use serde_json::{json, Value};

/// Fetch account status using the current access token.
///
/// This endpoint is also responsible for refreshing the access token
/// when:
/// - the access token is expired, AND
/// - a refresh token is available, AND
/// - refresh is allowed by policy
///
/// POST /account
///
/// Minimal payload (no refresh):
/// {
///   "action": "status",
///   "credentials": {
///     "access_token": "<access_token>"
///   }
/// }
///
/// Payload with refresh enabled:
/// {
///   "action": "status",
///   "credentials": {
///     "access_token": "<access_token>",
///     "refresh_token": "<refresh_token>"
///   },
///   "session_policy": {
///     "allow_refresh": true
///   }
/// }
///
/// Returns the raw JSON response from the infra API (success or failure).
pub async fn fetch_status(
    base_url: &str,
    access_token: &str,
    refresh_token: Option<&str>,
) -> Result<Value, reqwest::Error> {
    let url = format!("{}/account", base_url.trim_end_matches('/'));

    // --- Build credentials object ---
    let mut credentials = json!({
        "access_token": access_token
    });

    // Include refresh token only when present
    if let Some(rt) = refresh_token {
        credentials["refresh_token"] = json!(rt);
    }

    // --- Build request body ---
    //
    // NOTE:
    // Token refresh behavior is intentionally opaque to the CLI user.
    // Status acts as the canonical place where token freshness is enforced.
    let mut body = json!({
        "action": "status",
        "credentials": credentials
    });

    // Enable refresh policy only when a refresh token exists
    if refresh_token.is_some() {
        body["session_policy"] = json!({
            "allow_refresh": true
        });
    }

    let client = reqwest::Client::new();
    let resp = client.post(url).json(&body).send().await?;

    // Always attempt to return the JSON body, even for non-2xx responses,
    // so the CLI can surface infra error details directly.
    match resp.json::<Value>().await {
        Ok(v) => Ok(v),
        Err(e) => Err(e),
    }
}
