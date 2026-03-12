#[cfg(test)]
use crate::config::schema::CargoAiMetadata;
use serde_json::{json, Value};

fn build_fetch_preferences_body(access_token: &str) -> Value {
    super::with_cargo_ai_metadata(json!({
        "action": "mail_preferences",
        "credentials": {
            "access_token": access_token
        }
    }))
}

fn build_set_preferences_body(access_token: &str, all_emails_enabled: bool) -> Value {
    super::with_cargo_ai_metadata(json!({
        "action": "mail_preferences",
        "credentials": {
            "access_token": access_token
        },
        "mail_preferences": {
            "set": {
                "all_emails_enabled": all_emails_enabled
            }
        }
    }))
}

#[cfg(test)]
fn build_fetch_preferences_body_with_metadata(
    access_token: &str,
    metadata: Option<CargoAiMetadata>,
) -> Value {
    super::with_cargo_ai_metadata_override(
        json!({
            "action": "mail_preferences",
            "credentials": {
                "access_token": access_token
            }
        }),
        metadata,
    )
}

#[cfg(test)]
fn build_set_preferences_body_with_metadata(
    access_token: &str,
    all_emails_enabled: bool,
    metadata: Option<CargoAiMetadata>,
) -> Value {
    super::with_cargo_ai_metadata_override(
        json!({
            "action": "mail_preferences",
            "credentials": {
                "access_token": access_token
            },
            "mail_preferences": {
                "set": {
                    "all_emails_enabled": all_emails_enabled
                }
            }
        }),
        metadata,
    )
}

/// Fetch current account mail preferences using an access token.
///
/// POST /account
/// {
///   "action": "mail_preferences",
///   "credentials": {
///     "access_token": "<access_token>"
///   }
/// }
///
/// Returns the raw JSON response from the infra API (success or failure).
pub async fn fetch_preferences(
    base_url: &str,
    access_token: &str,
) -> Result<Value, reqwest::Error> {
    let url = format!("{}/account", base_url.trim_end_matches('/'));
    let body = build_fetch_preferences_body(access_token);

    let client = reqwest::Client::new();
    let resp = client.post(url).json(&body).send().await?;

    // Always attempt to return the JSON body even for non-2xx responses,
    // so the CLI can surface infra error details directly.
    match resp.json::<Value>().await {
        Ok(v) => Ok(v),
        Err(e) => Err(e),
    }
}

/// Set account-level mail preference using an access token.
///
/// POST /account
/// {
///   "action": "mail_preferences",
///   "credentials": {
///     "access_token": "<access_token>"
///   },
///   "mail_preferences": {
///     "set": {
///       "all_emails_enabled": <bool>
///     }
///   }
/// }
///
/// Returns the raw JSON response from the infra API (success or failure).
pub async fn set_all_emails_enabled(
    base_url: &str,
    access_token: &str,
    all_emails_enabled: bool,
) -> Result<Value, reqwest::Error> {
    let url = format!("{}/account", base_url.trim_end_matches('/'));
    let body = build_set_preferences_body(access_token, all_emails_enabled);

    let client = reqwest::Client::new();
    let resp = client.post(url).json(&body).send().await?;

    // Always attempt to return the JSON body even for non-2xx responses,
    // so the CLI can surface infra error details directly.
    match resp.json::<Value>().await {
        Ok(v) => Ok(v),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        build_fetch_preferences_body_with_metadata, build_set_preferences_body_with_metadata,
    };
    use crate::config::schema::CargoAiMetadata;
    use serde_json::json;

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
    fn build_fetch_preferences_body_matches_contract() {
        let body =
            build_fetch_preferences_body_with_metadata("access-token-123", Some(sample_metadata()));

        assert_eq!(
            body,
            json!({
                "action": "mail_preferences",
                "credentials": {
                    "access_token": "access-token-123"
                },
                "cargo_ai_metadata": {
                    "cargo_ai_version": env!("CARGO_PKG_VERSION"),
                    "template_schema_version": "2026-03-03.r1",
                    "cargo_ai_build_target": "aarch64-apple-darwin",
                    "cargo_ai_install_id": "install-123",
                    "cargo_ai_binary_sha256": "hash-456"
                }
            })
        );
    }

    #[test]
    fn build_set_preferences_body_matches_contract_for_disable() {
        let body = build_set_preferences_body_with_metadata(
            "access-token-123",
            false,
            Some(sample_metadata()),
        );

        assert_eq!(
            body,
            json!({
                "action": "mail_preferences",
                "credentials": {
                    "access_token": "access-token-123"
                },
                "mail_preferences": {
                    "set": {
                        "all_emails_enabled": false
                    }
                },
                "cargo_ai_metadata": {
                    "cargo_ai_version": env!("CARGO_PKG_VERSION"),
                    "template_schema_version": "2026-03-03.r1",
                    "cargo_ai_build_target": "aarch64-apple-darwin",
                    "cargo_ai_install_id": "install-123",
                    "cargo_ai_binary_sha256": "hash-456"
                }
            })
        );
    }

    #[test]
    fn build_set_preferences_body_matches_contract_for_enable() {
        let body = build_set_preferences_body_with_metadata(
            "access-token-123",
            true,
            Some(sample_metadata()),
        );

        assert_eq!(
            body,
            json!({
                "action": "mail_preferences",
                "credentials": {
                    "access_token": "access-token-123"
                },
                "mail_preferences": {
                    "set": {
                        "all_emails_enabled": true
                    }
                },
                "cargo_ai_metadata": {
                    "cargo_ai_version": env!("CARGO_PKG_VERSION"),
                    "template_schema_version": "2026-03-03.r1",
                    "cargo_ai_build_target": "aarch64-apple-darwin",
                    "cargo_ai_install_id": "install-123",
                    "cargo_ai_binary_sha256": "hash-456"
                }
            })
        );
    }
}
