//! Static connection-profile assessment for the existing hatch workflow.
use crate::config::loader::{config_path, find_profile, load_config_from_path, ConfigLoad};
use crate::config::schema::{Config, Profile, ProfileAuthMode};
use crate::providers::{validate_provider_compatibility, ProviderKind};
use crate::runtime_definition::RuntimeAgentDefinition;

fn selected_profile<'a>(config: &'a Config, name: &str) -> Result<&'a Profile, String> {
    find_profile(config, name).ok_or_else(|| {
        format!("Compatibility profile '{name}' not found. Select an existing profile; no default profile was substituted.")
    })
}

fn assess(definition: &RuntimeAgentDefinition, profile: &Profile) -> Result<String, String> {
    let provider = ProviderKind::from_server_value(&profile.server)
        .ok_or_else(|| "Compatibility profile has an unsupported server. Review `cargo ai profile show <name>`.".to_string())?;
    if !definition.has_output_schema_properties() {
        return Ok("Provider compatibility: root inference is skipped (action-only definition). Provider-backed child and image steps are checked when invoked.".to_string());
    }
    if profile.model.trim().is_empty() {
        return Err("Compatibility profile has no model. Set it with `cargo ai profile set <name> --model <model>`.".to_string());
    }
    let url = profile
        .url
        .as_deref()
        .filter(|url| !url.is_empty())
        .unwrap_or_else(|| provider.default_url());
    let parsed = reqwest::Url::parse(url).map_err(|_| {
        "Compatibility profile has an invalid provider URL. Review its URL setting.".to_string()
    })?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        return Err(
            "Compatibility profile requires an absolute HTTP or HTTPS provider URL.".to_string(),
        );
    }
    if provider == ProviderKind::TypeSafe && profile.auth_mode != ProfileAuthMode::ApiKey {
        return Err("Jev requires an API-key profile. Use `cargo ai profile set <name> --auth api_key`; this check does not verify or load the key.".to_string());
    }
    let schema = definition.json_schema_value();
    let inputs = definition.named_inputs();
    validate_provider_compatibility(
        provider,
        &schema,
        &inputs,
        profile.max_output_tokens,
        profile.temperature,
        definition.rubric_enabled(),
    )
    .map_err(|error| error.message().to_string())?;

    if provider != ProviderKind::TypeSafe {
        let capabilities = provider.capabilities();
        for (index, input) in inputs.iter().enumerate() {
            let unsupported = match input.kind {
                crate::InputKind::Image => !capabilities.supports_image_input,
                crate::InputKind::File => !capabilities.supports_file_input,
                _ => false,
            };
            if unsupported {
                return Err(format!("$.inputs[{index}].type: {} does not support this input kind. Select a compatible profile or intentionally supply supported input.", provider.display_name()));
            }
        }
    }
    let claim = if provider == ProviderKind::TypeSafe {
        "Jev compatibility: passed for declared schema and input types"
    } else {
        "Provider capability check: passed for declared input types"
    };
    let settings = if provider == ProviderKind::TypeSafe {
        "Known profile settings checked."
    } else {
        "Provider-specific schema and settings support remains runtime-dependent."
    };
    Ok(format!(
        "{claim} (profile: {}, model: {}).\n{settings} Credentials, model availability, token fit, fetched content and dynamic child steps remain runtime-dependent. The runtime profile is not pinned.",
        profile.name, profile.model
    ))
}

pub(crate) fn check(
    file_contents: &str,
    profile_name: Option<&str>,
) -> Result<Option<String>, String> {
    let Some(profile_name) = profile_name else {
        return Ok(None);
    };
    let definition = RuntimeAgentDefinition::from_str(file_contents)?;
    let loaded = match load_config_from_path(&config_path()).map_err(|error| error.to_string())? {
        ConfigLoad::Missing => return Err(
            "No connection profiles found. Add a profile or omit the hatch compatibility target."
                .to_string(),
        ),
        ConfigLoad::Loaded(loaded) => loaded,
    };
    // Metadata selection deliberately never calls credential-store or auth-refresh APIs.
    let profile = selected_profile(loaded.config(), profile_name)?;
    assess(&definition, profile).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn definition(schema: serde_json::Value) -> RuntimeAgentDefinition {
        RuntimeAgentDefinition::from_str(
            &json!({
                "agent_definition_schema_version": "2026-09-19.r1",
                "inputs": [{"type":"text", "text":"A billing question"}],
                "agent_schema": {"type":"object", "properties":schema},
                "actions": []
            })
            .to_string(),
        )
        .unwrap()
    }

    fn profile() -> Profile {
        serde_json::from_value(
            json!({"name":"jev", "server":"typesafe", "model":"jev-1.13.0", "auth_mode":"api_key"}),
        )
        .unwrap()
    }

    #[test]
    fn no_target_keeps_generic_hatch_without_profile_or_definition_reads() {
        assert_eq!(
            check("not parsed by optional compatibility assessment", None).unwrap(),
            None
        );
    }

    #[test]
    fn explicit_target_never_falls_back_to_default() {
        let config: Config =
            serde_json::from_value(json!({"profile":[profile()], "default_profile":"jev"}))
                .unwrap();
        assert!(selected_profile(&config, "missing")
            .unwrap_err()
            .contains("no default profile was substituted"));
        assert_eq!(selected_profile(&config, "jev").unwrap().name, "jev");
    }

    #[test]
    fn compatible_profile_needs_no_saved_token_and_reports_scoped_success() {
        let definition = definition(
            json!({"department":{"type":"string", "description":"Which department?", "enum":["billing","sales"]}}),
        );
        let profile = profile();
        assert!(profile.token.is_none());
        let message = assess(&definition, &profile).unwrap();
        assert!(message.contains("Jev compatibility: passed"));
        assert!(message.contains("runtime-dependent"));
        assert!(message.contains("not pinned"));
    }

    #[test]
    fn profile_settings_and_schema_fail_before_compilation() {
        let definition =
            definition(json!({"urgency":{"type":"number", "minimum":0, "maximum":100}}));
        assert!(assess(&definition, &profile())
            .unwrap_err()
            .contains("urgency"));
        let mut profile = profile();
        profile.auth_mode = ProfileAuthMode::None;
        assert!(assess(&definition, &profile)
            .unwrap_err()
            .contains("API-key profile"));
    }
}
