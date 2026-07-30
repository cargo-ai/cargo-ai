//! Internal provider/runtime boundaries for the CLI binary.
//!
//! These modules are implementation details for `cargo-ai` command execution.
//! They are intentionally kept out of a public SDK contract.
mod anthropic;
mod error;
mod ollama;
mod openai;
mod runtime;

pub(crate) use error::{
    provider_error_messages, validate_provider_content_parts, validate_provider_request,
    ProviderError, ProviderKind,
};
pub(crate) use ollama::send_image_request as send_ollama_image_request;
pub(crate) use openai::send_image_request as send_openai_image_request;
pub(crate) use runtime::{
    load_image_reference, resolve_inputs as resolve_provider_inputs, Cargo as AgentCargo,
    ImageReference, ProviderTextRequest, ProviderUsage, ValidatedResponse,
};

/// Default temperature used for model requests when not explicitly overridden.
pub(crate) const DEFAULT_TEMPERATURE: f64 = 0.0;

pub(crate) async fn send_text_request(
    provider: ProviderKind,
    url: &str,
    request: ProviderTextRequest<'_>,
) -> Result<runtime::ProviderTextResponse, ProviderError> {
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
        error::ProviderTransport::OllamaOpenAiCompatible => {
            ollama::send_request(
                &url.to_string(),
                &request.model.to_string(),
                request.content_parts,
                request.timeout_in_sec,
                request.response_schema.clone(),
            )
            .await
        }
        error::ProviderTransport::OpenAiNative => {
            let response_format = serde_json::json!({
                "type": "json_schema",
                "json_schema": {
                    "name": "Output",
                    "schema": request.response_schema,
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
            )
            .await
        }
    }
}
