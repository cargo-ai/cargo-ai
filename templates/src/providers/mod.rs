//! Internal provider/runtime boundaries for the CLI binary.
//!
//! These modules are implementation details for `cargo-ai` command execution.
//! They are intentionally kept out of a public SDK contract.
mod error;
mod ollama;
mod openai;
mod runtime;

pub(crate) use error::{
    provider_error_messages, validate_provider_content_parts, validate_provider_request,
    ProviderError, ProviderKind,
};
pub(crate) use ollama::send_request as send_ollama_request;
pub(crate) use openai::send_request as send_openai_request;
pub(crate) use runtime::{
    resolve_inputs as resolve_provider_inputs, Cargo as AgentCargo, ValidatedResponse,
};

/// Default temperature used for model requests when not explicitly overridden.
pub(crate) const DEFAULT_TEMPERATURE: f64 = 0.0;
