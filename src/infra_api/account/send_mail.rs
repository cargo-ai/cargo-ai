use serde_json::{json, Value};

/// Send a test email to the authenticated account email using an access token.
///
/// POST /account
/// {
///   "action": "send_mail",
///   "credentials": {
///     "access_token": "<access_token>"
///   },
///   "send_mail": {
///     "subject": "<subject>",
///     "text": "<text>"
///   }
/// }
///
/// Returns the raw JSON response from the infra API (success or failure).
pub async fn send_test_mail(
    base_url: &str,
    access_token: &str,
    subject: &str,
    text: &str,
) -> Result<Value, reqwest::Error> {
    let url = format!("{}/account", base_url.trim_end_matches('/'));

    let body = json!({
        "action": "send_mail",
        "credentials": {
            "access_token": access_token
        },
        "send_mail": {
            "subject": subject,
            "text": text
        }
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
