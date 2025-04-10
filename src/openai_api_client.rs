// External Crates
use reqwest::ClientBuilder; // HTTP client builder
use serde::{Deserialize, Serialize}; // Data format (e.g.,JSON, TOML) (de)serialization
use std::time::Duration; // Duration for timeout handling

#[derive(Serialize, Debug)]
pub struct Request {
    pub model: String,          // Model name for OpenAI
    pub messages: Vec<Message>, // List of messages for chat format
    pub temperature: f64,       // Temperature setting
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Message {
    pub role: String,    // "user", "assistant", or "system"
    pub content: String, // The actual prompt or message content
}

#[derive(Deserialize, Debug)]
#[allow(dead_code)] // Currently not using all response fields
pub struct Response {
    pub id: String,           // Unique identifier for the chat session
    pub object: String,       // Object type, usually "chat.completion"
    pub created: u64,         // Timestamp when the response was created
    pub model: String,        // Model name used for the response
    pub choices: Vec<Choice>, // List of choices (contains the actual answer)
    pub usage: Usage,         // Information about token usage
}

#[derive(Deserialize, Debug)]
#[allow(dead_code)] // Currently not using all response fields
pub struct Choice {
    pub message: Message,              // Contains the assistant's message
    pub finish_reason: Option<String>, // Reason for stopping (e.g., "stop")
    pub index: usize,                  // Index of the choice
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Usage {
    pub prompt_tokens: u32,     // Number of tokens in the prompt
    pub completion_tokens: u32, // Number of tokens in the completion
    pub total_tokens: u32,      // Total number of tokens used
}

pub async fn send_request(
    model: &String,
    prompt: &String,
    timeout_in_sec: u64,
    token: &String,
) -> Result<String, Box<dyn std::error::Error>> {
    let client = ClientBuilder::new()
        .timeout(Duration::from_secs(timeout_in_sec))
        .build()?; // 30 sec Default too short for some requests.

    let temperature: f64 = 0.75;

    let role = String::from("user");

    let message = Message {
        role,
        content: prompt.clone(),
    };

    let messages = vec![message];

    let request = Request {
        model: model.clone(),
        messages,
        temperature,
    };

    let response = client
        .post("https://api.openai.com/v1/chat/completions")
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .json(&request)
        .send()
        .await?
        .json::<Response>()
        .await?;

    let response_content = response
        .choices
        .get(0)
        .ok_or("No ChatGPT Response Index 0 Choice.")?
        .message
        .content
        .clone();

    Ok(response_content)
}
