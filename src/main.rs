mod args;

use std::io::stdin;

// Initialize Tokio runtime macro
// Executor: Responsible for polling and running to completion
#[tokio::main]
async fn main() {
    let cmd_args = args::build_cli();

    // Begin: Argument assignments
    let mut server = String::new();
    if let Some(server_arg) = cmd_args.get_one::<String>("server") {
        server.push_str(&server_arg.to_lowercase());
    }

    let mut token = String::from("NA");
    if let Some(cmd_token) = cmd_args.get_one::<String>("token") {
        token.clear();
        token.push_str(cmd_token);
    }

    let mut model = String::new();
    if let Some(model_arg) = cmd_args.get_one::<String>("model") {
        model.push_str(model_arg);
    }

    // cmd_args timeout_in_sec default to 60
    let timeout_in_sec = cmd_args
        .get_one::<String>("timeout_in_sec")
        .expect("Timeout value expected")
        .parse::<u64>()
        .expect("Expected unsigned int, u8");

    if !(server == "ollama" || server == "openai") {
        panic!("Unknown AI Server")
    }
    // End: Argument assignments

    let mut prompt = String::new();

    println!("Enter a prompt for {model}!"); // Request to use for input

    stdin().read_line(&mut prompt).expect("Failed to read line"); // Captures user input into prompt String

    let prompt = prompt.trim().to_string(); // Remove trailing newline from user input

    let mut response = String::new(); // Holds the LLM response

    if server == "ollama" {
        // Send request to Ollama and `await` the LLM response
        match cargo_ai::ollama_send_request(&model, &prompt, timeout_in_sec).await {
            Ok(r) => response.push_str(&r),
            Err(e) => {
                println!("We have an error {}", e);
            }
        };
    } else if server == "openai" {
        // Send request to OpenAI and `await` the LLM response
        match cargo_ai::openai_send_request(&model, &prompt, timeout_in_sec, &token).await {
            Ok(r) => response.push_str(&r),
            Err(e) => {
                println!("We have an error {}", e);
            }
        };
    }

    println!("{server} Response: {response}");
}
