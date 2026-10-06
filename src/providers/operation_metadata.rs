//! Native connection context identity and passive operation evidence, without inference or state repair.
#[cfg(any(cargo_ai_cli, test))]
use super::{thinking::ThinkingSetting, ProviderKind};
#[cfg(any(cargo_ai_cli, test))]
use reqwest::Url;
#[cfg(any(cargo_ai_cli, test))]
use serde::{Deserialize, Serialize};
use serde_json::json;
#[cfg(any(cargo_ai_cli, test))]
use serde_json::Value;

/// Bind role references and the independently validated fixed-profile snapshot together.
pub(crate) fn connection_context_identity(
    bindings: &crate::role_contract::BindingRevision,
    fixed_identity: &str,
) -> Result<String, String> {
    let references: Vec<_> = bindings
        .bindings
        .iter()
        .map(|binding| {
            json!({"role":binding.role,"profile_uuid":binding.profile_uuid,
            "connection_generation":binding.connection_generation,"provider":binding.provider})
        })
        .collect();
    crate::role_contract::canonical_identity(&json!({"role_connections":references,
        "fixed_connections":fixed_identity}))
    .map_err(|_| "role.context_unavailable".into())
}

/// Snapshot explicitly selected fixed profiles through existing protected context records.
/// Empty structural scopes identify no connections and never open configuration or credentials.
pub(crate) fn fixed_context_identity(
    home: &std::path::Path,
    selections: &[(String, Option<String>)],
) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    const UNAVAILABLE: &str = "role.fixed_context_unavailable";
    let mut selections = selections.to_vec();
    selections.sort();
    if selections.len() > 256
        || selections.windows(2).any(|pair| pair[0].0 == pair[1].0)
        || selections.iter().any(|(id, profile)| {
            id.is_empty()
                || id.len() > 128
                || profile
                    .as_ref()
                    .is_some_and(|name| name.is_empty() || name.len() > 1024)
        })
    {
        return Err(UNAVAILABLE.into());
    }
    let mut references = Vec::new();
    if !selections.is_empty() {
        let (config, _) = crate::credentials::role_context::config_at(home)
            .map_err(|_| UNAVAILABLE.to_owned())?;
        let default = config.get("default_profile").and_then(toml::Value::as_str);
        for (site, requested) in &selections {
            let name = requested.as_deref().or(default).ok_or(UNAVAILABLE)?;
            let validated = crate::credentials::role_context::resolve_named_at(home, name)
                .map_err(|_| UNAVAILABLE.to_owned())?;
            references.push(json!({"call_site":site,"requested_profile":requested,
                "selected_profile":validated.profile_name,"context":validated.context}));
        }
        // Repeat native lookup so a changed default, generation or secret invalidates this snapshot.
        let (after, _) = crate::credentials::role_context::config_at(home)
            .map_err(|_| UNAVAILABLE.to_owned())?;
        let after_default = after.get("default_profile").and_then(toml::Value::as_str);
        for ((site, requested), expected) in selections.iter().zip(&references) {
            let name = requested.as_deref().or(after_default).ok_or(UNAVAILABLE)?;
            let validated = crate::credentials::role_context::resolve_named_at(home, name)
                .map_err(|_| UNAVAILABLE.to_owned())?;
            let actual = json!({"call_site":site,"requested_profile":requested,
                "selected_profile":validated.profile_name,"context":validated.context});
            if actual != *expected {
                return Err(UNAVAILABLE.into());
            }
        }
    }
    let bytes = serde_json::to_vec(&json!({"version":1,"fixed_connections":references}))
        .map_err(|_| UNAVAILABLE.to_owned())?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

#[cfg(any(cargo_ai_cli, test))]
pub(crate) const EVIDENCE_REVISION: &str = "2026-10-06.r1";
#[cfg(any(cargo_ai_cli, test))]
const REVIEWED_AT: i64 = 1_791_244_800;
#[cfg(any(cargo_ai_cli, test))]
const EXPIRES_AT: i64 = REVIEWED_AT + 30 * 24 * 60 * 60;

#[cfg(any(cargo_ai_cli, test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum OperationStatus {
    Compatible,
    Incompatible,
    Unknown,
    Stale,
    Unavailable,
}

#[cfg(any(cargo_ai_cli, test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CatalogPresence {
    Listed,
    Absent,
    Unknown,
}

/// Account metadata can establish only the dimensions supplied by that catalog.
#[cfg(any(cargo_ai_cli, test))]
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AccountMetadataEvidence {
    pub model: String,
    pub input_modalities: Option<Vec<String>>,
    pub reasoning_choices: Option<Vec<String>>,
    pub fetched_at_unix: i64,
    pub expires_at_unix: i64,
}

#[cfg(any(cargo_ai_cli, test))]
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OperationRequest {
    pub provider: String,
    pub model: String,
    pub auth_transport: String,
    pub endpoint: String,
    pub operation: String,
    pub input_modalities: Vec<String>,
    pub structured_output: bool,
    pub settings: Value,
    pub catalog_presence: CatalogPresence,
    pub connection_available: bool,
    pub account_metadata: Option<AccountMetadataEvidence>,
}

#[cfg(any(cargo_ai_cli, test))]
#[derive(Clone, Debug, Serialize)]
pub(crate) struct OperationAssessment {
    pub schema_version: u32,
    pub status: OperationStatus,
    pub reason: String,
    pub provider: String,
    pub model: String,
    pub auth_transport: String,
    pub operation: String,
    pub endpoint_family: Option<String>,
    pub catalog_presence: CatalogPresence,
    pub invocation_access: &'static str,
    pub evidence: Vec<Value>,
    pub constraints: Value,
    pub dimensions: Value,
}

#[cfg(any(cargo_ai_cli, test))]
#[derive(Clone, Copy)]
struct Record {
    model: &'static str,
    operation: &'static str,
    path: &'static str,
    inputs: &'static [&'static str],
    output: &'static str,
    structured: bool,
    thinking: &'static [&'static str],
    formats: &'static [&'static str],
    voices: &'static [&'static str],
    sources: &'static [&'static str],
    adapter: &'static str,
    limitations: &'static [&'static str],
}

#[cfg(any(cargo_ai_cli, test))]
const RECORDS: &[Record] = &[
    Record {
        model: "gpt-image-2",
        operation: "image_generation",
        path: "/v1/images/generations",
        inputs: &["text"],
        output: "image",
        structured: false,
        thinking: &[],
        formats: &["png"],
        voices: &[],
        sources: &[
            "https://developers.openai.com/api/docs/models/gpt-image-2",
            "https://developers.openai.com/api/docs/guides/image-generation",
        ],
        adapter: "openai_images_generation",
        limitations: &[
            "Image references and non-PNG output are not qualified by this record.",
            "The direct Images endpoint has no named thinking request field.",
        ],
    },
    Record {
        model: "gpt-4o-mini-tts",
        operation: "speech_generation",
        path: "/v1/audio/speech",
        inputs: &["text"],
        output: "audio",
        structured: false,
        thinking: &[],
        formats: &["wav"],
        voices: &["coral"],
        sources: &[
            "https://developers.openai.com/api/docs/models/gpt-4o-mini-tts",
            "https://developers.openai.com/api/docs/guides/text-to-speech",
        ],
        adapter: "openai_audio_speech",
        limitations: &[
            "Only coral voice and WAV output are qualified by this record.",
            "The model is documented as deprecated; invocation availability remains unverified.",
            "The speech endpoint has no named thinking request field.",
        ],
    },
    Record {
        model: "gpt-5.2",
        operation: "text_generation",
        path: "/v1/chat/completions",
        inputs: &["text", "image"],
        output: "text",
        structured: true,
        thinking: &["none", "low", "medium", "high", "xhigh"],
        formats: &[],
        voices: &[],
        sources: &["https://developers.openai.com/api/docs/models/gpt-5.2"],
        adapter: "openai_chat_completions_closed_json_schema",
        limitations: &[
            "Structured output requires a schema supported by the existing adapter.",
            "Image input does not establish image output or review accuracy.",
            "Temperature overrides are not qualified by this record.",
        ],
    },
];

#[cfg(any(cargo_ai_cli, test))]
fn assessment(request: &OperationRequest) -> OperationAssessment {
    OperationAssessment {
        schema_version: 1,
        status: OperationStatus::Unknown,
        reason: "No reviewed evidence for this exact model, operation and connection.".into(),
        provider: request.provider.clone(),
        model: request.model.clone(),
        auth_transport: request.auth_transport.clone(),
        operation: request.operation.clone(),
        endpoint_family: None,
        catalog_presence: request.catalog_presence,
        invocation_access: "unverified",
        evidence: Vec::new(),
        constraints: json!({}),
        dimensions: json!({}),
    }
}

#[cfg(any(cargo_ai_cli, test))]
fn finish(
    mut result: OperationAssessment,
    status: OperationStatus,
    reason: &str,
) -> OperationAssessment {
    result.status = status;
    result.reason = reason.into();
    result
}

#[cfg(any(cargo_ai_cli, test))]
fn clean_url(endpoint: &str) -> Option<Url> {
    let url = Url::parse(endpoint).ok()?;
    (url.scheme() == "https"
        && url.port_or_known_default() == Some(443)
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none())
    .then_some(url)
}

#[cfg(any(cargo_ai_cli, test))]
fn qualified_api_url(endpoint: &str, record: &Record) -> bool {
    clean_url(endpoint).is_some_and(|url| {
        url.host_str() == Some("api.openai.com")
            && (url.path() == record.path
                // Existing media adapters derive their operation route from this profile URL.
                || (record.operation != "text_generation" && url.path() == "/v1/chat/completions"))
    })
}

#[cfg(any(cargo_ai_cli, test))]
fn reviewed_evidence(record: &Record, now: i64) -> Value {
    json!({"revision":EVIDENCE_REVISION,"source":"reviewed_provider_documentation",
        "provider":"openai","model":record.model,"auth_transport":"api_key",
        "operation":record.operation,"endpoint_family":format!("https://api.openai.com{}",record.path),
        "sources":record.sources,"reviewed_at":"2026-10-06","reviewed_at_unix":REVIEWED_AT,
        "expires_at_unix":EXPIRES_AT,"freshness":if now < REVIEWED_AT || now >= EXPIRES_AT {"stale"} else {"fresh"},
        "adapter":record.adapter,"qualification":"existing_adapter_operation"})
}

#[cfg(any(cargo_ai_cli, test))]
fn constraints(record: &Record) -> Value {
    json!({"input_modalities":record.inputs,"output_modality":record.output,
        "structured_output":record.structured,"thinking_choices":record.thinking,
        "provider_default_thinking":true,"formats":record.formats,"voices":record.voices,
        "temperature":"unqualified","limitations":record.limitations})
}

#[cfg(any(cargo_ai_cli, test))]
fn selected_thinking(settings: &Value) -> Result<ThinkingSetting, ()> {
    settings
        .get("thinking")
        .map_or(Ok(ThinkingSetting::ProviderDefault), |value| {
            serde_json::from_value(value.clone()).map_err(|_| ())
        })
}

#[cfg(any(cargo_ai_cli, test))]
fn evaluate_account(
    request: &OperationRequest,
    now: i64,
    mut result: OperationAssessment,
) -> OperationAssessment {
    if request.operation == "speech_generation" {
        return finish(
            result,
            OperationStatus::Incompatible,
            "The account transport does not support native speech generation.",
        );
    }
    let qualified = clean_url(&request.endpoint).is_some_and(|url| {
        url.host_str() == Some("chatgpt.com")
            && matches!(
                url.path(),
                "/backend-api/codex" | "/backend-api/codex/responses"
            )
    });
    if !qualified {
        return finish(
            result,
            OperationStatus::Unknown,
            "Account metadata is not qualified for this endpoint.",
        );
    }
    let Some(metadata) = &request.account_metadata else {
        return result;
    };
    if metadata.model != request.model
        || metadata.fetched_at_unix <= 0
        || metadata.expires_at_unix <= metadata.fetched_at_unix
    {
        return finish(
            result,
            OperationStatus::Unknown,
            "Account metadata identity or freshness is invalid.",
        );
    }
    result.endpoint_family = Some("https://chatgpt.com/backend-api/codex/responses".into());
    result.evidence.push(json!({"source":"selected_account_catalog","model":metadata.model,
        "provider":"openai","auth_transport":"openai_account",
        "sources":["https://chatgpt.com/backend-api/codex/models?client_version=0.159.2"],
        "fetched_at_unix":metadata.fetched_at_unix,"expires_at_unix":metadata.expires_at_unix,
        "freshness":if now < metadata.fetched_at_unix || now >= metadata.expires_at_unix {"stale"} else {"fresh"}}));
    if now < metadata.fetched_at_unix || now >= metadata.expires_at_unix {
        return finish(
            result,
            OperationStatus::Stale,
            "Selected account metadata is expired or from the future.",
        );
    }
    if let Some(inputs) = &metadata.input_modalities {
        if inputs.len() > 16
            || inputs
                .iter()
                .any(|value| !matches!(value.as_str(), "text" | "image" | "audio"))
        {
            return finish(
                result,
                OperationStatus::Unknown,
                "Account input-modality metadata is malformed.",
            );
        }
        let supported = request
            .input_modalities
            .iter()
            .all(|value| inputs.contains(value));
        result.dimensions["input_modalities"] =
            json!({"status":if supported {"compatible"} else {"incompatible"},"supported":inputs});
        if !supported {
            return finish(
                result,
                OperationStatus::Incompatible,
                "Selected account metadata excludes a required input modality.",
            );
        }
    }
    if let Some(choices) = &metadata.reasoning_choices {
        if choices.len() > 64
            || choices.iter().any(|value| {
                value.is_empty() || value.len() > 4096 || value.chars().any(char::is_control)
            })
        {
            return finish(
                result,
                OperationStatus::Unknown,
                "Account reasoning metadata is malformed.",
            );
        }
        let supported = match selected_thinking(&request.settings) {
            Ok(ThinkingSetting::ProviderDefault) => true,
            Ok(ThinkingSetting::Choice { value }) => choices.contains(&value),
            _ => false,
        };
        result.dimensions["thinking"] =
            json!({"status":if supported {"compatible"} else {"incompatible"},"choices":choices});
        if !supported {
            return finish(
                result,
                OperationStatus::Incompatible,
                "Selected account metadata does not support the required thinking setting.",
            );
        }
    }
    result.constraints = json!({"input_modalities":metadata.input_modalities,"thinking_choices":metadata.reasoning_choices,
        "output_operation":"unknown","structured_output":"unknown",
        "limitations":["Account catalog input and reasoning facts do not prove image/audio output or structured-output support."]});
    finish(result, OperationStatus::Unknown, "Account metadata establishes supplied input and reasoning dimensions only; output-operation evidence is missing.")
}

/// Catalog visibility and credential availability never establish invocation access.
#[cfg(any(cargo_ai_cli, test))]
pub(crate) fn evaluate(request: &OperationRequest, now_unix: i64) -> OperationAssessment {
    evaluate_with_records(request, now_unix, RECORDS)
}

#[cfg(any(cargo_ai_cli, test))]
fn evaluate_with_records(
    request: &OperationRequest,
    now: i64,
    records: &[Record],
) -> OperationAssessment {
    let mut result = assessment(request);
    if !request.connection_available {
        return finish(
            result,
            OperationStatus::Unavailable,
            "The selected connection is unavailable.",
        );
    }
    if request.catalog_presence == CatalogPresence::Absent {
        return finish(
            result,
            OperationStatus::Unavailable,
            "The selected model is absent from the current complete catalog.",
        );
    }
    if request.provider != request.provider.trim().to_ascii_lowercase() {
        return result;
    }
    let Some(provider) = ProviderKind::from_server_value(&request.provider) else {
        return result;
    };
    if request.operation == "speech_generation"
        && (provider == ProviderKind::Ollama || provider == ProviderKind::Xai)
    {
        return finish(
            result,
            OperationStatus::Incompatible,
            if provider == ProviderKind::Xai {
                "xAI speech is a fixed service without an exact-model selector."
            } else {
                "This adapter does not support native speech generation."
            },
        );
    }
    if request.operation == "text_generation"
        && request
            .input_modalities
            .iter()
            .any(|value| value == "image")
        && matches!(provider, ProviderKind::Mistral | ProviderKind::Xai)
    {
        return finish(
            result,
            OperationStatus::Incompatible,
            "This text adapter rejects image input.",
        );
    }
    if provider != ProviderKind::OpenAi {
        return result;
    }
    if request.auth_transport == "openai_account" {
        return evaluate_account(request, now, result);
    }
    if request.auth_transport != "api_key" {
        return result;
    }
    let Some(record) = records
        .iter()
        .find(|record| record.model == request.model && record.operation == request.operation)
    else {
        return result;
    };
    if !qualified_api_url(&request.endpoint, record) {
        return finish(
            result,
            OperationStatus::Unknown,
            "Reviewed API-key evidence does not qualify this endpoint or transport.",
        );
    }
    result.endpoint_family = Some(format!("https://api.openai.com{}", record.path));
    result.evidence.push(reviewed_evidence(record, now));
    result.constraints = constraints(record);
    if now < REVIEWED_AT || now >= EXPIRES_AT {
        return finish(
            result,
            OperationStatus::Stale,
            "Reviewed operation evidence is expired or from the future.",
        );
    }
    let Some(settings) = request.settings.as_object() else {
        return finish(
            result,
            OperationStatus::Unknown,
            "Operation settings must be an object.",
        );
    };
    if settings.keys().any(|key| {
        !matches!(
            key.as_str(),
            "thinking" | "voice" | "format" | "temperature"
        )
    }) {
        return finish(
            result,
            OperationStatus::Unknown,
            "A required setting has no reviewed operation evidence.",
        );
    }
    if settings.contains_key("temperature") {
        return finish(
            result,
            OperationStatus::Unknown,
            "Temperature overrides are not qualified by this record.",
        );
    }
    let Ok(thinking) = selected_thinking(&request.settings) else {
        return finish(
            result,
            OperationStatus::Incompatible,
            "The thinking setting is malformed.",
        );
    };
    if !match thinking {
        ThinkingSetting::ProviderDefault => true,
        ThinkingSetting::Choice { value } => record.thinking.contains(&value.as_str()),
        ThinkingSetting::On | ThinkingSetting::Off => false,
    } {
        return finish(
            result,
            OperationStatus::Incompatible,
            "The required thinking setting is unsupported by this exact operation.",
        );
    }
    if request.input_modalities.is_empty()
        || request
            .input_modalities
            .iter()
            .any(|input| !record.inputs.contains(&input.as_str()))
    {
        return finish(
            result,
            OperationStatus::Unknown,
            "A required input modality has no reviewed evidence for this operation.",
        );
    }
    if request.structured_output && !record.structured {
        return finish(
            result,
            OperationStatus::Incompatible,
            "This operation does not return structured text.",
        );
    }
    for (key, allowed) in [("format", record.formats), ("voice", record.voices)] {
        if allowed.is_empty() {
            if settings.contains_key(key) {
                return finish(
                    result,
                    OperationStatus::Incompatible,
                    "The required media setting is unsupported by this operation.",
                );
            }
        } else if !settings
            .get(key)
            .and_then(Value::as_str)
            .is_some_and(|value| allowed.contains(&value))
        {
            return finish(
                result,
                OperationStatus::Unknown,
                "A required media setting is missing or outside the qualified choices.",
            );
        }
    }
    result.dimensions = json!({"input_modalities":{"status":"compatible","supported":record.inputs},
        "output_operation":{"status":"compatible","modality":record.output},
        "structured_output":{"status":if request.structured_output {"compatible"} else {"not_required"}},
        "settings":{"status":"compatible","required":request.settings}});
    finish(result, OperationStatus::Compatible, "Fresh reviewed evidence supports this exact operation and its required settings; invocation access is unverified.")
}

/// Publish evidence, without treating a catalog listing as an operation requirement.
#[cfg(any(cargo_ai_cli, test))]
pub(crate) fn evidence_for_connection(
    provider: &str,
    model: &str,
    auth_transport: &str,
    endpoint: &str,
    now_unix: i64,
) -> Value {
    let records: Vec<Value> = RECORDS.iter().filter(|record| provider == "openai" && record.model == model && auth_transport == "api_key" && qualified_api_url(endpoint, record))
        .map(|record| json!({"evidence":reviewed_evidence(record, now_unix),"constraints":constraints(record)})).collect();
    json!({"schema_version":1,"revision":EVIDENCE_REVISION,"records":records,"invocation_access":"unverified"})
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(model: &str, operation: &str, settings: Value) -> OperationRequest {
        OperationRequest {
            provider: "openai".into(),
            model: model.into(),
            auth_transport: "api_key".into(),
            endpoint: "https://api.openai.com/v1/chat/completions".into(),
            operation: operation.into(),
            input_modalities: vec!["text".into()],
            structured_output: false,
            settings,
            catalog_presence: CatalogPresence::Listed,
            connection_available: true,
            account_metadata: None,
        }
    }

    #[test]
    fn production_records_qualify_all_four_exact_positive_operations() {
        let image = request("gpt-image-2", "image_generation", json!({"format":"png"}));
        let speech = request(
            "gpt-4o-mini-tts",
            "speech_generation",
            json!({"voice":"coral","format":"wav","thinking":{"mode":"provider_default"}}),
        );
        let mut text = request(
            "gpt-5.2",
            "text_generation",
            json!({"thinking":{"mode":"choice","value":"high"}}),
        );
        text.structured_output = true;
        let mut review = text.clone();
        review.input_modalities.push("image".into());
        for candidate in [image, speech, text, review] {
            let result = evaluate(&candidate, REVIEWED_AT + 1);
            assert_eq!(
                result.status,
                OperationStatus::Compatible,
                "{}",
                result.reason
            );
            assert_eq!(result.invocation_access, "unverified");
            assert_eq!(result.catalog_presence, CatalogPresence::Listed);
            assert_eq!(result.evidence[0]["revision"], EVIDENCE_REVISION);
            assert_eq!(result.evidence[0]["model"], candidate.model);
            assert_eq!(result.evidence[0]["auth_transport"], "api_key");
            assert_eq!(result.evidence[0]["freshness"], "fresh");
            assert_eq!(result.evidence[0]["expires_at_unix"], EXPIRES_AT);
            assert!(!result.evidence[0]["sources"].as_array().unwrap().is_empty());
            assert_eq!(
                serde_json::to_value(result).unwrap()["status"],
                "compatible"
            );
        }
    }

    #[test]
    fn evidence_removal_expiry_identity_transport_and_endpoint_cannot_preserve_compatible() {
        let candidate = request("gpt-image-2", "image_generation", json!({"format":"png"}));
        assert_eq!(
            evaluate_with_records(&candidate, REVIEWED_AT + 1, &[]).status,
            OperationStatus::Unknown
        );
        for now in [REVIEWED_AT - 1, EXPIRES_AT] {
            assert_eq!(evaluate(&candidate, now).status, OperationStatus::Stale);
        }
        for model in [
            "gpt-image-2-unknown",
            "gpt-image-2.5-sunburst",
            "GPT-IMAGE-2",
        ] {
            let mut changed = candidate.clone();
            changed.model = model.into();
            assert_eq!(
                evaluate(&changed, REVIEWED_AT + 1).status,
                OperationStatus::Unknown
            );
        }
        for endpoint in [
            "https://proxy.example/v1/chat/completions",
            "http://api.openai.com/v1/chat/completions",
            "https://api.openai.com:8443/v1/chat/completions",
            "https://api.openai.com/custom/v1/chat/completions",
            "https://api.openai.com/v1/responses",
            "https://api.openai.com/v1/chat/completions?x=1",
            "https://user:secret@api.openai.com/v1/chat/completions",
        ] {
            let mut changed = candidate.clone();
            changed.endpoint = endpoint.into();
            let result = evaluate(&changed, REVIEWED_AT + 1);
            assert_eq!(result.status, OperationStatus::Unknown);
            assert!(result.evidence.is_empty());
        }
        let mut changed = candidate.clone();
        changed.auth_transport = "openai_account".into();
        assert_ne!(
            evaluate(&changed, REVIEWED_AT + 1).status,
            OperationStatus::Compatible
        );
        changed.auth_transport = "none".into();
        assert_eq!(
            evaluate(&changed, REVIEWED_AT + 1).status,
            OperationStatus::Unknown
        );
    }

    #[test]
    fn required_settings_and_input_output_dimensions_do_not_silently_fallback() {
        for settings in [
            json!({}),
            json!({"voice":"coral"}),
            json!({"voice":"invented","format":"wav"}),
            json!({"voice":"coral","format":"mp3"}),
            json!({"voice":"coral","format":"wav","speed":1}),
        ] {
            assert_eq!(
                evaluate(
                    &request("gpt-4o-mini-tts", "speech_generation", settings),
                    REVIEWED_AT + 1
                )
                .status,
                OperationStatus::Unknown
            );
        }
        for model_op in [
            ("gpt-image-2", "image_generation"),
            ("gpt-4o-mini-tts", "speech_generation"),
        ] {
            let mut candidate = request(
                model_op.0,
                model_op.1,
                json!({"thinking":{"mode":"choice","value":"high"},"format":"png"}),
            );
            assert_eq!(
                evaluate(&candidate, REVIEWED_AT + 1).status,
                OperationStatus::Incompatible
            );
            candidate.settings = json!({"format":"png"});
            candidate.input_modalities.push("image".into());
            assert_eq!(
                evaluate(&candidate, REVIEWED_AT + 1).status,
                OperationStatus::Unknown
            );
        }
        let mut text = request(
            "gpt-5.2",
            "text_generation",
            json!({"thinking":{"mode":"choice","value":"HIGH"}}),
        );
        assert_eq!(
            evaluate(&text, REVIEWED_AT + 1).status,
            OperationStatus::Incompatible
        );
        text.settings = json!({"thinking":{"mode":"on"}});
        assert_eq!(
            evaluate(&text, REVIEWED_AT + 1).status,
            OperationStatus::Incompatible
        );
        text.settings = json!({"temperature":0.0});
        assert_eq!(
            evaluate(&text, REVIEWED_AT + 1).status,
            OperationStatus::Unknown
        );
        text.settings = json!({"thinking":{"mode":"provider_default","value":"high"}});
        assert_eq!(
            evaluate(&text, REVIEWED_AT + 1).status,
            OperationStatus::Incompatible
        );
        text.operation = "image_generation".into();
        text.settings = json!({"format":"png"});
        assert_eq!(
            evaluate(&text, REVIEWED_AT + 1).status,
            OperationStatus::Unknown
        );
    }

    #[test]
    fn catalog_and_connection_availability_remain_distinct_from_capability_and_access() {
        let mut candidate = request("gpt-image-2", "image_generation", json!({"format":"png"}));
        candidate.catalog_presence = CatalogPresence::Unknown;
        assert_eq!(
            evaluate(&candidate, REVIEWED_AT + 1).status,
            OperationStatus::Compatible
        );
        candidate.catalog_presence = CatalogPresence::Absent;
        assert_eq!(
            evaluate(&candidate, REVIEWED_AT + 1).status,
            OperationStatus::Unavailable
        );
        candidate.catalog_presence = CatalogPresence::Listed;
        candidate.connection_available = false;
        let result = evaluate(&candidate, REVIEWED_AT + 1);
        assert_eq!(result.status, OperationStatus::Unavailable);
        assert_eq!(result.invocation_access, "unverified");
        let candidate = request("catalog-only-model", "text_generation", json!({}));
        assert_eq!(
            evaluate(&candidate, REVIEWED_AT + 1).status,
            OperationStatus::Unknown
        );
    }

    #[test]
    fn account_metadata_supplies_only_actual_fresh_dimensions_never_api_output_evidence() {
        let mut candidate = request(
            "gpt-5.2",
            "text_generation",
            json!({"thinking":{"mode":"choice","value":"high"}}),
        );
        candidate.auth_transport = "openai_account".into();
        candidate.endpoint = "https://chatgpt.com/backend-api/codex/responses".into();
        candidate.input_modalities.push("image".into());
        candidate.structured_output = true;
        candidate.account_metadata = Some(AccountMetadataEvidence {
            model: candidate.model.clone(),
            input_modalities: Some(vec!["text".into(), "image".into()]),
            reasoning_choices: Some(vec!["high".into()]),
            fetched_at_unix: REVIEWED_AT,
            expires_at_unix: EXPIRES_AT,
        });
        let result = evaluate(&candidate, REVIEWED_AT + 1);
        assert_eq!(result.status, OperationStatus::Unknown);
        assert_eq!(
            result.dimensions["input_modalities"]["status"],
            "compatible"
        );
        assert_eq!(result.dimensions["thinking"]["status"], "compatible");
        assert_eq!(result.constraints["structured_output"], "unknown");
        assert_eq!(result.invocation_access, "unverified");
        assert_eq!(
            evaluate(&candidate, EXPIRES_AT).status,
            OperationStatus::Stale
        );
        candidate
            .account_metadata
            .as_mut()
            .unwrap()
            .input_modalities = None;
        let result = evaluate(&candidate, REVIEWED_AT + 1);
        assert!(result.dimensions.get("input_modalities").is_none());
        candidate.account_metadata.as_mut().unwrap().model = "other".into();
        assert!(evaluate(&candidate, REVIEWED_AT + 1).evidence.is_empty());
        candidate.operation = "speech_generation".into();
        assert_eq!(
            evaluate(&candidate, REVIEWED_AT + 1).status,
            OperationStatus::Incompatible
        );
    }

    #[test]
    fn existing_adapter_negatives_and_discovery_records_do_not_claim_exact_model_access() {
        for provider in ["mistral", "xai"] {
            let mut candidate = request("gpt-5.2", "text_generation", json!({}));
            candidate.provider = provider.into();
            candidate.input_modalities.push("image".into());
            assert_eq!(
                evaluate(&candidate, REVIEWED_AT + 1).status,
                OperationStatus::Incompatible
            );
        }
        for provider in ["ollama", "xai"] {
            let mut candidate = request(
                "model",
                "speech_generation",
                json!({"voice":"coral","format":"wav"}),
            );
            candidate.provider = provider.into();
            assert_eq!(
                evaluate(&candidate, REVIEWED_AT + 1).status,
                OperationStatus::Incompatible
            );
        }
        let evidence = evidence_for_connection(
            "openai",
            "gpt-image-2",
            "api_key",
            "https://api.openai.com/v1/chat/completions",
            REVIEWED_AT + 1,
        );
        assert_eq!(evidence["records"].as_array().unwrap().len(), 1);
        assert_eq!(evidence["invocation_access"], "unverified");
        for (auth, endpoint) in [
            (
                "openai_account",
                "https://chatgpt.com/backend-api/codex/responses",
            ),
            ("api_key", "https://custom.example/v1/chat/completions"),
        ] {
            assert!(evidence_for_connection(
                "openai",
                "gpt-image-2",
                auth,
                endpoint,
                REVIEWED_AT + 1
            )["records"]
                .as_array()
                .unwrap()
                .is_empty());
        }
    }
}
