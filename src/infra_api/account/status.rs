#[cfg(test)]
use crate::config::schema::CargoAiMetadata;
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
    let body = build_status_body(access_token, refresh_token);

    let client = reqwest::Client::new();
    let resp = client.post(url).json(&body).send().await?;

    // Always attempt to return the JSON body, even for non-2xx responses,
    // so the CLI can surface infra error details directly.
    match resp.json::<Value>().await {
        Ok(v) => Ok(v),
        Err(e) => Err(e),
    }
}

fn build_status_body(access_token: &str, refresh_token: Option<&str>) -> Value {
    let mut credentials = json!({
        "access_token": access_token
    });

    if let Some(rt) = refresh_token {
        credentials["refresh_token"] = json!(rt);
    }

    let mut body = json!({
        "action": "status",
        "credentials": credentials
    });

    if refresh_token.is_some() {
        body["session_policy"] = json!({
            "allow_refresh": true
        });
    }

    super::with_cargo_ai_metadata(body)
}

#[cfg(test)]
fn build_status_body_with_metadata(
    access_token: &str,
    refresh_token: Option<&str>,
    metadata: Option<CargoAiMetadata>,
) -> Value {
    let mut credentials = json!({
        "access_token": access_token
    });

    if let Some(rt) = refresh_token {
        credentials["refresh_token"] = json!(rt);
    }

    let mut body = json!({
        "action": "status",
        "credentials": credentials
    });

    if refresh_token.is_some() {
        body["session_policy"] = json!({
            "allow_refresh": true
        });
    }

    super::with_cargo_ai_metadata_override(body, metadata)
}

#[cfg(test)]
mod tests {
    use super::build_status_body_with_metadata;
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
    fn build_status_body_without_refresh_includes_top_level_metadata() {
        let body =
            build_status_body_with_metadata("access-token-123", None, Some(sample_metadata()));

        assert_eq!(body["action"], "status");
        assert_eq!(body["credentials"]["access_token"], "access-token-123");
        assert!(body.get("session_policy").is_none());
        assert_eq!(body["cargo_ai_metadata"]["cargo_ai_version"], "0.0.10");
    }

    #[test]
    fn build_status_body_with_refresh_includes_metadata_and_policy() {
        let body = build_status_body_with_metadata(
            "access-token-123",
            Some("refresh-token-456"),
            Some(sample_metadata()),
        );

        assert_eq!(body["credentials"]["refresh_token"], "refresh-token-456");
        assert_eq!(body["session_policy"]["allow_refresh"], true);
        assert_eq!(
            body["cargo_ai_metadata"]["cargo_ai_install_id"],
            "install-123"
        );
    }
}
