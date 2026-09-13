//! Internal provider/runtime boundaries for the CLI binary.
//!
//! These modules are implementation details for `cargo-ai` command execution.
//! They are intentionally kept out of a public SDK contract.
mod anthropic;
mod error;
mod gemini;
mod ollama;
mod openai;
mod openai_compatible;
mod runtime;
mod xai;

pub(crate) use error::{
    provider_error_messages, validate_provider_content_parts, validate_provider_request,
    AuthenticationPolicy, ProviderError, ProviderKind,
};
pub(crate) use ollama::send_image_request as send_ollama_image_request;
pub(crate) use openai::send_image_request as send_openai_image_request;
pub(crate) use runtime::{
    load_image_reference, resolve_inputs as resolve_provider_inputs, Cargo as AgentCargo,
    ImageReference, ProviderTextRequest, ProviderUsage, ValidatedResponse,
};

pub(crate) async fn send_text_request(
    provider: ProviderKind,
    url: &str,
    request: ProviderTextRequest<'_>,
) -> Result<runtime::ProviderTextResponse, ProviderError> {
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
                },
            )
            .await
            .unwrap_err();
            assert!(error.message().contains("temperature is unsupported"));
        }
    }
}
