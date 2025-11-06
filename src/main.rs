use reqwest;
mod args;
mod web_resources;
mod agent_builder;
mod config;

use serde::{Deserialize, Serialize};
use jsonlogic::apply;

use std::{fs, env};
use std::io::{Error, ErrorKind};

use config::loader::{load_config, find_profile};
use config::adder::add_profile;
use config::remover::remove_profile;
use config::schema::Profile;

include!(concat!(env!("OUT_DIR"), "/agent_model.rs"));

// Initialize Tokio runtime macro
// Executor: Responsible for polling and running to completion
#[tokio::main]
async fn main() {

    let cmd_args = args::build_cli();

    if let Some(sub_m) = cmd_args.subcommand_matches("preflight") {

        let prompt = if let Some(cli_prompt) = sub_m.get_one::<String>("prompt") {
            cli_prompt.to_string()
        } else {
            prompt() // JSON default.
        };

        // Begin: Argument assignments
        let mut server = String::new();
        let mut model = String::new();
        let mut token = String::new();
        let mut timeout_in_sec: u64 = 60; // Default

        // 1️⃣ If profile is set, load values from config
        if let Some(profile_name) = sub_m.get_one::<String>("profile") {
            if let Some(cfg) = load_config() {
                if let Some(profile) = find_profile(&cfg, profile_name) {
                    server = profile.server.clone();
                    model = profile.model.clone();
                    token = profile.token.clone().unwrap_or_default();
                    timeout_in_sec = profile.timeout_in_sec;
                    println!("Using profile '{}'", profile_name);
                } else {
                    eprintln!("Profile '{}' not found.", profile_name);
                }
            } else {
                eprintln!("No config file found.");
            }
        }

        // 2️⃣ Allow command-line args to override profile values
        if let Some(server_arg) = sub_m.get_one::<String>("server") {
            server = server_arg.to_lowercase();
        }

        if let Some(model_arg) = sub_m.get_one::<String>("model") {
            model = model_arg.to_string();
        }

        if let Some(cmd_token) = sub_m.get_one::<String>("token") {
            token = cmd_token.to_string();
        }

        if let Some(timeout_arg) = sub_m.get_one::<String>("timeout_in_sec") {
            timeout_in_sec = timeout_arg.parse::<u64>().unwrap_or(60);
        }

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


    } else if let Some(sub_m) = cmd_args.subcommand_matches("hatch") {

        let new_project_name = sub_m
            .get_one::<String>("name")
            .expect("project name is required");

        println!("Build new cargo agent: {new_project_name}");

        // Determine config source: use flag if provided, otherwise default to project name
        let agentcfg: &str = sub_m
            .get_one::<String>("config")
            .map(String::as_str)
            .unwrap_or(new_project_name);

        if sub_m.get_one::<String>("config").is_none() {
            println!("🌐 No --config flag detected. Fetching default template '{agentcfg}' from Cargo-AI registry...");
        }

        let file_contents = config_contents(agentcfg);

        match agent_builder::project::create_new_agent_project(&new_project_name, file_contents) {
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

    } else if let Some(sub_m) = cmd_args.subcommand_matches("profile") {
        if let Some(_) = sub_m.subcommand_matches("list") {
            if let Some(cfg) = load_config() {
                println!("Configured profiles:");
                println!("{:<20} {:<10} {}", "Name", "Server", "Model");
                println!("{:-<45}", "");
                for profile in cfg.profile {
                    println!("{:<20} {:<10} {}", profile.name, profile.server, profile.model);
                }
            } else {
                println!("No config file found.");
            }
        } else if let Some(add_m) = sub_m.subcommand_matches("add") {
            let name = add_m.get_one::<String>("name").expect("Profile name is required");
            let server = add_m.get_one::<String>("server").expect("Server is required");
            let model = add_m.get_one::<String>("model").expect("Model is required");
            let token = add_m.get_one::<String>("token").map(String::as_str).unwrap_or("(none)");
            let description = add_m.get_one::<String>("description").map(String::as_str).unwrap_or("(none)");

            println!("Adding profile:");
            println!("  Name: {}", name);
            println!("  Server: {}", server);
            println!("  Model: {}", model);
            println!("  Token: {}", token);
            println!("  Description: {}", description);


            let new_profile = Profile {
                name: name.to_string(),
                server: server.to_string(),
                model: model.to_string(),
                token: if token == "(none)" { None } else { Some(token.to_string()) },
                timeout_in_sec: 60, // default for now
                description: if description == "(none)" { None } else { Some(description.to_string()) },
            };

            if let Err(e) = add_profile(new_profile, false) {
                eprintln!("Failed to add profile: {}", e);
            }
        } else if let Some(remove_m) = sub_m.subcommand_matches("remove") {
            if let Some(name) = remove_m.get_one::<String>("name") {
                if let Some(cfg) = load_config() {
                    if cfg.profile.iter().any(|p| p.name == *name) {
                        use std::io::{self, Write};
                        print!("Are you sure you want to remove profile '{}'? [y/N]: ", name);
                        io::stdout().flush().unwrap();

                        let mut input = String::new();
                        io::stdin().read_line(&mut input).unwrap();

                        if input.trim().eq_ignore_ascii_case("y") || input.trim().eq_ignore_ascii_case("yes") {
                            if let Err(e) = remove_profile(name) {
                                eprintln!("Failed to remove profile '{}': {}", name, e);
                            }
                        } else {
                            println!("Operation canceled.");
                        }
                    } else {
                        println!("Profile '{}' not found.", name);
                    }
                } else {
                    println!("No config file found.");
                }
            } else {
                println!("Please provide a profile name to remove. Example: cargo ai profile remove openai-prod");
            }
        } else if let Some(show_m) = sub_m.subcommand_matches("show") {
            if let Some(name) = show_m.get_one::<String>("name") {
                if let Some(cfg) = load_config() {
                    if let Some(p) = find_profile(&cfg, name) {
                        println!("Profile: {}", p.name);
                        println!("Server:  {}", p.server);
                        println!("Model:   {}", p.model);
                        println!(
                            "Token:   {}",
                            p.token.as_ref().map(|_| "***********").unwrap_or("(none)")
                        );
                        println!("Timeout: {}", p.timeout_in_sec);
                        if let Some(desc) = &p.description {
                            println!("Description: {}", desc);
                        }
                    } else {
                        println!("Profile '{}' not found.", name);
                    }
                } else {
                    println!("No config file found.");
                }
            } else {
                println!("Please provide a profile name. Example: cargo ai profile show openai-prod");
            }
        } else {
            println!("No profile subcommand found. Try 'cargo ai profile list'.");
        }
    } else { println!("Provide subcommand.");
    }
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

fn config_contents(path: &str) -> Result<String, std::io::Error> {
    if path.contains('.') {
        // Local file path
        fs::read_to_string(path)
    } else {
        // Fetch from Cargo-AI registry
        fetch_from_registry(path)
    }
}

fn fetch_from_registry(name: &str) -> Result<String, Error> {
    let url = "https://api.cargo-ai.org/public";
    let client = reqwest::blocking::Client::new();

    let body = serde_json::json!({ "request": name });

    let resp = client
        .post(url)
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .map_err(|e| Error::new(ErrorKind::Other, format!("network error: {e}")))?;

    if !resp.status().is_success() {
        return Err(Error::new(
            ErrorKind::Other,
            format!("HTTP {} for {url}", resp.status()),
        ));
    }

    resp.text().map_err(|e| Error::new(ErrorKind::Other, e.to_string()))
}
