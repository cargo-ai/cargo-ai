mod ollama_api_client;
mod openai_api_client;

pub use ollama_api_client::send_request as ollama_send_request;
pub use openai_api_client::send_request as openai_send_request;
