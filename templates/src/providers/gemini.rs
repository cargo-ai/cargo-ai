use super::{
    runtime::{ContentPart, ProviderTextResponse, ProviderUsage},
    ProviderError, ProviderKind,
};
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use reqwest::ClientBuilder;
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Debug, Serialize)]
struct Request<'a> {
    model: &'a str,
    input: Vec<RequestContent>,
    response_format: ResponseFormat<'a>,
    store: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    generation_config: Option<GenerationConfig>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum RequestContent {
    Text { text: String },
    Image { data: String, mime_type: String },
}

#[derive(Debug, Serialize)]
struct ResponseFormat<'a> {
    r#type: &'static str,
    mime_type: &'static str,
    schema: &'a serde_json::Value,
}

#[derive(Debug, Serialize)]
struct GenerationConfig {
    max_output_tokens: u32,
}

#[derive(Debug, Deserialize)]
struct Response {
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    steps: Vec<ResponseStep>,
    #[serde(default)]
    usage: Option<Usage>,
}

#[derive(Debug, Deserialize)]
struct ResponseStep {
    r#type: String,
    #[serde(default)]
    content: Vec<ResponseContent>,
}

#[derive(Debug, Deserialize)]
struct ResponseContent {
    r#type: String,
    #[serde(default)]
    text: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Usage {
    #[serde(default)]
    total_input_tokens: Option<u64>,
    #[serde(default)]
    total_output_tokens: Option<u64>,
    #[serde(default)]
    total_tokens: Option<u64>,
    #[serde(default)]
    total_cached_tokens: Option<u64>,
    #[serde(default)]
    total_thought_tokens: Option<u64>,
    #[serde(default)]
    total_tool_use_tokens: Option<u64>,
    #[serde(default)]
    input_tokens_by_modality: Option<serde_json::Value>,
    #[serde(default)]
    output_tokens_by_modality: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct ErrorEnvelope {
    error: ErrorBody,
}

#[derive(Debug, Deserialize)]
struct ErrorBody {
    #[serde(default)]
    status: Option<String>,
    message: String,
}

fn image_content(data_url: &str) -> Result<RequestContent, ProviderError> {
    let (metadata, data) = data_url.split_once(',').ok_or_else(|| {
        ProviderError::invalid_response(
            ProviderKind::Gemini,
            "Image input was not encoded as a data URL.",
        )
    })?;
    let mime_type = metadata
        .strip_prefix("data:")
        .and_then(|value| value.strip_suffix(";base64"))
        .ok_or_else(|| {
            ProviderError::invalid_response(
                ProviderKind::Gemini,
                "Image input data URL must use base64 encoding.",
            )
        })?;
    BASE64_STANDARD.decode(data).map_err(|error| {
        ProviderError::invalid_response(
            ProviderKind::Gemini,
            format!("Image input contains invalid base64 data: {error}"),
        )
    })?;

    Ok(RequestContent::Image {
        data: data.to_string(),
        mime_type: mime_type.to_string(),
    })
}

fn request_content(content_parts: &[ContentPart]) -> Result<Vec<RequestContent>, ProviderError> {
    content_parts
        .iter()
        .map(|part| match part {
            ContentPart::Text(text) => Ok(RequestContent::Text { text: text.clone() }),
            ContentPart::Image { data_url } => image_content(data_url),
            ContentPart::File { filename, .. } => Err(ProviderError::invalid_request(
                ProviderKind::Gemini,
                format!(
                    "Gemini file input is not supported in this release ('{filename}'). Use text, URL-text, or image input."
                ),
            )),
        })
        .collect()
}

fn find_unsupported_schema_keyword(
    value: &serde_json::Value,
    path: &str,
) -> Option<(String, String)> {
    const UNSUPPORTED: &[&str] = &[
        "allOf",
        "const",
        "contains",
        "dependentRequired",
        "dependentSchemas",
        "else",
        "exclusiveMaximum",
        "exclusiveMinimum",
        "if",
        "maxContains",
        "maxLength",
        "maxProperties",
        "minContains",
        "minLength",
        "minProperties",
        "multipleOf",
        "not",
        "oneOf",
        "pattern",
        "patternProperties",
        "propertyNames",
        "then",
        "unevaluatedItems",
        "unevaluatedProperties",
        "uniqueItems",
    ];

    match value {
        serde_json::Value::Object(object) => {
            for (key, child) in object {
                let child_path = format!("{path}.{key}");
                if UNSUPPORTED.contains(&key.as_str()) {
                    return Some((key.clone(), child_path));
                }
                if let Some(found) = find_unsupported_schema_keyword(child, &child_path) {
                    return Some(found);
                }
            }
            None
        }
        serde_json::Value::Array(items) => items.iter().enumerate().find_map(|(index, child)| {
            find_unsupported_schema_keyword(child, &format!("{path}[{index}]"))
        }),
        _ => None,
    }
}

fn validate_response_schema(response_schema: &serde_json::Value) -> Result<(), ProviderError> {
    if let Some((keyword, path)) = find_unsupported_schema_keyword(response_schema, "$") {
        return Err(ProviderError::invalid_request(
            ProviderKind::Gemini,
            format!(
                "Gemini structured output does not support JSON Schema keyword `{keyword}` at `{path}`. Cargo AI will not remove or weaken the authored schema."
            ),
        ));
    }
    Ok(())
}

fn normalize_usage(usage: Option<Usage>) -> Option<ProviderUsage> {
    usage.map(|usage| {
        let input_token_details =
            if usage.total_cached_tokens.is_some() || usage.input_tokens_by_modality.is_some() {
                Some(serde_json::json!({
                    "total_cached_tokens": usage.total_cached_tokens,
                    "input_tokens_by_modality": usage.input_tokens_by_modality,
                }))
            } else {
                None
            };
        let output_token_details = if usage.total_thought_tokens.is_some()
            || usage.total_tool_use_tokens.is_some()
            || usage.output_tokens_by_modality.is_some()
        {
            Some(serde_json::json!({
                "total_thought_tokens": usage.total_thought_tokens,
                "total_tool_use_tokens": usage.total_tool_use_tokens,
                "output_tokens_by_modality": usage.output_tokens_by_modality,
            }))
        } else {
            None
        };
        ProviderUsage {
            input_tokens: usage.total_input_tokens,
            output_tokens: usage.total_output_tokens,
            total_tokens: usage.total_tokens,
            input_token_details,
            output_token_details,
        }
    })
}

fn error_message(body: &[u8]) -> String {
    serde_json::from_slice::<ErrorEnvelope>(body)
        .map(|envelope| match envelope.error.status {
            Some(status) if !status.trim().is_empty() => {
                format!("Gemini {status}: {}", envelope.error.message)
            }
            _ => envelope.error.message,
        })
        .unwrap_or_else(|_| "Gemini returned an HTTP error response.".to_string())
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
    validate_response_schema(response_schema)?;
    let request = Request {
        model,
        input: request_content(content_parts)?,
        response_format: ResponseFormat {
            r#type: "text",
            mime_type: "application/json",
            schema: response_schema,
        },
        store: false,
        generation_config: max_output_tokens
            .map(|max_output_tokens| GenerationConfig { max_output_tokens }),
    };
    let client = ClientBuilder::new()
        .timeout(Duration::from_secs(timeout_in_sec))
        .build()
        .map_err(|error| ProviderError::from_reqwest(ProviderKind::Gemini, error))?;
    let response = client
        .post(url)
        .header("x-goog-api-key", token)
        .json(&request)
        .send()
        .await
        .map_err(|error| ProviderError::from_reqwest(ProviderKind::Gemini, error))?;
    let status = response.status();
    let body = response
        .bytes()
        .await
        .map_err(|error| ProviderError::from_reqwest(ProviderKind::Gemini, error))?;
    if !status.is_success() {
        return Err(ProviderError::from_http_status(
            ProviderKind::Gemini,
            status,
            &error_message(&body),
        ));
    }

    let response: Response = serde_json::from_slice(&body).map_err(|error| {
        ProviderError::invalid_response(
            ProviderKind::Gemini,
            format!("Failed to parse Gemini response JSON: {error}"),
        )
    })?;
    let text = response
        .steps
        .iter()
        .rev()
        .find(|step| step.r#type == "model_output")
        .map(|step| {
            step.content
                .iter()
                .filter(|content| content.r#type == "text")
                .filter_map(|content| content.text.as_deref())
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default();
    if text.is_empty() {
        let status = response.status.as_deref().unwrap_or("unknown");
        return Err(ProviderError::invalid_response(
            ProviderKind::Gemini,
            format!("Gemini returned no text content in a model_output step (status: {status})."),
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
            "properties": {"answer": {"type": "integer", "minimum": 0}},
            "required": ["answer"],
            "additionalProperties": false
        })
    }

    #[tokio::test]
    async fn sends_stateless_native_schema_image_request_and_normalizes_usage() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/v1beta/interactions")
            .match_header("x-goog-api-key", "gemini-test-token")
            .match_body(Matcher::PartialJson(serde_json::json!({
                "model": "gemini-test",
                "store": false,
                "generation_config": {"max_output_tokens": 256},
                "input": [
                    {"type": "text", "text": "Return one."},
                    {"type": "image", "data": "aW1hZ2U=", "mime_type": "image/png"}
                ],
                "response_format": {
                    "type": "text",
                    "mime_type": "application/json",
                    "schema": schema()
                }
            })))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                serde_json::json!({
                    "status": "completed",
                    "steps": [{"type": "model_output", "content": [
                        {"type": "text", "text": "{\"answer\":1}"}
                    ]}],
                    "usage": {
                        "total_input_tokens": 14,
                        "total_output_tokens": 6,
                        "total_thought_tokens": 2,
                        "total_tokens": 22
                    }
                })
                .to_string(),
            )
            .create_async()
            .await;

        let response = send_request(
            &format!("{}/v1beta/interactions", server.url()),
            "gemini-test",
            &[
                ContentPart::Text("Return one.".to_string()),
                ContentPart::Image {
                    data_url: "data:image/png;base64,aW1hZ2U=".to_string(),
                },
            ],
            30,
            "gemini-test-token",
            &schema(),
            Some(256),
        )
        .await
        .expect("request should succeed");

        mock.assert_async().await;
        assert_eq!(response.text, "{\"answer\":1}");
        let usage = response.usage.expect("usage should be normalized");
        assert_eq!(usage.input_tokens, Some(14));
        assert_eq!(usage.output_tokens, Some(6));
        assert_eq!(usage.total_tokens, Some(22));
        assert_eq!(
            usage.output_token_details.unwrap()["total_thought_tokens"],
            2
        );
    }

    #[tokio::test]
    async fn omits_generation_config_without_override_and_redacts_raw_errors() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/v1beta/interactions")
            .match_body(Matcher::Json(serde_json::json!({
                "model": "gemini-test",
                "input": [{"type": "text", "text": "Return one."}],
                "response_format": {
                    "type": "text",
                    "mime_type": "application/json",
                    "schema": schema()
                },
                "store": false
            })))
            .with_status(500)
            .with_body("secret upstream dump")
            .create_async()
            .await;

        let error = send_request(
            &format!("{}/v1beta/interactions", server.url()),
            "gemini-test",
            &[ContentPart::Text("Return one.".to_string())],
            30,
            "gemini-test-token",
            &schema(),
            None,
        )
        .await
        .expect_err("request should fail");
        mock.assert_async().await;
        assert!(!error.message().contains("secret upstream dump"));
    }

    #[tokio::test]
    async fn rejects_file_and_unsupported_schema_before_request() {
        let file_error = send_request(
            "https://example.invalid/v1beta/interactions",
            "gemini-test",
            &[ContentPart::File {
                filename: "report.pdf".to_string(),
                file_data: "data:application/pdf;base64,cGRm".to_string(),
            }],
            30,
            "gemini-test-token",
            &schema(),
            None,
        )
        .await
        .expect_err("file input should fail");
        assert!(file_error.message().contains("file input is not supported"));

        let schema_error = send_request(
            "https://example.invalid/v1beta/interactions",
            "gemini-test",
            &[ContentPart::Text("Return one.".to_string())],
            30,
            "gemini-test-token",
            &serde_json::json!({"type": "string", "pattern": "^[a-z]+$"}),
            None,
        )
        .await
        .expect_err("unsupported schema should fail");
        assert!(schema_error.message().contains("keyword `pattern`"));
        assert!(schema_error.message().contains("will not remove or weaken"));
    }
}
