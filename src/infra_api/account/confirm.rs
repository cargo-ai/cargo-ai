use serde_json::{json, Value};

/// Confirm an account using the temporary code (temporary password) sent via email.
///
/// POST /account
/// { "action": "confirm", "email": "<email>", "code": "<code>" }
///
/// Returns the raw JSON response from the infra API (success or failure).
pub async fn confirm_email(base_url: &str, email: &str, code: &str) -> Result<Value, reqwest::Error> {
    let url = format!("{}/account", base_url.trim_end_matches('/'));

    let body = json!({
        "action": "confirm",
        "email": email,
        "code": code,
    });

    let client = reqwest::Client::new();
    let resp = client.post(url).json(&body).send().await?;

    // Always attempt to return the JSON body even for non-2xx responses,
    // so the CLI can surface infra error details directly.
    match resp.json::<Value>().await {
        Ok(v) => Ok(v),
        Err(e) => Err(e),
    }
}