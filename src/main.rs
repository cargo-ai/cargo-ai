use reqwest;
use futures::future::join_all;
mod args;
mod agent_builder;

use serde::{Deserialize, Serialize};
use jsonlogic::apply;

include!(concat!(env!("OUT_DIR"), "/agent_model.rs"));

// Initialize Tokio runtime macro
// Executor: Responsible for polling and running to completion
#[tokio::main]
async fn main() {

    let cmd_args = args::build_cli();

    let prompt = prompt();

    if let Some(sub_m) = cmd_args.subcommand_matches("preflight") {
        
        // Begin: Argument assignments
        let mut server = String::new();
        if let Some(server_arg) = sub_m.get_one::<String>("server") {
            server.push_str(&server_arg.to_lowercase());
        }

        let mut token = String::new();
        if let Some(cmd_token) = sub_m.get_one::<String>("token") {
            token.push_str(cmd_token);
        }

        let mut model = String::new();
        if let Some(model_arg) = sub_m.get_one::<String>("model") {
            model.push_str(model_arg);
        }

        // sub_m timeout_in_sec default to 60
        let timeout_in_sec = sub_m
            .get_one::<String>("timeout_in_sec")
            .expect("Timeout value expected")
            .parse::<u64>()
            .expect("Expected unsigned int, u64");

        // End: Argument assignments

        if !(server == "ollama" || server == "openai") {
            panic!("Unknown AI Server")
        }

        let static_context = "A question will be asked and you will need to return the answer in the specified JSON format.";
        
        let resources = resource_urls();

        // Build data block for LLM context
        let mut data_block = String::from("Here are some resources to aid in your response:\n");
        if !resources.is_empty() {
            let urls: Vec<&str> = resources.iter().map(|r| r.url).collect();
            if let Ok(results) = fetch_resources_parallel(&urls).await {
                for (res, content) in resources.iter().zip(results.iter()) {
                    data_block.push_str(&format!("- {} ({}): {}\n", res.description, res.url, content.trim()));
                }
            }
        }
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


    } else if let Some(sub_m) = cmd_args.subcommand_matches("hatch") {

        let new_project_name = sub_m
            .get_one::<String>("name")
            .expect("project name is required")
            .to_lowercase();
        println!("Build new cargo agent: {new_project_name}");

        let agentcfg: Option<&str> = sub_m.get_one::<String>("config").map(String::as_str);

        match agent_builder::project::create_new_agent_project(&new_project_name, agentcfg) {
            Ok(_) => println!("✅ Project created successfully."),
            Err(e) =>  println!("❌ Failed to create project: {e}") 
        }

        match agent_builder::build::build_agent_project(&new_project_name) {
            Ok(_) => println!("✅ Project built successfully."),
            Err(e) =>  println!("❌ Build failed: {e}") 
        }

        match agent_builder::export::export_binary(&new_project_name){
            Ok(_) => println!("✅ Project binary exported successfully."),
            Err(e) =>  println!("❌ Export failed: {e}") 
        }

        match agent_builder::cleanup::delete_agent_workspace(&new_project_name) {
            Ok(_) => println!("🧼 Agent workspace removed."),
            Err(e) => println!("⚠️ Failed to clean up workspace: {e}"),
        }

    } else {
        println!("Provide subcommand.");
    }
}

// TEMPORARY TEST FUNCTION: Fetch resources in parallel; will be relocated later.
pub async fn fetch_resources_parallel(urls: &[&str]) -> Result<Vec<String>, reqwest::Error> {
    let client = reqwest::Client::new();

    let futures = urls.iter().map(|&url| {
        let client = client.clone();
        async move {
            let res = client.get(url).send().await?;
            res.text().await
        }
    });

    let results = join_all(futures).await;
    results.into_iter().collect()
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
