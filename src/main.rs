use std::io::stdin;

// Initialize Tokio runtime macro
// Executor: Responsible for polling and running to completion
#[tokio::main]
async fn main() {

    // Ollama relevant call information
    let model = "mistral".to_string(); // LLM Model we're using
    let timeout_in_sec = 60; // Local LLM may take some time and not streaming.
    let mut prompt = String::new(); // User input prompt.

    println!("Enter a prompt for {model}!"); // Request to use for input
    stdin().read_line(&mut prompt).expect("Failed to read line"); // Captures user input into prompt String
    let prompt = prompt.trim().to_string(); // Remove trailing newline from user input

    let mut response = String::new(); // Holds the LLM response

    // Send request to Ollama and `await` the LLM response
    match cargo_ai::send_request(model, prompt, timeout_in_sec).await {
        Ok(r) => response.push_str(&r),
        Err(e) => {
            println!("We have an error {}", e);
        }
    };

    println!("Response: {response}");
}
