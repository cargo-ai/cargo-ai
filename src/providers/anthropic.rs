//! Anthropic Claude provider (`server=anthropic`).
//!
//! Posts to `POST https://api.anthropic.com/v1/messages` and pulls
//! structured JSON out via tool-use. Anthropic's API doesn't have
//! OpenAI's `response_format: json_schema` block; the standard way
//! to get a guaranteed-shape JSON object is to declare a single tool
//! whose `input_schema` is the schema you want and pin
//! `tool_choice` to that tool. The model is then forced to emit a
//! `tool_use` content block whose `input` matches the schema —
//! that's the agent's structured output.
//!
//! Auth uses the `x-api-key` header (not Bearer); the
//! `anthropic-version` header is required by the API. Both are
//! sent on every request.

use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use reqwest::ClientBuilder;
use serde::{Deserialize, Serialize};
use std::time::Duration;

use super::{runtime::ContentPart, ProviderError, ProviderKind};

/// API version pin sent on the `anthropic-version` header. Bump
/// deliberately when adopting new request/response fields.
const ANTHROPIC_VERSION: &str = "2023-06-01";

/// Hardcoded max tokens for the assistant reply. Anthropic's API
/// requires `max_tokens` on every request — we pick a generous
/// default since cargo-ai doesn't currently expose a per-run knob.
/// A follow-up can make this configurable alongside temperature.
const DEFAULT_MAX_TOKENS: u32 = 4096;

/// Name we give the synthetic tool that carries the structured
/// output. Stable so the response deserializer can find the right
/// content block even if the model emits other (e.g. text) blocks
/// alongside it.
const OUTPUT_TOOL_NAME: &str = "output";

#[derive(Serialize, Debug)]
struct MessagesRequest<'a> {
    model: &'a str,
    max_tokens: u32,
    temperature: f64,
    messages: Vec<MessageBlock>,
    tools: Vec<Tool<'a>>,
    tool_choice: ToolChoice<'a>,
}

#[derive(Serialize, Debug)]
struct MessageBlock {
    role: &'static str,
    content: Vec<ContentBlock>,
}

#[derive(Serialize, Debug)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ContentBlock {
    Text {
        text: String,
    },
    Image {
        source: Base64Source,
    },
    Document {
        source: Base64Source,
    },
}

#[derive(Serialize, Debug)]
struct Base64Source {
    #[serde(rename = "type")]
    kind: &'static str,
    media_type: String,
    data: String,
}

#[derive(Serialize, Debug)]
struct Tool<'a> {
    name: &'a str,
    description: &'a str,
    input_schema: serde_json::Value,
}

#[derive(Serialize, Debug)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ToolChoice<'a> {
    Tool { name: &'a str },
}

#[derive(Deserialize, Debug)]
struct MessagesResponse {
    content: Vec<ResponseContent>,
}

#[derive(Deserialize, Debug)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ResponseContent {
    Text {
        #[allow(dead_code)]
        text: String,
    },
    ToolUse {
        #[allow(dead_code)]
        name: String,
        input: serde_json::Value,
    },
    // Catch-all so unknown future content kinds (e.g. thinking
    // blocks, tool_result) don't break deserialization.
    #[serde(other)]
    Other,
}

/// Public entry. Mirrors `openai::send_request` and `ollama::send_request`
/// so the runtime dispatch in `commands/runtime.rs` can treat all
/// providers uniformly.
pub async fn send_request(
    url: &String,
    model: &String,
    content_parts: &[ContentPart],
    timeout_in_sec: u64,
    token: &String,
    response_format: serde_json::Value,
) -> Result<String, ProviderError> {
    let input_schema = extract_input_schema(&response_format)?;

    let client = ClientBuilder::new()
        .timeout(Duration::from_secs(timeout_in_sec))
        .build()
        .map_err(|error| ProviderError::from_reqwest(ProviderKind::Anthropic, error))?;

    let content = anthropic_content_parts(content_parts)?;
    let messages = vec![MessageBlock {
        role: "user",
        content,
    }];

    let request = MessagesRequest {
        model,
        max_tokens: DEFAULT_MAX_TOKENS,
        temperature: super::DEFAULT_TEMPERATURE,
        messages,
        tools: vec![Tool {
            name: OUTPUT_TOOL_NAME,
            description:
                "Return the agent's answer matching the supplied schema.",
            input_schema,
        }],
        tool_choice: ToolChoice::Tool {
            name: OUTPUT_TOOL_NAME,
        },
    };

    let http_resp = client
        .post(url)
        .header("x-api-key", token)
        .header("anthropic-version", ANTHROPIC_VERSION)
        .header("content-type", "application/json")
        .json(&request)
        .send()
        .await
        .map_err(|error| ProviderError::from_reqwest(ProviderKind::Anthropic, error))?;

    let status = http_resp.status();
    let body_bytes = http_resp
        .bytes()
        .await
        .map_err(|error| ProviderError::from_reqwest(ProviderKind::Anthropic, error))?;

    if !status.is_success() {
        let raw = String::from_utf8_lossy(&body_bytes);
        return Err(ProviderError::from_http_status(
            ProviderKind::Anthropic,
            status,
            &raw,
        ));
    }

    let response: MessagesResponse = match serde_json::from_slice(&body_bytes) {
        Ok(resp) => resp,
        Err(error) => {
            let raw = String::from_utf8_lossy(&body_bytes);
            return Err(ProviderError::invalid_response(
                ProviderKind::Anthropic,
                format!("Failed to parse JSON: {error}\nRaw response:\n{raw}"),
            ));
        }
    };

    // The forced tool_use should produce exactly one tool_use block
    // whose `input` is our structured output. Pick the first
    // tool_use block by name to stay defensive — the model can
    // emit text blocks before the tool call in some configurations,
    // and we'd rather keep working in that case than fail.
    let tool_input = response.content.iter().find_map(|block| match block {
        ResponseContent::ToolUse { name, input } if name == OUTPUT_TOOL_NAME => Some(input),
        _ => None,
    });

    match tool_input {
        Some(value) => Ok(value.to_string()),
        None => Err(ProviderError::invalid_response(
            ProviderKind::Anthropic,
            "Anthropic response had no tool_use block named '{}'; \
             confirm the model supports tool_choice forced output."
                .replace("{}", OUTPUT_TOOL_NAME),
        )),
    }
}

/// Pull the `json_schema.schema` out of cargo-ai's OpenAI-flavoured
/// response_format value. Anthropic's `tools[].input_schema` expects
/// the bare schema object (without OpenAI's `strict` / `name` wrapper).
fn extract_input_schema(
    response_format: &serde_json::Value,
) -> Result<serde_json::Value, ProviderError> {
    response_format
        .get("json_schema")
        .and_then(|js| js.get("schema"))
        .cloned()
        .ok_or_else(|| {
            ProviderError::invalid_response(
                ProviderKind::Anthropic,
                "response_format must include json_schema.schema so it can be \
                 forwarded as Anthropic tool_use input_schema",
            )
        })
}

/// Translate cargo-ai's neutral ContentPart array into Anthropic's
/// content block layout. Text passes through; Image and File
/// (assumed PDF) inputs get decoded from their data-URL prefix and
/// re-encoded into Anthropic's base64 source blocks.
fn anthropic_content_parts(
    content_parts: &[ContentPart],
) -> Result<Vec<ContentBlock>, ProviderError> {
    let mut blocks = Vec::with_capacity(content_parts.len());
    for part in content_parts {
        match part {
            ContentPart::Text(text) => {
                blocks.push(ContentBlock::Text { text: text.clone() });
            }
            ContentPart::Image { data_url } => {
                let (media_type, data) = decode_data_url(data_url)?;
                blocks.push(ContentBlock::Image {
                    source: Base64Source {
                        kind: "base64",
                        media_type,
                        data,
                    },
                });
            }
            ContentPart::File {
                file_data,
                filename: _,
            } => {
                // File inputs come in as data URLs encoding the bytes.
                // We forward them as documents (Anthropic's PDF
                // support); non-PDF media types fall through with
                // whatever the URL prefix declares, which may be
                // rejected upstream — that's the provider's call.
                let (media_type, data) = decode_data_url(file_data)?;
                blocks.push(ContentBlock::Document {
                    source: Base64Source {
                        kind: "base64",
                        media_type,
                        data,
                    },
                });
            }
        }
    }
    Ok(blocks)
}

/// Parse a `data:<media-type>;base64,<payload>` URL into its parts.
/// We re-encode to base64 in case the upstream consumed the prefix
/// (no-op when the payload is already canonical) so the Anthropic
/// source block sees clean bytes.
fn decode_data_url(data_url: &str) -> Result<(String, String), ProviderError> {
    let rest = data_url.strip_prefix("data:").ok_or_else(|| {
        ProviderError::invalid_response(
            ProviderKind::Anthropic,
            "Anthropic provider expects content data URLs of the form \
             `data:<media-type>;base64,<payload>`",
        )
    })?;
    let (header, payload) = rest.split_once(',').ok_or_else(|| {
        ProviderError::invalid_response(
            ProviderKind::Anthropic,
            "data URL missing `,` separator before its payload",
        )
    })?;
    // Strip the `;base64` suffix if present; we always send base64
    // regardless, decoding then re-encoding the payload when the
    // declared encoding is `base64` and using a fresh encode when
    // the payload is raw URL-encoded.
    let (media_type, is_base64) = match header.strip_suffix(";base64") {
        Some(prefix) => (prefix.to_string(), true),
        None => (header.to_string(), false),
    };
    let bytes = if is_base64 {
        BASE64_STANDARD.decode(payload.as_bytes()).map_err(|err| {
            ProviderError::invalid_response(
                ProviderKind::Anthropic,
                format!("data URL payload was not valid base64: {err}"),
            )
        })?
    } else {
        payload.as_bytes().to_vec()
    };
    Ok((media_type, BASE64_STANDARD.encode(bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::runtime::ContentPart;

    #[test]
    fn extract_input_schema_pulls_schema_out_of_openai_wrapper() {
        let rf = serde_json::json!({
            "type": "json_schema",
            "json_schema": {
                "name": "Output",
                "strict": true,
                "schema": {
                    "type": "object",
                    "properties": { "answer": { "type": "integer" } }
                }
            }
        });
        let schema = extract_input_schema(&rf).expect("schema should extract");
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["properties"]["answer"]["type"], "integer");
    }

    #[test]
    fn extract_input_schema_errors_on_missing_schema() {
        let rf = serde_json::json!({ "type": "json_schema", "json_schema": {} });
        assert!(extract_input_schema(&rf).is_err());
    }

    #[test]
    fn anthropic_content_parts_translate_text_image_and_file() {
        let parts = vec![
            ContentPart::Text("hi".to_string()),
            ContentPart::Image {
                data_url: "data:image/png;base64,aGVsbG8=".to_string(),
            },
            ContentPart::File {
                filename: "doc.pdf".to_string(),
                file_data: "data:application/pdf;base64,JVBERi0=".to_string(),
            },
        ];
        let blocks = anthropic_content_parts(&parts).expect("translation should succeed");
        match &blocks[0] {
            ContentBlock::Text { text } => assert_eq!(text, "hi"),
            other => panic!("expected Text, got {other:?}"),
        }
        match &blocks[1] {
            ContentBlock::Image { source } => {
                assert_eq!(source.kind, "base64");
                assert_eq!(source.media_type, "image/png");
            }
            other => panic!("expected Image, got {other:?}"),
        }
        match &blocks[2] {
            ContentBlock::Document { source } => {
                assert_eq!(source.kind, "base64");
                assert_eq!(source.media_type, "application/pdf");
            }
            other => panic!("expected Document, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn send_request_extracts_tool_use_input_as_json_string() {
        let mut server = mockito::Server::new_async().await;
        let body = serde_json::json!({
            "id": "msg_test",
            "type": "message",
            "role": "assistant",
            "model": "claude-sonnet-4-6",
            "content": [
                { "type": "tool_use", "id": "tu_1", "name": "output", "input": { "answer": "hi" } }
            ],
            "stop_reason": "tool_use",
            "usage": { "input_tokens": 12, "output_tokens": 7 }
        })
        .to_string();

        let _mock = server
            .mock("POST", "/v1/messages")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(body)
            .match_header("x-api-key", "test-token")
            .match_header("anthropic-version", ANTHROPIC_VERSION)
            .create_async()
            .await;

        let url = format!("{}/v1/messages", server.url());
        let model = "claude-sonnet-4-6".to_string();
        let token = "test-token".to_string();
        let content_parts = vec![ContentPart::Text("return json".to_string())];
        let response_format = serde_json::json!({
            "type": "json_schema",
            "json_schema": {
                "name": "Output",
                "schema": {
                    "type": "object",
                    "properties": { "answer": { "type": "string" } }
                },
                "strict": true
            }
        });

        let reply = send_request(&url, &model, &content_parts, 10, &token, response_format)
            .await
            .expect("anthropic happy path should return the tool_use input");
        // tool_use.input is the structured output as JSON.
        let parsed: serde_json::Value = serde_json::from_str(&reply).expect("reply is JSON");
        assert_eq!(parsed["answer"], "hi");
    }

    #[tokio::test]
    async fn send_request_errors_when_no_tool_use_block() {
        let mut server = mockito::Server::new_async().await;
        // Response contains only a text block — no tool_use. This
        // shouldn't happen when tool_choice is forced, but we
        // defend against odd model behavior.
        let body = serde_json::json!({
            "id": "msg_test",
            "type": "message",
            "content": [{ "type": "text", "text": "I refuse to use the tool." }],
            "stop_reason": "end_turn"
        })
        .to_string();
        let _mock = server
            .mock("POST", "/v1/messages")
            .with_status(200)
            .with_body(body)
            .create_async()
            .await;

        let url = format!("{}/v1/messages", server.url());
        let model = "claude-sonnet-4-6".to_string();
        let token = "test-token".to_string();
        let content_parts = vec![ContentPart::Text("hi".to_string())];
        let response_format = serde_json::json!({
            "type": "json_schema",
            "json_schema": { "schema": { "type": "object", "properties": {} } }
        });

        let result =
            send_request(&url, &model, &content_parts, 10, &token, response_format).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn send_request_surfaces_anthropic_http_errors() {
        let mut server = mockito::Server::new_async().await;
        let body = serde_json::json!({
            "type": "error",
            "error": { "type": "authentication_error", "message": "invalid x-api-key" }
        })
        .to_string();
        let _mock = server
            .mock("POST", "/v1/messages")
            .with_status(401)
            .with_body(body)
            .create_async()
            .await;

        let url = format!("{}/v1/messages", server.url());
        let model = "claude-sonnet-4-6".to_string();
        let token = "bad".to_string();
        let content_parts = vec![ContentPart::Text("hi".to_string())];
        let response_format = serde_json::json!({
            "type": "json_schema",
            "json_schema": { "schema": { "type": "object", "properties": {} } }
        });

        let err =
            send_request(&url, &model, &content_parts, 10, &token, response_format)
                .await
                .expect_err("401 should surface as ProviderError");
        // Sanity: error is classified as Unauthorized so the
        // hint engine routes to the credentials guidance.
        assert!(err.message().contains("HTTP error 401"));
    }
}
