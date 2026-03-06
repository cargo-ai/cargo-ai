#[cfg(test)]
use crate::config::schema::CargoAiMetadata;
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
    let body = build_send_test_mail_body(access_token, subject, text);

    let client = reqwest::Client::new();
    let resp = client.post(url).json(&body).send().await?;

    // Always attempt to return the JSON body even for non-2xx responses,
    // so the CLI can surface infra error details directly.
    match resp.json::<Value>().await {
        Ok(v) => Ok(v),
        Err(e) => Err(e),
    }
}

fn build_send_test_mail_body(access_token: &str, subject: &str, text: &str) -> Value {
    super::with_cargo_ai_metadata(json!({
        "action": "send_mail",
        "credentials": {
            "access_token": access_token
        },
        "send_mail": {
            "subject": subject,
            "text": text
        }
    }))
}

#[cfg(test)]
fn build_send_test_mail_body_with_metadata(
    access_token: &str,
    subject: &str,
    text: &str,
    metadata: Option<CargoAiMetadata>,
) -> Value {
    super::with_cargo_ai_metadata_override(
        json!({
            "action": "send_mail",
            "credentials": {
                "access_token": access_token
            },
            "send_mail": {
                "subject": subject,
                "text": text
            }
        }),
        metadata,
    )
}

#[cfg(test)]
mod tests {
    use super::build_send_test_mail_body_with_metadata;
    use crate::config::schema::CargoAiMetadata;

    fn sample_metadata() -> CargoAiMetadata {
        CargoAiMetadata {
            cargo_ai_version: Some("0.0.10".to_string()),
            template_schema_version: Some("2026-03-03.r1".to_string()),
            cargo_ai_build_target: Some("aarch64-apple-darwin".to_string()),
            cargo_ai_install_id: Some("install-123".to_string()),
            cargo_ai_binary_sha256: Some("hash-456".to_string()),
        }
    }

    #[test]
    fn build_send_test_mail_body_includes_top_level_metadata() {
        let body = build_send_test_mail_body_with_metadata(
            "access-token-123",
            "subject",
            "body",
            Some(sample_metadata()),
        );

        assert_eq!(body["action"], "send_mail");
        assert_eq!(body["send_mail"]["subject"], "subject");
        assert_eq!(body["send_mail"]["text"], "body");
        assert_eq!(
            body["cargo_ai_metadata"]["template_schema_version"],
            "2026-03-03.r1"
        );
    }
}
