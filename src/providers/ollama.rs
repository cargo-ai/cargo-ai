// External Crates
use super::{runtime::ContentPart, ProviderError, ProviderKind};
use reqwest::ClientBuilder;
use serde::{Deserialize, Serialize};
use std::time::Duration;

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
}

#[derive(Serialize, Debug)]
struct ImageUrl {
    url: String,
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
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use mockito::Server;

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

    #[tokio::test]
    async fn test_send_request_with_mock() {
        let mut server = Server::new_async().await;
        let mock_path = "/v1/chat/completions";

        let _m = server
            .mock("POST", mock_path)
            .match_header("content-type", "application/json")
            .with_status(200)
            .with_body(
                r#"{
                 "choices": [
                    {
                        "message": {
                            "role": "assistant",
                            "content": "Mocked response"
                        }
                    }
                 ]
             }"#,
            )
            .create();

        let result = send_request(
            &format!("{}{}", server.url(), mock_path),
            &"test-model".to_string(),
            &[ContentPart::Text("test prompt".to_string())],
            5,
            serde_json::json!({
                "type": "json_schema",
                "json_schema": {
                    "name": "Output",
                    "schema": {
                        "type": "object",
                        "properties": { "ok": { "type": "boolean" } },
                        "required": ["ok"]
                    },
                    "strict": true
                }
            }),
        )
        .await
        .expect("send_request failed");

        assert_eq!(result, "Mocked response");
    }
}
