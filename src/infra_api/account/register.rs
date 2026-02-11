// src/infra_api/account/register.rs
//
// POST /account
// Body: { "action": "register", "email": "<email>" }
//
// Notes:
// - We do NOT send the API Gateway event wrapper. Only the JSON body the Lambda parses.
// - This client surfaces the raw JSON returned by the service (success or failure)
//   and avoids interpreting application-level status at this stage.

use serde::Serialize;
use serde_json::Value;
use std::error::Error;
use std::fmt;

#[derive(Debug, Serialize)]
struct RegisterRequest<'a> {
    action: &'static str,
    email: &'a str,
}

#[derive(Debug)]
pub enum RegisterError {
    Http(reqwest::Error),
    Parse(String),
}

impl From<reqwest::Error> for RegisterError {
    fn from(e: reqwest::Error) -> Self {
        Self::Http(e)
    }
}

impl fmt::Display for RegisterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Http(e) => write!(f, "HTTP request error: {e}"),
            Self::Parse(e) => write!(f, "Failed to parse response JSON: {e}"),
        }
    }
}

impl Error for RegisterError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Http(e) => Some(e),
            Self::Parse(_) => None,
        }
    }
}

/// Registers an account email by calling cargo-ai-infra.
/// Returns the raw JSON payload from the service (success or failure).
pub async fn register_email(base_url: &str, email: &str) -> Result<Value, RegisterError> {
    let url = format!("{}/account", base_url.trim_end_matches('/'));

    let body = RegisterRequest {
        action: "register",
        email,
    };

    let client = reqwest::Client::new();
    let resp = client
        .post(url)
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await?;

    let text = resp.text().await?;

    serde_json::from_str::<Value>(&text)
        .map_err(|e| RegisterError::Parse(e.to_string()))
}
