//! Runtime behavior for `cargo ai preflight`.
use clap::ArgMatches;
use std::time::Duration;

use crate::config::loader::{find_profile, load_config};
use crate::config::schema::ProfileAuthMode;
use crate::credentials::{openai_oauth, store};
use crate::providers::{
    provider_error_messages, validate_provider_content_parts, validate_provider_request,
    ProviderKind,
};

const AGENT_ACTION_MAX_DEPTH_ENV: &str = "CARGO_AI_AGENT_ACTION_MAX_DEPTH";
const DEFAULT_AGENT_ACTION_MAX_DEPTH: u32 = 5;

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
        "Example: cargo ai preflight --server ollama --model mistral --input-text \"What is 2 + 2?\""
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

#[derive(Debug, Clone, Copy)]
enum LoadedProfileKind {
    Explicit,
    Default,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RuntimeInputMode {
    Replace,
    Append,
    Prepend,
}

fn profile_selection_messages(
    kind: LoadedProfileKind,
    profile_name: &str,
    overrides: &[String],
) -> Vec<String> {
    let base_message = match kind {
        LoadedProfileKind::Explicit => format!("Using profile '{}'", profile_name),
        LoadedProfileKind::Default => format!("Using default profile '{}'", profile_name),
    };

    if overrides.is_empty() {
        vec![base_message]
    } else {
        vec![
            format!("{base_message} as fallback."),
            format!("CLI overrides: {}", overrides.join(", ")),
        ]
    }
}

fn cli_override_descriptions(sub_m: &ArgMatches, include_token_override: bool) -> Vec<String> {
    let mut overrides = Vec::new();

    if let Some(server) = sub_m.get_one::<String>("server") {
        overrides.push(format!("server={}", server.to_lowercase()));
    }

    if let Some(model) = sub_m.get_one::<String>("model") {
        overrides.push(format!("model={model}"));
    }

    if let Some(url) = sub_m.get_one::<String>("url") {
        overrides.push(format!("url={url}"));
    }

    if let Some(timeout) = sub_m.get_one::<u64>("inference_timeout_in_sec") {
        overrides.push(format!("inference_timeout_in_sec={timeout}"));
    }

    if let Some(max_depth) = sub_m.get_one::<u32>("max_agent_depth") {
        overrides.push(format!("max_agent_depth={max_depth}"));
    }

    if let Some(max_runtime) = sub_m.get_one::<u64>("max_runtime_in_sec") {
        overrides.push(format!("max_runtime_in_sec={max_runtime}"));
    }

    if include_token_override {
        overrides.push("token=(explicit)".to_string());
    }

    overrides
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

fn runtime_input_overrides(sub_m: &ArgMatches) -> Vec<crate::Input> {
    let mut ordered = Vec::new();

    collect_flagged_inputs(sub_m, "input_text")
        .into_iter()
        .for_each(|(index, value)| ordered.push((index, crate::Input::Text { text: value })));
    collect_flagged_inputs(sub_m, "input_url")
        .into_iter()
        .for_each(|(index, value)| ordered.push((index, crate::Input::Url { url: value })));
    collect_flagged_inputs(sub_m, "input_image")
        .into_iter()
        .for_each(|(index, value)| ordered.push((index, crate::Input::Image { path: value })));
    collect_flagged_inputs(sub_m, "input_file")
        .into_iter()
        .for_each(|(index, value)| ordered.push((index, crate::Input::File { path: value })));

    ordered.sort_by_key(|(index, _)| *index);
    ordered.into_iter().map(|(_, input)| input).collect()
}

fn runtime_input_mode(sub_m: &ArgMatches) -> Result<RuntimeInputMode, String> {
    match sub_m.get_one::<String>("input_mode").map(String::as_str) {
        None | Some("replace") => Ok(RuntimeInputMode::Replace),
        Some("append") => Ok(RuntimeInputMode::Append),
        Some("prepend") => Ok(RuntimeInputMode::Prepend),
        Some(other) => Err(format!(
            "Unsupported --input-mode '{other}'. Expected replace, append, or prepend."
        )),
    }
}

fn collect_flagged_inputs(sub_m: &ArgMatches, id: &str) -> Vec<(usize, String)> {
    match (sub_m.indices_of(id), sub_m.get_many::<String>(id)) {
        (Some(indices), Some(values)) => indices
            .zip(values)
            .map(|(index, value)| (index, value.to_string()))
            .collect(),
        _ => Vec::new(),
    }
}

fn resolved_inputs_for_run(sub_m: &ArgMatches) -> Result<Vec<crate::Input>, String> {
    let runtime_inputs = runtime_input_overrides(sub_m);

    if runtime_inputs.is_empty() {
        if sub_m.get_one::<String>("input_mode").is_some() {
            return Err(
                "--input-mode requires at least one runtime input flag such as --input-text, --input-url, --input-image, or --input-file."
                    .to_string(),
            );
        }
        return Ok(crate::inputs());
    }

    let input_mode = runtime_input_mode(sub_m)?;
    Ok(match input_mode {
        RuntimeInputMode::Replace => runtime_inputs,
        RuntimeInputMode::Append => {
            let mut selected_inputs = crate::inputs();
            selected_inputs.extend(runtime_inputs);
            selected_inputs
        }
        RuntimeInputMode::Prepend => {
            let mut selected_inputs = runtime_inputs;
            selected_inputs.extend(crate::inputs());
            selected_inputs
        }
    })
}

fn validate_structural_action_only_inputs(
    has_output_schema_properties: bool,
    selected_inputs: &[crate::Input],
) -> Result<(), String> {
    if has_output_schema_properties || selected_inputs.is_empty() {
        return Ok(());
    }

    Err(
        "This agent declares empty `agent_schema.properties`; runtime model-facing input flags such as --input-text, --input-url, --input-image, and --input-file are not allowed because there is no model pass to consume them."
            .to_string(),
    )
}

fn empty_action_only_output() -> Result<crate::Output, String> {
    serde_json::from_value(serde_json::json!({})).map_err(|error| {
        format!("Internal error: failed to initialize action-only output placeholder: {error}")
    })
}

fn resolved_runtime_vars_for_run(
    sub_m: &ArgMatches,
) -> Result<serde_json::Map<String, serde_json::Value>, String> {
    resolve_runtime_vars_from_specs(sub_m, &crate::runtime_var_specs())
}

fn resolve_runtime_vars_from_specs(
    sub_m: &ArgMatches,
    specs: &[crate::RuntimeVarSpec],
) -> Result<serde_json::Map<String, serde_json::Value>, String> {
    let mut declared_specs = std::collections::BTreeMap::new();
    for spec in specs {
        declared_specs.insert(spec.name.as_str(), spec);
    }

    let mut resolved = serde_json::Map::new();
    let mut provided_names = std::collections::BTreeSet::new();

    for raw_assignment in sub_m.get_many::<String>("run_var").into_iter().flatten() {
        let (name, raw_value) = parse_runtime_var_assignment(raw_assignment)?;
        let Some(spec) = declared_specs.get(name) else {
            return Err(format!(
                "Runtime variable '{name}' was provided via --run-var but is not declared in runtime_vars."
            ));
        };

        if !provided_names.insert(name.to_string()) {
            return Err(format!(
                "Duplicate runtime variable '{name}' provided via --run-var; each runtime variable may be set at most once per invocation."
            ));
        }

        let parsed_value = parse_runtime_var_value(spec.field_type, raw_value, name)?;
        resolved.insert(name.to_string(), parsed_value);
    }

    for spec in specs {
        if resolved.contains_key(&spec.name) {
            continue;
        }

        if let Some(default_value) = spec.default_value.clone() {
            resolved.insert(spec.name.clone(), default_value);
            continue;
        }

        return Err(format!(
            "Runtime variable '{}' is declared in runtime_vars with no default; provide it via --run-var {}=<value>.",
            spec.name, spec.name
        ));
    }

    Ok(resolved)
}

fn parse_runtime_var_assignment(raw_assignment: &str) -> Result<(&str, &str), String> {
    let Some((name, value)) = raw_assignment.split_once('=') else {
        return Err(format!(
            "Invalid --run-var '{raw_assignment}'; expected NAME=VALUE."
        ));
    };

    let trimmed_name = name.trim();
    if trimmed_name.is_empty() {
        return Err(format!(
            "Invalid --run-var '{raw_assignment}'; runtime variable name cannot be empty."
        ));
    }

    Ok((trimmed_name, value))
}

fn parse_runtime_var_value(
    field_type: crate::RuntimeVarType,
    raw_value: &str,
    name: &str,
) -> Result<serde_json::Value, String> {
    match field_type {
        crate::RuntimeVarType::String => Ok(serde_json::Value::String(raw_value.to_string())),
        crate::RuntimeVarType::Boolean => match raw_value {
            "true" => Ok(serde_json::Value::Bool(true)),
            "false" => Ok(serde_json::Value::Bool(false)),
            "" => Err(format!(
                "Runtime variable '{name}' is declared as boolean and cannot be empty."
            )),
            _ => Err(format!(
                "Runtime variable '{name}' is declared as boolean; expected `true` or `false`, received '{raw_value}'."
            )),
        },
        crate::RuntimeVarType::Integer => {
            if raw_value.is_empty() {
                return Err(format!(
                    "Runtime variable '{name}' is declared as integer and cannot be empty."
                ));
            }

            raw_value.parse::<i64>().map(serde_json::Value::from).map_err(|_| {
                format!(
                    "Runtime variable '{name}' is declared as integer; expected a base-10 whole number, received '{raw_value}'."
                )
            })
        }
        crate::RuntimeVarType::Number => {
            if raw_value.is_empty() {
                return Err(format!(
                    "Runtime variable '{name}' is declared as number and cannot be empty."
                ));
            }

            let parsed = raw_value.parse::<f64>().map_err(|_| {
                format!(
                    "Runtime variable '{name}' is declared as number; expected a numeric value, received '{raw_value}'."
                )
            })?;
            if !parsed.is_finite() {
                return Err(format!(
                    "Runtime variable '{name}' must be a finite number, received '{raw_value}'."
                ));
            }

            let Some(number) = serde_json::Number::from_f64(parsed) else {
                return Err(format!(
                    "Runtime variable '{name}' must be a finite number, received '{raw_value}'."
                ));
            };
            Ok(serde_json::Value::Number(number))
        }
    }
}

fn inherited_agent_action_max_depth() -> Option<u32> {
    std::env::var(AGENT_ACTION_MAX_DEPTH_ENV)
        .ok()
        .and_then(|value| value.parse::<u32>().ok())
}

fn configured_agent_action_max_depth(cli_override: Option<u32>) -> u32 {
    cli_override
        .or_else(inherited_agent_action_max_depth)
        .unwrap_or(DEFAULT_AGENT_ACTION_MAX_DEPTH)
}

fn remaining_runtime_duration(
    runtime_budget: super::preflight_actions::InvocationRuntimeBudget,
    exhausted_context: &str,
) -> Result<Duration, String> {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0);

    if now_ms >= runtime_budget.deadline_ms {
        return Err(exhausted_context.to_string());
    }

    Ok(Duration::from_millis(
        runtime_budget.deadline_ms.saturating_sub(now_ms),
    ))
}

fn current_agent_runtime_timeout_message(
    runtime_budget: super::preflight_actions::InvocationRuntimeBudget,
    context: &str,
) -> String {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0);
    let elapsed_secs = now_ms
        .saturating_sub(runtime_budget.started_at_ms)
        .div_ceil(1000);

    format!(
        "Current agent exceeded max-runtime-in-sec {} after {} seconds {}.",
        runtime_budget.max_runtime_secs, elapsed_secs, context
    )
}

/// Executes the preflight flow: resolve runtime settings, call provider, and
/// run any configured post-response actions.
pub async fn run(sub_m: &ArgMatches) -> bool {
    // Begin: Argument assignments
    let mut server = String::new();
    let mut model = String::new();
    let mut url = String::new();
    let mut token = String::new();
    let mut inference_timeout_in_sec: u64 = 60; // Default
    let mut selected_profile: Option<SelectedProfile> = None;
    let mut loaded_profile_message: Option<(LoadedProfileKind, String)> = None;
    let mut use_openai_account_transport = false;

    // 1️⃣ If profile is set, load values from config
    if let Some(profile_name) = sub_m.get_one::<String>("profile") {
        if let Some(cfg) = load_config() {
            if let Some(profile) = find_profile(&cfg, profile_name) {
                server = profile.server.clone().to_lowercase();
                model = profile.model.clone();
                inference_timeout_in_sec = profile.timeout_in_sec;
                // Updated URL assignment logic:
                url = profile.url.clone().unwrap_or_default();
                selected_profile = Some(SelectedProfile {
                    name: profile.name.clone(),
                    auth_mode: profile.auth_mode,
                    legacy_token: profile.token.clone(),
                });
                loaded_profile_message =
                    Some((LoadedProfileKind::Explicit, profile_name.to_string()));
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
                    inference_timeout_in_sec = profile.timeout_in_sec;
                    url = profile.url.clone().unwrap_or_default();
                    selected_profile = Some(SelectedProfile {
                        name: profile.name.clone(),
                        auth_mode: profile.auth_mode,
                        legacy_token: profile.token.clone(),
                    });
                    loaded_profile_message =
                        Some((LoadedProfileKind::Default, default_profile_name.to_string()));
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

    if let Some(timeout_arg) = sub_m.get_one::<u64>("inference_timeout_in_sec").copied() {
        inference_timeout_in_sec = timeout_arg;
    }

    let max_agent_depth =
        configured_agent_action_max_depth(sub_m.get_one::<u32>("max_agent_depth").copied());
    let runtime_budget = super::preflight_actions::configured_agent_action_runtime_budget(
        sub_m.get_one::<u64>("max_runtime_in_sec").copied(),
    );

    let provider = match ProviderKind::from_server_value(&server) {
        Some(provider) => provider,
        None => {
            for line in unknown_server_messages(&server) {
                eprintln!("{}", line);
            }
            return false;
        }
    };

    if let Some((kind, profile_name)) = loaded_profile_message.as_ref() {
        for line in profile_selection_messages(
            *kind,
            profile_name,
            &cli_override_descriptions(
                sub_m,
                explicit_token_override.is_some() && provider == ProviderKind::OpenAi,
            ),
        ) {
            println!("{line}");
        }
    }

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

    let selected_inputs = match resolved_inputs_for_run(sub_m) {
        Ok(selected_inputs) => selected_inputs,
        Err(error) => {
            eprintln!("❌ {error}");
            return false;
        }
    };
    let runtime_vars = match resolved_runtime_vars_for_run(sub_m) {
        Ok(runtime_vars) => runtime_vars,
        Err(error) => {
            eprintln!("❌ {error}");
            return false;
        }
    };
    let has_output_schema_properties = crate::has_output_schema_properties();

    if let Err(error) =
        validate_structural_action_only_inputs(has_output_schema_properties, &selected_inputs)
    {
        eprintln!("❌ {error}");
        return false;
    }

    if !has_output_schema_properties {
        let output = match empty_action_only_output() {
            Ok(output) => output,
            Err(error) => {
                eprintln!("❌ {error}");
                return false;
            }
        };
        let actions = crate::actions();
        let action_provider_context = super::preflight_actions::ActionProviderContext {
            provider,
            url: url.clone(),
            token: token.clone(),
            inference_timeout_in_sec,
        };

        return match super::preflight_actions::apply_actions(
            &output,
            &actions,
            &runtime_vars,
            &action_provider_context,
            max_agent_depth,
            runtime_budget,
        )
        .await
        {
            Ok(()) => true,
            Err(error) => {
                eprintln!("❌ {error}");
                false
            }
        };
    }

    if let Err(validation_issues) = validate_provider_request(provider, &model, &url, &token) {
        for issue in validation_issues {
            eprintln!("{issue}");
        }
        return false;
    }

    // End: Argument assignments

    let resolved_inputs = match crate::providers::resolve_provider_inputs(&selected_inputs).await {
        Ok(resolved_inputs) => resolved_inputs,
        Err(error) => {
            eprintln!("❌ Failed to resolve runtime inputs.");
            eprintln!("Reason: {error}");
            return false;
        }
    };

    if let Err(validation_issues) =
        validate_provider_content_parts(provider, &url, &resolved_inputs)
    {
        for issue in validation_issues {
            eprintln!("{issue}");
        }
        return false;
    }

    let static_context = "A question will be asked and you will need to return the answer in the specified JSON format.";

    let mut ai_cargo = crate::providers::AgentCargo::<crate::Output>::new(
        resolved_inputs,
        static_context.to_string(),
    );

    let content_parts = ai_cargo.content_parts();

    let mut response = String::new(); // Holds the LLM response

    if provider == ProviderKind::Ollama {
        let remaining =
            match remaining_runtime_duration(runtime_budget, "before starting inference") {
                Ok(remaining) => remaining,
                Err(error) => {
                    eprintln!(
                        "❌ {}",
                        current_agent_runtime_timeout_message(runtime_budget, error.as_str())
                    );
                    return false;
                }
            };

        match tokio::time::timeout(
            remaining,
            crate::providers::send_ollama_request(
                &url,
                &model,
                &content_parts,
                inference_timeout_in_sec,
                crate::json_schema_value(),
            ),
        )
        .await
        {
            Ok(Ok(r)) => response.push_str(&r),
            Ok(Err(error)) => {
                for line in provider_error_messages(&error) {
                    eprintln!("{}", line);
                }
                return false;
            }
            Err(_) => {
                eprintln!(
                    "❌ {}",
                    current_agent_runtime_timeout_message(
                        runtime_budget,
                        "while waiting for the model response"
                    )
                );
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
        let remaining =
            match remaining_runtime_duration(runtime_budget, "before starting inference") {
                Ok(remaining) => remaining,
                Err(error) => {
                    eprintln!(
                        "❌ {}",
                        current_agent_runtime_timeout_message(runtime_budget, error.as_str())
                    );
                    return false;
                }
            };

        match tokio::time::timeout(
            remaining,
            crate::providers::send_openai_request(
                &url,
                &model,
                &content_parts,
                inference_timeout_in_sec,
                &token,
                fmt,
            ),
        )
        .await
        {
            Ok(Ok(r)) => response.push_str(&r),
            Ok(Err(error)) => {
                for line in provider_error_messages(&error) {
                    eprintln!("{}", line);
                }
                return false;
            }
            Err(_) => {
                eprintln!(
                    "❌ {}",
                    current_agent_runtime_timeout_message(
                        runtime_budget,
                        "while waiting for the model response"
                    )
                );
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
    let action_provider_context = super::preflight_actions::ActionProviderContext {
        provider,
        url: url.clone(),
        token: token.clone(),
        inference_timeout_in_sec,
    };

    match super::preflight_actions::apply_actions(
        &output,
        &actions,
        &runtime_vars,
        &action_provider_context,
        max_agent_depth,
        runtime_budget,
    )
    .await
    {
        Ok(()) => true,
        Err(error) => {
            eprintln!("❌ {error}");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        cli_override_descriptions, profile_selection_messages, resolve_runtime_vars_from_specs,
        unknown_server_messages, validate_structural_action_only_inputs, LoadedProfileKind,
    };
    use crate::args::test_cli_command;
    use serde_json::json;

    fn input_debug_strings(inputs: &[crate::Input]) -> Vec<String> {
        inputs.iter().map(|input| format!("{input:?}")).collect()
    }

    fn matches(args: &[&str]) -> clap::ArgMatches {
        test_cli_command("cargo-ai")
            .try_get_matches_from(args)
            .expect("cargo-ai args should parse")
    }

    fn runtime_var_spec(
        name: &str,
        field_type: crate::RuntimeVarType,
        default_value: Option<serde_json::Value>,
    ) -> crate::RuntimeVarSpec {
        crate::RuntimeVarSpec {
            name: name.to_string(),
            field_type,
            default_value,
        }
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

    #[test]
    fn profile_selection_messages_show_fallback_and_overrides() {
        let messages = profile_selection_messages(
            LoadedProfileKind::Default,
            "my_open_ai",
            &["server=ollama".to_string(), "model=mistral".to_string()],
        );

        assert_eq!(
            messages[0],
            "Using default profile 'my_open_ai' as fallback."
        );
        assert_eq!(messages[1], "CLI overrides: server=ollama, model=mistral");
    }

    #[test]
    fn cli_override_descriptions_capture_runtime_overrides() {
        let cmd = matches(&[
            "cargo-ai",
            "preflight",
            "--server",
            "Ollama",
            "--model",
            "mistral",
            "--inference-timeout-in-sec",
            "90",
            "--max-agent-depth",
            "3",
            "--max-runtime-in-sec",
            "180",
            "--input-text",
            "Return 4",
        ]);
        let preflight = cmd
            .subcommand_matches("preflight")
            .expect("preflight subcommand should parse");

        let overrides = cli_override_descriptions(preflight, false);

        assert_eq!(
            overrides,
            vec![
                "server=ollama".to_string(),
                "model=mistral".to_string(),
                "inference_timeout_in_sec=90".to_string(),
                "max_agent_depth=3".to_string(),
                "max_runtime_in_sec=180".to_string(),
            ]
        );
    }

    #[test]
    fn preflight_accepts_max_agent_depth_override() {
        let cmd = matches(&[
            "cargo-ai",
            "preflight",
            "--max-agent-depth",
            "4",
            "--input-text",
            "Return 4",
        ]);
        let preflight = cmd
            .subcommand_matches("preflight")
            .expect("preflight subcommand should parse");

        assert_eq!(
            preflight.get_one::<u32>("max_agent_depth").copied(),
            Some(4)
        );
    }

    #[test]
    fn preflight_accepts_max_runtime_override() {
        let cmd = matches(&[
            "cargo-ai",
            "preflight",
            "--max-runtime-in-sec",
            "240",
            "--input-text",
            "Return 4",
        ]);
        let preflight = cmd
            .subcommand_matches("preflight")
            .expect("preflight subcommand should parse");

        assert_eq!(
            preflight.get_one::<u64>("max_runtime_in_sec").copied(),
            Some(240)
        );
    }

    #[test]
    fn preflight_accepts_legacy_timeout_alias() {
        let cmd = matches(&[
            "cargo-ai",
            "preflight",
            "--timeout_in_sec",
            "45",
            "--input-text",
            "Return 4",
        ]);
        let preflight = cmd
            .subcommand_matches("preflight")
            .expect("preflight subcommand should parse");

        assert_eq!(
            preflight
                .get_one::<u64>("inference_timeout_in_sec")
                .copied(),
            Some(45)
        );
    }

    #[test]
    fn runtime_input_overrides_preserve_file_order() {
        let cmd = matches(&[
            "cargo-ai",
            "preflight",
            "--input-text",
            "hello",
            "--input-file",
            "./report.pdf",
            "--input-url",
            "https://example.com",
        ]);
        let preflight = cmd
            .subcommand_matches("preflight")
            .expect("preflight subcommand should parse");

        let overrides = super::runtime_input_overrides(preflight);
        assert_eq!(overrides.len(), 3);
        assert!(matches!(
            &overrides[0],
            crate::Input::Text { text } if text == "hello"
        ));
        assert!(matches!(
            &overrides[1],
            crate::Input::File { path } if path == "./report.pdf"
        ));
        assert!(matches!(
            &overrides[2],
            crate::Input::Url { url } if url == "https://example.com"
        ));
    }

    #[test]
    fn resolved_inputs_for_run_defaults_to_replace_mode() {
        let cmd = matches(&[
            "cargo-ai",
            "preflight",
            "--input-text",
            "hello",
            "--input-file",
            "./report.pdf",
        ]);
        let preflight = cmd
            .subcommand_matches("preflight")
            .expect("preflight subcommand should parse");

        let selected_inputs =
            super::resolved_inputs_for_run(preflight).expect("replace mode should resolve");

        assert_eq!(selected_inputs.len(), 2);
        assert!(matches!(
            &selected_inputs[0],
            crate::Input::Text { text } if text == "hello"
        ));
        assert!(matches!(
            &selected_inputs[1],
            crate::Input::File { path } if path == "./report.pdf"
        ));
    }

    #[test]
    fn resolved_inputs_for_run_explicit_append_keeps_baked_inputs_first() {
        let baked_inputs = crate::inputs();
        let baked_debug = input_debug_strings(&baked_inputs);
        let cmd = matches(&[
            "cargo-ai",
            "preflight",
            "--input-mode",
            "append",
            "--input-file",
            "./report.pdf",
            "--input-text",
            "hello",
        ]);
        let preflight = cmd
            .subcommand_matches("preflight")
            .expect("preflight subcommand should parse");

        let selected_inputs =
            super::resolved_inputs_for_run(preflight).expect("append mode should resolve");

        assert_eq!(selected_inputs.len(), baked_inputs.len() + 2);
        assert_eq!(
            input_debug_strings(&selected_inputs[..baked_inputs.len()]),
            baked_debug
        );
        assert!(matches!(
            &selected_inputs[baked_inputs.len()],
            crate::Input::File { path } if path == "./report.pdf"
        ));
        assert!(matches!(
            &selected_inputs[baked_inputs.len() + 1],
            crate::Input::Text { text } if text == "hello"
        ));
    }

    #[test]
    fn resolved_inputs_for_run_explicit_prepend_keeps_runtime_inputs_first() {
        let baked_inputs = crate::inputs();
        let baked_debug = input_debug_strings(&baked_inputs);
        let cmd = matches(&[
            "cargo-ai",
            "preflight",
            "--input-mode",
            "prepend",
            "--input-url",
            "https://example.com",
            "--input-image",
            "./image.png",
        ]);
        let preflight = cmd
            .subcommand_matches("preflight")
            .expect("preflight subcommand should parse");

        let selected_inputs =
            super::resolved_inputs_for_run(preflight).expect("prepend mode should resolve");

        assert_eq!(selected_inputs.len(), baked_inputs.len() + 2);
        assert!(matches!(
            &selected_inputs[0],
            crate::Input::Url { url } if url == "https://example.com"
        ));
        assert!(matches!(
            &selected_inputs[1],
            crate::Input::Image { path } if path == "./image.png"
        ));
        assert_eq!(input_debug_strings(&selected_inputs[2..]), baked_debug);
    }

    #[test]
    fn resolved_inputs_for_run_rejects_input_mode_without_runtime_inputs() {
        let cmd = matches(&["cargo-ai", "preflight", "--input-mode", "append"]);
        let preflight = cmd
            .subcommand_matches("preflight")
            .expect("preflight subcommand should parse");

        let error = super::resolved_inputs_for_run(preflight)
            .expect_err("missing runtime inputs should fail");

        assert!(error.contains("--input-mode requires at least one runtime input flag"));
    }

    #[test]
    fn resolve_runtime_vars_from_specs_parses_declared_types_and_defaults() {
        let cmd = matches(&[
            "cargo-ai",
            "preflight",
            "--run-var",
            "subject=Quarterly Review",
            "--run-var",
            "generate_images=true",
            "--run-var",
            "retry_count=3",
            "--run-var",
            "score_threshold=0.85",
        ]);
        let preflight = cmd
            .subcommand_matches("preflight")
            .expect("preflight subcommand should parse");

        let runtime_vars = resolve_runtime_vars_from_specs(
            preflight,
            &[
                runtime_var_spec("subject", crate::RuntimeVarType::String, None),
                runtime_var_spec("generate_images", crate::RuntimeVarType::Boolean, None),
                runtime_var_spec("retry_count", crate::RuntimeVarType::Integer, None),
                runtime_var_spec(
                    "score_threshold",
                    crate::RuntimeVarType::Number,
                    Some(json!(0.75)),
                ),
            ],
        )
        .expect("runtime vars should parse");

        assert_eq!(
            runtime_vars.get("subject"),
            Some(&json!("Quarterly Review"))
        );
        assert_eq!(runtime_vars.get("generate_images"), Some(&json!(true)));
        assert_eq!(runtime_vars.get("retry_count"), Some(&json!(3)));
        assert_eq!(runtime_vars.get("score_threshold"), Some(&json!(0.85)));
    }

    #[test]
    fn resolve_runtime_vars_from_specs_uses_defaults_when_cli_value_is_missing() {
        let cmd = matches(&["cargo-ai", "preflight"]);
        let preflight = cmd
            .subcommand_matches("preflight")
            .expect("preflight subcommand should parse");

        let runtime_vars = resolve_runtime_vars_from_specs(
            preflight,
            &[runtime_var_spec(
                "generate_images",
                crate::RuntimeVarType::Boolean,
                Some(json!(false)),
            )],
        )
        .expect("defaulted runtime vars should resolve");

        assert_eq!(runtime_vars.get("generate_images"), Some(&json!(false)));
    }

    #[test]
    fn resolve_runtime_vars_from_specs_rejects_duplicates() {
        let cmd = matches(&[
            "cargo-ai",
            "preflight",
            "--run-var",
            "subject=alpha",
            "--run-var",
            "subject=beta",
        ]);
        let preflight = cmd
            .subcommand_matches("preflight")
            .expect("preflight subcommand should parse");

        let error = resolve_runtime_vars_from_specs(
            preflight,
            &[runtime_var_spec(
                "subject",
                crate::RuntimeVarType::String,
                None,
            )],
        )
        .expect_err("duplicate runtime vars should fail");

        assert!(error.contains("Duplicate runtime variable 'subject'"));
    }

    #[test]
    fn resolve_runtime_vars_from_specs_rejects_undeclared_names() {
        let cmd = matches(&["cargo-ai", "preflight", "--run-var", "subject=alpha"]);
        let preflight = cmd
            .subcommand_matches("preflight")
            .expect("preflight subcommand should parse");

        let error = resolve_runtime_vars_from_specs(preflight, &[])
            .expect_err("undeclared runtime vars should fail");

        assert!(error.contains("is not declared in runtime_vars"));
    }

    #[test]
    fn resolve_runtime_vars_from_specs_requires_missing_non_defaulted_vars() {
        let cmd = matches(&["cargo-ai", "preflight"]);
        let preflight = cmd
            .subcommand_matches("preflight")
            .expect("preflight subcommand should parse");

        let error = resolve_runtime_vars_from_specs(
            preflight,
            &[runtime_var_spec(
                "subject",
                crate::RuntimeVarType::String,
                None,
            )],
        )
        .expect_err("missing runtime vars should fail");

        assert!(error.contains("provide it via --run-var subject=<value>"));
    }

    #[test]
    fn resolve_runtime_vars_from_specs_rejects_invalid_boolean_value() {
        let cmd = matches(&["cargo-ai", "preflight", "--run-var", "generate_images=yes"]);
        let preflight = cmd
            .subcommand_matches("preflight")
            .expect("preflight subcommand should parse");

        let error = resolve_runtime_vars_from_specs(
            preflight,
            &[runtime_var_spec(
                "generate_images",
                crate::RuntimeVarType::Boolean,
                None,
            )],
        )
        .expect_err("invalid booleans should fail");

        assert!(error.contains("expected `true` or `false`"));
    }

    #[test]
    fn resolve_runtime_vars_from_specs_rejects_empty_non_string_values() {
        let cmd = matches(&["cargo-ai", "preflight", "--run-var", "retry_count="]);
        let preflight = cmd
            .subcommand_matches("preflight")
            .expect("preflight subcommand should parse");

        let error = resolve_runtime_vars_from_specs(
            preflight,
            &[runtime_var_spec(
                "retry_count",
                crate::RuntimeVarType::Integer,
                None,
            )],
        )
        .expect_err("empty integers should fail");

        assert!(error.contains("cannot be empty"));
    }

    #[test]
    fn validate_structural_action_only_inputs_allows_empty_input_set() {
        let selected_inputs = Vec::new();

        let result = validate_structural_action_only_inputs(false, &selected_inputs);

        assert!(result.is_ok(), "empty input set should be allowed");
    }

    #[test]
    fn validate_structural_action_only_inputs_rejects_runtime_inputs() {
        let selected_inputs = vec![crate::Input::Text {
            text: "hello".to_string(),
        }];

        let error = validate_structural_action_only_inputs(false, &selected_inputs)
            .expect_err("runtime inputs should be rejected");

        assert!(error.contains("empty `agent_schema.properties`"));
        assert!(error.contains("--input-text"));
    }

    #[test]
    fn validate_structural_action_only_inputs_allows_schema_backed_agents() {
        let selected_inputs = vec![crate::Input::Text {
            text: "hello".to_string(),
        }];

        let result = validate_structural_action_only_inputs(true, &selected_inputs);

        assert!(
            result.is_ok(),
            "schema-backed agents should keep accepting inputs"
        );
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
            "--input-text",
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
            "--input-text",
            "Return 4",
        ]);
        let preflight = cmd
            .subcommand_matches("preflight")
            .expect("preflight subcommand should parse");

        assert!(!super::run(preflight).await);
    }
}
