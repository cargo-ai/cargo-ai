//! # cargo-ai Library
//!
//! This library provides API clients for interacting with AI services.
//! It includes modules for communicating with both the Ollama and OpenAI APIs.
//!
//! ## Usage
//!
//! The functions provided by this library are asynchronous and should be used within an async context.
//! They return a `Result<String, Error>`, where `String` is the successful API response.
//!
//! ```rust
//! use cargo_ai::{ollama_send_request, openai_send_request};
//!
//! # async {
//! // For the Ollama API:
//! // Provide the model name, prompt, and a timeout (in seconds).
//! let ollama_response = ollama_send_request("model_name", "Your prompt here", 60).await;
//!
//! // For the OpenAI API:
//! // Provide the model name, prompt, timeout (in seconds), and your API token.
//! let openai_response = openai_send_request("model_name", "Your prompt here", 60, "your_token_here").await;
//! # };
//! ```
//!
//! ## Modules
//!
//! - `ollama_api_client`: Functions for interacting with the Ollama API.
//! - `openai_api_client`: Functions for interacting with the OpenAI API.

mod ollama_api_client;
mod openai_api_client;

/// Re-exports the `send_request` function from the `ollama_api_client` module.
/// This function sends a request to the Ollama API and returns the response.
/// 
/// # Parameters
/// - `model`: The name of the model to query.
/// - `prompt`: The query prompt.
/// - `timeout_in_sec`: Timeout in seconds for the request.
pub use ollama_api_client::send_request as ollama_send_request;

/// Re-exports the `send_request` function from the `openai_api_client` module.
/// This function sends a request to the OpenAI API and returns the response.
/// 
/// # Parameters
/// - `model`: The name of the model to query.
/// - `prompt`: The query prompt.
/// - `timeout_in_sec`: Timeout in seconds for the request.
/// - `token`: Your OpenAI API token.
pub use openai_api_client::send_request as openai_send_request;
