//! Native xAI Responses API adapter.

use super::{
    error::sanitized_http_error_body,
    runtime::{ContentPart, ProviderTextResponse, ProviderUsage},
    ProviderError, ProviderKind,
};
use reqwest::ClientBuilder;
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Serialize, Debug)]
struct Request<'a> {
    model: &'a str,
    input: Vec<RequestMessage>,
    text: TextConfig<'a>,
    store: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_output_tokens: Option<u32>,
}

#[derive(Serialize, Debug)]
struct RequestMessage {
    role: &'static str,
    content: Vec<RequestContentPart>,
}

#[derive(Serialize, Debug)]
#[serde(tag = "type", rename_all = "snake_case")]
enum RequestContentPart {
    InputText { text: String },
}

#[derive(Serialize, Debug)]
struct TextConfig<'a> {
    format: TextFormat<'a>,
}

#[derive(Serialize, Debug)]
struct TextFormat<'a> {
    r#type: &'static str,
    name: &'static str,
    schema: &'a serde_json::Value,
    strict: bool,
}

#[derive(Deserialize, Debug)]
struct Response {
    #[serde(default)]
    output: Vec<OutputItem>,
    #[serde(default)]
    output_text: Option<String>,
    #[serde(default)]
    usage: Option<Usage>,
}

#[derive(Deserialize, Debug)]
struct OutputItem {
    #[serde(default)]
    r#type: String,
    #[serde(default)]
    content: Vec<OutputContent>,
}

#[derive(Deserialize, Debug)]
struct OutputContent {
    #[serde(default)]
    r#type: String,
    #[serde(default)]
    text: Option<String>,
}

#[derive(Deserialize, Debug)]
struct Usage {
    #[serde(default)]
    input_tokens: Option<u64>,
    #[serde(default)]
    output_tokens: Option<u64>,
    #[serde(default)]
    total_tokens: Option<u64>,
    #[serde(default)]
    input_tokens_details: Option<serde_json::Value>,
    #[serde(default)]
    output_tokens_details: Option<serde_json::Value>,
}

fn request_content_parts(
    content_parts: &[ContentPart],
) -> Result<Vec<RequestContentPart>, ProviderError> {
    content_parts
        .iter()
        .map(|part| match part {
            ContentPart::Text(text) => Ok(RequestContentPart::InputText { text: text.clone() }),
            ContentPart::Image { .. } => Err(ProviderError::invalid_request(
                ProviderKind::Xai,
                "xAI image input is outside the current Cargo AI compatibility slice.",
            )),
            ContentPart::File { .. } => Err(ProviderError::invalid_request(
                ProviderKind::Xai,
                "xAI file input is outside the current Cargo AI compatibility slice.",
            )),
        })
        .collect()
}

fn response_text(response: &Response) -> Option<String> {
    if let Some(text) = response
        .output_text
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty())
    {
        return Some(text.to_string());
    }

    let text = response
        .output
        .iter()
        .filter(|item| item.r#type == "message")
        .flat_map(|item| &item.content)
        .filter(|content| content.r#type == "output_text")
        .filter_map(|content| content.text.as_deref())
        .collect::<Vec<_>>()
        .join("");

    (!text.trim().is_empty()).then(|| text.trim().to_string())
}

fn normalize_usage(usage: Option<Usage>) -> Option<ProviderUsage> {
    usage.map(|usage| ProviderUsage {
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        total_tokens: usage.total_tokens,
        input_token_details: usage.input_tokens_details,
        output_token_details: usage.output_tokens_details,
    })
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
        input: vec![RequestMessage {
            role: "user",
            content: request_content_parts(content_parts)?,
        }],
        text: TextConfig {
            format: TextFormat {
                r#type: "json_schema",
                name: "Output",
                schema: response_schema,
                strict: true,
            },
        },
        store: false,
        max_output_tokens,
    };

    let client = ClientBuilder::new()
        .timeout(Duration::from_secs(timeout_in_sec))
        .build()
        .map_err(|error| ProviderError::from_reqwest(ProviderKind::Xai, error))?;
    let response = client
        .post(url)
        .header("Authorization", format!("Bearer {token}"))
        .header("Content-Type", "application/json")
        .json(&request)
        .send()
        .await
        .map_err(|error| ProviderError::from_reqwest(ProviderKind::Xai, error))?;
    let status = response.status();
    let body = response
        .bytes()
        .await
        .map_err(|error| ProviderError::from_reqwest(ProviderKind::Xai, error))?;

    if !status.is_success() {
        return Err(ProviderError::from_http_status(
            ProviderKind::Xai,
            status,
            sanitized_http_error_body(ProviderKind::Xai, &body).as_str(),
        ));
    }

    let response: Response = serde_json::from_slice(&body).map_err(|error| {
        ProviderError::invalid_response(
            ProviderKind::Xai,
            format!("Failed to parse xAI response JSON: {error}"),
        )
    })?;
    let text = response_text(&response).ok_or_else(|| {
        ProviderError::invalid_response(ProviderKind::Xai, "xAI returned no output_text content.")
    })?;

    Ok(ProviderTextResponse {
        text,
        usage: normalize_usage(response.usage),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use mockito::{Matcher, Server};

    fn schema() -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": { "answer": { "type": "integer" } },
            "required": ["answer"],
            "additionalProperties": false
        })
    }

    #[tokio::test]
    async fn sends_stateless_responses_schema_and_normalizes_usage() {
        let mut server = Server::new_async().await;
        let mock = server
            .mock("POST", "/v1/responses")
            .match_header("authorization", "Bearer xai-test-token")
            .match_body(Matcher::PartialJson(serde_json::json!({
                "model": "grok-test",
                "input": [{
                    "role": "user",
                    "content": [{"type": "input_text", "text": "Return two."}]
                }],
                "text": {
                    "format": {
                        "type": "json_schema",
                        "name": "Output",
                        "schema": schema(),
                        "strict": true
                    }
                },
                "store": false,
                "max_output_tokens": 128
            })))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                serde_json::json!({
                    "output": [{
                        "type": "message",
                        "role": "assistant",
                        "content": [{
                            "type": "output_text",
                            "text": "{\"answer\":2}"
                        }]
                    }],
                    "usage": {
                        "input_tokens": 12,
                        "output_tokens": 4,
                        "total_tokens": 16
                    }
                })
                .to_string(),
            )
            .create_async()
            .await;

        let response = send_request(
            format!("{}/v1/responses", server.url()).as_str(),
            "grok-test",
            &[ContentPart::Text("Return two.".to_string())],
            30,
            "xai-test-token",
            &schema(),
            Some(128),
        )
        .await
        .expect("xAI request should succeed");

        mock.assert_async().await;
        assert_eq!(response.text, "{\"answer\":2}");
        let usage = response.usage.expect("usage should be normalized");
        assert_eq!(usage.input_tokens, Some(12));
        assert_eq!(usage.output_tokens, Some(4));
        assert_eq!(usage.total_tokens, Some(16));
    }

    #[tokio::test]
    async fn provider_error_preserves_xai_identity_without_raw_body() {
        let mut server = Server::new_async().await;
        let mock = server
            .mock("POST", "/v1/responses")
            .with_status(400)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"error":{"message":"selected model rejected schema"},"debug":"do-not-print"}"#,
            )
            .create_async()
            .await;

        let error = send_request(
            format!("{}/v1/responses", server.url()).as_str(),
            "grok-test",
            &[ContentPart::Text("Return two.".to_string())],
            30,
            "xai-test-token",
            &schema(),
            None,
        )
        .await
        .expect_err("xAI request should fail");

        mock.assert_async().await;
        assert_eq!(error.provider(), ProviderKind::Xai);
        assert!(error.message().contains("selected model rejected schema"));
        assert!(!error.message().contains("do-not-print"));
    }

    #[tokio::test]
    async fn rejects_success_without_output_text() {
        let mut server = Server::new_async().await;
        let mock = server
            .mock("POST", "/v1/responses")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"output":[{"type":"reasoning","content":[]}],"usage":{}}"#)
            .create_async()
            .await;

        let error = send_request(
            format!("{}/v1/responses", server.url()).as_str(),
            "grok-test",
            &[ContentPart::Text("Return two.".to_string())],
            30,
            "xai-test-token",
            &schema(),
            None,
        )
        .await
        .expect_err("xAI response should fail");

        mock.assert_async().await;
        assert_eq!(error.provider(), ProviderKind::Xai);
        assert!(error.message().contains("no output_text"));
    }
}
