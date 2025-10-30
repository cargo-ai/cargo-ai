mod args;
mod web_resources;

use serde::{Deserialize, Serialize};
use jsonlogic::apply;

include!(concat!(env!("OUT_DIR"), "/agent_model.rs"));

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

    let prompt = if let Some(cli_prompt) = cmd_args.get_one::<String>("prompt") {
        cli_prompt.to_string()
    } else {
        prompt() // JSON default.
    };

    // End: Argument assignments

    if !(server == "ollama" || server == "openai") {
        panic!("Unknown AI Server")
    }

    let static_context = "A question will be asked and you will need to return the answer in the specified JSON format.";
    
    let resources = resource_urls();

    // Build data block for LLM context
    let data_block = web_resources::build_data_block(&resources)
        .await
        .expect("Failed to fetch required web resources");

    let context = format!("{}\n\n{}", static_context, data_block);

    let mut ai_cargo = cargo_ai::Cargo::<Output>::new(prompt.clone(), context);

    let structured_prompt = ai_cargo.prompt();
    
    let mut response = String::new(); // Holds the LLM response

    if server == "ollama" {
        // Send request to Ollama and `await` the LLM response
        match cargo_ai::ollama_send_request(&model, &structured_prompt, timeout_in_sec, json_schema_value()).await {
            Ok(r) => {
                response.push_str(&r);
            },
            Err(e) => {
                println!("We have an error {}", e);
            }
        }
    } else if server == "openai" {

    let mut schema = json_schema_value(); // this is a serde_json::Value (object)
    if let Some(obj) = schema.as_object_mut() {
        obj.insert("additionalProperties".into(), serde_json::Value::Bool(false));
    }

    let fmt = serde_json::json!({
    "type": "json_schema",
    "json_schema": {
        "name": "Output",
        "schema": schema,     // now with additionalProperties: false
        "strict": true
    }
    });

        // Send request to OpenAI and `await` the LLM response
        match cargo_ai::openai_send_request(&model, &structured_prompt, timeout_in_sec, &token, fmt).await {
            Ok(r) => response.push_str(&r),
            Err(e) => {
                println!("We have an error {}", e);
            }
        };
    }

    // println!("{server} Response: {response}");
    
    ai_cargo.set_response(response.clone());


    // Get Output 
    let output: Output = ai_cargo.get_response().unwrap();
    // println!("Output: {:?}", output);

    // Get Actions
    let actions = actions();
    // println!("Actions {:?}", actions);

    apply_actions(&output, &actions);

    // println!("AI Cargo: {ai_cargo:#?}");
}

pub fn apply_actions(output: &Output, actions: &[Action]) {

    // println!("DEBUG: Applying actions -> {:?}", actions);

    let data = serde_json::to_value(output).unwrap();

    for action in actions {
        if let Ok(result) = apply(&action.logic, &data) {
            // println!("Action Loop: {:?}", action);
            if result.as_bool() == Some(true) {
                for step in &action.run {
                    println!("Running '{}': {} {:?}", action.name, step.program, step.args);

                    // Execute the command
                    let status = std::process::Command::new(&step.program)
                        .args(&step.args)
                        .status();

                    match status {
                        Ok(status) if status.success() => {
                            println!("Command completed successfully.");
                        }
                        Ok(status) => {
                            println!("Command exited with status: {}", status);
                        }
                        Err(err) => {
                            println!("Failed to execute command: {}", err);
                        }
                    }
                }
            }
        } else {
            println!("Failed to evaluate logic for action: {}", action.name);
        }
    }

}
