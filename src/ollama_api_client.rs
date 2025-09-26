// External Crates
use reqwest::ClientBuilder; // HTTP client builder
use serde::{Deserialize, Serialize}; // Data format (e.g.,JSON, TOML) (de)serialization
use std::env;
use std::time::Duration; // Duration for timeout handling // for overriding the API URL in tests

// Request as per Ollama API Guide
#[derive(Serialize, Debug)]
struct Request {
    model: String,
    prompt: String,
    format: serde_json::Value,
    stream: bool,
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
}

pub async fn send_request(
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
        options: Options { temperature: crate::DEFAULT_TEMPERATURE}
    };

    let client = ClientBuilder::new()
        .timeout(Duration::from_secs(timeout_in_sec))
        .build()?; // 30 sec Default too short for some LLMs.

    // Allow overriding Ollama API URL in tests via OLLAMA_API_URL env var
    let api_url = env::var("OLLAMA_API_URL")
        .unwrap_or_else(|_| "http://localhost:11434/api/generate".to_string());

    let reply = client
        .post(&api_url)
        .json(&request)
        .send()
        .await?
        .json::<Response>()
        .await?;

    Ok(reply.response)
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
