// External Crates
use reqwest::ClientBuilder; // HTTP client builder
use serde::{Deserialize, Serialize}; // Data format (e.g.,JSON, TOML) (de)serialization
use std::time::Duration; // Duration for timeout handling

// Request as per Ollama API Guide
#[derive(Serialize, Debug)]
struct Request {
    model: String,
    prompt: String,
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

// `async` allows us to pause execution and yield control back to the runtime,
// enabling other tasks to run concurrently and make progress.
// It's syntactic sugar for implementing the `Future` trait,
// which uses the `poll` method to return `Ready` or `Pending`.
pub async fn send_request(
    model: &String,
    prompt: &String,
    timeout_in_sec: u64,
) -> Result<String, Box<dyn std::error::Error>> {
    let request = Request {
        model: model.clone(),
        prompt: prompt.clone(),
        stream: false,
        options: Options { temperature: 0.75 },
    };

    let client = ClientBuilder::new()
        .timeout(Duration::from_secs(timeout_in_sec))
        .build()?; // 30 sec Default too short for some LLMs.

    let reply = client
        .post("http://localhost:11434/api/generate")
        .json(&request)
        .send()
        .await?
        .json::<Response>()
        .await?;

    Ok(reply.response)
}
