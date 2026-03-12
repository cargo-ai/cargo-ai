#[cfg(test)]
use crate::config::schema::CargoAiMetadata;
use serde_json::{json, Value};

/// Confirm an account using the temporary code (temporary password) sent via email.
///
/// POST /account
/// { "action": "confirm", "email": "<email>", "code": "<code>" }
///
/// Returns the raw JSON response from the infra API (success or failure).
pub async fn confirm_email(
    base_url: &str,
    email: &str,
    code: &str,
) -> Result<Value, reqwest::Error> {
    let url = format!("{}/account", base_url.trim_end_matches('/'));
    let body = build_confirm_body(email, code);

    let client = reqwest::Client::new();
    let resp = client.post(url).json(&body).send().await?;

    // Always attempt to return the JSON body even for non-2xx responses,
    // so the CLI can surface infra error details directly.
    match resp.json::<Value>().await {
        Ok(v) => Ok(v),
        Err(e) => Err(e),
    }
}

fn build_confirm_body(email: &str, code: &str) -> Value {
    super::with_cargo_ai_metadata(json!({
        "action": "confirm",
        "email": email,
        "code": code,
    }))
}

#[cfg(test)]
fn build_confirm_body_with_metadata(
    email: &str,
    code: &str,
    metadata: Option<CargoAiMetadata>,
) -> Value {
    super::with_cargo_ai_metadata_override(
        json!({
            "action": "confirm",
            "email": email,
            "code": code,
        }),
        metadata,
    )
}

#[cfg(test)]
mod tests {
    use super::build_confirm_body_with_metadata;
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
    fn build_confirm_body_includes_top_level_metadata() {
        let body = build_confirm_body_with_metadata(
            "person@example.com",
            "123456",
            Some(sample_metadata()),
        );

        assert_eq!(body["action"], "confirm");
        assert_eq!(body["email"], "person@example.com");
        assert_eq!(body["code"], "123456");
        assert_eq!(
            body["cargo_ai_metadata"]["cargo_ai_build_target"],
            "aarch64-apple-darwin"
        );
    }
}
