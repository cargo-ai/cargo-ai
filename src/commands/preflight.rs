//! Runtime behavior for `cargo ai preflight`.
use clap::ArgMatches;

use crate::config::loader::{find_profile, load_config};
use crate::config::schema::ProfileAuthMode;
use crate::credentials::{openai_oauth, store};
use crate::providers::{provider_error_messages, validate_provider_request, ProviderKind};

fn unknown_server_messages(server: &str) -> Vec<String> {
    let display_server = if server.trim().is_empty() {
        "(not set)"
    } else {
        server
    };

    vec![
        format!("❌ Unknown AI server '{}'.", display_server),
        "Use `--server ollama` or `--server openai`.".to_string(),
        "Hint: Set `--server` explicitly or configure a default profile with a supported server."
            .to_string(),
        "Example: cargo ai preflight --server ollama --model mistral --prompt \"What is 2 + 2?\""
            .to_string(),
    ]
}

#[derive(Debug, Clone)]
struct SelectedProfile {
    name: String,
    auth_mode: ProfileAuthMode,
    legacy_token: Option<String>,
}

#[derive(Debug, Clone)]
struct ResolvedOpenAiToken {
    token: String,
    uses_account_session: bool,
}

fn resolve_profile_api_token(profile: &SelectedProfile) -> Result<String, String> {
    match store::load_profile_token(&profile.name) {
        Ok(Some(token)) if !token.trim().is_empty() => Ok(token),
        Ok(Some(_)) | Ok(None) => profile
            .legacy_token
            .as_deref()
            .map(str::trim)
            .filter(|token| !token.is_empty())
            .map(str::to_string)
            .ok_or_else(|| {
                format!(
                    "Missing API token for profile '{}'. Use `cargo ai profile set {} --token <TOKEN> --auth api_key`.",
                    profile.name, profile.name
                )
            }),
        Err(error) => {
            Err(format!(
                "Failed to load profile token for '{}': {error}",
                profile.name
            ))
        }
    }
}

async fn resolve_openai_token_for_request(
    selected_profile: Option<&SelectedProfile>,
) -> Result<ResolvedOpenAiToken, String> {
    match selected_profile {
        Some(profile) => match profile.auth_mode {
            ProfileAuthMode::ApiKey => Ok(ResolvedOpenAiToken {
                token: resolve_profile_api_token(profile)?,
                uses_account_session: false,
            }),
            ProfileAuthMode::OpenaiAccount => {
                let session = openai_oauth::resolve_session_for_runtime().await?;
                Ok(ResolvedOpenAiToken {
                    token: session.access_token,
                    uses_account_session: true,
                })
            }
            ProfileAuthMode::None => Err(format!(
                "Profile '{}' auth mode is '{}'. Set it to '{}' or '{}' before using OpenAI without `--token`.",
                profile.name,
                ProfileAuthMode::None.as_str(),
                ProfileAuthMode::ApiKey.as_str(),
                ProfileAuthMode::OpenaiAccount.as_str()
            )),
        },
        None => {
            let session = openai_oauth::resolve_session_for_runtime().await?;
            Ok(ResolvedOpenAiToken {
                token: session.access_token,
                uses_account_session: true,
            })
        }
    }
}

/// Executes the preflight flow: resolve runtime settings, call provider, and
/// run any configured post-response actions.
pub async fn run(sub_m: &ArgMatches) -> bool {
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
    let mut selected_profile: Option<SelectedProfile> = None;
    let mut use_openai_account_transport = false;

    // 1️⃣ If profile is set, load values from config
    if let Some(profile_name) = sub_m.get_one::<String>("profile") {
        if let Some(cfg) = load_config() {
            if let Some(profile) = find_profile(&cfg, profile_name) {
                server = profile.server.clone().to_lowercase();
                model = profile.model.clone();
                timeout_in_sec = profile.timeout_in_sec;
                // Updated URL assignment logic:
                url = profile.url.clone().unwrap_or_default();
                selected_profile = Some(SelectedProfile {
                    name: profile.name.clone(),
                    auth_mode: profile.auth_mode,
                    legacy_token: profile.token.clone(),
                });
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
                    timeout_in_sec = profile.timeout_in_sec;
                    url = profile.url.clone().unwrap_or_default();
                    selected_profile = Some(SelectedProfile {
                        name: profile.name.clone(),
                        auth_mode: profile.auth_mode,
                        legacy_token: profile.token.clone(),
                    });
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

    if let Some(url_arg) = sub_m.get_one::<String>("url") {
        url = url_arg.to_string();
    }

    let explicit_token_override = sub_m
        .get_one::<String>("token")
        .map(|token| token.to_string());

    if let Some(timeout_arg) = sub_m.get_one::<String>("timeout_in_sec") {
        timeout_in_sec = timeout_arg.parse::<u64>().unwrap_or(60);
    }

    let provider = match ProviderKind::from_server_value(&server) {
        Some(provider) => provider,
        None => {
            for line in unknown_server_messages(&server) {
                eprintln!("{}", line);
            }
            return false;
        }
    };

    if let Some(cmd_token) = explicit_token_override {
        if provider == ProviderKind::OpenAi {
            println!("Using explicit --token override; bypassing profile auth-mode resolution.");
        }
        token = cmd_token;
    } else if provider == ProviderKind::OpenAi {
        token = match resolve_openai_token_for_request(selected_profile.as_ref()).await {
            Ok(resolved_token) => {
                use_openai_account_transport = resolved_token.uses_account_session;
                resolved_token.token
            }
            Err(error) => {
                eprintln!("❌ {error}");
                return false;
            }
        };
    }

    // Final URL fallback based on resolved server/auth mode.
    if url.is_empty() {
        if provider == ProviderKind::OpenAi && use_openai_account_transport {
            url = openai_oauth::OPENAI_ACCOUNT_RESPONSES_URL.to_string();
        } else {
            url = provider.default_url().to_string();
        }
    }

    if let Err(validation_issues) = validate_provider_request(provider, &model, &url, &token) {
        for issue in validation_issues {
            eprintln!("{issue}");
        }
        return false;
    }

    // End: Argument assignments

    let static_context = "A question will be asked and you will need to return the answer in the specified JSON format.";

    let resources = crate::resource_urls();

    // Build data block for LLM context
    let data_block = match crate::web_resources::build_data_block(&resources).await {
        Ok(data_block) => data_block,
        Err(error) => {
            eprintln!("❌ Failed to fetch required web resources.");
            eprintln!("Reason: {error}");
            return false;
        }
    };

    let context = format!("{}\n\n{}", static_context, data_block);

    let mut ai_cargo = crate::providers::AgentCargo::<crate::Output>::new(prompt.clone(), context);

    let structured_prompt = ai_cargo.prompt();

    let mut response = String::new(); // Holds the LLM response

    if provider == ProviderKind::Ollama {
        match crate::providers::send_ollama_request(
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
            Err(error) => {
                for line in provider_error_messages(&error) {
                    eprintln!("{}", line);
                }
                return false;
            }
        }
    } else if provider == ProviderKind::OpenAi {
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
        match crate::providers::send_openai_request(
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
            Err(error) => {
                for line in provider_error_messages(&error) {
                    eprintln!("{}", line);
                }
                return false;
            }
        };
    }

    // Attempt to conform the LLM response to the Output schema
    if !ai_cargo.set_response(response.clone()) {
        eprintln!("❌ LLM output did NOT conform to the required JSON schema.");
        eprintln!("Raw output received from server:\n{}\n", response);
        return false; // Stop execution cleanly — do NOT continue to unwrap
    }

    let output = match ai_cargo.get_response() {
        Some(o) => o,
        None => {
            eprintln!("❌ Internal error: response was expected but missing.");
            eprintln!("Raw output received from server:\n{}\n", response);
            return false;
        }
    };

    // Get Actions
    let actions = crate::actions();
    // println!("Actions {:?}", actions);

    match super::preflight_actions::apply_actions(&output, &actions) {
        Ok(()) => true,
        Err(error) => {
            eprintln!("❌ {error}");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::unknown_server_messages;
    use crate::args::test_cli_command;

    fn matches(args: &[&str]) -> clap::ArgMatches {
        test_cli_command("cargo-ai")
            .try_get_matches_from(args)
            .expect("cargo-ai args should parse")
    }

    #[test]
    fn unknown_server_messages_include_actionable_guidance() {
        let messages = unknown_server_messages("wat");
        assert!(messages
            .iter()
            .any(|line| line.contains("Unknown AI server 'wat'")));
        assert!(messages.iter().any(|line| line.contains("--server ollama")));
        assert!(messages
            .iter()
            .any(|line| line.contains("cargo ai preflight --server ollama")));
    }

    #[test]
    fn unknown_server_messages_handle_empty_value() {
        let messages = unknown_server_messages("");
        assert!(messages
            .iter()
            .any(|line| line.contains("Unknown AI server '(not set)'")));
    }

    #[tokio::test]
    async fn run_fails_closed_on_unknown_server() {
        let cmd = matches(&[
            "cargo-ai",
            "preflight",
            "--server",
            "wat",
            "--model",
            "mistral",
            "--prompt",
            "What is 2 + 2?",
        ]);
        let preflight = cmd
            .subcommand_matches("preflight")
            .expect("preflight subcommand should parse");

        assert!(!super::run(preflight).await);
    }

    #[tokio::test]
    async fn run_fails_closed_on_missing_openai_token() {
        let cmd = matches(&[
            "cargo-ai",
            "preflight",
            "--server",
            "openai",
            "--model",
            "gpt-4o-mini",
            "--token",
            "",
            "--prompt",
            "Return 4",
        ]);
        let preflight = cmd
            .subcommand_matches("preflight")
            .expect("preflight subcommand should parse");

        assert!(!super::run(preflight).await);
    }
}
