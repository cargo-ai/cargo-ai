//! Pure capability assessment shared by authoring checks and effective invocations.

use super::{ProviderError, ProviderKind};
use serde_json::Value;
use std::borrow::Cow;

pub(crate) fn validate_provider_compatibility(
    provider: ProviderKind,
    schema: &Value,
    inputs: &[crate::Input],
    max_output_tokens: Option<u32>,
    temperature: Option<f64>,
    rubric_enabled: bool,
) -> Result<(), ProviderError> {
    if provider != ProviderKind::TypeSafe || is_empty_output(schema) {
        return Ok(());
    }
    super::typesafe::questions(schema, rubric_enabled)?;
    for (index, input) in inputs.iter().enumerate() {
        if !matches!(input.kind, crate::InputKind::Text | crate::InputKind::Url) {
            return Err(ProviderError::invalid_request(provider, format!(
                "$.inputs[{index}].type: Jev supports text and URL-text inputs; select a compatible profile for image or file inputs."
            )));
        }
    }
    validate_typesafe_settings(max_output_tokens, temperature)
}

pub(super) fn is_empty_output(schema: &Value) -> bool {
    schema
        .get("properties")
        .and_then(Value::as_object)
        .is_some_and(|properties| properties.is_empty())
}

pub(super) fn validate_typesafe_settings(
    max_output_tokens: Option<u32>,
    temperature: Option<f64>,
) -> Result<(), ProviderError> {
    if temperature.is_some() {
        return Err(ProviderError::invalid_request(ProviderKind::TypeSafe,
            "$.profile.temperature: Explicit profile temperature is unsupported by TypeSafe; clear it with `profile set <name> --clear-temperature`."));
    }
    if max_output_tokens.is_some() {
        return Err(ProviderError::invalid_request(ProviderKind::TypeSafe,
            "$.profile.max_output_tokens: Explicit max-output-tokens is unsupported by TypeSafe; clear it with `profile set <name> --clear-max-output-tokens`."));
    }
    Ok(())
}

/// Preserve the authored schema locally while expressing rubric semantics in
/// ordinary provider descriptions. Schemas without metadata are borrowed intact.
pub(super) fn general_provider_schema(schema: &Value) -> Cow<'_, Value> {
    let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
        return Cow::Borrowed(schema);
    };
    if !properties
        .values()
        .any(|field| field.get("rubric").is_some())
    {
        return Cow::Borrowed(schema);
    }
    let mut wire_schema = schema.clone();
    for field in wire_schema["properties"]
        .as_object_mut()
        .unwrap()
        .values_mut()
    {
        let Some(field) = field.as_object_mut() else {
            continue;
        };
        let Some(rubric) = field.remove("rubric") else {
            continue;
        };
        let mut description = field
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        description.push_str(&format!(
            "\nScoring rubric in low-to-high order for the inclusive numeric range [{}, {}]. Levels are equally spaced across this range; fractional scores between levels are allowed:",
            field.get("minimum").unwrap_or(&Value::Null), field.get("maximum").unwrap_or(&Value::Null)
        ));
        if let Some(levels) = rubric.as_array() {
            for (index, level) in levels.iter().enumerate() {
                description.push_str(&format!(
                    "\n{}: {}",
                    index + 1,
                    level.as_str().unwrap_or("")
                ));
            }
        }
        field.insert("description".to_string(), Value::String(description));
    }
    Cow::Owned(wire_schema)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn wire_rubric_retains_semantics_and_leaves_plain_schemas_unchanged() {
        let plain =
            json!({"type":"object","properties":{"n":{"type":"number","minimum":0,"maximum":100}}});
        assert!(matches!(general_provider_schema(&plain), Cow::Borrowed(_)));
        let mut authored = plain.clone();
        authored["properties"]["n"]["rubric"] = json!(["Routine", "Critical"]);
        authored["properties"]["n"]["description"] = json!("How urgent?");
        let wire = general_provider_schema(&authored);
        assert!(wire["properties"]["n"].get("rubric").is_none());
        assert!(authored["properties"]["n"].get("rubric").is_some());
        let description = wire["properties"]["n"]["description"].as_str().unwrap();
        for text in [
            "How urgent?",
            "[0, 100]",
            "1: Routine",
            "2: Critical",
            "fractional",
        ] {
            assert!(description.contains(text));
        }
    }

    #[test]
    fn skipped_inference_does_not_certify_inputs_or_settings() {
        let input = crate::Input {
            name: None,
            kind: crate::InputKind::File,
            value: None,
        };
        assert!(validate_provider_compatibility(
            ProviderKind::TypeSafe,
            &json!({"type":"object","properties":{}}),
            &[input],
            Some(10),
            Some(0.2),
            true
        )
        .is_ok());
    }

    #[test]
    fn declared_inputs_and_known_settings_match_effective_invocation_rules() {
        let schema = json!({"type":"object","properties":{"answer":{"type":"string","enum":["yes","no"],"description":"Is it ready?"}}});
        for kind in [crate::InputKind::Text, crate::InputKind::Url] {
            let input = crate::Input {
                name: Some("message".into()),
                kind,
                value: None,
            };
            assert!(validate_provider_compatibility(
                ProviderKind::TypeSafe,
                &schema,
                &[input],
                None,
                None,
                false
            )
            .is_ok());
        }
        for kind in [crate::InputKind::Image, crate::InputKind::File] {
            let input = crate::Input {
                name: Some("message".into()),
                kind,
                value: Some("private-content".into()),
            };
            let error = validate_provider_compatibility(
                ProviderKind::TypeSafe,
                &schema,
                &[input],
                None,
                None,
                true,
            )
            .unwrap_err();
            assert!(error.to_string().contains("$.inputs[0].type"));
            assert!(!error.to_string().contains("private-content"));
        }
        assert!(validate_provider_compatibility(
            ProviderKind::TypeSafe,
            &schema,
            &[],
            Some(1),
            None,
            true
        )
        .is_err());
        assert!(validate_provider_compatibility(
            ProviderKind::TypeSafe,
            &schema,
            &[],
            None,
            Some(0.0),
            true
        )
        .is_err());
        assert!(validate_provider_compatibility(
            ProviderKind::OpenAi,
            &json!({"type":"string"}),
            &[],
            Some(1),
            Some(0.0),
            false
        )
        .is_ok());
    }
}
