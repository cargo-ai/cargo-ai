// External Crates
use reqwest::ClientBuilder; // HTTP client builder
use serde::{Deserialize, Serialize}; // Data format (e.g.,JSON, TOML) (de)serialization
use std::time::Duration; // Duration for timeout handling // for overriding the API URL in tests

// Request as per Ollama API Guide
#[derive(Serialize, Debug)]
struct Request {
    model: String,
    prompt: String,
    format: serde_json::Value,
    stream: bool,
    think: bool,
    options: Options,
}

#[derive(Serialize, Debug)]
struct Options {
    temperature: f64,
}

#[derive(Deserialize, Debug)]
#[allow(dead_code)] // Currently not using all response fields
struct Response {
    model: String,
    created_at: String,
    response: String,
    done: bool,
    thinking: Option<String>,
}

pub async fn send_request(
    url: &String,
    model: &String,
    prompt: &String,
    timeout_in_sec: u64,
    format: serde_json::Value,
) -> Result<String, Box<dyn std::error::Error>> {
    let request = Request {
        model: model.clone(),
        prompt: prompt.clone(),
        format: format.clone(),
        stream: false,
        think: false,
        options: Options {
            temperature: super::DEFAULT_TEMPERATURE,
        },
    };

    let client = ClientBuilder::new()
        .timeout(Duration::from_secs(timeout_in_sec))
        .build()?; // 30 sec Default too short for some LLMs.

    let http_resp = client.post(url).json(&request).send().await?;

    let status = http_resp.status();
    let body_bytes = http_resp.bytes().await?;

    if !status.is_success() {
        let raw = String::from_utf8_lossy(&body_bytes);
        return Err(format!("HTTP error {}: {}", status, raw).into());
    }

    let reply: Response = match serde_json::from_slice(&body_bytes) {
        Ok(resp) => resp,
        Err(e) => {
            let raw = String::from_utf8_lossy(&body_bytes);
            return Err(format!("Failed to parse JSON: {}\nRaw response:\n{}", e, raw).into());
        }
    };

    // NOTE: Some Ollama "thinking" models may incorrectly place final output in the `thinking` field
    // instead of the `response` field, even when `think` is set to false. This appears to be an
    // Ollama-side issue. The logic below first checks `response`, and if empty, falls back to
    // `thinking` to support current behavior.
    let final_output = if !reply.response.is_empty() {
        reply.response
    } else if let Some(t) = reply.thinking {
        t
    } else {
        return Err("Ollama returned neither response nor thinking fields.".into());
    };

    Ok(final_output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mockito::Server;
    use tokio;

    #[tokio::test]
    async fn test_send_request_with_mock() {
        // Start an async mock server instance
        let mut server = Server::new_async().await;
        let mock_path = "/api/generate";

        // Override the API URL to point to our mock server
        std::env::set_var(
            "OLLAMA_API_URL",
            format!("{}{}", &server.url().to_string(), &mock_path),
        );

        // Set up the mock endpoint on this server
        let _m = server
            .mock("POST", mock_path)
            .match_header("content-type", "application/json")
            .with_status(200)
            .with_body(
                r#"{
                 "model": "test-model",
                 "created_at": "2025-04-19T00:00:00Z",
                 "response": "Mocked response",
                 "done": true
             }"#,
            )
            .create();

        // Execute the client against the mock
        let result = send_request(
            &format!("{}{}", server.url().to_string(), mock_path),
            &"test-model".to_string(),
            &"test prompt".to_string(),
            5,
            serde_json::json!({
                "type": "object",
                "properties": { "ok": { "type": "boolean" } },
                "required": ["ok"]
            }),
        )
        .await
        .expect("send_request failed");

        assert_eq!(result, "Mocked response");
    }
}
