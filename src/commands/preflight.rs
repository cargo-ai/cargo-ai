use clap::ArgMatches;

use crate::config::loader::{find_profile, load_config};

pub async fn run(sub_m: &ArgMatches) {
    let prompt = if let Some(cli_prompt) = sub_m.get_one::<String>("prompt") {
        cli_prompt.to_string()
    } else {
        crate::prompt()
    };

    // Begin: Argument assignments
    let mut server = String::new();
    let mut model = String::new();
    let mut url = String::new();
    let mut token = String::new();
    let mut timeout_in_sec: u64 = 60; // Default

    // 1️⃣ If profile is set, load values from config
    if let Some(profile_name) = sub_m.get_one::<String>("profile") {
        if let Some(cfg) = load_config() {
            if let Some(profile) = find_profile(&cfg, profile_name) {
                server = profile.server.clone().to_lowercase();
                model = profile.model.clone();
                token = profile.token.clone().unwrap_or_default();
                timeout_in_sec = profile.timeout_in_sec;
                // Updated URL assignment logic:
                url = profile.url.clone().unwrap_or_default();
                println!("Using profile '{}'", profile_name);
            } else {
                eprintln!("Profile '{}' not found.", profile_name);
            }
        } else {
            eprintln!("No config file found.");
        }
    }

    // Default profile if no explicit profile was provided
    //
    // If no --profile flag is provided, attempt to use the configured default profile.
    //
    // Precedence order:
    //   CLI args > explicit --profile > default_profile (from config) > empty values
    if server.is_empty() {
        if let Some(cfg) = load_config() {
            if let Some(ref default_profile_name) = cfg.default_profile {
                if let Some(profile) = find_profile(&cfg, default_profile_name) {
                    server = profile.server.clone().to_lowercase();
                    model = profile.model.clone();
                    token = profile.token.clone().unwrap_or_default();
                    timeout_in_sec = profile.timeout_in_sec;
                    url = profile.url.clone().unwrap_or_default();
                    println!("Using default profile '{}'", default_profile_name);
                }
            }
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

    // Final URL fallback based on resolved server
    if url.is_empty() {
        url = if server == "ollama" {
            "http://localhost:11434/api/generate".to_string()
        } else if server == "openai" {
            "https://api.openai.com/v1/chat/completions".to_string()
        } else {
            String::new()
        };
    }

    // End: Argument assignments

    if !(server == "ollama" || server == "openai") {
        println!("{server}");
        panic!("Unknown AI Server")
    }

    let static_context = "A question will be asked and you will need to return the answer in the specified JSON format.";

    let resources = crate::resource_urls();

    // Build data block for LLM context
    let data_block = crate::web_resources::build_data_block(&resources)
        .await
        .expect("Failed to fetch required web resources");

    let context = format!("{}\n\n{}", static_context, data_block);

    let mut ai_cargo = cargo_ai::Cargo::<crate::Output>::new(prompt.clone(), context);

    let structured_prompt = ai_cargo.prompt();

    let mut response = String::new(); // Holds the LLM response

    if server == "ollama" {
        // Send request to Ollama and `await` the LLM response
        match cargo_ai::ollama_send_request(
            &url,
            &model,
            &structured_prompt,
            timeout_in_sec,
            crate::json_schema_value(),
        )
        .await
        {
            Ok(r) => {
                response.push_str(&r);
            }
            Err(e) => {
                eprintln!("❌ Issue communicating with the AI server (Ollama).");
                eprintln!("Reason: {}\n", e);
                return;
            }
        }
    } else if server == "openai" {
        let mut schema = crate::json_schema_value(); // this is a serde_json::Value (object)
        if let Some(obj) = schema.as_object_mut() {
            obj.insert(
                "additionalProperties".into(),
                serde_json::Value::Bool(false),
            );
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
        match cargo_ai::openai_send_request(
            &url,
            &model,
            &structured_prompt,
            timeout_in_sec,
            &token,
            fmt,
        )
        .await
        {
            Ok(r) => response.push_str(&r),
            Err(e) => {
                eprintln!("❌ Issue communicating with the AI server (OpenAI).");
                eprintln!("Reason: {}\n", e);
                return;
            }
        };
    }

    // Attempt to conform the LLM response to the Output schema
    if !ai_cargo.set_response(response.clone()) {
        eprintln!("❌ LLM output did NOT conform to the required JSON schema.");
        eprintln!("Raw output received from server:\n{}\n", response);
        return; // Stop execution cleanly — do NOT continue to unwrap
    }

    let output = match ai_cargo.get_response() {
        Some(o) => o,
        None => {
            eprintln!("❌ Internal error: response was expected but missing.");
            eprintln!("Raw output received from server:\n{}\n", response);
            return;
        }
    };

    // Get Actions
    let actions = crate::actions();
    // println!("Actions {:?}", actions);

    super::preflight_actions::apply_actions(&output, &actions);
}
