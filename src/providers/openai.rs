// External Crates
use super::{
    runtime::{
        ContentPart, ImageReference, ProviderImageResponse, ProviderTextResponse, ProviderUsage,
    },
    ProviderError, ProviderKind,
};
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use reqwest::ClientBuilder;
use serde::{Deserialize, Serialize};
use std::time::Duration;

const CHATGPT_CODEX_ENDPOINT_MARKER: &str = "chatgpt.com/backend-api/codex";

#[derive(Serialize, Debug)]
pub struct ChatCompletionsRequest {
    pub model: String,
    pub messages: Vec<ChatRequestMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    pub response_format: serde_json::Value,
}

#[derive(Serialize, Debug)]
pub struct ChatRequestMessage {
    pub role: String,
    pub content: Vec<ChatRequestContentPart>,
}

#[derive(Serialize, Debug)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ChatRequestContentPart {
    Text { text: String },
    ImageUrl { image_url: ImageUrl },
    File { file: FileInput },
}

#[derive(Serialize, Debug)]
pub struct ImageUrl {
    pub url: String,
}

#[derive(Serialize, Debug)]
pub struct FileInput {
    pub filename: String,
    pub file_data: String,
}

#[derive(Serialize, Debug)]
struct ImageGenerationRequest {
    model: String,
    prompt: String,
    n: u8,
    output_format: String,
}

#[derive(Deserialize, Debug)]
struct ImageGenerationResponse {
    data: Vec<ImageGenerationData>,
    #[serde(default)]
    usage: Option<ImageUsage>,
}

#[derive(Deserialize, Debug)]
struct ImageGenerationData {
    b64_json: String,
}

#[derive(Deserialize, Debug)]
struct ImageUsage {
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

#[derive(Serialize, Deserialize, Debug)]
pub struct ChatResponseMessage {
    pub role: String,
    pub content: String,
}

#[derive(Deserialize, Debug)]
#[allow(dead_code)]
pub struct ChatCompletionsResponse {
    pub id: String,
    pub object: String,
    pub created: u64,
    pub model: String,
    pub choices: Vec<Choice>,
    #[serde(default)]
    pub usage: Option<Usage>,
}

#[derive(Deserialize, Debug)]
#[allow(dead_code)]
pub struct Choice {
    pub message: ChatResponseMessage,
    pub finish_reason: Option<String>,
    pub index: usize,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Usage {
    #[serde(default)]
    pub prompt_tokens: Option<u64>,
    #[serde(default)]
    pub completion_tokens: Option<u64>,
    #[serde(default)]
    pub total_tokens: Option<u64>,
    #[serde(default)]
    pub prompt_tokens_details: Option<serde_json::Value>,
    #[serde(default)]
    pub completion_tokens_details: Option<serde_json::Value>,
}

fn is_chatgpt_codex_responses_endpoint(url: &str) -> bool {
    url.contains(CHATGPT_CODEX_ENDPOINT_MARKER)
}

fn normalize_chatgpt_responses_url(url: &str) -> String {
    let trimmed = url.trim_end_matches('/');
    if trimmed.ends_with("/responses") {
        trimmed.to_string()
    } else {
        format!("{trimmed}/responses")
    }
}

fn normalize_openai_images_url(url: &str) -> Result<String, ProviderError> {
    let trimmed = url.trim_end_matches('/');
    if let Some(index) = trimmed.find("/v1/") {
        return Ok(format!("{}/v1/images/generations", &trimmed[..index]));
    }

    if trimmed.ends_with("/v1") {
        Ok(format!("{trimmed}/images/generations"))
    } else {
        Ok(format!("{trimmed}/v1/images/generations"))
    }
}

fn normalize_openai_image_edits_url(url: &str) -> Result<String, ProviderError> {
    let trimmed = url.trim_end_matches('/');
    if let Some(index) = trimmed.find("/v1/") {
        return Ok(format!("{}/v1/images/edits", &trimmed[..index]));
    }

    if trimmed.ends_with("/v1") {
        Ok(format!("{trimmed}/images/edits"))
    } else {
        Ok(format!("{trimmed}/v1/images/edits"))
    }
}

fn responses_text_format_from_chat_response_format(
    response_format: &serde_json::Value,
) -> serde_json::Value {
    if response_format
        .get("type")
        .and_then(serde_json::Value::as_str)
        == Some("json_schema")
    {
        if let Some(json_schema) = response_format.get("json_schema") {
            let mut format = serde_json::Map::new();
            format.insert(
                "type".to_string(),
                serde_json::Value::String("json_schema".to_string()),
            );

            if let Some(name) = json_schema.get("name") {
                format.insert("name".to_string(), name.clone());
            }
            if let Some(schema) = json_schema.get("schema") {
                format.insert("schema".to_string(), schema.clone());
            }
            if let Some(strict) = json_schema.get("strict") {
                format.insert("strict".to_string(), strict.clone());
            }

            return serde_json::Value::Object(format);
        }
    }

    serde_json::json!({ "type": "text" })
}

fn parse_stream_failure_message(payload: &serde_json::Value) -> Option<String> {
    payload
        .get("error")
        .and_then(|error| {
            error
                .get("message")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
                .or_else(|| {
                    error
                        .get("code")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string)
                })
        })
        .or_else(|| {
            payload
                .get("response")
                .and_then(|response| response.get("error"))
                .and_then(|error| {
                    error
                        .get("message")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string)
                })
        })
        .or_else(|| {
            payload
                .get("message")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
}

fn find_image_generation_result(payload: &serde_json::Value) -> Option<&str> {
    match payload {
        serde_json::Value::Object(map) => {
            let is_image_generation_call = map.get("type").and_then(serde_json::Value::as_str)
                == Some("image_generation_call");
            if is_image_generation_call {
                if let Some(result) = map
                    .get("result")
                    .and_then(serde_json::Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                {
                    return Some(result);
                }
            }

            map.values().find_map(find_image_generation_result)
        }
        serde_json::Value::Array(items) => items.iter().find_map(find_image_generation_result),
        _ => None,
    }
}

fn normalize_chat_usage(usage: Option<Usage>) -> Option<ProviderUsage> {
    usage.map(|usage| ProviderUsage {
        total_tokens_source: Some("reported".to_string()),
        input_tokens: usage.prompt_tokens,
        output_tokens: usage.completion_tokens,
        total_tokens: usage.total_tokens,
        input_token_details: usage.prompt_tokens_details,
        output_token_details: usage.completion_tokens_details,
    })
}

fn normalize_image_usage(usage: Option<ImageUsage>) -> Option<ProviderUsage> {
    usage.map(|usage| ProviderUsage {
        total_tokens_source: Some("reported".to_string()),
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        total_tokens: usage.total_tokens,
        input_token_details: usage.input_tokens_details,
        output_token_details: usage.output_tokens_details,
    })
}

fn normalize_responses_usage(payload: &serde_json::Value) -> Option<ProviderUsage> {
    let usage = payload
        .get("response")
        .and_then(|response| response.get("usage"))
        .or_else(|| payload.get("usage"))?;

    Some(ProviderUsage {
        total_tokens_source: Some("reported".to_string()),
        input_tokens: usage
            .get("input_tokens")
            .and_then(serde_json::Value::as_u64),
        output_tokens: usage
            .get("output_tokens")
            .and_then(serde_json::Value::as_u64),
        total_tokens: usage
            .get("total_tokens")
            .and_then(serde_json::Value::as_u64),
        input_token_details: usage.get("input_tokens_details").cloned(),
        output_token_details: usage.get("output_tokens_details").cloned(),
    })
}

async fn send_chat_completions_request(
    url: &String,
    model: &String,
    content_parts: &[ContentPart],
    timeout_in_sec: u64,
    token: &String,
    response_format: serde_json::Value,
    temperature: Option<f64>,
) -> Result<ProviderTextResponse, ProviderError> {
    let client = ClientBuilder::new()
        .timeout(Duration::from_secs(timeout_in_sec))
        .build()
        .map_err(|error| ProviderError::from_reqwest(ProviderKind::OpenAi, error))?;

    let message = ChatRequestMessage {
        role: "user".to_string(),
        content: chat_request_content_parts(content_parts),
    };

    let request = ChatCompletionsRequest {
        model: model.clone(),
        messages: vec![message],
        temperature,
        response_format,
    };

    let http_resp = client
        .post(url)
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .json(&request)
        .send()
        .await
        .map_err(|error| ProviderError::from_reqwest(ProviderKind::OpenAi, error))?;

    let response_id = http_resp
        .headers()
        .get("x-request-id")
        .or_else(|| http_resp.headers().get("request-id"))
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let status = http_resp.status();
    let body_bytes = http_resp
        .bytes()
        .await
        .map_err(|error| ProviderError::from_reqwest(ProviderKind::OpenAi, error))?;

    if !status.is_success() {
        return Err(ProviderError::from_http_status(
            ProviderKind::OpenAi,
            status,
            &super::error::sanitized_http_error_body(ProviderKind::OpenAi, &body_bytes),
        )
        .with_request_id(response_id.as_deref(), token));
    }

    let facts = super::runtime::ProviderFacts::from_body(&body_bytes, ProviderKind::OpenAi)
        .with_request_id(response_id.as_deref())
        .redact_token(token);
    (|| {
        let response: ChatCompletionsResponse = match serde_json::from_slice(&body_bytes) {
            Ok(resp) => resp,
            Err(error) => {
                return Err(ProviderError::invalid_response(
                    ProviderKind::OpenAi,
                    format!(
                        "Failed to parse JSON at line {} column {}.",
                        error.line(),
                        error.column()
                    ),
                ));
            }
        };

        let ChatCompletionsResponse { choices, usage, .. } = response;
        let usage = normalize_chat_usage(usage);
        match choices.first() {
            Some(choice) => Ok(ProviderTextResponse {
                provider_request_id: None,
                finish_reason: None,
                resolved_model: None,
                text: choice.message.content.clone(),
                usage,
            }),
            None => Err(ProviderError::invalid_response(
                ProviderKind::OpenAi,
                "No ChatGPT response choice at index 0.",
            )),
        }
    })()
    .map(|response| facts.text(response))
    .map_err(|error: ProviderError| facts.error(error.redact_token(token)))
}

async fn send_chatgpt_codex_responses_request(
    url: &String,
    model: &String,
    content_parts: &[ContentPart],
    timeout_in_sec: u64,
    token: &String,
    response_format: serde_json::Value,
) -> Result<ProviderTextResponse, ProviderError> {
    let client = ClientBuilder::new()
        .timeout(Duration::from_secs(timeout_in_sec))
        .build()
        .map_err(|error| ProviderError::from_reqwest(ProviderKind::OpenAi, error))?;

    let request_payload = serde_json::json!({
        "model": model,
        "instructions": "Return a valid response for the provided prompt.",
        "input": [
            {
                "role": "user",
                "content": responses_request_content_parts(content_parts)
            }
        ],
        "text": {
            "format": responses_text_format_from_chat_response_format(&response_format)
        },
        "store": false,
        "stream": true
    });

    let endpoint = normalize_chatgpt_responses_url(url);
    let http_resp = client
        .post(endpoint.as_str())
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .json(&request_payload)
        .send()
        .await
        .map_err(|error| ProviderError::from_reqwest(ProviderKind::OpenAi, error))?;

    let response_id = http_resp
        .headers()
        .get("x-request-id")
        .or_else(|| http_resp.headers().get("request-id"))
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let status = http_resp.status();
    if !status.is_success() {
        let raw = http_resp
            .text()
            .await
            .map_err(|error| ProviderError::from_reqwest(ProviderKind::OpenAi, error))?;
        return Err(ProviderError::from_http_status(
            ProviderKind::OpenAi,
            status,
            &super::error::sanitized_http_error_body(ProviderKind::OpenAi, raw.as_bytes()),
        )
        .with_request_id(response_id.as_deref(), token));
    }

    let raw_stream = http_resp
        .text()
        .await
        .map_err(|error| ProviderError::from_reqwest(ProviderKind::OpenAi, error))?;

    let mut facts = super::runtime::ProviderFacts::default()
        .with_request_id(response_id.as_deref())
        .redact_token(token);
    (|| {
        let mut accumulated_text = String::new();
        let mut completed_text: Option<String> = None;
        let mut usage: Option<ProviderUsage> = None;

        for raw_line in raw_stream.lines() {
            let line = raw_line.trim_end_matches('\r');
            let Some(payload) = line.strip_prefix("data: ") else {
                continue;
            };
            if payload.trim().is_empty() || payload.trim() == "[DONE]" {
                continue;
            }

            let event_json =
                serde_json::from_str::<serde_json::Value>(payload).map_err(|error| {
                    ProviderError::invalid_response(
                        ProviderKind::OpenAi,
                        format!(
                            "Failed to parse streaming JSON at line {} column {}.",
                            error.line(),
                            error.column()
                        ),
                    )
                })?;

            facts.merge_value(&event_json, token);
            let event_type = event_json
                .get("type")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");

            if let Some(event_usage) = normalize_responses_usage(&event_json) {
                usage = Some(event_usage);
            }

            if event_type == "response.output_text.delta" {
                if let Some(delta) = event_json.get("delta").and_then(serde_json::Value::as_str) {
                    accumulated_text.push_str(delta);
                }
                continue;
            }

            if event_type == "response.output_text.done" {
                if let Some(text) = event_json
                    .get("text")
                    .and_then(serde_json::Value::as_str)
                    .map(str::trim)
                    .filter(|text| !text.is_empty())
                {
                    completed_text = Some(text.to_string());
                }
                continue;
            }

            if event_type == "response.failed" || event_type == "error" {
                let detail = parse_stream_failure_message(&event_json)
                    .unwrap_or_else(|| "No error details were provided.".to_string());
                return Err(ProviderError::invalid_response(
                    ProviderKind::OpenAi,
                    format!("OpenAI stream failed: {detail}"),
                ));
            }
        }

        if let Some(done) = completed_text {
            return Ok(ProviderTextResponse {
                provider_request_id: None,
                finish_reason: None,
                resolved_model: None,
                text: done,
                usage,
            });
        }

        let fallback = accumulated_text.trim().to_string();
        if !fallback.is_empty() {
            return Ok(ProviderTextResponse {
                provider_request_id: None,
                finish_reason: None,
                resolved_model: None,
                text: fallback,
                usage,
            });
        }

        Err(ProviderError::invalid_response(
            ProviderKind::OpenAi,
            "OpenAI stream completed without output text.",
        ))
    })()
    .map(|response| facts.text(response))
    .map_err(|error: ProviderError| facts.error(error.redact_token(token)))
}

async fn send_chatgpt_codex_image_request(
    url: &String,
    model: &String,
    prompt: &str,
    timeout_in_sec: u64,
    token: &String,
    output_format: &str,
    reference_images: &[ImageReference],
) -> Result<ProviderImageResponse, ProviderError> {
    let client = ClientBuilder::new()
        .timeout(Duration::from_secs(timeout_in_sec))
        .build()
        .map_err(|error| ProviderError::from_reqwest(ProviderKind::OpenAi, error))?;

    let mut content = Vec::with_capacity(reference_images.len() + 1);
    content.push(serde_json::json!({
        "type": "input_text",
        "text": prompt
    }));
    for reference_image in reference_images {
        content.push(serde_json::json!({
            "type": "input_image",
            "image_url": reference_image.data_url.clone()
        }));
    }

    let request_payload = serde_json::json!({
        "model": model,
        "instructions": "Generate the requested image and return it using the image_generation tool.",
        "input": [
            {
                "role": "user",
                "content": content
            }
        ],
        "tools": [
            {
                "type": "image_generation",
                "output_format": output_format
            }
        ],
        "store": false,
        "stream": true
    });

    let endpoint = normalize_chatgpt_responses_url(url);
    let http_resp = client
        .post(endpoint.as_str())
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .json(&request_payload)
        .send()
        .await
        .map_err(|error| ProviderError::from_reqwest(ProviderKind::OpenAi, error))?;

    let response_id = http_resp
        .headers()
        .get("x-request-id")
        .or_else(|| http_resp.headers().get("request-id"))
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let status = http_resp.status();
    let raw_stream = http_resp
        .text()
        .await
        .map_err(|error| ProviderError::from_reqwest(ProviderKind::OpenAi, error))?;

    if !status.is_success() {
        return Err(ProviderError::from_http_status(
            ProviderKind::OpenAi,
            status,
            &super::error::sanitized_http_error_body(ProviderKind::OpenAi, raw_stream.as_bytes()),
        )
        .with_request_id(response_id.as_deref(), token));
    }

    let mut facts = super::runtime::ProviderFacts::default()
        .with_request_id(response_id.as_deref())
        .redact_token(token);
    (|| {
        let mut encoded_image: Option<String> = None;
        let mut usage: Option<ProviderUsage> = None;

        for raw_line in raw_stream.lines() {
            let line = raw_line.trim_end_matches('\r');
            let Some(payload) = line.strip_prefix("data: ") else {
                continue;
            };
            if payload.trim().is_empty() || payload.trim() == "[DONE]" {
                continue;
            }

            let event_json =
                serde_json::from_str::<serde_json::Value>(payload).map_err(|error| {
                    ProviderError::invalid_response(
                        ProviderKind::OpenAi,
                        format!(
                            "Failed to parse streaming JSON at line {} column {}.",
                            error.line(),
                            error.column()
                        ),
                    )
                })?;

            facts.merge_value(&event_json, token);
            if let Some(event_usage) = normalize_responses_usage(&event_json) {
                usage = Some(event_usage);
            }
            if let Some(result) = find_image_generation_result(&event_json) {
                encoded_image = Some(result.to_string());
                continue;
            }

            facts.merge_value(&event_json, token);
            let event_type = event_json
                .get("type")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");

            if let Some(event_usage) = normalize_responses_usage(&event_json) {
                usage = Some(event_usage);
            }

            if event_type == "response.failed" || event_type == "error" {
                let detail = parse_stream_failure_message(&event_json)
                    .unwrap_or_else(|| "No error details were provided.".to_string());
                return Err(ProviderError::invalid_response(
                    ProviderKind::OpenAi,
                    format!("OpenAI stream failed: {detail}"),
                ));
            }
        }

        let encoded_image = encoded_image.ok_or_else(|| {
            ProviderError::invalid_response(
            ProviderKind::OpenAi,
            "Image generation stream did not include an `image_generation_call` result payload.",
        )
        })?;

        let bytes = BASE64_STANDARD.decode(encoded_image).map_err(|error| {
            ProviderError::invalid_response(
                ProviderKind::OpenAi,
                format!("Failed to decode generated image bytes: {error}"),
            )
        })?;

        Ok(ProviderImageResponse {
            resolved_model: None,
            provider_request_id: None,
            finish_reason: None,
            bytes,
            usage,
        })
    })()
    .map(|response| facts.image(response))
    .map_err(|error: ProviderError| facts.error(error.redact_token(token)))
}

async fn send_image_edit_request(
    url: &String,
    model: &String,
    prompt: &str,
    timeout_in_sec: u64,
    token: &String,
    output_format: &str,
    reference_images: &[ImageReference],
) -> Result<ProviderImageResponse, ProviderError> {
    let endpoint = normalize_openai_image_edits_url(url)?;
    let client = ClientBuilder::new()
        .timeout(Duration::from_secs(timeout_in_sec))
        .build()
        .map_err(|error| ProviderError::from_reqwest(ProviderKind::OpenAi, error))?;

    let boundary = multipart_boundary();
    let body = image_edit_multipart_body(
        boundary.as_str(),
        model,
        prompt,
        output_format,
        reference_images,
    );

    let http_resp = client
        .post(endpoint.as_str())
        .header("Authorization", format!("Bearer {}", token))
        .header(
            "Content-Type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(body)
        .send()
        .await
        .map_err(|error| ProviderError::from_reqwest(ProviderKind::OpenAi, error))?;

    decode_image_generation_response(http_resp, token).await
}

fn multipart_boundary() -> String {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    format!("cargo-ai-image-{millis}-{}", std::process::id())
}

fn image_edit_multipart_body(
    boundary: &str,
    model: &str,
    prompt: &str,
    output_format: &str,
    reference_images: &[ImageReference],
) -> Vec<u8> {
    let mut body = Vec::new();
    push_multipart_text(&mut body, boundary, "model", model);
    push_multipart_text(&mut body, boundary, "prompt", prompt);
    push_multipart_text(&mut body, boundary, "output_format", output_format);

    for reference_image in reference_images {
        push_multipart_file(
            &mut body,
            boundary,
            "image[]",
            reference_image.filename.as_str(),
            reference_image.media_type.as_str(),
            &reference_image.bytes,
        );
    }

    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    body
}

fn push_multipart_text(body: &mut Vec<u8>, boundary: &str, name: &str, value: &str) {
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n").as_bytes(),
    );
    body.extend_from_slice(value.as_bytes());
    body.extend_from_slice(b"\r\n");
}

fn push_multipart_file(
    body: &mut Vec<u8>,
    boundary: &str,
    name: &str,
    filename: &str,
    media_type: &str,
    bytes: &[u8],
) {
    let filename = sanitize_multipart_filename(filename);
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        format!("Content-Disposition: form-data; name=\"{name}\"; filename=\"{filename}\"\r\n")
            .as_bytes(),
    );
    body.extend_from_slice(format!("Content-Type: {media_type}\r\n\r\n").as_bytes());
    body.extend_from_slice(bytes);
    body.extend_from_slice(b"\r\n");
}

fn sanitize_multipart_filename(filename: &str) -> String {
    filename
        .chars()
        .map(|ch| match ch {
            '"' | '\\' | '\r' | '\n' => '_',
            other => other,
        })
        .collect()
}

async fn decode_image_generation_response(
    http_resp: reqwest::Response,
    token: &str,
) -> Result<ProviderImageResponse, ProviderError> {
    let response_id = http_resp
        .headers()
        .get("x-request-id")
        .or_else(|| http_resp.headers().get("request-id"))
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let status = http_resp.status();
    let body_bytes = http_resp
        .bytes()
        .await
        .map_err(|error| ProviderError::from_reqwest(ProviderKind::OpenAi, error))?;

    if !status.is_success() {
        return Err(ProviderError::from_http_status(
            ProviderKind::OpenAi,
            status,
            &super::error::sanitized_http_error_body(ProviderKind::OpenAi, &body_bytes),
        )
        .with_request_id(response_id.as_deref(), token));
    }

    let facts = super::runtime::ProviderFacts::from_body(&body_bytes, ProviderKind::OpenAi)
        .with_request_id(response_id.as_deref())
        .redact_token(token);
    (|| {
        let response: ImageGenerationResponse =
            serde_json::from_slice(&body_bytes).map_err(|error| {
                ProviderError::invalid_response(
                    ProviderKind::OpenAi,
                    format!(
                        "Failed to parse image-generation JSON at line {} column {}.",
                        error.line(),
                        error.column()
                    ),
                )
            })?;

        let encoded_image = response
            .data
            .first()
            .map(|image| image.b64_json.trim())
            .filter(|image| !image.is_empty())
            .ok_or_else(|| {
                ProviderError::invalid_response(
                    ProviderKind::OpenAi,
                    "Image generation response did not include `data[0].b64_json`.",
                )
            })?;

        let bytes = BASE64_STANDARD.decode(encoded_image).map_err(|error| {
            ProviderError::invalid_response(
                ProviderKind::OpenAi,
                format!("Failed to decode generated image bytes: {error}"),
            )
        })?;

        Ok(ProviderImageResponse {
            resolved_model: None,
            provider_request_id: None,
            finish_reason: None,
            bytes,
            usage: normalize_image_usage(response.usage),
        })
    })()
    .map(|response| facts.image(response))
    .map_err(|error: ProviderError| facts.error(error.redact_token(token)))
}

pub async fn send_request(
    url: &String,
    model: &String,
    content_parts: &[ContentPart],
    timeout_in_sec: u64,
    token: &String,
    response_format: serde_json::Value,
    temperature: Option<f64>,
) -> Result<ProviderTextResponse, ProviderError> {
    if is_chatgpt_codex_responses_endpoint(url) {
        if temperature.is_some() {
            return Err(ProviderError::invalid_request(ProviderKind::OpenAi, "Explicit profile temperature is unsupported by the OpenAI account transport; clear it with `profile set <name> --clear-temperature`."));
        }
        send_chatgpt_codex_responses_request(
            url,
            model,
            content_parts,
            timeout_in_sec,
            token,
            response_format,
        )
        .await
    } else {
        send_chat_completions_request(
            url,
            model,
            content_parts,
            timeout_in_sec,
            token,
            response_format,
            temperature,
        )
        .await
    }
}

pub async fn send_image_request(
    url: &String,
    model: &String,
    prompt: &str,
    timeout_in_sec: u64,
    token: &String,
    output_format: &str,
    reference_images: &[ImageReference],
) -> Result<ProviderImageResponse, ProviderError> {
    if is_chatgpt_codex_responses_endpoint(url) {
        return send_chatgpt_codex_image_request(
            url,
            model,
            prompt,
            timeout_in_sec,
            token,
            output_format,
            reference_images,
        )
        .await;
    }

    if !reference_images.is_empty() {
        return send_image_edit_request(
            url,
            model,
            prompt,
            timeout_in_sec,
            token,
            output_format,
            reference_images,
        )
        .await;
    }

    let endpoint = normalize_openai_images_url(url)?;
    let client = ClientBuilder::new()
        .timeout(Duration::from_secs(timeout_in_sec))
        .build()
        .map_err(|error| ProviderError::from_reqwest(ProviderKind::OpenAi, error))?;

    let request = ImageGenerationRequest {
        model: model.clone(),
        prompt: prompt.to_string(),
        n: 1,
        output_format: output_format.to_string(),
    };

    let http_resp = client
        .post(endpoint.as_str())
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .json(&request)
        .send()
        .await
        .map_err(|error| ProviderError::from_reqwest(ProviderKind::OpenAi, error))?;

    decode_image_generation_response(http_resp, token).await
}

fn chat_request_content_parts(content_parts: &[ContentPart]) -> Vec<ChatRequestContentPart> {
    content_parts
        .iter()
        .map(|part| match part {
            ContentPart::Text(text) => ChatRequestContentPart::Text { text: text.clone() },
            ContentPart::Image { data_url } => ChatRequestContentPart::ImageUrl {
                image_url: ImageUrl {
                    url: data_url.clone(),
                },
            },
            ContentPart::File {
                filename,
                file_data,
            } => ChatRequestContentPart::File {
                file: FileInput {
                    filename: filename.clone(),
                    file_data: file_data.clone(),
                },
            },
        })
        .collect()
}

fn responses_request_content_parts(content_parts: &[ContentPart]) -> Vec<serde_json::Value> {
    content_parts
        .iter()
        .map(|part| match part {
            ContentPart::Text(text) => serde_json::json!({
                "type": "input_text",
                "text": text
            }),
            ContentPart::Image { data_url } => serde_json::json!({
                "type": "input_image",
                "image_url": data_url
            }),
            ContentPart::File {
                filename,
                file_data,
            } => serde_json::json!({
                "type": "input_file",
                "filename": filename,
                "file_data": file_data
            }),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        chat_request_content_parts, responses_request_content_parts, send_image_request,
        send_request, ChatRequestContentPart,
    };
    use crate::providers::runtime::{ContentPart, ImageReference};
    use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};

    #[tokio::test]
    async fn request_metadata_and_errors_reject_credential_echoes() {
        let token = "sk-fixture-credential".to_string();
        for echo in [
            token.clone(),
            format!("req-{token}"),
            format!("{token}-model"),
            format!("req-{token}-end"),
        ] {
            for image in [false, true] {
                for outcome in ["success", "invalid", "http_error"] {
                    let mut server = mockito::Server::new_async().await;
                    let mut body = serde_json::json!({
                        "id": echo, "model": echo, "status": echo, "object": "chat.completion", "created": 1,
                        "usage": {"input_tokens": 7, "output_tokens": 3, "total_tokens": 10},
                        "choices": [{"index": 0, "message": {"role": "assistant", "content": "safe answer"}, "finish_reason": echo}],
                        "data": [{"b64_json": BASE64_STANDARD.encode(b"safe image")}]
                    });
                    if outcome == "invalid" {
                        body["choices"] = serde_json::json!("private-response-payload");
                        body["data"] = serde_json::json!("private-response-payload");
                    } else if outcome == "http_error" {
                        body = serde_json::json!({"error": {"message": format!("failed-{echo}")}, "payload": "private-response-payload"});
                    }
                    let mock = server
                        .mock(
                            "POST",
                            if image {
                                "/v1/images/generations"
                            } else {
                                "/v1/chat/completions"
                            },
                        )
                        .match_header("authorization", format!("Bearer {token}").as_str())
                        .with_status(if outcome == "http_error" { 400 } else { 200 })
                        .with_header("x-request-id", &echo)
                        .with_body(body.to_string())
                        .create_async()
                        .await;
                    let url = format!("{}/v1/chat/completions", server.url());
                    let model = "requested-model".to_string();
                    // This is the metadata boundary consumed by history/export and backup.
                    let metadata = if image {
                        send_image_request(&url, &model, "private-prompt", 10, &token, "png", &[]).await.map(|response| {
                            serde_json::json!({"model": response.resolved_model, "request_id": response.provider_request_id, "finish_reason": response.finish_reason, "usage": response.usage})
                        })
                    } else {
                        send_request(&url, &model, &[ContentPart::Text("private-prompt".into())], 10, &token, serde_json::json!({"type":"json_object"}), None).await.map(|response| {
                            serde_json::json!({"model": response.resolved_model, "request_id": response.provider_request_id, "finish_reason": response.finish_reason, "usage": response.usage})
                        })
                    };
                    mock.assert_async().await;
                    let metadata = if outcome == "success" {
                        metadata.expect("valid response")
                    } else {
                        let error = metadata.expect_err("invalid response");
                        assert!(!error.message().contains(&token));
                        assert!(!error.message().contains("private-response-payload"));
                        serde_json::json!({"model": error.resolved_model, "request_id": error.provider_request_id, "finish_reason": error.finish_reason, "usage": error.usage})
                    };
                    let serialized = metadata.to_string();
                    for excluded in [
                        &token,
                        "private-prompt",
                        "private-response-payload",
                        "safe answer",
                    ] {
                        assert!(!serialized.contains(excluded), "{image}/{outcome}");
                    }
                    assert!(metadata["model"].is_null());
                    assert!(metadata["request_id"].is_null());
                    assert!(metadata["finish_reason"].is_null());
                    if outcome != "http_error" {
                        assert_eq!(metadata["usage"]["total_tokens"], 10);
                    }
                }
            }
        }
    }

    #[test]
    fn chat_request_content_parts_encode_pdf_files() {
        let parts = chat_request_content_parts(&[ContentPart::File {
            filename: "report.pdf".to_string(),
            file_data: "data:application/pdf;base64,JVBERi0xLjQK".to_string(),
        }]);

        match &parts[0] {
            ChatRequestContentPart::File { file } => {
                assert_eq!(file.filename, "report.pdf");
                assert_eq!(file.file_data, "data:application/pdf;base64,JVBERi0xLjQK");
            }
            other => panic!("expected file content part, got {other:?}"),
        }
    }

    #[test]
    fn responses_request_content_parts_encode_pdf_files() {
        let parts = responses_request_content_parts(&[ContentPart::File {
            filename: "report.pdf".to_string(),
            file_data: "data:application/pdf;base64,JVBERi0xLjQK".to_string(),
        }]);

        assert_eq!(
            parts,
            vec![serde_json::json!({
                "type": "input_file",
                "filename": "report.pdf",
                "file_data": "data:application/pdf;base64,JVBERi0xLjQK"
            })]
        );
    }

    #[tokio::test]
    async fn parses_chatgpt_codex_stream_done_payload() {
        let mut server = mockito::Server::new_async().await;
        let stream_body = concat!(
            "event: response.output_text.delta\n",
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"{\\\"answer\\\":\\\"\"}\n\n",
            "event: response.output_text.delta\n",
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"hi\"}\n\n",
            "event: response.output_text.done\n",
            "data: {\"type\":\"response.output_text.done\",\"text\":\"{\\\"answer\\\":\\\"hi\\\"}\"}\n\n"
        );

        let _mock = server
            .mock("POST", "/chatgpt.com/backend-api/codex/responses")
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(stream_body)
            .create_async()
            .await;

        let url = format!("{}/chatgpt.com/backend-api/codex", server.url());
        let model = "gpt-5".to_string();
        let content_parts = vec![ContentPart::Text("return json".to_string())];
        let token = "test-token".to_string();
        let response_format = serde_json::json!({
            "type": "json_schema",
            "json_schema": {
                "name": "Output",
                "schema": {
                    "type": "object",
                    "properties": {
                        "answer": { "type": "string" }
                    },
                    "required": ["answer"],
                    "additionalProperties": false
                },
                "strict": true
            }
        });

        let response = send_request(
            &url,
            &model,
            &content_parts,
            10,
            &token,
            response_format,
            None,
        )
        .await
        .expect("stream response should parse");
        assert_eq!(response.text, "{\"answer\":\"hi\"}");
    }

    #[tokio::test]
    async fn image_request_uses_images_endpoint_and_decodes_bytes() {
        let mut server = mockito::Server::new_async().await;
        let expected_bytes = b"fake-png";
        let encoded_image = BASE64_STANDARD.encode(expected_bytes);
        let _mock = server
            .mock("POST", "/v1/images/generations")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(format!(
                r#"{{"data":[{{"b64_json":"{}"}}]}}"#,
                encoded_image
            ))
            .create_async()
            .await;

        let url = format!("{}/v1/chat/completions", server.url());
        let model = "gpt-image-1".to_string();
        let token = "test-token".to_string();

        let image = send_image_request(&url, &model, "draw a square", 10, &token, "png", &[])
            .await
            .expect("image request should decode");

        assert_eq!(image.bytes, expected_bytes);
        assert_eq!(image.usage, None);
    }

    #[tokio::test]
    async fn image_request_with_references_uses_image_edits_endpoint() {
        let mut server = mockito::Server::new_async().await;
        let expected_bytes = b"fake-png-edit";
        let encoded_image = BASE64_STANDARD.encode(expected_bytes);
        let _mock = server
            .mock("POST", "/v1/images/edits")
            .match_header(
                "content-type",
                mockito::Matcher::Regex("multipart/form-data; boundary=".into()),
            )
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(format!(
                r#"{{"data":[{{"b64_json":"{}"}}]}}"#,
                encoded_image
            ))
            .create_async()
            .await;

        let url = format!("{}/v1/chat/completions", server.url());
        let model = "gpt-image-2".to_string();
        let token = "test-token".to_string();
        let reference_images = vec![ImageReference {
            source: "./reference.png".to_string(),
            filename: "reference.png".to_string(),
            media_type: "image/png".to_string(),
            data_url: "data:image/png;base64,ZmFrZS1yZWY=".to_string(),
            bytes: b"fake-ref".to_vec(),
        }];

        let image = send_image_request(
            &url,
            &model,
            "draw a square using the reference",
            10,
            &token,
            "png",
            &reference_images,
        )
        .await
        .expect("image edit request should decode");

        assert_eq!(image.bytes, expected_bytes);
    }

    #[tokio::test]
    async fn image_request_uses_chatgpt_responses_tool_and_decodes_bytes() {
        let mut server = mockito::Server::new_async().await;
        let expected_bytes = b"fake-webp";
        let encoded_image = BASE64_STANDARD.encode(expected_bytes);
        let _mock = server
            .mock("POST", "/chatgpt.com/backend-api/codex/responses")
            .match_body(mockito::Matcher::PartialJson(serde_json::json!({
                "model": "gpt-5.2",
                "tools": [
                    {
                        "type": "image_generation",
                        "output_format": "webp"
                    }
                ],
                "store": false,
                "stream": true
            })))
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(format!(
                "event: response.output_item.done\n\
data: {{\"item\":{{\"type\":\"image_generation_call\",\"result\":\"{}\"}}}}\n\n\
data: [DONE]\n",
                encoded_image,
            ))
            .create_async()
            .await;

        let url = format!("{}/chatgpt.com/backend-api/codex", server.url());
        let model = "gpt-5.2".to_string();
        let token = "test-token".to_string();

        let image = send_image_request(&url, &model, "draw a square", 10, &token, "webp", &[])
            .await
            .expect("chatgpt account transport should decode image bytes");

        assert_eq!(image.bytes, expected_bytes);
    }

    #[tokio::test]
    async fn chatgpt_image_request_sends_reference_images_as_input_images() {
        let mut server = mockito::Server::new_async().await;
        let expected_bytes = b"fake-webp-reference";
        let encoded_image = BASE64_STANDARD.encode(expected_bytes);
        let _mock = server
            .mock("POST", "/chatgpt.com/backend-api/codex/responses")
            .match_body(mockito::Matcher::Regex("\"type\":\"input_image\"".into()))
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(format!(
                "event: response.output_item.done\n\
data: {{\"item\":{{\"type\":\"image_generation_call\",\"result\":\"{}\"}}}}\n\n\
data: [DONE]\n",
                encoded_image,
            ))
            .create_async()
            .await;

        let url = format!("{}/chatgpt.com/backend-api/codex", server.url());
        let model = "gpt-5.2".to_string();
        let token = "test-token".to_string();
        let reference_images = vec![ImageReference {
            source: "./reference.png".to_string(),
            filename: "reference.png".to_string(),
            media_type: "image/png".to_string(),
            data_url: "data:image/png;base64,ZmFrZS1yZWY=".to_string(),
            bytes: b"fake-ref".to_vec(),
        }];

        let image = send_image_request(
            &url,
            &model,
            "draw a square using the reference",
            10,
            &token,
            "webp",
            &reference_images,
        )
        .await
        .expect("chatgpt account transport should include image references");

        assert_eq!(image.bytes, expected_bytes);
    }
}

#[cfg(test)]
mod temperature_tests {
    #[test]
    fn temperature_request_omission_and_explicit_values() {
        for temperature in [None, Some(0.0), Some(0.7)] {
            let request = super::ChatCompletionsRequest {
                model: "gpt-5-example".into(),
                messages: vec![],
                temperature,
                response_format: serde_json::json!({}),
            };
            let value = serde_json::to_value(request).unwrap();
            match temperature {
                None => assert!(value.get("temperature").is_none()),
                Some(expected) => assert_eq!(value["temperature"], expected),
            }
        }
    }
}
