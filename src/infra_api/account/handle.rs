use serde_json::{json, Value};

/// Fetch the current account handle using an access token.
///
/// POST /account
/// {
///   "action": "handle",
///   "credentials": {
///     "access_token": "<access_token>"
///   }
/// }
///
/// Returns the raw JSON response from the infra API (success or failure).
pub async fn fetch_handle(base_url: &str, access_token: &str) -> Result<Value, reqwest::Error> {
    let url = format!("{}/account", base_url.trim_end_matches('/'));

    let body = json!({
        "action": "handle",
        "credentials": {
            "access_token": access_token
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

/// Set the current account handle using an access token.
///
/// POST /account
/// {
///   "action": "handle",
///   "credentials": {
///     "access_token": "<access_token>"
///   },
///   "handle": {
///     "set": "<new_handle>"
///   }
/// }
///
/// Returns the raw JSON response from the infra API (success or failure).
pub async fn set_handle(
    base_url: &str,
    access_token: &str,
    new_handle: &str,
) -> Result<Value, reqwest::Error> {
    let url = format!("{}/account", base_url.trim_end_matches('/'));

    let body = json!({
        "action": "handle",
        "credentials": {
            "access_token": access_token
        },
        "handle": {
            "set": new_handle
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
