//! Internal provider/runtime boundaries for the CLI binary.
//!
//! These modules are implementation details for `cargo-ai` command execution.
//! They are intentionally kept out of a public SDK contract.
mod anthropic;
mod compatibility;
mod error;
mod gemini;
mod image;
mod media;
mod ollama;
mod openai;
mod openai_compatible;
pub(crate) mod runtime;
mod typesafe;
mod xai;

pub(crate) use compatibility::validate_provider_compatibility;

pub(crate) use error::{
    provider_error_messages, provider_url_origin, validate_provider_content_parts,
    validate_provider_request, AuthenticationPolicy, ProviderError, ProviderKind,
};
pub(crate) use image::send_image_request;
pub(crate) use media::{
    send_speech_request, send_transcription_request, valid_mp3, ProviderSpeechRequest,
    ProviderTranscriptionRequest,
};
pub(crate) use runtime::{
    load_image_reference, load_image_reference_with_limit,
    resolve_inputs as resolve_provider_inputs, Cargo as AgentCargo, ImageReference,
    ProviderTextRequest, ProviderUsage, ValidatedResponse,
};

pub(crate) async fn send_text_request(
    provider: ProviderKind,
    url: &str,
    request: ProviderTextRequest<'_>,
) -> Result<runtime::ProviderTextResponse, ProviderError> {
    if provider == ProviderKind::TypeSafe {
        return typesafe::send_request(url, request).await;
    }
    let wire_schema = if request.rubric_enabled {
        compatibility::general_provider_schema(request.response_schema)
    } else {
        std::borrow::Cow::Borrowed(request.response_schema)
    };
    let request = ProviderTextRequest {
        response_schema: wire_schema.as_ref(),
        ..request
    };
    if let Some(temperature) = request.temperature {
        if !temperature.is_finite() || temperature < 0.0 {
            return Err(ProviderError::invalid_request(
                provider,
                "temperature must be finite and nonnegative",
            ));
        }
        if !matches!(
            provider.transport(),
            error::ProviderTransport::OpenAiNative | error::ProviderTransport::OpenAiCompatibleChat
        ) {
            return Err(ProviderError::invalid_request(provider, "Explicit profile temperature is unsupported by this transport; clear it with `profile set <name> --clear-temperature`."));
        }
    }
    match provider.transport() {
        error::ProviderTransport::TypeSafeSystemOne => {
            unreachable!("TypeSafe is dispatched before JSON Schema preparation")
        }
        error::ProviderTransport::AnthropicMessages => {
            anthropic::send_request(
                url,
                request.model,
                request.content_parts,
                request.timeout_in_sec,
                request.token,
                request.response_schema,
                request.max_output_tokens,
            )
            .await
        }
        error::ProviderTransport::GeminiInteractions => {
            gemini::send_request(
                url,
                request.model,
                request.content_parts,
                request.timeout_in_sec,
                request.token,
                request.response_schema,
                request.max_output_tokens,
            )
            .await
        }
        error::ProviderTransport::OpenAiCompatibleChat => {
            openai_compatible::send_request(
                provider,
                url,
                request.model,
                request.content_parts,
                request.timeout_in_sec,
                request.token,
                request.response_schema,
                request.max_output_tokens,
                request.temperature,
            )
            .await
        }
        error::ProviderTransport::OpenAiNative => {
            let mut schema = request.response_schema.clone();
            if let Some(object) = schema.as_object_mut() {
                object.insert(
                    "additionalProperties".to_string(),
                    serde_json::Value::Bool(false),
                );
            }
            let response_format = serde_json::json!({
                "type": "json_schema",
                "json_schema": {
                    "name": "Output",
                    "schema": schema,
                    "strict": true
                }
            });
            openai::send_request(
                &url.to_string(),
                &request.model.to_string(),
                request.content_parts,
                request.timeout_in_sec,
                &request.token.to_string(),
                response_format,
                request.temperature,
            )
            .await
        }
        error::ProviderTransport::XaiResponses => {
            xai::send_request(
                url,
                request.model,
                request.content_parts,
                request.timeout_in_sec,
                request.token,
                request.response_schema,
                request.max_output_tokens,
            )
            .await
        }
    }
}

#[cfg(test)]
mod temperature_tests {
    use super::*;
    #[tokio::test]
    async fn temperature_unsupported_transports_fail_before_network() {
        for (provider, url) in [
            (ProviderKind::Anthropic, "http://unused.invalid"),
            (ProviderKind::Gemini, "http://unused.invalid"),
            (ProviderKind::Xai, "http://unused.invalid"),
            (
                ProviderKind::OpenAi,
                "https://chatgpt.com/backend-api/codex",
            ),
        ] {
            let error = send_text_request(
                provider,
                url,
                ProviderTextRequest {
                    model: "example",
                    content_parts: &[],
                    timeout_in_sec: 1,
                    token: "",
                    response_schema: &serde_json::json!({}),
                    max_output_tokens: None,
                    temperature: Some(0.0),
                    rubric_enabled: false,
                },
            )
            .await
            .unwrap_err();
            assert!(error.message().contains("temperature is unsupported"));
        }
    }

    #[tokio::test]
    async fn rubric_wire_adaptation_is_version_gated_for_general_providers() {
        let mut server = mockito::Server::new_async().await;
        let schema = serde_json::json!({"type":"object","properties":{"n":{
            "type":"number","minimum":0,"maximum":100,"description":"Urgency?","rubric":["Routine","Critical"]
        }},"required":["n"],"additionalProperties":false});
        for enabled in [false, true] {
            let expected_schema = if enabled {
                compatibility::general_provider_schema(&schema).into_owned()
            } else {
                schema.clone()
            };
            let mock = server.mock("POST", "/v1/chat/completions")
                .match_body(mockito::Matcher::PartialJson(serde_json::json!({"response_format":{"json_schema":{"schema":expected_schema}}})))
                .with_status(200)
                .with_body(r#"{"choices":[{"message":{"content":"{\"n\":50}"}}]}"#)
                .create_async().await;
            let response = send_text_request(
                ProviderKind::Mistral,
                &format!("{}/v1/chat/completions", server.url()),
                ProviderTextRequest {
                    model: "mistral-test",
                    content_parts: &[],
                    timeout_in_sec: 5,
                    token: "test-token",
                    response_schema: &schema,
                    rubric_enabled: enabled,
                    max_output_tokens: None,
                    temperature: None,
                },
            )
            .await
            .unwrap();
            assert_eq!(response.text, "{\"n\":50}");
            assert_eq!(response.resolved_model, None);
            mock.assert_async().await;
        }
    }
}

#[cfg(test)]
mod usage_retention_tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn every_adapter_retains_usage_when_response_output_is_invalid() {
        let schema = json!({"type":"object","properties":{"answer":{"type":"string","enum":["a","b"],"description":"Choose one"}},"required":["answer"],"additionalProperties":false});
        for provider in [
            ProviderKind::Anthropic,
            ProviderKind::Gemini,
            ProviderKind::Mistral,
            ProviderKind::Ollama,
            ProviderKind::OpenAi,
            ProviderKind::TypeSafe,
            ProviderKind::Xai,
        ] {
            let mut server = mockito::Server::new_async().await;
            let mock = server.mock("POST", "/")
                .with_status(200).with_header("x-request-id", "fixture-request")
                .with_body(json!({"model":"resolved-model","content":[],"steps":[],"output":[],"choices":[],"answers":{},
                    "usage":{"input_tokens":7,"output_tokens":2,"total_tokens":9,"prompt_tokens":7,"completion_tokens":2,"total_input_tokens":7,"total_output_tokens":2}}).to_string())
                .create_async().await;
            let error = send_text_request(
                provider,
                &server.url(),
                ProviderTextRequest {
                    rubric_enabled: true,
                    model: "requested-model",
                    content_parts: &[runtime::ContentPart::Text("fixture".into())],
                    timeout_in_sec: 5,
                    token: "fixture-token",
                    response_schema: &schema,
                    max_output_tokens: None,
                    temperature: None,
                },
            )
            .await
            .expect_err("invalid output must remain a failure");
            mock.assert_async().await;
            assert_eq!(
                error.usage.as_ref().and_then(|usage| usage.input_tokens),
                Some(7),
                "{provider:?}"
            );
            assert_eq!(
                error.usage.as_ref().and_then(|usage| usage.output_tokens),
                Some(2),
                "{provider:?}"
            );
            assert_eq!(
                error.resolved_model.as_deref(),
                Some("resolved-model"),
                "{provider:?}"
            );
            assert_eq!(
                error.provider_request_id.as_deref(),
                Some("fixture-request"),
                "{provider:?}"
            );
        }
    }
}
