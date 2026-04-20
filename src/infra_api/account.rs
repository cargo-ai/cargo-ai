// src/infra_api/account.rs
//
// Account operations against cargo-ai-infra.
// Keep this module free of CLI concerns so it can be extracted into a library later.

use crate::cargo_ai_metadata;
use crate::config::schema::CargoAiMetadata;
use serde_json::Value;

pub mod agents;
pub mod confirm;
pub mod handle;
pub mod mail_preferences;
pub mod projects;
pub mod register;
pub mod send_mail;
pub mod status;

pub(crate) fn with_cargo_ai_metadata(body: Value) -> Value {
    with_cargo_ai_metadata_override(body, cargo_ai_metadata::load_request_metadata())
}

pub(crate) fn with_cargo_ai_metadata_override(
    mut body: Value,
    metadata: Option<CargoAiMetadata>,
) -> Value {
    if let Some(metadata) = metadata {
        if let Ok(metadata_value) = serde_json::to_value(metadata) {
            body["cargo_ai_metadata"] = metadata_value;
        }
    }

    body
}

#[cfg(test)]
mod tests {
    use super::{with_cargo_ai_metadata_override, CargoAiMetadata};
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
    fn with_cargo_ai_metadata_override_adds_top_level_metadata_block() {
        let body = with_cargo_ai_metadata_override(
            json!({
                "action": "status"
            }),
            Some(sample_metadata()),
        );

        assert_eq!(
            body,
            json!({
                "action": "status",
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
    fn with_cargo_ai_metadata_override_keeps_body_unchanged_when_metadata_missing() {
        let body = with_cargo_ai_metadata_override(
            json!({
                "action": "status"
            }),
            None,
        );

        assert_eq!(
            body,
            json!({
                "action": "status"
            })
        );
    }
}
