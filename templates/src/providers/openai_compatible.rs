//! Provider-aware OpenAI-compatible Chat Completions transport.

use super::{
    error::sanitized_http_error_body,
    runtime::{ContentPart, ProviderTextResponse, ProviderUsage},
    ProviderError, ProviderKind,
};
use reqwest::ClientBuilder;
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Serialize, Debug)]
struct Request {
    model: String,
    messages: Vec<RequestMessage>,
    temperature: f64,
    response_format: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
}

#[derive(Serialize, Debug)]
struct RequestMessage {
    role: String,
    content: Vec<RequestContentPart>,
}

#[derive(Serialize, Debug)]
#[serde(tag = "type", rename_all = "snake_case")]
enum RequestContentPart {
    Text { text: String },
    ImageUrl { image_url: ImageUrl },
    File { file: FileInput },
}

#[derive(Serialize, Debug)]
struct ImageUrl {
    url: String,
}

#[derive(Serialize, Debug)]
struct FileInput {
    filename: String,
    file_data: String,
}

#[derive(Deserialize, Debug)]
struct ResponseMessage {
    content: serde_json::Value,
}

#[derive(Deserialize, Debug)]
struct Choice {
    message: ResponseMessage,
}

#[derive(Deserialize, Debug)]
struct Response {
    choices: Vec<Choice>,
    #[serde(default)]
    usage: Option<Usage>,
    #[serde(default)]
    prompt_eval_count: Option<u64>,
    #[serde(default)]
    eval_count: Option<u64>,
}

#[derive(Deserialize, Debug)]
struct Usage {
    #[serde(default)]
    prompt_tokens: Option<u64>,
    #[serde(default)]
    completion_tokens: Option<u64>,
    #[serde(default)]
    total_tokens: Option<u64>,
    #[serde(default)]
    prompt_tokens_details: Option<serde_json::Value>,
    #[serde(default)]
    completion_tokens_details: Option<serde_json::Value>,
}

fn normalize_usage(
    usage: Option<Usage>,
    prompt_eval_count: Option<u64>,
    eval_count: Option<u64>,
) -> Option<ProviderUsage> {
    if let Some(usage) = usage {
        return Some(ProviderUsage {
            input_tokens: usage.prompt_tokens,
            output_tokens: usage.completion_tokens,
            total_tokens: usage.total_tokens,
            input_token_details: usage.prompt_tokens_details,
            output_token_details: usage.completion_tokens_details,
        });
    }

    if prompt_eval_count.is_none() && eval_count.is_none() {
        return None;
    }

    Some(ProviderUsage {
        input_tokens: prompt_eval_count,
        output_tokens: eval_count,
        total_tokens: prompt_eval_count.and_then(|input| eval_count.map(|output| input + output)),
        input_token_details: None,
        output_token_details: None,
    })
}

fn response_format(response_schema: &serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "type": "json_schema",
        "json_schema": {
            "name": "Output",
            "schema": response_schema,
            "strict": true
        }
    })
}

fn request_content_parts(content_parts: &[ContentPart]) -> Vec<RequestContentPart> {
    content_parts
        .iter()
        .map(|part| match part {
            ContentPart::Text(text) => RequestContentPart::Text { text: text.clone() },
            ContentPart::Image { data_url } => RequestContentPart::ImageUrl {
                image_url: ImageUrl {
                    url: data_url.clone(),
                },
            },
            ContentPart::File {
                filename,
                file_data,
            } => RequestContentPart::File {
                file: FileInput {
                    filename: filename.clone(),
                    file_data: file_data.clone(),
                },
            },
        })
        .collect()
}

fn response_text(content: &serde_json::Value) -> Option<String> {
    if let Some(text) = content.as_str().map(str::trim).filter(|text| !text.is_empty()) {
        return Some(text.to_string());
    }

    let text = content
        .as_array()?
        .iter()
        .filter(|part| {
            part.get("type")
                .and_then(serde_json::Value::as_str)
                .is_none_or(|kind| kind == "text")
        })
        .filter_map(|part| part.get("text").and_then(serde_json::Value::as_str))
        .collect::<Vec<_>>()
        .join("");

    (!text.trim().is_empty()).then(|| text.trim().to_string())
}

pub(crate) async fn send_request(
    provider: ProviderKind,
    url: &str,
    model: &str,
    content_parts: &[ContentPart],
    timeout_in_sec: u64,
    token: &str,
    response_schema: &serde_json::Value,
    max_output_tokens: Option<u32>,
) -> Result<ProviderTextResponse, ProviderError> {
    let request = Request {
        model: model.to_string(),
        messages: vec![RequestMessage {
            role: "user".to_string(),
            content: request_content_parts(content_parts),
        }],
        temperature: super::DEFAULT_TEMPERATURE,
        response_format: response_format(response_schema),
        max_tokens: max_output_tokens,
    };

    let client = ClientBuilder::new()
        .timeout(Duration::from_secs(timeout_in_sec))
        .build()
        .map_err(|error| ProviderError::from_reqwest(provider, error))?;
    let mut request_builder = client
        .post(url)
        .header("Content-Type", "application/json");
    if !token.trim().is_empty() {
        request_builder = request_builder.header("Authorization", format!("Bearer {token}"));
    }

    let response = request_builder
        .json(&request)
        .send()
        .await
        .map_err(|error| ProviderError::from_reqwest(provider, error))?;
    let status = response.status();
    let body = response
        .bytes()
        .await
        .map_err(|error| ProviderError::from_reqwest(provider, error))?;

    if !status.is_success() {
        return Err(ProviderError::from_http_status(
            provider,
            status,
            sanitized_http_error_body(provider, &body).as_str(),
        ));
    }

    let response: Response = serde_json::from_slice(&body).map_err(|error| {
        ProviderError::invalid_response(
            provider,
            format!(
                "Failed to parse {} response JSON: {error}",
                provider.display_name()
            ),
        )
    })?;

    let Response {
        choices,
        usage,
        prompt_eval_count,
        eval_count,
    } = response;
    let text = choices
        .first()
        .and_then(|choice| response_text(&choice.message.content))
        .ok_or_else(|| {
            ProviderError::invalid_response(
                provider,
                format!(
                    "{} returned no text chat completion choice.",
                    provider.display_name()
                ),
            )
        })?;

    Ok(ProviderTextResponse {
        text,
        usage: normalize_usage(usage, prompt_eval_count, eval_count),
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
    async fn mistral_uses_bearer_schema_output_limit_and_normalized_usage() {
        let mut server = Server::new_async().await;
        let mock = server
            .mock("POST", "/v1/chat/completions")
            .match_header("authorization", "Bearer mistral-test-token")
            .match_body(Matcher::PartialJson(serde_json::json!({
                "model": "mistral-test",
                "messages": [{
                    "role": "user",
                    "content": [{"type": "text", "text": "Return two."}]
                }],
                "temperature": 0.0,
                "response_format": {
                    "type": "json_schema",
                    "json_schema": {
                        "name": "Output",
                        "schema": schema(),
                        "strict": true
                    }
                },
                "max_tokens": 128
            })))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                serde_json::json!({
                    "choices": [{
                        "message": {
                            "role": "assistant",
                            "content": "{\"answer\":2}"
                        }
                    }],
                    "usage": {
                        "prompt_tokens": 11,
                        "completion_tokens": 4,
                        "total_tokens": 15
                    }
                })
                .to_string(),
            )
            .create_async()
            .await;

        let response = send_request(
            ProviderKind::Mistral,
            format!("{}/v1/chat/completions", server.url()).as_str(),
            "mistral-test",
            &[ContentPart::Text("Return two.".to_string())],
            30,
            "mistral-test-token",
            &schema(),
            Some(128),
        )
        .await
        .expect("Mistral request should succeed");

        mock.assert_async().await;
        assert_eq!(response.text, "{\"answer\":2}");
        let usage = response.usage.expect("usage should be normalized");
        assert_eq!(usage.input_tokens, Some(11));
        assert_eq!(usage.output_tokens, Some(4));
        assert_eq!(usage.total_tokens, Some(15));
    }

    #[tokio::test]
    async fn ollama_omits_auth_and_accepts_native_usage_counters() {
        let mut server = Server::new_async().await;
        let mock = server
            .mock("POST", "/v1/chat/completions")
            .match_header("authorization", Matcher::Missing)
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                serde_json::json!({
                    "choices": [{
                        "message": {
                            "role": "assistant",
                            "content": "{\"answer\":2}"
                        }
                    }],
                    "prompt_eval_count": 7,
                    "eval_count": 3
                })
                .to_string(),
            )
            .create_async()
            .await;

        let response = send_request(
            ProviderKind::Ollama,
            format!("{}/v1/chat/completions", server.url()).as_str(),
            "local-test",
            &[ContentPart::Text("Return two.".to_string())],
            30,
            "",
            &schema(),
            None,
        )
        .await
        .expect("Ollama request should succeed");

        mock.assert_async().await;
        let usage = response.usage.expect("usage should be normalized");
        assert_eq!(usage.input_tokens, Some(7));
        assert_eq!(usage.output_tokens, Some(3));
        assert_eq!(usage.total_tokens, Some(10));
    }

    #[tokio::test]
    async fn provider_error_preserves_mistral_identity_without_raw_body() {
        let mut server = Server::new_async().await;
        let mock = server
            .mock("POST", "/v1/chat/completions")
            .with_status(400)
            .with_header("content-type", "application/json")
            .with_body(r#"{"message":"selected model rejected json_schema","debug":"do-not-print"}"#)
            .create_async()
            .await;

        let error = send_request(
            ProviderKind::Mistral,
            format!("{}/v1/chat/completions", server.url()).as_str(),
            "mistral-test",
            &[ContentPart::Text("Return two.".to_string())],
            30,
            "mistral-test-token",
            &schema(),
            None,
        )
        .await
        .expect_err("Mistral request should fail");

        mock.assert_async().await;
        assert_eq!(error.provider(), ProviderKind::Mistral);
        assert!(error.message().contains("selected model rejected json_schema"));
        assert!(!error.message().contains("do-not-print"));
    }
}
