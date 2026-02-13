use reqwest;
mod args;
mod web_resources;
mod agent_builder;
mod config;
mod infra_api;
mod ui;

const INFRA_BASE_URL: &str = "https://api.cargo-ai.org";

use serde::{Deserialize, Serialize};
use jsonlogic::apply;

use std::{env, fs, path::Path};
use std::io::{self, Error, ErrorKind, Write};

use config::loader::{load_config, find_profile};
use config::adder::{add_profile, set_account_email, set_account_tokens};
use config::remover::remove_profile;
use config::schema::Profile;
use config::setup::{config_path, ensure_config_file_exists};

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
            match cargo_ai::ollama_send_request(&url, &model, &structured_prompt, timeout_in_sec, json_schema_value()).await {
                Ok(r) => {
                    response.push_str(&r);
                },
                Err(e) => {
                    eprintln!("❌ Issue communicating with the AI server (Ollama).");
                    eprintln!("Reason: {}\n", e);
                    return;
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
            match cargo_ai::openai_send_request(&url, &model, &structured_prompt, timeout_in_sec, &token, fmt).await {
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

        let file_contents = match config_contents(agentcfg) {
            Ok(contents) => contents,
            Err(e) => {
                println!("❌ Failed to fetch agent configuration for '{agentcfg}'.");
                println!("Reason: {e}");
                println!("Hint: Ensure the agent name exists in the Cargo-AI registry or provide a local .json file.");
                return;
            }
        };

        match agent_builder::project::create_new_agent_project(&new_project_name, Ok(file_contents)) {
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

    } else if let Some(sub_m) = cmd_args.subcommand_matches("account") {

        if let Some(reg_m) = sub_m.subcommand_matches("register") {

            let email = reg_m
                .get_one::<String>("email")
                .expect("email is required");

            if let Err(e) = ensure_config_file_exists() {
                eprintln!(
                    "❌ Failed to initialize local config at '{}': {e}",
                    config_path().display()
                );
                return;
            }

            // If an account email is already configured and differs, confirm before proceeding.
            if let Some(cfg) = load_config() {
                if let Some(acct) = cfg.account.as_ref() {
                    if let Some(existing_email) = acct.email.as_ref() {
                        if existing_email != email {
                            print!(
                                "Account email is already set to '{}'. Replace with '{}'? [y/N]: ",
                                existing_email, email
                            );
                            if let Err(e) = io::stdout().flush() {
                                eprintln!("⚠️ Failed to flush stdout: {e}");
                                return;
                            }

                            let mut input = String::new();
                            if let Err(e) = io::stdin().read_line(&mut input) {
                                eprintln!("⚠️ Failed to read input: {e}");
                                return;
                            }

                            if !input.trim().eq_ignore_ascii_case("y") {
                                println!("Operation canceled.");
                                return;
                            }
                        }
                    }
                }
            }

            match infra_api::account::register::register_email(INFRA_BASE_URL, email).await {
                Ok(json) => {
                    if !ui::account_status::render_backend_ui(&json) {
                        match serde_json::to_string_pretty(&json) {
                            Ok(pretty) => println!("{pretty}"),
                            Err(_) => println!("{json:?}"),
                        }
                    }

                    // Persist the active account email locally only on successful registration.
                    if json
                        .get("status")
                        .and_then(|s| s.as_str())
                        .map(|s| s.eq_ignore_ascii_case("success"))
                        .unwrap_or(false)
                    {
                        if let Err(e) = set_account_email(email.to_string(), true) {
                            eprintln!("⚠️ Failed to save account email to config: {e}");
                        }
                    }
                }
                Err(e) => {
                    eprintln!("❌ Request failed: {e}");
                }
            }
        } else if let Some(conf_m) = sub_m.subcommand_matches("confirm") {

            let code = conf_m
                .get_one::<String>("code")
                .expect("code is required");

            let cfg = match load_config() {
                Some(cfg) => cfg,
                None => {
                    eprintln!(
                        "❌ No local config file found at '{}'. Run `cargo ai account register <email>` on this machine, or copy your config from another machine.",
                        config_path().display()
                    );
                    return;
                }
            };

            // Load the configured account email (set during registration)
            let email = match cfg.account.and_then(|acct| acct.email) {
                Some(e) => e,
                None => {
                    eprintln!("❌ No account email found in config. Run `cargo ai account register <email>` first.");
                    return;
                }
            };

            match infra_api::account::confirm::confirm_email(INFRA_BASE_URL, &email, code).await {
                Ok(json) => {
                    if !ui::account_status::render_backend_ui(&json) {
                        match serde_json::to_string_pretty(&json) {
                            Ok(pretty) => println!("{pretty}"),
                            Err(_) => println!("{json:?}"),
                        }
                    }

                    // Persist tokens locally only on successful confirmation.
                    if json
                        .get("status")
                        .and_then(|s| s.as_str())
                        .map(|s| s.eq_ignore_ascii_case("success"))
                        .unwrap_or(false)
                    {
                        let creds = json.get("credentials");

                        let access_token = creds
                            .and_then(|c| c.get("access_token"))
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string());

                        let refresh_token = creds
                            .and_then(|c| c.get("refresh_token"))
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string());

                        let expires_in = creds
                            .and_then(|c| c.get("expires_in"))
                            .and_then(|v| v.as_i64())
                            .map(|n| n as i32);

                        match (access_token, refresh_token, expires_in) {
                            (Some(at), Some(rt), Some(ex)) => {
                                if let Err(e) = set_account_tokens(at, rt, ex) {
                                    eprintln!("⚠️ Failed to save account tokens to config: {e}");
                                }
                            }
                            _ => {
                                eprintln!("⚠️ Confirmation succeeded, but expected credentials were missing from the response.");
                            }
                        }
                    }
                }
                Err(e) => {
                    eprintln!("❌ Request failed: {e:?}");
                }
            }
        } else if let Some(_status_m) = sub_m.subcommand_matches("status") {
            // Account status: check and optionally refresh tokens, print status.
            //
            // Behavior:
            // - Prefer using ONLY the access token.
            // - If local timestamps indicate the token is expired (or near expiry), include refresh_token.
            // - If the server still reports `access_token_expired`, retry once with refresh_token.
            //
            // NOTE: We avoid refreshing unless needed for security reasons.

            // 1. Load config
            let cfg = match load_config() {
                Some(cfg) => cfg,
                None => {
                    eprintln!(
                        "❌ No local config file found at '{}'. Run `cargo ai account register <email>` on this machine, or copy your config from another machine.",
                        config_path().display()
                    );
                    return;
                }
            };

            // 2. Extract account
            let acct = match cfg.account.as_ref() {
                Some(acct) => acct,
                None => {
                    eprintln!("❌ No account found in config. You must confirm your account first.");
                    return;
                }
            };

            // 3. Extract tokens and token metadata
            let access_token = match acct.access_token.as_ref() {
                Some(t) => t,
                None => {
                    eprintln!("❌ No access token found in config. Run `cargo ai account confirm <code>` first.");
                    return;
                }
            };

            let refresh_token = acct.refresh_token.as_ref();
            if refresh_token.is_none() {
                eprintln!("⚠️ No refresh token found in config. Status will work only while the access token remains valid.");
            }

            // Compute token expiration using consistent integer types.
            //
            // access_token_issued_at: unix timestamp (seconds)
            // access_token_expires_in: duration in seconds
            //
            // We use a small safety buffer so we refresh slightly *before* expiry when needed.
            const EXPIRY_SAFETY_BUFFER_SEC: i64 = 30;

            let issued_at = acct.access_token_issued_at.unwrap_or(0); // i64 unix timestamp
            let expires_in_i64 = acct
                .access_token_expires_in
                .map(|n| n as i64)
                .unwrap_or(0);

            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .ok()
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);

            // If we don't have timestamps yet, do NOT pre-emptively refresh; let the server decide.
            let have_local_expiry = issued_at > 0 && expires_in_i64 > 0;
            let token_expired_or_near = if have_local_expiry {
                (issued_at + expires_in_i64 - EXPIRY_SAFETY_BUFFER_SEC) <= now
            } else {
                false
            };

            // 4. First attempt: access token only (unless local expiry suggests refresh)
            // This keeps refresh traffic low and makes token rotation explicit on demand.
            // NOTE: avoid an async closure here to keep lifetimes simple.
            let mut used_refresh = false;

            // Own the tokens so any futures we create don't borrow locals with tricky lifetimes.
            let access_token_owned = access_token.clone();
            let refresh_token_owned: Option<String> = refresh_token.cloned();

            let first_refresh_token_opt: Option<&str> = if token_expired_or_near {
                used_refresh = refresh_token_owned.is_some();
                refresh_token_owned.as_deref()
            } else {
                None
            };

            let mut response = match infra_api::account::status::fetch_status(
                INFRA_BASE_URL,
                access_token_owned.as_str(),
                first_refresh_token_opt,
            )
            .await
            {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("❌ Request failed: {e:?}");
                    return;
                }
            };

            // 5. Retry once with refresh token if the server reports expired and we didn't already refresh.
            let is_expired_error = response
                .get("status")
                .and_then(|v| v.as_str())
                .map(|s| s.eq_ignore_ascii_case("error"))
                .unwrap_or(false)
                && response
                    .get("type")
                    .and_then(|v| v.as_str())
                    .map(|t| t == "access_token_expired")
                    .unwrap_or(false);

            if is_expired_error && !used_refresh {
                if let Some(rt) = refresh_token.map(|s| s.as_str()) {
                    match infra_api::account::status::fetch_status(
                        INFRA_BASE_URL,
                        access_token_owned.as_str(),
                        Some(rt),
                    )
                    .await
                    {
                        Ok(r) => response = r,
                        Err(e) => {
                            eprintln!("❌ Request failed: {e:?}");
                            return;
                        }
                    }
                }
            }

            // 6. Render backend-provided UI when available, fallback to raw JSON.
            if !ui::account_status::render_account_status_ui(&response) {
                match serde_json::to_string_pretty(&response) {
                    Ok(pretty) => println!("{pretty}"),
                    Err(_) => println!("{response:?}"),
                }
            }

            // 7. Persist refreshed access token if present in response.
            //
            // Infra contract: when refresh occurred (and return_refreshed_access_token=true), response includes:
            //   session: { refreshed: true, access_token: "...", expires_in_seconds: 123 }
            if let Some(session) = response.get("session") {
                let new_access_token = session
                    .get("access_token")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty());

                let new_expires_in_seconds: Option<i32> = session
                    .get("expires_in_seconds")
                    .and_then(|v| v.as_i64())
                    .and_then(|n| i32::try_from(n).ok());

                if let (Some(at), Some(expires_in)) = (new_access_token, new_expires_in_seconds) {
                    // We only update if the access token actually changed.
                    if at != access_token {
                        let rt = match refresh_token {
                            Some(rt) => rt.clone(),
                            None => {
                                // Shouldn't happen in the refresh scenario, but don't clobber anything.
                                eprintln!("⚠️ Refreshed access token returned, but no refresh token exists in config to persist alongside it.");
                                return;
                            }
                        };

                        if let Err(e) = set_account_tokens(at.to_string(), rt, expires_in) {
                            eprintln!("⚠️ Failed to update account tokens in config: {e}");
                        }
                    }
                }
            }
        } else if let Some(mail_m) = sub_m.subcommand_matches("mail") {
            if let Some(test_m) = mail_m.subcommand_matches("test") {
                const DEFAULT_TEST_MAIL_SUBJECT: &str = "Cargo-AI deliverability test";
                const DEFAULT_TEST_MAIL_TEXT: &str = "This is a setup test email from Cargo-AI.";

                let subject = test_m
                    .get_one::<String>("subject")
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| DEFAULT_TEST_MAIL_SUBJECT.to_string());

                let text = test_m
                    .get_one::<String>("text")
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| DEFAULT_TEST_MAIL_TEXT.to_string());

                // 1. Load config
                let cfg = match load_config() {
                    Some(cfg) => cfg,
                    None => {
                        eprintln!(
                            "❌ No local config file found at '{}'. Run `cargo ai account register <email>` on this machine, or copy your config from another machine.",
                            config_path().display()
                        );
                        return;
                    }
                };

                // 2. Extract account
                let acct = match cfg.account.as_ref() {
                    Some(acct) => acct,
                    None => {
                        eprintln!("❌ No account found in config. You must confirm your account first.");
                        return;
                    }
                };

                // 3. Extract access token
                let access_token = match acct.access_token.as_ref() {
                    Some(t) => t,
                    None => {
                        eprintln!("❌ No access token found in config. Run `cargo ai account confirm <code>` first.");
                        return;
                    }
                };
                let refresh_token = acct.refresh_token.as_ref();

                // 4. First attempt with current access token.
                let access_token_owned = access_token.clone();
                let mut response = match infra_api::account::send_mail::send_test_mail(
                    INFRA_BASE_URL,
                    access_token_owned.as_str(),
                    subject.as_str(),
                    text.as_str(),
                )
                .await
                {
                    Ok(r) => r,
                    Err(e) => {
                        eprintln!("❌ Request failed: {e:?}");
                        return;
                    }
                };

                // 5. If access token is expired, refresh via status and retry once.
                let is_expired_error = response
                    .get("type")
                    .and_then(|v| v.as_str())
                    .map(|t| t == "access_token_expired")
                    .unwrap_or(false);

                if is_expired_error {
                    let rt = match refresh_token {
                        Some(rt) => rt,
                        None => {
                            eprintln!("⚠️ Access token expired, and no refresh token exists in config. Run `cargo ai account status` or re-confirm account.");
                            if !ui::account_status::render_backend_ui(&response) {
                                match serde_json::to_string_pretty(&response) {
                                    Ok(pretty) => println!("{pretty}"),
                                    Err(_) => println!("{response:?}"),
                                }
                            }
                            return;
                        }
                    };

                    let refresh_response = match infra_api::account::status::fetch_status(
                        INFRA_BASE_URL,
                        access_token_owned.as_str(),
                        Some(rt.as_str()),
                    )
                    .await
                    {
                        Ok(r) => r,
                        Err(e) => {
                            eprintln!("❌ Request failed while refreshing session: {e:?}");
                            return;
                        }
                    };

                    let refreshed_access_token = refresh_response
                        .get("session")
                        .and_then(|s| s.get("access_token"))
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty())
                        .map(|s| s.to_string());

                    let refreshed_expires_in: Option<i32> = refresh_response
                        .get("session")
                        .and_then(|s| s.get("expires_in_seconds"))
                        .and_then(|v| v.as_i64())
                        .and_then(|n| i32::try_from(n).ok());

                    let retry_access_token = match refreshed_access_token {
                        Some(ref at) => {
                            if let Some(expires_in) = refreshed_expires_in {
                                if let Err(e) =
                                    set_account_tokens(at.to_string(), rt.clone(), expires_in)
                                {
                                    eprintln!("⚠️ Failed to update account tokens in config: {e}");
                                }
                            }
                            at.clone()
                        }
                        None => {
                            eprintln!("⚠️ Session refresh did not return a new access token. Cannot retry send-mail request.");
                            if !ui::account_status::render_backend_ui(&refresh_response) {
                                match serde_json::to_string_pretty(&refresh_response) {
                                    Ok(pretty) => println!("{pretty}"),
                                    Err(_) => println!("{refresh_response:?}"),
                                }
                            }
                            return;
                        }
                    };

                    response = match infra_api::account::send_mail::send_test_mail(
                        INFRA_BASE_URL,
                        retry_access_token.as_str(),
                        subject.as_str(),
                        text.as_str(),
                    )
                    .await
                    {
                        Ok(r) => r,
                        Err(e) => {
                            eprintln!("❌ Request failed after session refresh: {e:?}");
                            return;
                        }
                    };
                }

                // 6. Render backend-provided UI when available, fallback to raw JSON.
                if !ui::account_status::render_backend_ui(&response) {
                    match serde_json::to_string_pretty(&response) {
                        Ok(pretty) => println!("{pretty}"),
                        Err(_) => println!("{response:?}"),
                    }
                }
            } else {
                println!("No mail subcommand found. Try 'cargo ai account mail test'.");
            }
        } else if let Some(handle_m) = sub_m.subcommand_matches("handle") {
            // Account handle: get current handle or set a new one.
            //
            // Behavior:
            // - `cargo ai account handle` => GET current handle
            // - `cargo ai account handle --set <HANDLE>` => SET handle

            // 1. Load config
            let cfg = match load_config() {
                Some(cfg) => cfg,
                None => {
                    eprintln!(
                        "❌ No local config file found at '{}'. Run `cargo ai account register <email>` on this machine, or copy your config from another machine.",
                        config_path().display()
                    );
                    return;
                }
            };

            // 2. Extract account
            let acct = match cfg.account.as_ref() {
                Some(acct) => acct,
                None => {
                    eprintln!("❌ No account found in config. You must confirm your account first.");
                    return;
                }
            };

            // 3. Extract access token
            let access_token = match acct.access_token.as_ref() {
                Some(t) => t,
                None => {
                    eprintln!("❌ No access token found in config. Run `cargo ai account confirm <code>` first.");
                    return;
                }
            };
            let refresh_token = acct.refresh_token.as_ref();

            // 4. Route GET vs SET (first attempt with current access token)
            let access_token_owned = access_token.clone();
            let new_handle_opt = handle_m
                .get_one::<String>("set")
                .map(|s| s.to_string());

            let mut response = if let Some(new_handle) = new_handle_opt.as_deref() {
                match infra_api::account::handle::set_handle(
                    INFRA_BASE_URL,
                    access_token_owned.as_str(),
                    new_handle,
                )
                .await
                {
                    Ok(r) => r,
                    Err(e) => {
                        eprintln!("❌ Request failed: {e:?}");
                        return;
                    }
                }
            } else {
                match infra_api::account::handle::fetch_handle(
                    INFRA_BASE_URL,
                    access_token_owned.as_str(),
                )
                .await
                {
                    Ok(r) => r,
                    Err(e) => {
                        eprintln!("❌ Request failed: {e:?}");
                        return;
                    }
                }
            };

            // 5. If access token is expired, refresh via status and retry handle once.
            let is_expired_error = response
                .get("type")
                .and_then(|v| v.as_str())
                .map(|t| t == "access_token_expired")
                .unwrap_or(false);

            if is_expired_error {
                let rt = match refresh_token {
                    Some(rt) => rt,
                    None => {
                        eprintln!("⚠️ Access token expired, and no refresh token exists in config. Run `cargo ai account status` or re-confirm account.");
                        if !ui::account_status::render_backend_ui(&response) {
                            match serde_json::to_string_pretty(&response) {
                                Ok(pretty) => println!("{pretty}"),
                                Err(_) => println!("{response:?}"),
                            }
                        }
                        return;
                    }
                };

                let refresh_response = match infra_api::account::status::fetch_status(
                    INFRA_BASE_URL,
                    access_token_owned.as_str(),
                    Some(rt.as_str()),
                )
                .await
                {
                    Ok(r) => r,
                    Err(e) => {
                        eprintln!("❌ Request failed while refreshing session: {e:?}");
                        return;
                    }
                };

                let refreshed_access_token = refresh_response
                    .get("session")
                    .and_then(|s| s.get("access_token"))
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string());

                let refreshed_expires_in: Option<i32> = refresh_response
                    .get("session")
                    .and_then(|s| s.get("expires_in_seconds"))
                    .and_then(|v| v.as_i64())
                    .and_then(|n| i32::try_from(n).ok());

                let retry_access_token = match refreshed_access_token {
                    Some(ref at) => {
                        if let Some(expires_in) = refreshed_expires_in {
                            if let Err(e) =
                                set_account_tokens(at.to_string(), rt.clone(), expires_in)
                            {
                                eprintln!("⚠️ Failed to update account tokens in config: {e}");
                            }
                        }
                        at.clone()
                    }
                    None => {
                        eprintln!("⚠️ Session refresh did not return a new access token. Cannot retry handle request.");
                        if !ui::account_status::render_backend_ui(&refresh_response) {
                            match serde_json::to_string_pretty(&refresh_response) {
                                Ok(pretty) => println!("{pretty}"),
                                Err(_) => println!("{refresh_response:?}"),
                            }
                        }
                        return;
                    }
                };

                response = if let Some(new_handle) = new_handle_opt.as_deref() {
                    match infra_api::account::handle::set_handle(
                        INFRA_BASE_URL,
                        retry_access_token.as_str(),
                        new_handle,
                    )
                    .await
                    {
                        Ok(r) => r,
                        Err(e) => {
                            eprintln!("❌ Request failed after session refresh: {e:?}");
                            return;
                        }
                    }
                } else {
                    match infra_api::account::handle::fetch_handle(
                        INFRA_BASE_URL,
                        retry_access_token.as_str(),
                    )
                    .await
                    {
                        Ok(r) => r,
                        Err(e) => {
                            eprintln!("❌ Request failed after session refresh: {e:?}");
                            return;
                        }
                    }
                };
            }

            // 6. Render backend-provided UI when available, fallback to raw JSON.
            if !ui::account_status::render_backend_ui(&response) {
                match serde_json::to_string_pretty(&response) {
                    Ok(pretty) => println!("{pretty}"),
                    Err(_) => println!("{response:?}"),
                }
            }
        } else if let Some(agents_m) = sub_m.subcommand_matches("agents") {
            enum AgentsCommand {
                List {
                    owner_handle: Option<String>,
                    include_archived: bool,
                },
                Push {
                    name: String,
                    definition_path: Option<String>,
                    definition_json: serde_json::Value,
                },
                Pull {
                    name: String,
                    owner_handle: Option<String>,
                    definition_path: Option<String>,
                },
                Visibility {
                    name: String,
                    definition_path: Option<String>,
                    is_public: bool,
                    public_from: Option<String>,
                    public_until: Option<String>,
                },
                Archive {
                    name: String,
                    definition_path: Option<String>,
                    is_archived: bool,
                },
            }

            let agents_command = if let Some(list_m) = agents_m.subcommand_matches("list") {
                AgentsCommand::List {
                    owner_handle: list_m
                        .get_one::<String>("owner_handle")
                        .map(|s| s.to_string()),
                    include_archived: list_m.get_flag("include_archived"),
                }
            } else if let Some(push_m) = agents_m.subcommand_matches("push") {
                // TODO: Keep future push shortcuts routed through this branch so
                // name inference, validation, and request payload stay consistent.
                let json_file_path = push_m
                    .get_one::<String>("json_file")
                    .map(|s| s.to_string());

                let is_valid_inferred_name = |candidate: &str| {
                    let normalized = candidate.trim().to_lowercase();
                    if normalized.len() < 3 || normalized.len() > 32 {
                        return false;
                    }

                    let mut chars = normalized.chars();
                    match chars.next() {
                        Some(c) if c.is_ascii_lowercase() || c.is_ascii_digit() => {}
                        _ => return false,
                    }

                    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
                };

                let name = if let Some(name) = push_m.get_one::<String>("name") {
                    name.to_string()
                } else if let Some(file_path) = json_file_path.as_deref() {
                    let stem = match Path::new(file_path)
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                    {
                        Some(s) => s,
                        None => {
                            eprintln!(
                                "❌ Could not infer agent name from file '{}'. Use --name explicitly.",
                                file_path
                            );
                            return;
                        }
                    };

                    if !is_valid_inferred_name(stem) {
                        eprintln!(
                            "❌ Inferred agent name '{}' from '{}' is invalid. Use --name explicitly.",
                            stem, file_path
                        );
                        return;
                    }

                    println!("ℹ️ Using inferred agent name from file: {}", stem);
                    stem.to_string()
                } else {
                    eprintln!("❌ Missing agent name. Provide --name or use --json-file.");
                    return;
                };

                let definition_path = push_m
                    .get_one::<String>("path")
                    .map(|s| s.to_string());
                let definition_json_raw = if let Some(raw) = push_m.get_one::<String>("json") {
                    raw.to_string()
                } else if let Some(file_path) = json_file_path.as_deref() {
                    match fs::read_to_string(file_path) {
                        Ok(contents) => contents,
                        Err(e) => {
                            eprintln!("❌ Failed to read JSON file '{}': {e}", file_path);
                            return;
                        }
                    }
                } else {
                    eprintln!("❌ Missing required input: provide either --json or --json-file.");
                    return;
                };

                let definition_json = match serde_json::from_str::<serde_json::Value>(&definition_json_raw) {
                    Ok(v) => v,
                    Err(e) => {
                        eprintln!("❌ Invalid JSON provided for agent definition: {e}");
                        return;
                    }
                };

                AgentsCommand::Push {
                    name,
                    definition_path,
                    definition_json,
                }
            } else if let Some(pull_m) = agents_m.subcommand_matches("pull") {
                AgentsCommand::Pull {
                    name: pull_m
                        .get_one::<String>("name")
                        .expect("name is required")
                        .to_string(),
                    owner_handle: pull_m
                        .get_one::<String>("owner_handle")
                        .map(|s| s.to_string()),
                    definition_path: pull_m
                        .get_one::<String>("path")
                        .map(|s| s.to_string()),
                }
            } else if let Some(visibility_m) = agents_m.subcommand_matches("visibility") {
                AgentsCommand::Visibility {
                    name: visibility_m
                        .get_one::<String>("name")
                        .expect("name is required")
                        .to_string(),
                    definition_path: visibility_m
                        .get_one::<String>("path")
                        .map(|s| s.to_string()),
                    is_public: visibility_m.get_flag("public"),
                    public_from: visibility_m
                        .get_one::<String>("public_from")
                        .map(|s| s.to_string()),
                    public_until: visibility_m
                        .get_one::<String>("public_until")
                        .map(|s| s.to_string()),
                }
            } else if let Some(archive_m) = agents_m.subcommand_matches("archive") {
                AgentsCommand::Archive {
                    name: archive_m
                        .get_one::<String>("name")
                        .expect("name is required")
                        .to_string(),
                    definition_path: archive_m
                        .get_one::<String>("path")
                        .map(|s| s.to_string()),
                    is_archived: archive_m.get_flag("archive"),
                }
            } else {
                println!(
                    "No agents subcommand found. Try 'cargo ai account agents list|push|pull|visibility|archive'."
                );
                return;
            };

            // 1. Load config
            let cfg = match load_config() {
                Some(cfg) => cfg,
                None => {
                    eprintln!(
                        "❌ No local config file found at '{}'. Run `cargo ai account register <email>` on this machine, or copy your config from another machine.",
                        config_path().display()
                    );
                    return;
                }
            };

            // 2. Extract account
            let acct = match cfg.account.as_ref() {
                Some(acct) => acct,
                None => {
                    eprintln!("❌ No account found in config. You must confirm your account first.");
                    return;
                }
            };

            // 3. Extract access token
            let access_token = match acct.access_token.as_ref() {
                Some(t) => t,
                None => {
                    eprintln!("❌ No access token found in config. Run `cargo ai account confirm <code>` first.");
                    return;
                }
            };
            let refresh_token = acct.refresh_token.as_ref();

            // 4. Execute first attempt using current access token.
            let access_token_owned = access_token.clone();
            let mut response = match &agents_command {
                AgentsCommand::List {
                    owner_handle,
                    include_archived,
                } => match infra_api::account::agents::list_agents(
                    INFRA_BASE_URL,
                    access_token_owned.as_str(),
                    owner_handle.as_deref(),
                    *include_archived,
                )
                .await
                {
                    Ok(r) => r,
                    Err(e) => {
                        eprintln!("❌ Request failed: {e:?}");
                        return;
                    }
                },
                AgentsCommand::Push {
                    name,
                    definition_path,
                    definition_json,
                } => match infra_api::account::agents::push_agent(
                    INFRA_BASE_URL,
                    access_token_owned.as_str(),
                    name,
                    definition_path.as_deref(),
                    definition_json.clone(),
                )
                .await
                {
                    Ok(r) => r,
                    Err(e) => {
                        eprintln!("❌ Request failed: {e:?}");
                        return;
                    }
                },
                AgentsCommand::Pull {
                    name,
                    owner_handle,
                    definition_path,
                } => match infra_api::account::agents::pull_agent(
                    INFRA_BASE_URL,
                    access_token_owned.as_str(),
                    name,
                    owner_handle.as_deref(),
                    definition_path.as_deref(),
                )
                .await
                {
                    Ok(r) => r,
                    Err(e) => {
                        eprintln!("❌ Request failed: {e:?}");
                        return;
                    }
                },
                AgentsCommand::Visibility {
                    name,
                    definition_path,
                    is_public,
                    public_from,
                    public_until,
                } => match infra_api::account::agents::set_agent_visibility(
                    INFRA_BASE_URL,
                    access_token_owned.as_str(),
                    name,
                    definition_path.as_deref(),
                    *is_public,
                    public_from.as_deref(),
                    public_until.as_deref(),
                )
                .await
                {
                    Ok(r) => r,
                    Err(e) => {
                        eprintln!("❌ Request failed: {e:?}");
                        return;
                    }
                },
                AgentsCommand::Archive {
                    name,
                    definition_path,
                    is_archived,
                } => match infra_api::account::agents::set_agent_archive(
                    INFRA_BASE_URL,
                    access_token_owned.as_str(),
                    name,
                    definition_path.as_deref(),
                    *is_archived,
                )
                .await
                {
                    Ok(r) => r,
                    Err(e) => {
                        eprintln!("❌ Request failed: {e:?}");
                        return;
                    }
                },
            };

            // 5. If access token is expired, refresh via status and retry agents once.
            let is_expired_error = response
                .get("type")
                .and_then(|v| v.as_str())
                .map(|t| t == "access_token_expired")
                .unwrap_or(false);

            if is_expired_error {
                let rt = match refresh_token {
                    Some(rt) => rt,
                    None => {
                        eprintln!("⚠️ Access token expired, and no refresh token exists in config. Run `cargo ai account status` or re-confirm account.");
                        if !ui::account_status::render_backend_ui(&response) {
                            match serde_json::to_string_pretty(&response) {
                                Ok(pretty) => println!("{pretty}"),
                                Err(_) => println!("{response:?}"),
                            }
                        }
                        return;
                    }
                };

                let refresh_response = match infra_api::account::status::fetch_status(
                    INFRA_BASE_URL,
                    access_token_owned.as_str(),
                    Some(rt.as_str()),
                )
                .await
                {
                    Ok(r) => r,
                    Err(e) => {
                        eprintln!("❌ Request failed while refreshing session: {e:?}");
                        return;
                    }
                };

                let refreshed_access_token = refresh_response
                    .get("session")
                    .and_then(|s| s.get("access_token"))
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string());

                let refreshed_expires_in: Option<i32> = refresh_response
                    .get("session")
                    .and_then(|s| s.get("expires_in_seconds"))
                    .and_then(|v| v.as_i64())
                    .and_then(|n| i32::try_from(n).ok());

                let retry_access_token = match refreshed_access_token {
                    Some(ref at) => {
                        if let Some(expires_in) = refreshed_expires_in {
                            if let Err(e) =
                                set_account_tokens(at.to_string(), rt.clone(), expires_in)
                            {
                                eprintln!("⚠️ Failed to update account tokens in config: {e}");
                            }
                        }
                        at.clone()
                    }
                    None => {
                        eprintln!("⚠️ Session refresh did not return a new access token. Cannot retry agents request.");
                        if !ui::account_status::render_backend_ui(&refresh_response) {
                            match serde_json::to_string_pretty(&refresh_response) {
                                Ok(pretty) => println!("{pretty}"),
                                Err(_) => println!("{refresh_response:?}"),
                            }
                        }
                        return;
                    }
                };

                response = match &agents_command {
                    AgentsCommand::List {
                        owner_handle,
                        include_archived,
                    } => match infra_api::account::agents::list_agents(
                        INFRA_BASE_URL,
                        retry_access_token.as_str(),
                        owner_handle.as_deref(),
                        *include_archived,
                    )
                    .await
                    {
                        Ok(r) => r,
                        Err(e) => {
                            eprintln!("❌ Request failed after session refresh: {e:?}");
                            return;
                        }
                    },
                    AgentsCommand::Push {
                        name,
                        definition_path,
                        definition_json,
                    } => match infra_api::account::agents::push_agent(
                        INFRA_BASE_URL,
                        retry_access_token.as_str(),
                        name,
                        definition_path.as_deref(),
                        definition_json.clone(),
                    )
                    .await
                    {
                        Ok(r) => r,
                        Err(e) => {
                            eprintln!("❌ Request failed after session refresh: {e:?}");
                            return;
                        }
                    },
                    AgentsCommand::Pull {
                        name,
                        owner_handle,
                        definition_path,
                    } => match infra_api::account::agents::pull_agent(
                        INFRA_BASE_URL,
                        retry_access_token.as_str(),
                        name,
                        owner_handle.as_deref(),
                        definition_path.as_deref(),
                    )
                    .await
                    {
                        Ok(r) => r,
                        Err(e) => {
                            eprintln!("❌ Request failed after session refresh: {e:?}");
                            return;
                        }
                    },
                    AgentsCommand::Visibility {
                        name,
                        definition_path,
                        is_public,
                        public_from,
                        public_until,
                    } => match infra_api::account::agents::set_agent_visibility(
                        INFRA_BASE_URL,
                        retry_access_token.as_str(),
                        name,
                        definition_path.as_deref(),
                        *is_public,
                        public_from.as_deref(),
                        public_until.as_deref(),
                    )
                    .await
                    {
                        Ok(r) => r,
                        Err(e) => {
                            eprintln!("❌ Request failed after session refresh: {e:?}");
                            return;
                        }
                    },
                    AgentsCommand::Archive {
                        name,
                        definition_path,
                        is_archived,
                    } => match infra_api::account::agents::set_agent_archive(
                        INFRA_BASE_URL,
                        retry_access_token.as_str(),
                        name,
                        definition_path.as_deref(),
                        *is_archived,
                    )
                    .await
                    {
                        Ok(r) => r,
                        Err(e) => {
                            eprintln!("❌ Request failed after session refresh: {e:?}");
                            return;
                        }
                    },
                };
            }

            // 6. Render backend-provided UI when available, fallback to raw JSON.
            if !ui::account_status::render_backend_ui(&response) {
                match serde_json::to_string_pretty(&response) {
                    Ok(pretty) => println!("{pretty}"),
                    Err(_) => println!("{response:?}"),
                }
            }
        } else {
            println!("No account subcommand found. Try 'cargo ai account register <email>', 'cargo ai account confirm <code>', 'cargo ai account status', 'cargo ai account mail test [--subject <text>] [--text <text>]', 'cargo ai account handle [--set <handle>]', or 'cargo ai account agents <list|push|pull|visibility|archive>'.");
        }

    } else if let Some(sub_m) = cmd_args.subcommand_matches("profile") {
        if let Some(_) = sub_m.subcommand_matches("list") {
            if let Some(cfg) = load_config() {
                println!("Configured profiles:");
                println!("{:<20} {:<10} {:<15} {}", "Name", "Server", "Model", "Default");
                println!("{:-<65}", "");

                let default_name = cfg.default_profile.clone();

                for profile in cfg.profile {
                    let is_default = default_name.as_ref().map(|d| d == &profile.name).unwrap_or(false);
                    let mark = if is_default { "✓" } else { "" };

                    println!("{:<20} {:<10} {:<15} {}", profile.name, profile.server, profile.model, mark);
                }
            } else {
                println!("No config file found.");
            }
        } else if let Some(add_m) = sub_m.subcommand_matches("add") {
            let name = add_m.get_one::<String>("name").expect("Profile name is required");
            let server = add_m.get_one::<String>("server").expect("Server is required");
            let model = add_m.get_one::<String>("model").expect("Model is required");
            let url = add_m.get_one::<String>("url").map(String::as_str).unwrap_or("(none)");
            let token = add_m.get_one::<String>("token").map(String::as_str).unwrap_or("(none)");
            let description = add_m.get_one::<String>("description").map(String::as_str).unwrap_or("(none)");

            println!("Adding profile:");
            println!("  Name: {}", name);
            println!("  Server: {}", server);
            println!("  Model: {}", model);
            println!("  URL: {}", url);
            println!("  Token: {}", token);
            println!("  Description: {}", description);


            let new_profile = Profile {
                name: name.to_string(),
                server: server.to_string(),
                model: model.to_string(),
                url: if url == "(none)" { None } else { Some(url.to_string()) },
                token: if token == "(none)" { None } else { Some(token.to_string()) },
                timeout_in_sec: 60, // default for now
                description: if description == "(none)" { None } else { Some(description.to_string()) },
            };

            let set_as_default = add_m.get_flag("default");

            if let Err(e) = add_profile(new_profile, false, set_as_default) {
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
                        let is_default = cfg.default_profile.as_ref().map(|d| d == &p.name).unwrap_or(false);
                        if is_default {
                            println!("Default: Yes");
                        } else {
                            println!("Default: No");
                        }
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

    let text = resp
        .text()
        .map_err(|e| Error::new(ErrorKind::Other, e.to_string()))?;

    // If the registry returns a JSON object with an `error` field,
    // treat it as an error instead of passing it through as config.
    if let Ok(val) = serde_json::from_str::<serde_json::Value>(&text) {
        if let Some(err_msg) = val.get("error").and_then(|e| e.as_str()) {
            return Err(Error::new(
                ErrorKind::Other,
                format!("registry error for '{name}': {err_msg}"),
            ));
        }
    }

    Ok(text)
}
