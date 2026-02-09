// src/infra_api/account/register.rs
//
// POST /account
// Body: { "action": "register", "email": "<email>" }
//
// Notes:
// - We do NOT send the API Gateway event wrapper. Only the JSON body the Lambda parses.
// - Success responses may evolve; we treat presence of {"error": "..."} as failure.

use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize)]
struct RegisterRequest<'a> {
    action: &'static str,
    email: &'a str,
}

#[derive(Debug, Deserialize)]
struct ErrorEnvelope {
    error: String,
}

#[derive(Debug)]
pub enum RegisterError {
    Http(reqwest::Error),
    NonSuccessStatus(u16, String),
    ServiceError(String),
}

impl From<reqwest::Error> for RegisterError {
    fn from(e: reqwest::Error) -> Self {
        Self::Http(e)
    }
}

/// Registers an account email by calling cargo-ai-infra.
/// Returns Ok(()) if the service indicates success.
pub async fn register_email(base_url: &str, email: &str) -> Result<(), RegisterError> {
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

    let status = resp.status();
    let text = resp.text().await?;

    // If the HTTP status is not success, still try to surface {"error": "..."} if present.
    if !status.is_success() {
        if let Ok(err) = serde_json::from_str::<ErrorEnvelope>(&text) {
            return Err(RegisterError::ServiceError(err.error));
        }
        return Err(RegisterError::NonSuccessStatus(status.as_u16(), text));
    }

    // Even on 2xx, the service might return {"error": "..."}.
    if let Ok(err) = serde_json::from_str::<ErrorEnvelope>(&text) {
        return Err(RegisterError::ServiceError(err.error));
    }

    // Otherwise, treat it as success (payload can evolve).
    Ok(())
}