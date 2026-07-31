//! Shared provider error taxonomy and user-facing diagnostics policy.

use super::runtime::ContentPart;
use reqwest::StatusCode;
use std::fmt;

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum ProviderKind {
    Anthropic,
    Gemini,
    Mistral,
    Ollama,
    OpenAi,
    Xai,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum ProviderTransport {
    AnthropicMessages,
    GeminiInteractions,
    OpenAiCompatibleChat,
    OpenAiNative,
    XaiResponses,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum AuthenticationPolicy {
    #[allow(dead_code)]
    None,
    OptionalApiKey,
    RequiredApiKey,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) struct ProviderCapabilities {
    pub(crate) authentication: AuthenticationPolicy,
    pub(crate) supports_image_input: bool,
    pub(crate) supports_file_input: bool,
    pub(crate) supports_generate_image: bool,
}

impl ProviderKind {
    pub(crate) fn from_server_value(server: &str) -> Option<Self> {
        match server.trim().to_ascii_lowercase().as_str() {
            "anthropic" => Some(Self::Anthropic),
            "gemini" => Some(Self::Gemini),
            "mistral" => Some(Self::Mistral),
            "ollama" => Some(Self::Ollama),
            "openai" => Some(Self::OpenAi),
            "xai" => Some(Self::Xai),
            _ => None,
        }
    }

    pub(crate) fn display_name(self) -> &'static str {
        match self {
            Self::Anthropic => "Anthropic",
            Self::Gemini => "Google Gemini",
            Self::Mistral => "Mistral API",
            Self::Ollama => "Ollama",
            Self::OpenAi => "OpenAI",
            Self::Xai => "xAI",
        }
    }

    pub(crate) fn default_url(self) -> &'static str {
        match self {
            Self::Anthropic => "https://api.anthropic.com/v1/messages",
            Self::Gemini => "https://generativelanguage.googleapis.com/v1beta/interactions",
            Self::Mistral => "https://api.mistral.ai/v1/chat/completions",
            Self::Ollama => "http://localhost:11434/v1/chat/completions",
            Self::OpenAi => "https://api.openai.com/v1/chat/completions",
            Self::Xai => "https://api.x.ai/v1/responses",
        }
    }

    pub(crate) fn transport(self) -> ProviderTransport {
        match self {
            Self::Anthropic => ProviderTransport::AnthropicMessages,
            Self::Gemini => ProviderTransport::GeminiInteractions,
            Self::Mistral | Self::Ollama => ProviderTransport::OpenAiCompatibleChat,
            Self::OpenAi => ProviderTransport::OpenAiNative,
            Self::Xai => ProviderTransport::XaiResponses,
        }
    }

    pub(crate) fn capabilities(self) -> ProviderCapabilities {
        match self {
            Self::Anthropic => ProviderCapabilities {
                authentication: AuthenticationPolicy::RequiredApiKey,
                supports_image_input: true,
                supports_file_input: false,
                supports_generate_image: false,
            },
            Self::Gemini => ProviderCapabilities {
                authentication: AuthenticationPolicy::RequiredApiKey,
                supports_image_input: true,
                supports_file_input: false,
                supports_generate_image: false,
            },
            Self::Mistral => ProviderCapabilities {
                authentication: AuthenticationPolicy::RequiredApiKey,
                supports_image_input: false,
                supports_file_input: false,
                supports_generate_image: false,
            },
            Self::Ollama => ProviderCapabilities {
                authentication: AuthenticationPolicy::OptionalApiKey,
                supports_image_input: true,
                supports_file_input: true,
                supports_generate_image: true,
            },
            Self::OpenAi => ProviderCapabilities {
                authentication: AuthenticationPolicy::RequiredApiKey,
                supports_image_input: true,
                supports_file_input: true,
                supports_generate_image: true,
            },
            Self::Xai => ProviderCapabilities {
                authentication: AuthenticationPolicy::RequiredApiKey,
                supports_image_input: false,
                supports_file_input: false,
                supports_generate_image: false,
            },
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum ProviderErrorKind {
    ModelNotFound,
    Unauthorized,
    RateLimited,
    Timeout,
    Connectivity,
    InvalidRequest,
    InvalidResponse,
    Unknown,
}

#[derive(Debug, Clone)]
pub(crate) struct ProviderError {
    provider: ProviderKind,
    kind: ProviderErrorKind,
    message: String,
}

impl ProviderError {
    pub(crate) fn from_reqwest(provider: ProviderKind, error: reqwest::Error) -> Self {
        let kind = if error.is_timeout() {
            ProviderErrorKind::Timeout
        } else if error.is_connect() {
            ProviderErrorKind::Connectivity
        } else if error.is_request() {
            ProviderErrorKind::InvalidRequest
        } else if error.is_decode() {
            ProviderErrorKind::InvalidResponse
        } else {
            ProviderErrorKind::Unknown
        };

        Self {
            provider,
            kind,
            message: format!("Request failed: {error}"),
        }
    }

    pub(crate) fn from_http_status(provider: ProviderKind, status: StatusCode, body: &str) -> Self {
        Self {
            provider,
            kind: classify_http_status(status, body),
            message: format!("HTTP error {status}: {body}"),
        }
    }

    pub(crate) fn invalid_response(provider: ProviderKind, message: impl Into<String>) -> Self {
        Self {
            provider,
            kind: ProviderErrorKind::InvalidResponse,
            message: message.into(),
        }
    }

    pub(crate) fn invalid_request(provider: ProviderKind, message: impl Into<String>) -> Self {
        Self {
            provider,
            kind: ProviderErrorKind::InvalidRequest,
            message: message.into(),
        }
    }

    pub(crate) fn provider(&self) -> ProviderKind {
        self.provider
    }

    pub(crate) fn kind(&self) -> ProviderErrorKind {
        self.kind
    }

    pub(crate) fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for ProviderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for ProviderError {}

fn classify_http_status(status: StatusCode, body: &str) -> ProviderErrorKind {
    let normalized_body = body.to_ascii_lowercase();
    let is_model_not_found = normalized_body.contains("model")
        && (normalized_body.contains("not found") || normalized_body.contains("does not exist"));

    if status == StatusCode::NOT_FOUND && is_model_not_found {
        return ProviderErrorKind::ModelNotFound;
    }

    match status {
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => ProviderErrorKind::Unauthorized,
        StatusCode::TOO_MANY_REQUESTS => ProviderErrorKind::RateLimited,
        StatusCode::BAD_REQUEST => ProviderErrorKind::InvalidRequest,
        _ => ProviderErrorKind::Unknown,
    }
}

pub(crate) fn sanitized_http_error_body(provider: ProviderKind, body: &[u8]) -> String {
    let parsed = serde_json::from_slice::<serde_json::Value>(body).ok();
    let message = parsed.as_ref().and_then(|value| {
        value
            .pointer("/error/message")
            .and_then(serde_json::Value::as_str)
            .or_else(|| value.get("message").and_then(serde_json::Value::as_str))
            .or_else(|| value.get("detail").and_then(serde_json::Value::as_str))
            .or_else(|| value.get("error").and_then(serde_json::Value::as_str))
    });

    let Some(message) = message.map(str::trim).filter(|message| !message.is_empty()) else {
        return format!("{} returned an HTTP error response.", provider.display_name());
    };

    const MAX_MESSAGE_CHARS: usize = 1_000;
    if message.chars().count() <= MAX_MESSAGE_CHARS {
        message.to_string()
    } else {
        let mut truncated = message.chars().take(MAX_MESSAGE_CHARS).collect::<String>();
        truncated.push_str("...");
        truncated
    }
}

fn provider_hint(
    kind: ProviderErrorKind,
    provider: ProviderKind,
    message: &str,
) -> Option<&'static str> {
    match kind {
        ProviderErrorKind::ModelNotFound => match provider {
            ProviderKind::Anthropic => {
                Some("Verify the Claude model name and confirm your Anthropic Console organization has access to it.")
            }
            ProviderKind::Gemini => {
                Some("Verify the Gemini model name and confirm your Google AI project has access to it.")
            }
            ProviderKind::Mistral => {
                Some("Verify the Mistral model name and confirm your Mistral workspace has access to it.")
            }
            ProviderKind::Ollama => Some(
                "Run `ollama list` to inspect installed models, then `ollama pull <model>` for missing models.",
            ),
            ProviderKind::OpenAi => {
                Some("Verify the model name and confirm your account has access to it.")
            }
            ProviderKind::Xai => {
                Some("Verify the Grok model name and confirm your xAI team and API key have access to it.")
            }
        },
        ProviderErrorKind::Unauthorized => match provider {
            ProviderKind::Anthropic => Some(
                "Verify your Anthropic API key (`--token` or profile token), Console API credits, and model access. Claude.ai subscriptions do not include API usage.",
            ),
            ProviderKind::Gemini => Some(
                "Verify your Gemini API key (`--token` or profile token), Google AI project, billing or quota, and model access.",
            ),
            ProviderKind::Mistral => Some(
                "Verify your Mistral API key (`--token` or profile token), activated API payments, and model access.",
            ),
            ProviderKind::OpenAi => {
                Some("Verify your OpenAI token (`--token` or profile token), or re-run `cargo ai auth login openai`, and confirm model access.")
            }
            ProviderKind::Ollama => Some(
                "Verify your Ollama endpoint and credentials (if your deployment requires auth).",
            ),
            ProviderKind::Xai => Some(
                "Verify your xAI API key (`--token` or profile token), key ACLs, credits, and Grok model access.",
            ),
        },
        ProviderErrorKind::RateLimited => match provider {
            ProviderKind::Anthropic => Some(
                "Anthropic rate limit reached; retry later or review your Console usage limits.",
            ),
            ProviderKind::Gemini => Some(
                "Gemini rate limit reached; retry later or review your Google AI project quota and billing.",
            ),
            ProviderKind::Mistral => Some(
                "Mistral rate limit reached; retry later or review your workspace limits and billing.",
            ),
            ProviderKind::OpenAi => {
                Some("OpenAI rate limit reached; retry later or adjust your account/model limits.")
            }
            ProviderKind::Ollama => Some(
                "Ollama appears rate-limited; retry shortly or reduce concurrent local requests.",
            ),
            ProviderKind::Xai => Some(
                "xAI rate limit reached; retry later or review your team limits and credits.",
            ),
        },
        ProviderErrorKind::Connectivity => match provider {
            ProviderKind::Anthropic => Some(
                "Check network connectivity and ensure the configured Anthropic Messages URL is reachable.",
            ),
            ProviderKind::Gemini => Some(
                "Check network connectivity and ensure the configured Gemini Interactions URL is reachable.",
            ),
            ProviderKind::Mistral => Some(
                "Check network connectivity and ensure the configured Mistral Chat Completions URL is reachable.",
            ),
            ProviderKind::Ollama => {
                Some("Ensure Ollama is running (`ollama serve`) and the configured URL is reachable.")
            }
            ProviderKind::OpenAi => Some(
                "Check network connectivity and ensure the configured OpenAI URL is reachable.",
            ),
            ProviderKind::Xai => Some(
                "Check network connectivity and ensure the configured xAI Responses URL is reachable.",
            ),
        },
        ProviderErrorKind::Timeout => match provider {
            ProviderKind::Anthropic => Some(
                "Request timed out; retry later or increase `--inference-timeout-in-sec`.",
            ),
            ProviderKind::Gemini => Some(
                "Request timed out; retry later or increase `--inference-timeout-in-sec`.",
            ),
            ProviderKind::Mistral => Some(
                "Request timed out; retry later or increase `--inference-timeout-in-sec`.",
            ),
            ProviderKind::Ollama => {
                Some("Request timed out; ensure Ollama/model is responsive or increase `--inference-timeout-in-sec`.")
            }
            ProviderKind::OpenAi => {
                Some("Request timed out; retry later or increase `--inference-timeout-in-sec`.")
            }
            ProviderKind::Xai => {
                Some("Request timed out; retry later or increase `--inference-timeout-in-sec`.")
            }
        },
        ProviderErrorKind::InvalidRequest => {
            let normalized_message = message.to_ascii_lowercase();
            if normalized_message.contains("file")
                || normalized_message.contains("pdf")
                || normalized_message.contains("docx")
                || normalized_message.contains("csv")
            {
                Some(
                    "The selected provider/model rejected the supplied file input. Verify that the model and endpoint support the current file type, or retry without `file` / `--input-file`.",
                )
            } else {
                Some("Check `--model`, `--url`, and request parameters for invalid values.")
            }
        }
        ProviderErrorKind::InvalidResponse => {
            Some("The provider returned an unexpected response shape; verify model and endpoint compatibility.")
        }
        ProviderErrorKind::Unknown => None,
    }
}

pub(crate) fn provider_error_messages(error: &ProviderError) -> Vec<String> {
    let mut messages = vec![
        format!(
            "❌ Issue communicating with the AI server ({}).",
            error.provider().display_name()
        ),
        format!("Reason: {}", error.message()),
    ];

    if let Some(hint) = provider_hint(error.kind(), error.provider(), error.message()) {
        messages.push(format!("Hint: {hint}"));
    }

    messages
}

pub(crate) fn validate_provider_request(
    provider: ProviderKind,
    model: &str,
    url: &str,
    token: &str,
) -> Result<(), Vec<String>> {
    let mut issues = Vec::new();

    if model.trim().is_empty() {
        issues.push("❌ Missing model. Provide `--model <name>` or configure a default profile with a model.".to_string());
    }

    if url.trim().is_empty() {
        issues.push(format!(
            "❌ Missing URL for {} server.",
            provider.display_name()
        ));
    } else if !(url.starts_with("http://") || url.starts_with("https://")) {
        issues.push(format!(
            "❌ Invalid URL '{}'. Use an absolute URL beginning with `http://` or `https://`.",
            url
        ));
    }

    if provider.capabilities().authentication == AuthenticationPolicy::RequiredApiKey
        && token.trim().is_empty()
    {
        issues.push(match provider {
            ProviderKind::Anthropic => "❌ Missing Anthropic API key. Provide `--token <TOKEN>` or configure `cargo ai profile set <name> --token <TOKEN> --auth api_key`. Claude.ai subscriptions do not provide API credentials.".to_string(),
            ProviderKind::Gemini => "❌ Missing Gemini API key. Provide `--token <TOKEN>` or configure `cargo ai profile set <name> --token <TOKEN> --auth api_key`.".to_string(),
            ProviderKind::Mistral => "❌ Missing Mistral API key. Provide `--token <TOKEN>` or configure `cargo ai profile set <name> --token <TOKEN> --auth api_key`.".to_string(),
            ProviderKind::OpenAi => "❌ Missing OpenAI token. Provide `--token <TOKEN>`, run `cargo ai auth login openai`, or configure `cargo ai profile set <name> --token <TOKEN> --auth api_key`.".to_string(),
            ProviderKind::Xai => "❌ Missing xAI API key. Provide `--token <TOKEN>` or configure `cargo ai profile set <name> --token <TOKEN> --auth api_key`.".to_string(),
            ProviderKind::Ollama => unreachable!("Ollama accepts an optional API key"),
        });
    }

    if issues.is_empty() {
        Ok(())
    } else {
        Err(issues)
    }
}

pub(crate) fn validate_provider_content_parts(
    provider: ProviderKind,
    url: &str,
    content_parts: &[ContentPart],
) -> Result<(), Vec<String>> {
    let includes_images = content_parts
        .iter()
        .any(|part| matches!(part, ContentPart::Image { .. }));
    let includes_files = content_parts
        .iter()
        .any(|part| matches!(part, ContentPart::File { .. }));

    if !includes_images && !includes_files {
        return Ok(());
    }

    let normalized_url = url.trim().to_ascii_lowercase();
    let mut issues = Vec::new();

    if includes_images && !provider.capabilities().supports_image_input {
        issues.push(format!(
            "❌ Image inputs are not supported by the {} adapter.",
            provider.display_name()
        ));
    }
    if includes_files && !provider.capabilities().supports_file_input {
        issues.push(format!(
            "❌ File inputs are not supported by the {} adapter. Use text, URL-text, or a supported image input.",
            provider.display_name()
        ));
    }

    if provider == ProviderKind::Ollama
        && (normalized_url.contains("/api/generate") || normalized_url.contains("/api/chat"))
    {
        if includes_images {
            issues.push(
                "❌ Image inputs require Ollama's OpenAI-compatible `/v1/chat/completions` transport. Update `--url` or your profile URL before retrying."
                    .to_string(),
            );
        }
        if includes_files {
            issues.push(
                "❌ File inputs require a transport that accepts OpenAI-style file content parts. Ollama `/api/generate` and `/api/chat` are not compatible with `file` / `--input-file`."
                    .to_string(),
            );
        }
    }

    if issues.is_empty() {
        Ok(())
    } else {
        Err(issues)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        provider_error_messages, sanitized_http_error_body, validate_provider_content_parts,
        validate_provider_request, ProviderError, ProviderKind,
    };
    use crate::providers::runtime::ContentPart;
    use reqwest::StatusCode;
    use tokio::net::TcpListener;

    #[test]
    fn parses_provider_kind_from_server_value() {
        assert_eq!(
            ProviderKind::from_server_value("Anthropic"),
            Some(ProviderKind::Anthropic)
        );
        assert_eq!(
            ProviderKind::from_server_value("ollama"),
            Some(ProviderKind::Ollama)
        );
        assert_eq!(
            ProviderKind::from_server_value("Gemini"),
            Some(ProviderKind::Gemini)
        );
        assert_eq!(
            ProviderKind::from_server_value("OPENAI"),
            Some(ProviderKind::OpenAi)
        );
        assert_eq!(
            ProviderKind::from_server_value("mistral"),
            Some(ProviderKind::Mistral)
        );
        assert_eq!(
            ProviderKind::from_server_value("XAI"),
            Some(ProviderKind::Xai)
        );
        assert_eq!(ProviderKind::from_server_value("wat"), None);
    }

    #[test]
    fn provider_identity_is_distinct_from_transport_and_capabilities() {
        assert_eq!(
            ProviderKind::Anthropic.transport(),
            super::ProviderTransport::AnthropicMessages
        );
        assert_eq!(
            ProviderKind::Ollama.transport(),
            super::ProviderTransport::OpenAiCompatibleChat
        );
        assert_eq!(
            ProviderKind::Mistral.transport(),
            super::ProviderTransport::OpenAiCompatibleChat
        );
        assert_eq!(
            ProviderKind::Xai.transport(),
            super::ProviderTransport::XaiResponses
        );
        assert_eq!(
            ProviderKind::Gemini.transport(),
            super::ProviderTransport::GeminiInteractions
        );
        assert!(
            !ProviderKind::Anthropic
                .capabilities()
                .supports_generate_image
        );
        assert!(ProviderKind::Anthropic.capabilities().supports_image_input);
        assert!(!ProviderKind::Anthropic.capabilities().supports_file_input);
        assert!(ProviderKind::Gemini.capabilities().supports_image_input);
        assert!(!ProviderKind::Gemini.capabilities().supports_file_input);
        assert!(!ProviderKind::Gemini.capabilities().supports_generate_image);
        assert_eq!(
            ProviderKind::Mistral.capabilities().authentication,
            super::AuthenticationPolicy::RequiredApiKey
        );
        assert_eq!(
            ProviderKind::Ollama.capabilities().authentication,
            super::AuthenticationPolicy::OptionalApiKey
        );
        assert!(!ProviderKind::Xai.capabilities().supports_image_input);
    }

    #[test]
    fn classifies_model_not_found_from_http_status() {
        let error = ProviderError::from_http_status(
            ProviderKind::Ollama,
            StatusCode::NOT_FOUND,
            "{\"error\":\"model 'mixtral' not found\"}",
        );
        let messages = provider_error_messages(&error);
        assert!(messages
            .iter()
            .any(|line| line.contains("Issue communicating with the AI server (Ollama)")));
        assert!(messages
            .iter()
            .any(|line| line.contains("ollama pull <model>")));
    }

    #[test]
    fn classifies_unauthorized_with_openai_hint() {
        let error = ProviderError::from_http_status(
            ProviderKind::OpenAi,
            StatusCode::UNAUTHORIZED,
            "{\"error\":\"invalid api key\"}",
        );
        let messages = provider_error_messages(&error);
        assert!(messages
            .iter()
            .any(|line| line.contains("Issue communicating with the AI server (OpenAI)")));
        assert!(messages
            .iter()
            .any(|line| line.contains("Verify your OpenAI token")));
    }

    #[test]
    fn validates_openai_token_requirement() {
        let issues = validate_provider_request(
            ProviderKind::OpenAi,
            "gpt-4o-mini",
            "https://api.openai.com/v1/chat/completions",
            "",
        )
        .expect_err("expected token validation failure");
        assert!(issues
            .iter()
            .any(|line| line.contains("Missing OpenAI token")));
    }

    #[test]
    fn validates_anthropic_token_and_file_capability() {
        let issues = validate_provider_request(
            ProviderKind::Anthropic,
            "claude-test",
            ProviderKind::Anthropic.default_url(),
            "",
        )
        .expect_err("expected token validation failure");
        assert!(issues.iter().any(|line| line.contains("Anthropic API key")));

        let issues = validate_provider_content_parts(
            ProviderKind::Anthropic,
            ProviderKind::Anthropic.default_url(),
            &[ContentPart::File {
                filename: "report.pdf".to_string(),
                file_data: "data:application/pdf;base64,cGRm".to_string(),
            }],
        )
        .expect_err("expected file capability failure");
        assert!(issues
            .iter()
            .any(|line| line.contains("File inputs are not supported")));
    }

    #[test]
    fn validates_gemini_token_and_file_capability() {
        let issues = validate_provider_request(
            ProviderKind::Gemini,
            "gemini-test",
            ProviderKind::Gemini.default_url(),
            "",
        )
        .expect_err("expected token validation failure");
        assert!(issues.iter().any(|line| line.contains("Gemini API key")));

        let issues = validate_provider_content_parts(
            ProviderKind::Gemini,
            ProviderKind::Gemini.default_url(),
            &[ContentPart::File {
                filename: "report.pdf".to_string(),
                file_data: "data:application/pdf;base64,cGRm".to_string(),
            }],
        )
        .expect_err("expected file capability failure");
        assert!(issues
            .iter()
            .any(|line| line.contains("File inputs are not supported")));
    }

    #[test]
    fn validates_hosted_provider_tokens_and_sanitizes_error_envelopes() {
        for provider in [ProviderKind::Mistral, ProviderKind::Xai] {
            let issues = validate_provider_request(
                provider,
                "operator-selected-model",
                provider.default_url(),
                "",
            )
            .expect_err("hosted provider should require an API key");
            assert!(issues.iter().any(|line| line.contains("API key")));
        }

        let body = br#"{"error":{"message":"schema rejected"},"debug":"do-not-print"}"#;
        assert_eq!(
            sanitized_http_error_body(ProviderKind::Xai, body),
            "schema rejected"
        );
        assert_eq!(
            sanitized_http_error_body(ProviderKind::Mistral, b"not-json"),
            "Mistral API returned an HTTP error response."
        );
    }

    #[test]
    fn invalid_response_uses_actionable_hint() {
        let error = ProviderError::invalid_response(
            ProviderKind::OpenAi,
            "Failed to parse JSON from provider",
        );
        let messages = provider_error_messages(&error);
        assert!(messages
            .iter()
            .any(|line| line.contains("unexpected response shape")));
    }

    #[test]
    fn invalid_request_with_file_input_uses_file_specific_hint() {
        let error = ProviderError::from_http_status(
            ProviderKind::OpenAi,
            StatusCode::BAD_REQUEST,
            "{\"error\":\"file inputs are not supported for this model\"}",
        );
        let messages = provider_error_messages(&error);
        assert!(messages
            .iter()
            .any(|line| line.contains("rejected the supplied file input")));
    }

    #[test]
    fn rejects_file_inputs_on_non_openai_ollama_transport() {
        let issues = validate_provider_content_parts(
            ProviderKind::Ollama,
            "http://localhost:11434/api/chat",
            &[ContentPart::File {
                filename: "report.pdf".to_string(),
                file_data: "data:application/pdf;base64,JVBERi0xLjQK".to_string(),
            }],
        )
        .expect_err("expected transport validation failure");

        assert!(issues
            .iter()
            .any(|line| line.contains("File inputs require a transport")));
    }

    #[tokio::test]
    async fn classifies_connectivity_reqwest_errors() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind ephemeral port");
        let addr = listener.local_addr().expect("capture local address");
        drop(listener); // no server listening now -> connection refused

        let request_error = reqwest::Client::new()
            .get(format!("http://{addr}/"))
            .send()
            .await
            .expect_err("request should fail with connectivity error");

        let provider_error = ProviderError::from_reqwest(ProviderKind::Ollama, request_error);
        assert_eq!(
            provider_error.kind(),
            super::ProviderErrorKind::Connectivity
        );
    }
}
