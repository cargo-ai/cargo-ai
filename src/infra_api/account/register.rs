// src/infra_api/account/register.rs
//
// POST /account
// Body: { "action": "register", "email": "<email>" }
//
// Notes:
// - We do NOT send the API Gateway event wrapper. Only the JSON body the Lambda parses.
// - This client surfaces the raw JSON returned by the service (success or failure)
//   and avoids interpreting application-level status at this stage.

#[cfg(test)]
use crate::config::schema::CargoAiMetadata;
use serde_json::{json, Value};
use std::error::Error;
use std::fmt;

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
    let body = build_register_body(email);

    let client = reqwest::Client::new();
    let resp = client
        .post(url)
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await?;

    let text = resp.text().await?;

    serde_json::from_str::<Value>(&text).map_err(|e| RegisterError::Parse(e.to_string()))
}

fn build_register_body(email: &str) -> Value {
    super::with_cargo_ai_metadata(json!({
        "action": "register",
        "email": email
    }))
}

#[cfg(test)]
fn build_register_body_with_metadata(email: &str, metadata: Option<CargoAiMetadata>) -> Value {
    super::with_cargo_ai_metadata_override(
        json!({
            "action": "register",
            "email": email
        }),
        metadata,
    )
}

#[cfg(test)]
mod tests {
    use super::build_register_body_with_metadata;
    use crate::config::schema::CargoAiMetadata;

    fn sample_metadata() -> CargoAiMetadata {
        CargoAiMetadata {
            cargo_ai_version: Some(env!("CARGO_PKG_VERSION").to_string()),
            template_schema_version: Some("2026-03-03.r1".to_string()),
            cargo_ai_build_target: Some("aarch64-apple-darwin".to_string()),
            cargo_ai_install_id: Some("install-123".to_string()),
            cargo_ai_binary_sha256: Some("hash-456".to_string()),
        }
    }

    #[test]
    fn build_register_body_includes_top_level_metadata() {
        let body = build_register_body_with_metadata("person@example.com", Some(sample_metadata()));

        assert_eq!(body["action"], "register");
        assert_eq!(body["email"], "person@example.com");
        assert_eq!(
            body["cargo_ai_metadata"]["cargo_ai_version"],
            env!("CARGO_PKG_VERSION")
        );
    }
}
