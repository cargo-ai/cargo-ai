mod args;

use std::io::stdin;

use serde::{Deserialize, Serialize};

include!(concat!(env!("OUT_DIR"), "/.agentcfg"));

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

    let mut token = String::new();
    if let Some(cmd_token) = cmd_args.get_one::<String>("token") {
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
        .expect("Expected unsigned int, u64");

    if !(server == "ollama" || server == "openai") {
        panic!("Unknown AI Server")
    }
    // End: Argument assignments

    let mut prompt = String::new();

    println!("Enter a prompt for {model}!"); // Request to use for input

    stdin().read_line(&mut prompt).expect("Failed to read line"); // Captures user input into prompt String

    let prompt = prompt.trim().to_string(); // Remove trailing newline from user input

    if let Some(_) = cmd_args.subcommand_matches("json-sample-response") {

        println!("JSON Sample Mode");

        let samples = sample_outputs();
        println!("Build-script sample responses: {:#?}", samples);

        let context = format!("A question will be asked and you will need to return the answer in the specified JSON format.");

        let mut ai_cargo = cargo_ai::Cargo::new(prompt.clone(), context, samples);

        println!("Cargo Contents: {ai_cargo:#?}");

        let structured_prompt = ai_cargo.prompt();
        
        println!("Structured Prompt: {structured_prompt}");

        let mut response = String::new(); // Holds the LLM response

        if server == "ollama" {
            // Send request to Ollama and `await` the LLM response
            match cargo_ai::ollama_send_request(&model, &structured_prompt, timeout_in_sec, true).await {
                Ok(r) => {
                    response.push_str(&r);
                },
                Err(e) => {
                    println!("We have an error {}", e);
                }
            }
        }

        println!("{server} Response: {response}");
        
        ai_cargo.set_response(response);

        println!("AI Cargo: {ai_cargo:#?}");

    } else if let Some(_) = cmd_args.subcommand_matches("float-answer") {

        println!("Float Answer Mode Activite");

        #[derive(Clone, Debug, Deserialize, Serialize)]
        struct Answer {
            number: f64,
        }

       let samples = vec![
            Answer { number: 4.78 },
            Answer { number: 2.0 },
            Answer { number: 3.3333 },
        ];

        let context = format!("A math question will be asked and you will need to return the answer in the specified JSON format.");

        let mut ai_cargo = cargo_ai::Cargo::new(prompt.clone(), context, samples);

        println!("Cargo Contents: {ai_cargo:#?}");

        let structured_prompt = ai_cargo.prompt();
        
        println!("Structured Prompt: {structured_prompt}");

        let mut response = String::new(); // Holds the LLM response

        if server == "ollama" {
            // Send request to Ollama and `await` the LLM response
            match cargo_ai::ollama_send_request(&model, &structured_prompt, timeout_in_sec, true).await {
                Ok(r) => {
                    response.push_str(&r);
                },
                Err(e) => {
                    println!("We have an error {}", e);
                }
            }
        }

        println!("{server} Response: {response}");
        
        ai_cargo.set_response(response);

        println!("AI Cargo: {ai_cargo:#?}");

        let x: f64 = ai_cargo.get_response().unwrap().number;
        println!("Return Value:{x}");

    } else if let Some(_) = cmd_args.subcommand_matches("response-time") {
        println!("response-time");

        #[derive(Clone, Debug, Deserialize, Serialize)]
        struct ResponseTime {
            response_required: bool,
            days: i32,
        }

        let samples = vec![
            ResponseTime {
                response_required: true,
                days: 2,
            },
            ResponseTime {
                response_required: false,
                days: 0
            },
        ]; 
        
        // Generic
        let context = format!(
            "The user received a message, which will be provided in the prompt, \
            you are to indicate if a response is required and, if so, \
            how soon a response is required in days. A response is not requied unless \
            the message sender is explicitly eliciting a response.  Purely informational \
            messages do not require a response.  Messages that require a response that day \
            should have a required response days value of 0."
        );
        let mut ai_cargo = cargo_ai::Cargo::new(prompt.clone(), context, samples);

        println!("Cargo Contents: {ai_cargo:#?}");

        let structured_prompt = ai_cargo.prompt();
        
        println!("Structured Prompt: {structured_prompt}");

        let mut response = String::new(); // Holds the LLM response

        if server == "ollama" {
            // Send request to Ollama and `await` the LLM response
            
            match cargo_ai::ollama_send_request(&model, &structured_prompt, timeout_in_sec, true).await {
                Ok(r) => {
                    println!("I'm here");
                    response.push_str(&r);
                },
                Err(e) => {
                    println!("We have an error {}", e);
                }
            }
        }
        println!("{server} Response: {response}");
        let is_response_set = ai_cargo.set_response(response);
        println!("AI Cargo: {ai_cargo:#?}");
        
        // Non-Generic code begins here.
        if is_response_set {
            let days = ai_cargo.get_response().unwrap().days;
            let response_required = ai_cargo.get_response().unwrap().response_required;
            if response_required { 
                println!("Respond in {days} days.");
            } else {println!("Response not required.");}
        } else {
            panic!("Response Error");
        }
    } else {
        let mut response = String::new(); // Holds the LLM response
        if server == "ollama" {
            // Send request to Ollama and `await` the LLM response
            match cargo_ai::ollama_send_request(&model, &prompt, timeout_in_sec, false).await {
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
}
