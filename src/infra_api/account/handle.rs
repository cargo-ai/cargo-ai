#[cfg(test)]
use crate::config::schema::CargoAiMetadata;
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
    let body = build_fetch_handle_body(access_token);

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
    let body = build_set_handle_body(access_token, new_handle);

    let client = reqwest::Client::new();
    let resp = client.post(url).json(&body).send().await?;

    // Always attempt to return the JSON body even for non-2xx responses,
    // so the CLI can surface infra error details directly.
    match resp.json::<Value>().await {
        Ok(v) => Ok(v),
        Err(e) => Err(e),
    }
}

fn build_fetch_handle_body(access_token: &str) -> Value {
    super::with_cargo_ai_metadata(json!({
        "action": "handle",
        "credentials": {
            "access_token": access_token
        }
    }))
}

#[cfg(test)]
fn build_fetch_handle_body_with_metadata(
    access_token: &str,
    metadata: Option<CargoAiMetadata>,
) -> Value {
    super::with_cargo_ai_metadata_override(
        json!({
            "action": "handle",
            "credentials": {
                "access_token": access_token
            }
        }),
        metadata,
    )
}

fn build_set_handle_body(access_token: &str, new_handle: &str) -> Value {
    super::with_cargo_ai_metadata(json!({
        "action": "handle",
        "credentials": {
            "access_token": access_token
        },
        "handle": {
            "set": new_handle
        }
    }))
}

#[cfg(test)]
fn build_set_handle_body_with_metadata(
    access_token: &str,
    new_handle: &str,
    metadata: Option<CargoAiMetadata>,
) -> Value {
    super::with_cargo_ai_metadata_override(
        json!({
            "action": "handle",
            "credentials": {
                "access_token": access_token
            },
            "handle": {
                "set": new_handle
            }
        }),
        metadata,
    )
}

#[cfg(test)]
mod tests {
    use super::{build_fetch_handle_body_with_metadata, build_set_handle_body_with_metadata};
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
    fn build_fetch_handle_body_includes_top_level_metadata() {
        let body =
            build_fetch_handle_body_with_metadata("access-token-123", Some(sample_metadata()));

        assert_eq!(body["action"], "handle");
        assert_eq!(body["credentials"]["access_token"], "access-token-123");
        assert_eq!(
            body["cargo_ai_metadata"]["cargo_ai_install_id"],
            "install-123"
        );
    }

    #[test]
    fn build_set_handle_body_includes_top_level_metadata() {
        let body = build_set_handle_body_with_metadata(
            "access-token-123",
            "new_handle",
            Some(sample_metadata()),
        );

        assert_eq!(body["action"], "handle");
        assert_eq!(body["handle"]["set"], "new_handle");
        assert_eq!(
            body["cargo_ai_metadata"]["cargo_ai_binary_sha256"],
            "hash-456"
        );
    }
}
