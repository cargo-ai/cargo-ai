mod args;
mod web_resources;
mod config;
mod credentials;
mod providers;

use jsonlogic::apply;
use serde::{Deserialize, Serialize};

use config::loader::{find_profile, load_config};
use config::schema::{Profile, ProfileAuthMode};
use providers::{provider_error_messages, validate_provider_request, ProviderKind};

include!(concat!(env!("OUT_DIR"), "/agent_model.rs"));

const OPENAI_INFRA_BASE_URL: &str = "https://api.cargo-ai.org";
const OPENAI_REFRESH_BUFFER_SEC: i64 = 30;

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

fn resolve_profile_api_token(profile: &SelectedProfile) -> Result<String, String> {
    match credentials::store::load_profile_token(&profile.name) {
        Ok(Some(token)) if !token.trim().is_empty() => Ok(token),
        Ok(Some(_)) | Ok(None) => profile
            .legacy_token
            .as_deref()
            .map(str::trim)
            .filter(|token| !token.is_empty())
            .map(str::to_string)
            .ok_or_else(|| {
                format!(
                    "Missing API token for profile '{}'. Use `cargo ai profile token set {} --token <TOKEN>`.",
                    profile.name, profile.name
                )
            }),
        Err(error) => Err(format!(
            "Failed to load profile token for '{}': {error}",
            profile.name
        )),
    }
}

fn now_unix_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

fn openai_token_expired_or_near(config: Option<&config::schema::Config>) -> bool {
    let Some(config) = config else {
        return false;
    };
    let Some(openai_auth) = config.openai_auth.as_ref() else {
        return false;
    };
    let Some(issued_at) = openai_auth.access_token_issued_at else {
        return false;
    };
    let Some(expires_in) = openai_auth.access_token_expires_in else {
        return false;
    };

    if issued_at <= 0 || expires_in <= 0 {
        return false;
    }

    let expires_at = issued_at.saturating_add(expires_in as i64);
    expires_at.saturating_sub(OPENAI_REFRESH_BUFFER_SEC) <= now_unix_seconds()
}

#[derive(Debug, Deserialize)]
struct OpenAiSessionStatusResponse {
    #[serde(default)]
    session: Option<OpenAiSessionStatusSession>,

    #[serde(default)]
    message: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpenAiSessionStatusSession {
    #[serde(default)]
    access_token: Option<String>,
}

async fn refresh_openai_oauth_access_token(
    access_token: &str,
    refresh_token: &str,
) -> Result<String, String> {
    let payload = serde_json::json!({
        "action": "session_status",
        "credentials": {
            "access_token": access_token,
            "refresh_token": refresh_token
        }
    });

    let url = format!("{}/auth/openai", OPENAI_INFRA_BASE_URL.trim_end_matches('/'));
    let response = reqwest::Client::new()
        .post(url)
        .header("Content-Type", "application/json")
        .json(&payload)
        .send()
        .await
        .map_err(|error| format!("request failed: {error}"))?;

    let body = response
        .json::<OpenAiSessionStatusResponse>()
        .await
        .map_err(|error| format!("failed to parse response JSON: {error}"))?;

    body.session
        .and_then(|session| session.access_token)
        .map(|access_token| access_token.trim().to_string())
        .filter(|access_token| !access_token.is_empty())
        .ok_or_else(|| {
            body.message.unwrap_or_else(|| {
                "OpenAI session refresh succeeded but no access token was returned.".to_string()
            })
        })
}

async fn resolve_openai_oauth_access_token(
    config: Option<&config::schema::Config>,
) -> Result<String, String> {
    let tokens = credentials::store::load_openai_oauth_tokens().map_err(|error| {
        format!("failed to load OpenAI OAuth session from secret store: {error}")
    })?;

    let Some(tokens) = tokens else {
        return Err(
            "OpenAI authentication is missing. Run `cargo ai auth login openai` or pass `--token`."
                .to_string(),
        );
    };

    let access_token = tokens.access_token.trim().to_string();
    if access_token.is_empty() {
        return Err(
            "OpenAI OAuth session exists but access token is empty. Re-run `cargo ai auth login openai`."
                .to_string(),
        );
    }

    if openai_token_expired_or_near(config) {
        let Some(refresh_token) = tokens
            .refresh_token
            .as_deref()
            .map(str::trim)
            .filter(|token| !token.is_empty())
        else {
            return Err(
                "OpenAI access token is expired/near expiry and no refresh token is available. Re-run `cargo ai auth login openai`."
                    .to_string(),
            );
        };

        let refreshed_token =
            refresh_openai_oauth_access_token(access_token.as_str(), refresh_token).await?;
        println!("Refreshed OpenAI account session for this invocation.");
        return Ok(refreshed_token);
    }

    Ok(access_token)
}

async fn resolve_openai_token_for_request(
    selected_profile: Option<&SelectedProfile>,
    config: Option<&config::schema::Config>,
) -> Result<String, String> {
    match selected_profile {
        Some(profile) => match profile.auth_mode {
            ProfileAuthMode::ApiKey => resolve_profile_api_token(profile),
            ProfileAuthMode::OpenaiAccount => resolve_openai_oauth_access_token(config).await,
            ProfileAuthMode::None => Err(format!(
                "Profile '{}' auth mode is '{}'. Set it to '{}' or '{}' before using OpenAI without `--token`.",
                profile.name,
                ProfileAuthMode::None.as_str(),
                ProfileAuthMode::ApiKey.as_str(),
                ProfileAuthMode::OpenaiAccount.as_str()
            )),
        },
        None => resolve_openai_oauth_access_token(config).await,
    }
}

fn apply_profile(profile: &Profile, server: &mut String, model: &mut String, timeout_in_sec: &mut u64, url: &mut String) -> SelectedProfile {
    *server = profile.server.clone().to_lowercase();
    *model = profile.model.clone();
    *timeout_in_sec = profile.timeout_in_sec;
    *url = profile.url.clone().unwrap_or_default();

    SelectedProfile {
        name: profile.name.clone(),
        auth_mode: profile.auth_mode,
        legacy_token: profile.token.clone(),
    }
}

// Initialize Tokio runtime macro
#[tokio::main]
async fn main() {
    let cmd_args = args::build_cli();
    let config = load_config();

    let mut server = String::new();
    let mut model = String::new();
    let mut url = String::new();
    let mut token = String::new();
    let mut timeout_in_sec: u64 = 60;
    let mut selected_profile: Option<SelectedProfile> = None;

    if let Some(profile_name) = cmd_args.get_one::<String>("profile") {
        if let Some(profile) = config
            .as_ref()
            .and_then(|cfg| find_profile(cfg, profile_name))
        {
            selected_profile = Some(apply_profile(
                profile,
                &mut server,
                &mut model,
                &mut timeout_in_sec,
                &mut url,
            ));
            println!("Using profile '{}'", profile_name);
        } else if config.is_some() {
            eprintln!("Profile '{}' not found.", profile_name);
        } else {
            eprintln!("No config file found.");
        }
    }

    if server.is_empty() {
        if let Some(profile) = config
            .as_ref()
            .and_then(|cfg| cfg.default_profile.as_deref().and_then(|name| find_profile(cfg, name)))
        {
            selected_profile = Some(apply_profile(
                profile,
                &mut server,
                &mut model,
                &mut timeout_in_sec,
                &mut url,
            ));
            println!("Using default profile '{}'", profile.name);
        }
    }

    if let Some(server_arg) = cmd_args.get_one::<String>("server") {
        server = server_arg.to_lowercase();
    }

    if let Some(model_arg) = cmd_args.get_one::<String>("model") {
        model = model_arg.to_string();
    }

    if let Some(url_arg) = cmd_args.get_one::<String>("url") {
        url = url_arg.to_string();
    }

    if let Some(timeout_arg) = cmd_args.get_one::<String>("timeout_in_sec") {
        timeout_in_sec = timeout_arg.parse::<u64>().unwrap_or(60);
    }

    let prompt = if let Some(prompt_arg) = cmd_args.get_one::<String>("prompt") {
        prompt_arg.to_string()
    } else {
        prompt()
    };

    let provider = match ProviderKind::from_server_value(&server) {
        Some(provider) => provider,
        None => {
            for line in unknown_server_messages(&server) {
                eprintln!("{line}");
            }
            return;
        }
    };

    if url.is_empty() {
        url = provider.default_url().to_string();
    }

    let explicit_token_override = cmd_args.get_one::<String>("token").map(|token| token.to_string());
    if let Some(cmd_token) = explicit_token_override {
        if provider == ProviderKind::OpenAi {
            println!("Using explicit --token override; bypassing profile auth-mode resolution.");
        }
        token = cmd_token;
    } else if provider == ProviderKind::OpenAi {
        token = match resolve_openai_token_for_request(selected_profile.as_ref(), config.as_ref()).await {
            Ok(token) => token,
            Err(error) => {
                eprintln!("❌ {error}");
                return;
            }
        };
    }

    if let Err(validation_issues) = validate_provider_request(provider, &model, &url, &token) {
        for issue in validation_issues {
            eprintln!("{issue}");
        }
        return;
    }

    let static_context =
        "A question will be asked and you will need to return the answer in the specified JSON format.";

    let resources = resource_urls();
    let data_block = match web_resources::build_data_block(&resources).await {
        Ok(data_block) => data_block,
        Err(error) => {
            eprintln!("❌ Failed to fetch required web resources.");
            eprintln!("Reason: {error}");
            return;
        }
    };

    let context = format!("{}\n\n{}", static_context, data_block);
    let mut ai_cargo = crate::providers::AgentCargo::<Output>::new(prompt.clone(), context);

    let structured_prompt = ai_cargo.prompt();
    let mut response = String::new();

    if provider == ProviderKind::Ollama {
        match crate::providers::send_ollama_request(
            &url,
            &model,
            &structured_prompt,
            timeout_in_sec,
            json_schema_value(),
        )
        .await
        {
            Ok(r) => {
                response.push_str(&r);
            }
            Err(error) => {
                for line in provider_error_messages(&error) {
                    eprintln!("{line}");
                }
                return;
            }
        }
    } else if provider == ProviderKind::OpenAi {
        let mut schema = json_schema_value();
        if let Some(obj) = schema.as_object_mut() {
            obj.insert("additionalProperties".into(), serde_json::Value::Bool(false));
        }

        let fmt = serde_json::json!({
            "type": "json_schema",
            "json_schema": {
                "name": "Output",
                "schema": schema,
                "strict": true
            }
        });

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
                    eprintln!("{line}");
                }
                return;
            }
        };
    }

    if !ai_cargo.set_response(response.clone()) {
        eprintln!("❌ LLM output did NOT conform to the required JSON schema.");
        eprintln!("Raw output received from server:\n{}\n", response);
        return;
    }

    let output = match ai_cargo.get_response() {
        Some(o) => o,
        None => {
            eprintln!("❌ Internal error: response was expected but missing.");
            eprintln!("Raw output received from server:\n{}\n", response);
            return;
        }
    };

    let actions = actions();
    apply_actions(&output, &actions);
}

pub fn apply_actions(output: &Output, actions: &[Action]) {
    let data = serde_json::to_value(output).unwrap();

    for action in actions {
        if let Ok(result) = apply(&action.logic, &data) {
            if result.as_bool() == Some(true) {
                for step in &action.run {
                    println!("Running '{}': {} {:?}", action.name, step.program, step.args);

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
