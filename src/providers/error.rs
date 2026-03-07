//! Shared provider error taxonomy and user-facing diagnostics policy.

use super::runtime::ContentPart;
use reqwest::StatusCode;
use std::fmt;

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum ProviderKind {
    Ollama,
    OpenAi,
}

impl ProviderKind {
    pub(crate) fn from_server_value(server: &str) -> Option<Self> {
        match server.trim().to_ascii_lowercase().as_str() {
            "ollama" => Some(Self::Ollama),
            "openai" => Some(Self::OpenAi),
            _ => None,
        }
    }

    pub(crate) fn display_name(self) -> &'static str {
        match self {
            Self::Ollama => "Ollama",
            Self::OpenAi => "OpenAI",
        }
    }

    pub(crate) fn default_url(self) -> &'static str {
        match self {
            Self::Ollama => "http://localhost:11434/v1/chat/completions",
            Self::OpenAi => "https://api.openai.com/v1/chat/completions",
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

fn provider_hint(kind: ProviderErrorKind, provider: ProviderKind) -> Option<&'static str> {
    match kind {
        ProviderErrorKind::ModelNotFound => match provider {
            ProviderKind::Ollama => Some(
                "Run `ollama list` to inspect installed models, then `ollama pull <model>` for missing models.",
            ),
            ProviderKind::OpenAi => {
                Some("Verify the model name and confirm your account has access to it.")
            }
        },
        ProviderErrorKind::Unauthorized => match provider {
            ProviderKind::OpenAi => {
                Some("Verify your OpenAI token (`--token` or profile token), or re-run `cargo ai auth login openai`, and confirm model access.")
            }
            ProviderKind::Ollama => Some(
                "Verify your Ollama endpoint and credentials (if your deployment requires auth).",
            ),
        },
        ProviderErrorKind::RateLimited => match provider {
            ProviderKind::OpenAi => {
                Some("OpenAI rate limit reached; retry later or adjust your account/model limits.")
            }
            ProviderKind::Ollama => Some(
                "Ollama appears rate-limited; retry shortly or reduce concurrent local requests.",
            ),
        },
        ProviderErrorKind::Connectivity => match provider {
            ProviderKind::Ollama => {
                Some("Ensure Ollama is running (`ollama serve`) and the configured URL is reachable.")
            }
            ProviderKind::OpenAi => Some(
                "Check network connectivity and ensure the configured OpenAI URL is reachable.",
            ),
        },
        ProviderErrorKind::Timeout => match provider {
            ProviderKind::Ollama => {
                Some("Request timed out; ensure Ollama/model is responsive or increase `--timeout_in_sec`.")
            }
            ProviderKind::OpenAi => {
                Some("Request timed out; retry later or increase `--timeout_in_sec`.")
            }
        },
        ProviderErrorKind::InvalidRequest => {
            Some("Check `--model`, `--url`, and request parameters for invalid values.")
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

    if let Some(hint) = provider_hint(error.kind(), error.provider()) {
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

    if provider == ProviderKind::OpenAi && token.trim().is_empty() {
        issues.push(
            "❌ Missing OpenAI token. Provide `--token <TOKEN>`, run `cargo ai auth login openai`, or configure `cargo ai profile set <name> --token <TOKEN> --auth api_key`."
                .to_string(),
        );
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

    if !includes_images {
        return Ok(());
    }

    let normalized_url = url.trim().to_ascii_lowercase();
    let mut issues = Vec::new();

    if provider == ProviderKind::Ollama
        && (normalized_url.contains("/api/generate") || normalized_url.contains("/api/chat"))
    {
        issues.push(
            "❌ Image inputs require Ollama's OpenAI-compatible `/v1/chat/completions` transport. Update `--url` or your profile URL before retrying."
                .to_string(),
        );
    }

    if issues.is_empty() {
        Ok(())
    } else {
        Err(issues)
    }
}

#[cfg(test)]
mod tests {
    use super::{provider_error_messages, validate_provider_request, ProviderError, ProviderKind};
    use reqwest::StatusCode;
    use tokio::net::TcpListener;

    #[test]
    fn parses_provider_kind_from_server_value() {
        assert_eq!(
            ProviderKind::from_server_value("ollama"),
            Some(ProviderKind::Ollama)
        );
        assert_eq!(
            ProviderKind::from_server_value("OPENAI"),
            Some(ProviderKind::OpenAi)
        );
        assert_eq!(ProviderKind::from_server_value("wat"), None);
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
