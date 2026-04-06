// External Crates
use super::{runtime::ContentPart, ProviderError, ProviderKind};
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use reqwest::ClientBuilder;
use serde::{Deserialize, Serialize};
use std::time::Duration;

const DEFAULT_IMAGE_SIZE: &str = "1024x1024";

#[derive(Serialize, Debug)]
struct Request {
    model: String,
    messages: Vec<RequestMessage>,
    temperature: f64,
    response_format: serde_json::Value,
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
    #[allow(dead_code)]
    role: String,
    content: String,
}

#[derive(Deserialize, Debug)]
struct Choice {
    message: ResponseMessage,
}

#[derive(Deserialize, Debug)]
struct Response {
    choices: Vec<Choice>,
}

#[derive(Serialize, Debug)]
struct ImageGenerationRequest {
    model: String,
    prompt: String,
    n: u8,
    size: String,
    response_format: String,
}

#[derive(Deserialize, Debug)]
struct ImageGenerationResponse {
    data: Vec<ImageGenerationData>,
}

#[derive(Deserialize, Debug)]
struct ImageGenerationData {
    b64_json: String,
}

fn normalize_response_format(response_format: serde_json::Value) -> serde_json::Value {
    if response_format
        .get("type")
        .and_then(serde_json::Value::as_str)
        == Some("json_schema")
    {
        return response_format;
    }

    serde_json::json!({
        "type": "json_schema",
        "json_schema": {
            "name": "Output",
            "schema": response_format,
            "strict": true
        }
    })
}

pub async fn send_request(
    url: &String,
    model: &String,
    content_parts: &[ContentPart],
    timeout_in_sec: u64,
    response_format: serde_json::Value,
) -> Result<String, ProviderError> {
    let request = Request {
        model: model.clone(),
        messages: vec![RequestMessage {
            role: "user".to_string(),
            content: request_content_parts(content_parts),
        }],
        temperature: super::DEFAULT_TEMPERATURE,
        response_format: normalize_response_format(response_format),
    };

    let client = ClientBuilder::new()
        .timeout(Duration::from_secs(timeout_in_sec))
        .build()
        .map_err(|error| ProviderError::from_reqwest(ProviderKind::Ollama, error))?;

    let http_resp = client
        .post(url)
        .json(&request)
        .send()
        .await
        .map_err(|error| ProviderError::from_reqwest(ProviderKind::Ollama, error))?;

    let status = http_resp.status();
    let body_bytes = http_resp
        .bytes()
        .await
        .map_err(|error| ProviderError::from_reqwest(ProviderKind::Ollama, error))?;

    if !status.is_success() {
        let raw = String::from_utf8_lossy(&body_bytes);
        return Err(ProviderError::from_http_status(
            ProviderKind::Ollama,
            status,
            &raw,
        ));
    }

    let reply: Response = match serde_json::from_slice(&body_bytes) {
        Ok(resp) => resp,
        Err(error) => {
            let raw = String::from_utf8_lossy(&body_bytes);
            return Err(ProviderError::invalid_response(
                ProviderKind::Ollama,
                format!("Failed to parse JSON: {error}\nRaw response:\n{raw}"),
            ));
        }
    };

    match reply.choices.first() {
        Some(choice) => Ok(choice.message.content.clone()),
        None => Err(ProviderError::invalid_response(
            ProviderKind::Ollama,
            "Ollama returned no chat completion choices.",
        )),
    }
}

fn normalize_images_url(url: &str) -> String {
    let trimmed = url.trim_end_matches('/');
    if let Some(index) = trimmed.find("/v1/") {
        return format!("{}/v1/images/generations", &trimmed[..index]);
    }

    if trimmed.ends_with("/v1") {
        format!("{trimmed}/images/generations")
    } else {
        format!("{trimmed}/v1/images/generations")
    }
}

pub async fn send_image_request(
    url: &String,
    model: &String,
    prompt: &str,
    timeout_in_sec: u64,
    token: &str,
) -> Result<Vec<u8>, ProviderError> {
    let request = ImageGenerationRequest {
        model: model.clone(),
        prompt: prompt.to_string(),
        n: 1,
        size: DEFAULT_IMAGE_SIZE.to_string(),
        response_format: "b64_json".to_string(),
    };

    let client = ClientBuilder::new()
        .timeout(Duration::from_secs(timeout_in_sec))
        .build()
        .map_err(|error| ProviderError::from_reqwest(ProviderKind::Ollama, error))?;

    let endpoint = normalize_images_url(url);
    let mut request_builder = client
        .post(endpoint.as_str())
        .header("Content-Type", "application/json");
    if !token.trim().is_empty() {
        request_builder = request_builder.header("Authorization", format!("Bearer {}", token));
    }

    let http_resp = request_builder
        .json(&request)
        .send()
        .await
        .map_err(|error| ProviderError::from_reqwest(ProviderKind::Ollama, error))?;

    let status = http_resp.status();
    let body_bytes = http_resp
        .bytes()
        .await
        .map_err(|error| ProviderError::from_reqwest(ProviderKind::Ollama, error))?;

    if !status.is_success() {
        let raw = String::from_utf8_lossy(&body_bytes);
        return Err(ProviderError::from_http_status(
            ProviderKind::Ollama,
            status,
            &raw,
        ));
    }

    let response: ImageGenerationResponse =
        serde_json::from_slice(&body_bytes).map_err(|error| {
            let raw = String::from_utf8_lossy(&body_bytes);
            ProviderError::invalid_response(
                ProviderKind::Ollama,
                format!("Failed to parse image-generation JSON: {error}\nRaw response:\n{raw}"),
            )
        })?;

    let encoded_image = response
        .data
        .first()
        .map(|image| image.b64_json.trim())
        .filter(|image| !image.is_empty())
        .ok_or_else(|| {
            ProviderError::invalid_response(
                ProviderKind::Ollama,
                "Image generation response did not include `data[0].b64_json`.",
            )
        })?;

    BASE64_STANDARD.decode(encoded_image).map_err(|error| {
        ProviderError::invalid_response(
            ProviderKind::Ollama,
            format!("Failed to decode generated image bytes: {error}"),
        )
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_plain_schema_response_format_for_ollama_chat_completions() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "ok": { "type": "boolean" }
            },
            "required": ["ok"]
        });

        let wrapped = normalize_response_format(schema.clone());
        assert_eq!(
            wrapped,
            serde_json::json!({
                "type": "json_schema",
                "json_schema": {
                    "name": "Output",
                    "schema": schema,
                    "strict": true
                }
            })
        );
    }
}
