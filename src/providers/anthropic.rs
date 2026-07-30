use super::{
    runtime::{ContentPart, ProviderTextResponse, ProviderUsage},
    ProviderError, ProviderKind, DEFAULT_TEMPERATURE,
};
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use reqwest::ClientBuilder;
use serde::{Deserialize, Serialize};
use std::time::Duration;

pub(crate) const ANTHROPIC_API_VERSION: &str = "2023-06-01";
pub(crate) const DEFAULT_MAX_OUTPUT_TOKENS: u32 = 4096;

#[derive(Debug, Serialize)]
struct Request<'a> {
    model: &'a str,
    max_tokens: u32,
    messages: Vec<RequestMessage>,
    temperature: f64,
    output_config: OutputConfig<'a>,
}

#[derive(Debug, Serialize)]
struct RequestMessage {
    role: &'static str,
    content: Vec<RequestContentBlock>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum RequestContentBlock {
    Text { text: String },
    Image { source: ImageSource },
}

#[derive(Debug, Serialize)]
struct ImageSource {
    r#type: &'static str,
    media_type: String,
    data: String,
}

#[derive(Debug, Serialize)]
struct OutputConfig<'a> {
    format: OutputFormat<'a>,
}

#[derive(Debug, Serialize)]
struct OutputFormat<'a> {
    r#type: &'static str,
    schema: &'a serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct Response {
    content: Vec<ResponseContentBlock>,
    #[serde(default)]
    usage: Option<Usage>,
}

#[derive(Debug, Deserialize)]
struct ResponseContentBlock {
    r#type: String,
    #[serde(default)]
    text: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Usage {
    input_tokens: u64,
    output_tokens: u64,
    #[serde(default)]
    cache_creation_input_tokens: Option<u64>,
    #[serde(default)]
    cache_read_input_tokens: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct ErrorEnvelope {
    error: ErrorBody,
}

#[derive(Debug, Deserialize)]
struct ErrorBody {
    #[serde(default)]
    r#type: Option<String>,
    message: String,
}

fn image_source(data_url: &str) -> Result<ImageSource, ProviderError> {
    let (metadata, encoded) = data_url.split_once(',').ok_or_else(|| {
        ProviderError::invalid_response(
            ProviderKind::Anthropic,
            "Image input was not encoded as a data URL.",
        )
    })?;
    let media_type = metadata
        .strip_prefix("data:")
        .and_then(|value| value.strip_suffix(";base64"))
        .ok_or_else(|| {
            ProviderError::invalid_response(
                ProviderKind::Anthropic,
                "Image input data URL must use base64 encoding.",
            )
        })?;

    BASE64_STANDARD.decode(encoded).map_err(|error| {
        ProviderError::invalid_response(
            ProviderKind::Anthropic,
            format!("Image input contains invalid base64 data: {error}"),
        )
    })?;

    Ok(ImageSource {
        r#type: "base64",
        media_type: media_type.to_string(),
        data: encoded.to_string(),
    })
}

fn request_content_blocks(
    content_parts: &[ContentPart],
) -> Result<Vec<RequestContentBlock>, ProviderError> {
    content_parts
        .iter()
        .map(|part| match part {
            ContentPart::Text(text) => Ok(RequestContentBlock::Text { text: text.clone() }),
            ContentPart::Image { data_url } => Ok(RequestContentBlock::Image {
                source: image_source(data_url)?,
            }),
            ContentPart::File { filename, .. } => Err(ProviderError::invalid_request(
                ProviderKind::Anthropic,
                format!(
                    "Anthropic file input is not supported in this release ('{filename}'). Use text, URL-text, or image input."
                ),
            )),
        })
        .collect()
}

fn normalize_usage(usage: Option<Usage>) -> Option<ProviderUsage> {
    usage.map(|usage| {
        let input_token_details = if usage.cache_creation_input_tokens.is_some()
            || usage.cache_read_input_tokens.is_some()
        {
            Some(serde_json::json!({
                "cache_creation_input_tokens": usage.cache_creation_input_tokens,
                "cache_read_input_tokens": usage.cache_read_input_tokens,
            }))
        } else {
            None
        };
        ProviderUsage {
            input_tokens: Some(usage.input_tokens),
            output_tokens: Some(usage.output_tokens),
            total_tokens: usage.input_tokens.checked_add(usage.output_tokens),
            input_token_details,
            output_token_details: None,
        }
    })
}

fn error_message(body: &[u8]) -> String {
    serde_json::from_slice::<ErrorEnvelope>(body)
        .map(|envelope| match envelope.error.r#type {
            Some(kind) if !kind.trim().is_empty() => {
                format!("Anthropic {kind}: {}", envelope.error.message)
            }
            _ => envelope.error.message,
        })
        .unwrap_or_else(|_| "Anthropic returned an HTTP error response.".to_string())
}

pub(crate) async fn send_request(
    url: &str,
    model: &str,
    content_parts: &[ContentPart],
    timeout_in_sec: u64,
    token: &str,
    response_schema: &serde_json::Value,
    max_output_tokens: Option<u32>,
) -> Result<ProviderTextResponse, ProviderError> {
    let request = Request {
        model,
        max_tokens: max_output_tokens.unwrap_or(DEFAULT_MAX_OUTPUT_TOKENS),
        messages: vec![RequestMessage {
            role: "user",
            content: request_content_blocks(content_parts)?,
        }],
        temperature: DEFAULT_TEMPERATURE,
        output_config: OutputConfig {
            format: OutputFormat {
                r#type: "json_schema",
                schema: response_schema,
            },
        },
    };

    let client = ClientBuilder::new()
        .timeout(Duration::from_secs(timeout_in_sec))
        .build()
        .map_err(|error| ProviderError::from_reqwest(ProviderKind::Anthropic, error))?;
    let response = client
        .post(url)
        .header("x-api-key", token)
        .header("anthropic-version", ANTHROPIC_API_VERSION)
        .json(&request)
        .send()
        .await
        .map_err(|error| ProviderError::from_reqwest(ProviderKind::Anthropic, error))?;
    let status = response.status();
    let body = response
        .bytes()
        .await
        .map_err(|error| ProviderError::from_reqwest(ProviderKind::Anthropic, error))?;

    if !status.is_success() {
        return Err(ProviderError::from_http_status(
            ProviderKind::Anthropic,
            status,
            error_message(&body).as_str(),
        ));
    }

    let response: Response = serde_json::from_slice(&body).map_err(|error| {
        ProviderError::invalid_response(
            ProviderKind::Anthropic,
            format!("Failed to parse Anthropic response JSON: {error}"),
        )
    })?;
    let text = response
        .content
        .iter()
        .filter(|block| block.r#type == "text")
        .filter_map(|block| block.text.as_deref())
        .collect::<Vec<_>>()
        .join("");
    if text.is_empty() {
        return Err(ProviderError::invalid_response(
            ProviderKind::Anthropic,
            "Anthropic returned no text content blocks.",
        ));
    }

    Ok(ProviderTextResponse {
        text,
        usage: normalize_usage(response.usage),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use mockito::Matcher;

    fn schema() -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": { "answer": { "type": "integer" } },
            "required": ["answer"],
            "additionalProperties": false
        })
    }

    #[tokio::test]
    async fn sends_native_headers_schema_image_and_usage() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/v1/messages")
            .match_header("x-api-key", "anthropic-test-token")
            .match_header("anthropic-version", ANTHROPIC_API_VERSION)
            .match_body(Matcher::PartialJson(serde_json::json!({
                "model": "claude-test",
                "max_tokens": 512,
                "messages": [{
                    "role": "user",
                    "content": [
                        {"type": "text", "text": "Return one."},
                        {"type": "image", "source": {
                            "type": "base64",
                            "media_type": "image/png",
                            "data": "aW1hZ2U="
                        }}
                    ]
                }],
                "output_config": {"format": {
                    "type": "json_schema",
                    "schema": schema()
                }}
            })))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                serde_json::json!({
                    "content": [{"type": "text", "text": "{\"answer\":1}"}],
                    "usage": {
                        "input_tokens": 12,
                        "output_tokens": 5,
                        "cache_read_input_tokens": 3
                    }
                })
                .to_string(),
            )
            .create_async()
            .await;

        let response = send_request(
            format!("{}/v1/messages", server.url()).as_str(),
            "claude-test",
            &[
                ContentPart::Text("Return one.".to_string()),
                ContentPart::Image {
                    data_url: "data:image/png;base64,aW1hZ2U=".to_string(),
                },
            ],
            30,
            "anthropic-test-token",
            &schema(),
            Some(512),
        )
        .await
        .expect("request should succeed");

        mock.assert_async().await;
        assert_eq!(response.text, "{\"answer\":1}");
        let usage = response.usage.expect("usage should be normalized");
        assert_eq!(usage.input_tokens, Some(12));
        assert_eq!(usage.output_tokens, Some(5));
        assert_eq!(usage.total_tokens, Some(17));
        assert_eq!(
            usage.input_token_details.as_ref().unwrap()["cache_read_input_tokens"],
            3
        );
    }

    #[tokio::test]
    async fn defaults_max_tokens_and_redacts_unstructured_http_body() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/v1/messages")
            .match_body(Matcher::PartialJson(serde_json::json!({
                "max_tokens": DEFAULT_MAX_OUTPUT_TOKENS
            })))
            .with_status(500)
            .with_body("secret upstream dump")
            .create_async()
            .await;

        let error = send_request(
            format!("{}/v1/messages", server.url()).as_str(),
            "claude-test",
            &[ContentPart::Text("Return one.".to_string())],
            30,
            "anthropic-test-token",
            &schema(),
            None,
        )
        .await
        .expect_err("request should fail");

        mock.assert_async().await;
        assert!(!error.message().contains("secret upstream dump"));
    }

    #[tokio::test]
    async fn rejects_file_parts_before_request() {
        let error = send_request(
            "https://example.invalid/v1/messages",
            "claude-test",
            &[ContentPart::File {
                filename: "report.pdf".to_string(),
                file_data: "data:application/pdf;base64,cGRm".to_string(),
            }],
            30,
            "anthropic-test-token",
            &schema(),
            None,
        )
        .await
        .expect_err("file input should fail");

        assert!(error.message().contains("file input is not supported"));
    }
}
