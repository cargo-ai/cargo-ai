// External Crates
use super::{runtime::ContentPart, ProviderError, ProviderKind};
use reqwest::ClientBuilder;
use serde::{Deserialize, Serialize};
use std::time::Duration;

const CHATGPT_CODEX_ENDPOINT_MARKER: &str = "chatgpt.com/backend-api/codex";

#[derive(Serialize, Debug)]
pub struct ChatCompletionsRequest {
    pub model: String,
    pub messages: Vec<ChatRequestMessage>,
    pub temperature: f64,
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
    pub usage: Usage,
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
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
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

async fn send_chat_completions_request(
    url: &String,
    model: &String,
    content_parts: &[ContentPart],
    timeout_in_sec: u64,
    token: &String,
    response_format: serde_json::Value,
) -> Result<String, ProviderError> {
    let client = ClientBuilder::new()
        .timeout(Duration::from_secs(timeout_in_sec))
        .build()
        .map_err(|error| ProviderError::from_reqwest(ProviderKind::OpenAi, error))?;

    let temperature = if model.starts_with("gpt-5") {
        1.0
    } else {
        super::DEFAULT_TEMPERATURE
    };

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

    let status = http_resp.status();
    let body_bytes = http_resp
        .bytes()
        .await
        .map_err(|error| ProviderError::from_reqwest(ProviderKind::OpenAi, error))?;

    if !status.is_success() {
        let raw = String::from_utf8_lossy(&body_bytes);
        return Err(ProviderError::from_http_status(
            ProviderKind::OpenAi,
            status,
            &raw,
        ));
    }

    let response: ChatCompletionsResponse = match serde_json::from_slice(&body_bytes) {
        Ok(resp) => resp,
        Err(error) => {
            let raw = String::from_utf8_lossy(&body_bytes);
            return Err(ProviderError::invalid_response(
                ProviderKind::OpenAi,
                format!("Failed to parse JSON: {error}\\nRaw response:\\n{raw}"),
            ));
        }
    };

    match response.choices.first() {
        Some(choice) => Ok(choice.message.content.clone()),
        None => Err(ProviderError::invalid_response(
            ProviderKind::OpenAi,
            "No ChatGPT response choice at index 0.",
        )),
    }
}

async fn send_chatgpt_codex_responses_request(
    url: &String,
    model: &String,
    content_parts: &[ContentPart],
    timeout_in_sec: u64,
    token: &String,
    response_format: serde_json::Value,
) -> Result<String, ProviderError> {
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

    let status = http_resp.status();
    if !status.is_success() {
        let raw = http_resp
            .text()
            .await
            .map_err(|error| ProviderError::from_reqwest(ProviderKind::OpenAi, error))?;
        return Err(ProviderError::from_http_status(
            ProviderKind::OpenAi,
            status,
            raw.as_str(),
        ));
    }

    let raw_stream = http_resp
        .text()
        .await
        .map_err(|error| ProviderError::from_reqwest(ProviderKind::OpenAi, error))?;

    let mut accumulated_text = String::new();
    let mut completed_text: Option<String> = None;

    for raw_line in raw_stream.lines() {
        let line = raw_line.trim_end_matches('\r');
        let Some(payload) = line.strip_prefix("data: ") else {
            continue;
        };
        if payload.trim().is_empty() || payload.trim() == "[DONE]" {
            continue;
        }

        let event_json = serde_json::from_str::<serde_json::Value>(payload).map_err(|error| {
            ProviderError::invalid_response(
                ProviderKind::OpenAi,
                format!("Failed to parse streaming payload: {error}\\nPayload: {payload}"),
            )
        })?;

        let event_type = event_json
            .get("type")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");

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
            let detail =
                parse_stream_failure_message(&event_json).unwrap_or_else(|| payload.to_string());
            return Err(ProviderError::invalid_response(
                ProviderKind::OpenAi,
                format!("OpenAI stream failed: {detail}"),
            ));
        }
    }

    if let Some(done) = completed_text {
        return Ok(done);
    }

    let fallback = accumulated_text.trim().to_string();
    if !fallback.is_empty() {
        return Ok(fallback);
    }

    Err(ProviderError::invalid_response(
        ProviderKind::OpenAi,
        "OpenAI stream completed without output text.",
    ))
}

pub async fn send_request(
    url: &String,
    model: &String,
    content_parts: &[ContentPart],
    timeout_in_sec: u64,
    token: &String,
    response_format: serde_json::Value,
) -> Result<String, ProviderError> {
    if is_chatgpt_codex_responses_endpoint(url) {
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
        )
        .await
    }
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
        chat_request_content_parts, responses_request_content_parts, send_request,
        ChatRequestContentPart,
    };
    use crate::providers::runtime::ContentPart;

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

        let response = send_request(&url, &model, &content_parts, 10, &token, response_format)
            .await
            .expect("stream response should parse");
        assert_eq!(response, "{\"answer\":\"hi\"}");
    }
}
