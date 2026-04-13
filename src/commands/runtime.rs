//! Shared interpreted runtime behavior for Cargo AI commands.
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
        format!("x Unknown AI server '{}'.", display_server),
        "Use `--server ollama` or `--server openai`.".to_string(),
        "Hint: Set `--server` explicitly or configure a default profile with a supported server."
            .to_string(),
        "Example: cargo ai run --config ./agent.json --server ollama --model mistral --input-text \"What is 2 + 2?\""
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

pub(crate) trait InvocationDefinition {
    fn named_inputs(&self) -> Vec<crate::Input>;
    fn runtime_var_specs(&self) -> Vec<crate::RuntimeVarSpec>;
    fn action_execution(&self) -> crate::ActionExecutionMode;
    fn has_output_schema_properties(&self) -> bool;
    fn json_schema_value(&self) -> serde_json::Value;
    fn actions(&self) -> Vec<crate::Action>;
    fn validate_provider_output(&self, raw: &str) -> Result<serde_json::Value, String>;
}

fn profile_selection_messages(
    kind: LoadedProfileKind,
    profile_name: &str,
    overrides: &[String],
) -> Vec<String> {
    let base_message = match kind {
        LoadedProfileKind::Explicit => format!("loaded profile: {}", profile_name),
        LoadedProfileKind::Default => format!("loaded profile: {} (default)", profile_name),
    };

    if overrides.is_empty() {
        vec![base_message]
    } else {
        vec![
            base_message,
            format!("applied overrides: {}", overrides.join(", ")),
        ]
    }
}

fn provider_display_name(provider: ProviderKind) -> &'static str {
    match provider {
        ProviderKind::Ollama => "ollama",
        ProviderKind::OpenAi => "openai",
    }
}

fn display_profile_name(profile_name: Option<&str>) -> &str {
    profile_name.unwrap_or("none")
}

fn display_model_name(model: &str) -> &str {
    if model.trim().is_empty() {
        "none"
    } else {
        model
    }
}

fn normalize_cli_issue(message: &str) -> String {
    message
        .trim()
        .strip_prefix("x ")
        .or_else(|| message.trim().strip_prefix("❌ "))
        .unwrap_or(message.trim())
        .to_string()
}

fn push_aligned_section(lines: &mut Vec<String>, title: &str, items: &[(&str, String)]) {
    if items.is_empty() {
        return;
    }

    lines.push(String::new());
    lines.push(title.to_string());

    let label_width = items
        .iter()
        .map(|(label, _)| label.len())
        .max()
        .unwrap_or(0);
    for (label, value) in items {
        lines.push(format!("  {label:<width$}  {value}", width = label_width));
    }
}

fn push_list_section(lines: &mut Vec<String>, title: &str, items: &[String]) {
    let rendered_items: Vec<String> = items
        .iter()
        .map(|item| normalize_cli_issue(item))
        .filter(|item| !item.trim().is_empty())
        .collect();

    if rendered_items.is_empty() {
        return;
    }

    lines.push(String::new());
    lines.push(title.to_string());
    for item in rendered_items {
        lines.push(format!("- {item}"));
    }
}

fn push_plain_section(lines: &mut Vec<String>, title: &str, items: &[String]) {
    let rendered_items: Vec<String> = items
        .iter()
        .map(|item| item.trim_end().to_string())
        .filter(|item| !item.trim().is_empty())
        .collect();

    if rendered_items.is_empty() {
        return;
    }

    lines.push(String::new());
    lines.push(title.to_string());
    lines.extend(rendered_items);
}

fn runtime_context_items(
    context: &super::runtime_actions::ActionProviderContext,
) -> Vec<(&'static str, String)> {
    let mut items = vec![
        (
            "Profile",
            display_profile_name(context.profile_name.as_deref()).to_string(),
        ),
        ("Auth", context.auth_mode.clone()),
        (
            "Server",
            provider_display_name(context.provider).to_string(),
        ),
        (
            "Model",
            display_model_name(context.model.as_str()).to_string(),
        ),
    ];

    let trimmed_url = context.url.trim();
    if !trimmed_url.is_empty() && trimmed_url != context.provider.default_url() {
        items.push(("URL", trimmed_url.to_string()));
    }

    items
}

fn render_runtime_failure_lines(
    summary: &str,
    context: Option<&super::runtime_actions::ActionProviderContext>,
    problems: &[String],
    detail_title: Option<&str>,
    detail_lines: &[String],
    next_steps: &[(&str, String)],
) -> Vec<String> {
    let mut lines = vec!["x Run failed".to_string(), summary.to_string()];

    if let Some(context) = context {
        let items = runtime_context_items(context);
        push_aligned_section(&mut lines, "Context", &items);
    }

    push_list_section(&mut lines, "Problem", problems);
    push_plain_section(&mut lines, detail_title.unwrap_or("Details"), detail_lines);
    let recovery_items = next_steps
        .iter()
        .map(|(label, value)| (*label, format_recovery_value(value)))
        .collect::<Vec<_>>();
    push_aligned_section(&mut lines, "Recovery", &recovery_items);

    lines
}

fn format_recovery_value(value: &str) -> String {
    if value.contains("cargo ai ") || value.contains("codex ") {
        format!("`{value}`")
    } else {
        value.to_string()
    }
}

fn print_runtime_failure(
    summary: &str,
    context: Option<&super::runtime_actions::ActionProviderContext>,
    problems: &[String],
    detail_title: Option<&str>,
    detail_lines: &[String],
    next_steps: &[(&str, String)],
) {
    for line in render_runtime_failure_lines(
        summary,
        context,
        problems,
        detail_title,
        detail_lines,
        next_steps,
    ) {
        eprintln!("{line}");
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

    if let Some(action_execution) = sub_m.get_one::<String>("action_execution") {
        overrides.push(format!("action_execution={action_execution}"));
    }

    if let Some(render_mode) = sub_m.get_one::<String>("render_mode") {
        overrides.push(format!("render_mode={render_mode}"));
    }

    if include_token_override {
        overrides.push("token=(explicit)".to_string());
    }

    overrides
}

fn resolved_action_execution_override_for_run(
    sub_m: &ArgMatches,
) -> Result<Option<crate::ActionExecutionMode>, String> {
    match sub_m
        .get_one::<String>("action_execution")
        .map(String::as_str)
    {
        None => Ok(None),
        Some("sequential") => Ok(Some(crate::ActionExecutionMode::Sequential)),
        Some(other) => Err(format!(
            "Unsupported --action-execution '{other}'. Expected sequential."
        )),
    }
}

fn effective_action_execution_for_run(
    action_execution_override: Option<crate::ActionExecutionMode>,
    default_action_execution: crate::ActionExecutionMode,
) -> crate::ActionExecutionMode {
    action_execution_override.unwrap_or(default_action_execution)
}

fn resolved_render_mode_for_run(
    sub_m: &ArgMatches,
) -> Result<super::runtime_actions::RequestedActionRenderMode, String> {
    match sub_m.get_one::<String>("render_mode").map(String::as_str) {
        None | Some("auto") => Ok(super::runtime_actions::RequestedActionRenderMode::Auto),
        Some("live") => Ok(super::runtime_actions::RequestedActionRenderMode::Live),
        Some("append-only") => Ok(super::runtime_actions::RequestedActionRenderMode::AppendOnly),
        Some(other) => Err(format!(
            "Unsupported --render-mode '{other}'. Expected auto, live, or append-only."
        )),
    }
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

fn resolved_invocation_auth_mode(
    provider: ProviderKind,
    selected_profile: Option<&SelectedProfile>,
    explicit_token_override: bool,
    use_openai_account_transport: bool,
) -> &'static str {
    match provider {
        ProviderKind::Ollama => "none",
        ProviderKind::OpenAi => {
            if explicit_token_override {
                return "api_key";
            }
            if let Some(profile) = selected_profile {
                return match profile.auth_mode {
                    ProfileAuthMode::None => "none",
                    ProfileAuthMode::ApiKey => "api_key",
                    ProfileAuthMode::OpenaiAccount => "chatgpt_account",
                };
            }
            if use_openai_account_transport {
                "chatgpt_account"
            } else {
                "none"
            }
        }
    }
}

fn runtime_input_overrides(sub_m: &ArgMatches) -> Vec<crate::Input> {
    let mut ordered = Vec::new();

    collect_flagged_inputs(sub_m, "input_text")
        .into_iter()
        .for_each(|(index, value)| {
            ordered.push((
                index,
                crate::Input {
                    name: None,
                    kind: crate::InputKind::Text,
                    value: Some(value),
                },
            ))
        });
    collect_flagged_inputs(sub_m, "input_url")
        .into_iter()
        .for_each(|(index, value)| {
            ordered.push((
                index,
                crate::Input {
                    name: None,
                    kind: crate::InputKind::Url,
                    value: Some(value),
                },
            ))
        });
    collect_flagged_inputs(sub_m, "input_image")
        .into_iter()
        .for_each(|(index, value)| {
            ordered.push((
                index,
                crate::Input {
                    name: None,
                    kind: crate::InputKind::Image,
                    value: Some(value),
                },
            ))
        });
    collect_flagged_inputs(sub_m, "input_file")
        .into_iter()
        .for_each(|(index, value)| {
            ordered.push((
                index,
                crate::Input {
                    name: None,
                    kind: crate::InputKind::File,
                    value: Some(value),
                },
            ))
        });

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

fn resolved_named_inputs_for_run(
    sub_m: &ArgMatches,
    baked_inputs: &[crate::Input],
) -> Result<Vec<crate::Input>, String> {
    let mut named_inputs = baked_inputs.to_vec();

    for raw_assignment in sub_m
        .get_many::<String>("input_override")
        .into_iter()
        .flatten()
    {
        let (name, raw_value) = parse_input_override_assignment(raw_assignment)?;
        let input = named_inputs
            .iter_mut()
            .find(|input| input.name.as_deref() == Some(name.as_str()))
            .ok_or_else(|| {
                format!(
                    "Named input override '{}' is not declared in top-level `inputs`.",
                    name
                )
            })?;

        input.value = Some(validate_input_override_value(
            input.kind, &raw_value, &name,
        )?);
    }

    Ok(named_inputs)
}

fn resolved_inputs_for_run(
    sub_m: &ArgMatches,
    named_inputs: &[crate::Input],
) -> Result<Vec<crate::Input>, String> {
    let runtime_inputs = runtime_input_overrides(sub_m);

    if runtime_inputs.is_empty() {
        if sub_m.get_one::<String>("input_mode").is_some() {
            return Err(
                "--input-mode requires at least one runtime input flag such as --input-text, --input-url, --input-image, or --input-file."
                    .to_string(),
            );
        }
        return Ok(named_inputs.to_vec());
    }

    let input_mode = runtime_input_mode(sub_m)?;
    Ok(match input_mode {
        RuntimeInputMode::Replace => runtime_inputs,
        RuntimeInputMode::Append => {
            let mut selected_inputs = named_inputs.to_vec();
            selected_inputs.extend(runtime_inputs);
            selected_inputs
        }
        RuntimeInputMode::Prepend => {
            let mut selected_inputs = runtime_inputs;
            selected_inputs.extend(named_inputs.to_vec());
            selected_inputs
        }
    })
}

fn parse_input_override_assignment(raw_assignment: &str) -> Result<(String, String), String> {
    let Some((name, raw_value)) = raw_assignment.split_once('=') else {
        return Err(format!(
            "Invalid --input-override assignment '{}'. Expected NAME=VALUE.",
            raw_assignment
        ));
    };

    if name.trim().is_empty() {
        return Err(format!(
            "Invalid --input-override assignment '{}'. Input name cannot be empty.",
            raw_assignment
        ));
    }
    if name != name.trim() || name.chars().any(char::is_whitespace) || name.contains('.') {
        return Err(format!(
            "Invalid --input-override assignment '{}'. Input names must be flat and cannot contain whitespace.",
            raw_assignment
        ));
    }

    Ok((name.to_string(), raw_value.to_string()))
}

fn validate_input_override_value(
    kind: crate::InputKind,
    raw_value: &str,
    name: &str,
) -> Result<String, String> {
    match kind {
        crate::InputKind::Text => Ok(raw_value.to_string()),
        crate::InputKind::Url => {
            if raw_value.starts_with("http://") || raw_value.starts_with("https://") {
                Ok(raw_value.to_string())
            } else {
                Err(format!(
                    "Named input override '{}' must be an absolute http(s) URL.",
                    name
                ))
            }
        }
        crate::InputKind::Image => Ok(raw_value.to_string()),
        crate::InputKind::File => {
            validate_runtime_file_extension(raw_value, name)?;
            Ok(raw_value.to_string())
        }
    }
}

fn validate_runtime_file_extension(path: &str, name: &str) -> Result<(), String> {
    let extension = std::path::Path::new(path)
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.to_ascii_lowercase());

    match extension.as_deref() {
        Some(
            "pdf" | "docx" | "csv" | "xla" | "xlb" | "xlc" | "xlm" | "xls" | "xlsx" | "xlt"
            | "xlw" | "tsv" | "iif" | "doc" | "dot" | "odt" | "rtf" | "pot" | "ppa" | "pps"
            | "ppt" | "pptx" | "pwz" | "wiz",
        ) => Ok(()),
        _ => Err(format!(
            "Named input override '{}' must use a supported file extension: pdf, docx, csv, xla, xlb, xlc, xlm, xls, xlsx, xlt, xlw, tsv, iif, doc, dot, odt, rtf, pot, ppa, pps, ppt, pptx, pwz, wiz.",
            name
        )),
    }
}

fn validate_structural_action_only_inputs(
    has_output_schema_properties: bool,
    named_inputs: &[crate::Input],
    selected_inputs: &[crate::Input],
) -> Result<(), String> {
    if has_output_schema_properties || selected_inputs.is_empty() {
        return Ok(());
    }

    let declared_named_inputs = named_inputs
        .iter()
        .filter_map(|input| input.name.as_deref())
        .collect::<std::collections::BTreeSet<_>>();

    if selected_inputs.iter().all(|input| {
        input
            .name
            .as_deref()
            .is_some_and(|name| declared_named_inputs.contains(name))
    }) {
        return Ok(());
    }

    Err(
        "This agent declares empty `agent_schema.properties`; anonymous runtime model-facing input flags such as --input-text, --input-url, --input-image, and --input-file are not allowed because there is no model pass to consume them. Use declared named top-level inputs and --input-override instead."
            .to_string(),
    )
}

fn empty_action_only_output() -> serde_json::Value {
    serde_json::json!({})
}

fn resolved_runtime_vars_for_run(
    sub_m: &ArgMatches,
    runtime_var_specs: &[crate::RuntimeVarSpec],
) -> Result<serde_json::Map<String, serde_json::Value>, String> {
    resolve_runtime_vars_from_specs(sub_m, runtime_var_specs)
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
    runtime_budget: super::runtime_actions::InvocationRuntimeBudget,
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
    runtime_budget: super::runtime_actions::InvocationRuntimeBudget,
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

pub(crate) async fn run_with_definition(
    sub_m: &ArgMatches,
    definition: &dyn InvocationDefinition,
) -> bool {
    let full_run_started_at = std::time::Instant::now();

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
    let runtime_budget = super::runtime_actions::configured_agent_action_runtime_budget(
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

    let has_explicit_token_override = explicit_token_override.is_some();
    if let Some((kind, profile_name)) = loaded_profile_message.as_ref() {
        for line in profile_selection_messages(
            *kind,
            profile_name,
            &cli_override_descriptions(
                sub_m,
                has_explicit_token_override && provider == ProviderKind::OpenAi,
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
                eprintln!("x {error}");
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

    let action_provider_context = super::runtime_actions::ActionProviderContext {
        provider,
        profile_name: selected_profile
            .as_ref()
            .map(|profile| profile.name.clone()),
        auth_mode: resolved_invocation_auth_mode(
            provider,
            selected_profile.as_ref(),
            has_explicit_token_override,
            use_openai_account_transport,
        )
        .to_string(),
        model: model.clone(),
        url: url.clone(),
        token: token.clone(),
        inference_timeout_in_sec,
    };

    let named_inputs_template = definition.named_inputs();
    let runtime_var_specs = definition.runtime_var_specs();
    let default_action_execution = definition.action_execution();
    let has_output_schema_properties = definition.has_output_schema_properties();
    let actions = definition.actions();

    let named_inputs = match resolved_named_inputs_for_run(sub_m, &named_inputs_template) {
        Ok(named_inputs) => named_inputs,
        Err(error) => {
            eprintln!("x {error}");
            return false;
        }
    };
    let selected_inputs = match resolved_inputs_for_run(sub_m, &named_inputs) {
        Ok(selected_inputs) => selected_inputs,
        Err(error) => {
            eprintln!("x {error}");
            return false;
        }
    };
    let runtime_vars = match resolved_runtime_vars_for_run(sub_m, &runtime_var_specs) {
        Ok(runtime_vars) => runtime_vars,
        Err(error) => {
            eprintln!("x {error}");
            return false;
        }
    };
    let action_execution_override = match resolved_action_execution_override_for_run(sub_m) {
        Ok(action_execution_override) => action_execution_override,
        Err(error) => {
            eprintln!("x {error}");
            return false;
        }
    };
    let requested_render_mode = match resolved_render_mode_for_run(sub_m) {
        Ok(requested_render_mode) => requested_render_mode,
        Err(error) => {
            eprintln!("x {error}");
            return false;
        }
    };
    let effective_action_execution =
        effective_action_execution_for_run(action_execution_override, default_action_execution);

    if let Err(error) = validate_structural_action_only_inputs(
        has_output_schema_properties,
        &named_inputs,
        &selected_inputs,
    ) {
        eprintln!("x {error}");
        return false;
    }

    if !has_output_schema_properties {
        let output = empty_action_only_output();
        println!("{}", action_provider_context.using_line());

        return match super::runtime_actions::apply_actions_with_data(
            &output,
            &actions,
            &runtime_vars,
            &named_inputs,
            effective_action_execution,
            action_execution_override,
            requested_render_mode,
            &action_provider_context,
            max_agent_depth,
            runtime_budget,
            full_run_started_at,
        )
        .await
        {
            Ok(()) => true,
            Err(error) => {
                print_runtime_failure(
                    "Action execution failed during run.",
                    Some(&action_provider_context),
                    &[error],
                    None,
                    &[],
                    &[],
                );
                false
            }
        };
    }

    if let Err(validation_issues) = validate_provider_request(provider, &model, &url, &token) {
        print_runtime_failure(
            "Provider request settings are incomplete or invalid.",
            Some(&action_provider_context),
            &validation_issues,
            None,
            &[],
            &[(
                "Fix settings",
                "Review server, model, URL, and auth inputs.".to_string(),
            )],
        );
        return false;
    }

    // End: Argument assignments

    let resolved_inputs = match crate::providers::resolve_provider_inputs(&selected_inputs).await {
        Ok(resolved_inputs) => resolved_inputs,
        Err(error) => {
            print_runtime_failure(
                "Runtime inputs could not be resolved.",
                Some(&action_provider_context),
                &[format!("Reason: {error}")],
                None,
                &[],
                &[(
                    "Fix inputs",
                    "Verify referenced files and URLs exist and are readable.".to_string(),
                )],
            );
            return false;
        }
    };

    if let Err(validation_issues) =
        validate_provider_content_parts(provider, &url, &resolved_inputs)
    {
        print_runtime_failure(
            "Resolved inputs are not valid for the provider request.",
            Some(&action_provider_context),
            &validation_issues,
            None,
            &[],
            &[(
                "Update inputs",
                "Adjust the selected input files, images, or URLs and retry.".to_string(),
            )],
        );
        return false;
    }
    println!("{}", action_provider_context.using_line());

    let static_context = "A question will be asked and you will need to return the answer in the specified JSON format.";

    let ai_cargo = crate::providers::AgentCargo::<serde_json::Value>::new(
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
                        "x {}",
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
                definition.json_schema_value(),
            ),
        )
        .await
        {
            Ok(Ok(r)) => response.push_str(&r),
            Ok(Err(error)) => {
                let details = provider_error_messages(&error);
                let summary = details
                    .first()
                    .map(|line| normalize_cli_issue(line))
                    .unwrap_or_else(|| "Issue communicating with the AI server.".to_string());
                print_runtime_failure(
                    summary.as_str(),
                    Some(&action_provider_context),
                    &[],
                    Some("Details"),
                    &details[1..],
                    &[(
                        "Retry request",
                        "Check connectivity or credentials, then retry the run.".to_string(),
                    )],
                );
                return false;
            }
            Err(_) => {
                print_runtime_failure(
                    "The provider did not return a response before the runtime budget expired.",
                    Some(&action_provider_context),
                    &[current_agent_runtime_timeout_message(
                        runtime_budget,
                        "while waiting for the model response",
                    )],
                    None,
                    &[],
                    &[(
                        "Reduce runtime",
                        "Shorten the request or increase the allowed runtime budget.".to_string(),
                    )],
                );
                return false;
            }
        }
    } else if provider == ProviderKind::OpenAi {
        let schema = definition.json_schema_value();
        let fmt = serde_json::json!({
        "type": "json_schema",
        "json_schema": {
            "name": "Output",
            "schema": schema,
            "strict": true
        }
        });

        // Send request to OpenAI and `await` the LLM response
        let remaining =
            match remaining_runtime_duration(runtime_budget, "before starting inference") {
                Ok(remaining) => remaining,
                Err(error) => {
                    eprintln!(
                        "x {}",
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
                let details = provider_error_messages(&error);
                let summary = details
                    .first()
                    .map(|line| normalize_cli_issue(line))
                    .unwrap_or_else(|| "Issue communicating with the AI server.".to_string());
                print_runtime_failure(
                    summary.as_str(),
                    Some(&action_provider_context),
                    &[],
                    Some("Details"),
                    &details[1..],
                    &[(
                        "Retry request",
                        "Check connectivity or credentials, then retry the run.".to_string(),
                    )],
                );
                return false;
            }
            Err(_) => {
                print_runtime_failure(
                    "The provider did not return a response before the runtime budget expired.",
                    Some(&action_provider_context),
                    &[current_agent_runtime_timeout_message(
                        runtime_budget,
                        "while waiting for the model response",
                    )],
                    None,
                    &[],
                    &[(
                        "Reduce runtime",
                        "Shorten the request or increase the allowed runtime budget.".to_string(),
                    )],
                );
                return false;
            }
        };
    }

    let output = match definition.validate_provider_output(response.as_str()) {
        Ok(output) => output,
        Err(problem) => {
            print_runtime_failure(
                "Provider output did not match the required JSON schema.",
                Some(&action_provider_context),
                &[problem],
                Some("Raw output"),
                &response
                    .lines()
                    .map(|line| line.to_string())
                    .collect::<Vec<_>>(),
                &[(
                    "Retry request",
                    "Retry the run after reviewing the provider output and selected inputs."
                        .to_string(),
                )],
            );
            return false;
        }
    };

    match super::runtime_actions::apply_actions_with_data(
        &output,
        &actions,
        &runtime_vars,
        &named_inputs,
        effective_action_execution,
        action_execution_override,
        requested_render_mode,
        &action_provider_context,
        max_agent_depth,
        runtime_budget,
        full_run_started_at,
    )
    .await
    {
        Ok(()) => true,
        Err(error) => {
            print_runtime_failure(
                "Action execution failed during run.",
                Some(&action_provider_context),
                &[error],
                None,
                &[],
                &[],
            );
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        cli_override_descriptions, effective_action_execution_for_run, profile_selection_messages,
        render_runtime_failure_lines, resolve_runtime_vars_from_specs,
        resolved_action_execution_override_for_run, resolved_render_mode_for_run,
        unknown_server_messages, validate_structural_action_only_inputs, LoadedProfileKind,
    };
    use crate::commands::runtime_actions::RequestedActionRenderMode;
    use crate::providers::ProviderKind;
    use clap::Command;
    use serde_json::json;

    fn input_debug_strings(inputs: &[crate::Input]) -> Vec<String> {
        inputs.iter().map(|input| format!("{input:?}")).collect()
    }

    fn resolved_named_inputs(runtime_m: &clap::ArgMatches) -> Vec<crate::Input> {
        super::resolved_named_inputs_for_run(runtime_m, &crate::inputs())
            .expect("named inputs should resolve")
    }

    fn matches(args: &[&str]) -> clap::ArgMatches {
        Command::new("cargo-ai")
            .subcommand(crate::args::runtime_common::runtime_command(
                "run",
                "Runtime test command",
            ))
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

    fn test_runtime_definition() -> crate::runtime_definition::RuntimeAgentDefinition {
        crate::runtime_definition::RuntimeAgentDefinition::from_str(
            r#"{
                "version": "2026-03-03.r1",
                "inputs": [{ "type": "text", "text": "Return an answer." }],
                "agent_schema": {
                    "type": "object",
                    "properties": {
                        "answer": { "type": "string" }
                    },
                    "required": ["answer"]
                },
                "actions": [
                    {
                        "name": "print",
                        "logic": { "==": [{ "var": "answer" }, "ok"] },
                        "run": [{ "kind": "exec", "program": "echo", "args": ["ok"] }]
                    }
                ]
            }"#,
        )
        .expect("test runtime definition should parse")
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
            .any(|line| line.contains("cargo ai run --config ./agent.json --server ollama")));
    }

    #[test]
    fn unknown_server_messages_handle_empty_value() {
        let messages = unknown_server_messages("");
        assert!(messages
            .iter()
            .any(|line| line.contains("Unknown AI server '(not set)'")));
    }

    #[test]
    fn profile_selection_messages_show_loaded_profile_and_overrides() {
        let messages = profile_selection_messages(
            LoadedProfileKind::Default,
            "my_open_ai",
            &["server=ollama".to_string(), "model=mistral".to_string()],
        );

        assert_eq!(messages[0], "loaded profile: my_open_ai (default)");
        assert_eq!(
            messages[1],
            "applied overrides: server=ollama, model=mistral"
        );
    }

    #[test]
    fn render_runtime_failure_lines_include_context_and_recovery() {
        let context = crate::commands::runtime_actions::ActionProviderContext {
            provider: ProviderKind::OpenAi,
            profile_name: Some("my_open_ai".to_string()),
            auth_mode: "chatgpt_account".to_string(),
            model: "gpt-5".to_string(),
            url: "https://api.openai.com/v1/responses".to_string(),
            token: "secret".to_string(),
            inference_timeout_in_sec: 60,
        };

        let lines = render_runtime_failure_lines(
            "Runtime inputs could not be resolved.",
            Some(&context),
            &[String::from("Reason: missing /tmp/demo.pdf")],
            None,
            &[],
            &[(
                "Fix inputs",
                "Verify referenced files and URLs exist and are readable.".to_string(),
            )],
        );
        let rendered = lines.join("\n");

        assert!(rendered.contains("x Run failed"));
        assert!(rendered.contains("Profile  my_open_ai"));
        assert!(rendered.contains("Auth     chatgpt_account"));
        assert!(rendered.contains("- Reason: missing /tmp/demo.pdf"));
        assert!(rendered.contains("\nRecovery\n"));
        assert!(rendered
            .contains("Fix inputs  Verify referenced files and URLs exist and are readable."));
    }

    #[test]
    fn render_runtime_failure_lines_backtick_command_recovery_values() {
        let lines = render_runtime_failure_lines(
            "Provider output did not match the required schema.",
            None,
            &[String::from("Reason: invalid JSON body")],
            None,
            &[],
            &[(
                "Retry",
                "cargo ai run --config ./agent.json --profile my_open_ai".to_string(),
            )],
        );
        let rendered = lines.join("\n");

        assert!(
            rendered.contains("Retry  `cargo ai run --config ./agent.json --profile my_open_ai`")
        );
    }

    #[test]
    fn cli_override_descriptions_capture_runtime_overrides() {
        let cmd = matches(&[
            "cargo-ai",
            "run",
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
            "--action-execution",
            "sequential",
            "--input-text",
            "Return 4",
        ]);
        let runtime_m = cmd
            .subcommand_matches("run")
            .expect("run subcommand should parse");

        let overrides = cli_override_descriptions(runtime_m, false);

        assert_eq!(
            overrides,
            vec![
                "server=ollama".to_string(),
                "model=mistral".to_string(),
                "inference_timeout_in_sec=90".to_string(),
                "max_agent_depth=3".to_string(),
                "max_runtime_in_sec=180".to_string(),
                "action_execution=sequential".to_string(),
            ]
        );
    }

    #[test]
    fn runtime_command_accepts_action_execution_override() {
        let cmd = matches(&[
            "cargo-ai",
            "run",
            "--action-execution",
            "sequential",
            "--input-text",
            "Return 4",
        ]);
        let runtime_m = cmd
            .subcommand_matches("run")
            .expect("run subcommand should parse");

        assert_eq!(
            runtime_m
                .get_one::<String>("action_execution")
                .map(String::as_str),
            Some("sequential")
        );
    }

    #[test]
    fn runtime_command_accepts_render_mode_override() {
        let cmd = matches(&[
            "cargo-ai",
            "run",
            "--render-mode",
            "append-only",
            "--input-text",
            "Return 4",
        ]);
        let runtime_m = cmd
            .subcommand_matches("run")
            .expect("run subcommand should parse");

        assert_eq!(
            runtime_m
                .get_one::<String>("render_mode")
                .map(String::as_str),
            Some("append-only")
        );
    }

    #[test]
    fn resolved_action_execution_override_for_run_reads_cli_override() {
        let cmd = matches(&[
            "cargo-ai",
            "run",
            "--action-execution",
            "sequential",
            "--input-text",
            "Return 4",
        ]);
        let runtime_m = cmd
            .subcommand_matches("run")
            .expect("run subcommand should parse");

        assert_eq!(
            resolved_action_execution_override_for_run(runtime_m)
                .expect("action execution override should resolve"),
            Some(crate::ActionExecutionMode::Sequential)
        );
    }

    #[test]
    fn effective_action_execution_for_run_prefers_runtime_override() {
        assert_eq!(
            effective_action_execution_for_run(
                Some(crate::ActionExecutionMode::Sequential),
                crate::ActionExecutionMode::Parallel,
            ),
            crate::ActionExecutionMode::Sequential
        );
    }

    #[test]
    fn resolved_render_mode_for_run_defaults_to_auto() {
        let cmd = matches(&["cargo-ai", "run", "--input-text", "Return 4"]);
        let runtime_m = cmd
            .subcommand_matches("run")
            .expect("run subcommand should parse");

        assert_eq!(
            resolved_render_mode_for_run(runtime_m).expect("render mode should resolve"),
            RequestedActionRenderMode::Auto
        );
    }

    #[test]
    fn resolved_render_mode_for_run_reads_cli_override() {
        let cmd = matches(&[
            "cargo-ai",
            "run",
            "--render-mode",
            "live",
            "--input-text",
            "Return 4",
        ]);
        let runtime_m = cmd
            .subcommand_matches("run")
            .expect("run subcommand should parse");

        assert_eq!(
            resolved_render_mode_for_run(runtime_m).expect("render mode should resolve"),
            RequestedActionRenderMode::Live
        );
    }

    #[test]
    fn runtime_command_accepts_max_agent_depth_override() {
        let cmd = matches(&[
            "cargo-ai",
            "run",
            "--max-agent-depth",
            "4",
            "--input-text",
            "Return 4",
        ]);
        let runtime_m = cmd
            .subcommand_matches("run")
            .expect("run subcommand should parse");

        assert_eq!(
            runtime_m.get_one::<u32>("max_agent_depth").copied(),
            Some(4)
        );
    }

    #[test]
    fn runtime_command_accepts_max_runtime_override() {
        let cmd = matches(&[
            "cargo-ai",
            "run",
            "--max-runtime-in-sec",
            "240",
            "--input-text",
            "Return 4",
        ]);
        let runtime_m = cmd
            .subcommand_matches("run")
            .expect("run subcommand should parse");

        assert_eq!(
            runtime_m.get_one::<u64>("max_runtime_in_sec").copied(),
            Some(240)
        );
    }

    #[test]
    fn runtime_command_accepts_legacy_timeout_alias() {
        let cmd = matches(&[
            "cargo-ai",
            "run",
            "--timeout_in_sec",
            "45",
            "--input-text",
            "Return 4",
        ]);
        let runtime_m = cmd
            .subcommand_matches("run")
            .expect("run subcommand should parse");

        assert_eq!(
            runtime_m
                .get_one::<u64>("inference_timeout_in_sec")
                .copied(),
            Some(45)
        );
    }

    #[test]
    fn runtime_input_overrides_preserve_file_order() {
        let cmd = matches(&[
            "cargo-ai",
            "run",
            "--input-text",
            "hello",
            "--input-file",
            "./report.pdf",
            "--input-url",
            "https://example.com",
        ]);
        let runtime_m = cmd
            .subcommand_matches("run")
            .expect("run subcommand should parse");

        let overrides = super::runtime_input_overrides(runtime_m);
        assert_eq!(overrides.len(), 3);
        assert_eq!(overrides[0].kind, crate::InputKind::Text);
        assert_eq!(overrides[0].value.as_deref(), Some("hello"));
        assert_eq!(overrides[1].kind, crate::InputKind::File);
        assert_eq!(overrides[1].value.as_deref(), Some("./report.pdf"));
        assert_eq!(overrides[2].kind, crate::InputKind::Url);
        assert_eq!(overrides[2].value.as_deref(), Some("https://example.com"));
    }

    #[test]
    fn parse_input_override_assignment_splits_on_first_equals() {
        let (name, value) = super::parse_input_override_assignment("menu_note=a=b=c")
            .expect("override assignment should parse");

        assert_eq!(name, "menu_note");
        assert_eq!(value, "a=b=c");
    }

    #[test]
    fn parse_input_override_assignment_rejects_invalid_name() {
        let error = super::parse_input_override_assignment("menu note=value")
            .expect_err("whitespace in input names should fail");

        assert!(error.contains("Input names must be flat"));
    }

    #[test]
    fn validate_input_override_value_applies_kind_specific_rules() {
        assert_eq!(
            super::validate_input_override_value(crate::InputKind::Text, "", "menu_note")
                .expect("empty text should be allowed"),
            ""
        );
        assert_eq!(
            super::validate_input_override_value(
                crate::InputKind::Url,
                "https://example.com/menu",
                "source_url"
            )
            .expect("valid urls should pass"),
            "https://example.com/menu"
        );
        assert_eq!(
            super::validate_input_override_value(
                crate::InputKind::Image,
                "./artifacts/menu.png",
                "menu_image"
            )
            .expect("valid image paths should pass"),
            "./artifacts/menu.png"
        );
        assert_eq!(
            super::validate_input_override_value(
                crate::InputKind::File,
                "./reports/menu.pdf",
                "source_doc"
            )
            .expect("valid file paths should pass"),
            "./reports/menu.pdf"
        );
    }

    #[test]
    fn validate_input_override_value_rejects_invalid_url() {
        let error = super::validate_input_override_value(
            crate::InputKind::Url,
            "ftp://example.com",
            "source_url",
        )
        .expect_err("non-http urls should fail");

        assert!(error.contains("must be an absolute http(s) URL"));
    }

    #[test]
    fn validate_input_override_value_rejects_unsupported_file_extension() {
        let error = super::validate_input_override_value(
            crate::InputKind::File,
            "./reports/menu.exe",
            "source_doc",
        )
        .expect_err("unsupported file extensions should fail");

        assert!(error.contains("supported file extension"));
    }

    #[test]
    fn resolved_inputs_for_run_defaults_to_replace_mode() {
        let cmd = matches(&[
            "cargo-ai",
            "run",
            "--input-text",
            "hello",
            "--input-file",
            "./report.pdf",
        ]);
        let runtime_m = cmd
            .subcommand_matches("run")
            .expect("run subcommand should parse");

        let named_inputs = resolved_named_inputs(runtime_m);
        let selected_inputs = super::resolved_inputs_for_run(runtime_m, &named_inputs)
            .expect("replace mode should resolve");

        assert_eq!(selected_inputs.len(), 2);
        assert_eq!(selected_inputs[0].kind, crate::InputKind::Text);
        assert_eq!(selected_inputs[0].value.as_deref(), Some("hello"));
        assert_eq!(selected_inputs[1].kind, crate::InputKind::File);
        assert_eq!(selected_inputs[1].value.as_deref(), Some("./report.pdf"));
    }

    #[test]
    fn resolved_inputs_for_run_explicit_append_keeps_baked_inputs_first() {
        let baked_inputs = crate::inputs();
        let baked_debug = input_debug_strings(&baked_inputs);
        let cmd = matches(&[
            "cargo-ai",
            "run",
            "--input-mode",
            "append",
            "--input-file",
            "./report.pdf",
            "--input-text",
            "hello",
        ]);
        let runtime_m = cmd
            .subcommand_matches("run")
            .expect("run subcommand should parse");

        let named_inputs = resolved_named_inputs(runtime_m);
        let selected_inputs = super::resolved_inputs_for_run(runtime_m, &named_inputs)
            .expect("append mode should resolve");

        assert_eq!(selected_inputs.len(), baked_inputs.len() + 2);
        assert_eq!(
            input_debug_strings(&selected_inputs[..baked_inputs.len()]),
            baked_debug
        );
        assert_eq!(
            selected_inputs[baked_inputs.len()].kind,
            crate::InputKind::File
        );
        assert_eq!(
            selected_inputs[baked_inputs.len()].value.as_deref(),
            Some("./report.pdf")
        );
        assert_eq!(
            selected_inputs[baked_inputs.len() + 1].kind,
            crate::InputKind::Text
        );
        assert_eq!(
            selected_inputs[baked_inputs.len() + 1].value.as_deref(),
            Some("hello")
        );
    }

    #[test]
    fn resolved_inputs_for_run_explicit_prepend_keeps_runtime_inputs_first() {
        let baked_inputs = crate::inputs();
        let baked_debug = input_debug_strings(&baked_inputs);
        let cmd = matches(&[
            "cargo-ai",
            "run",
            "--input-mode",
            "prepend",
            "--input-url",
            "https://example.com",
            "--input-image",
            "./image.png",
        ]);
        let runtime_m = cmd
            .subcommand_matches("run")
            .expect("run subcommand should parse");

        let named_inputs = resolved_named_inputs(runtime_m);
        let selected_inputs = super::resolved_inputs_for_run(runtime_m, &named_inputs)
            .expect("prepend mode should resolve");

        assert_eq!(selected_inputs.len(), baked_inputs.len() + 2);
        assert_eq!(selected_inputs[0].kind, crate::InputKind::Url);
        assert_eq!(
            selected_inputs[0].value.as_deref(),
            Some("https://example.com")
        );
        assert_eq!(selected_inputs[1].kind, crate::InputKind::Image);
        assert_eq!(selected_inputs[1].value.as_deref(), Some("./image.png"));
        assert_eq!(input_debug_strings(&selected_inputs[2..]), baked_debug);
    }

    #[test]
    fn resolved_inputs_for_run_rejects_input_mode_without_runtime_inputs() {
        let cmd = matches(&["cargo-ai", "run", "--input-mode", "append"]);
        let runtime_m = cmd
            .subcommand_matches("run")
            .expect("run subcommand should parse");

        let named_inputs = resolved_named_inputs(runtime_m);
        let error = super::resolved_inputs_for_run(runtime_m, &named_inputs)
            .expect_err("missing runtime inputs should fail");

        assert!(error.contains("--input-mode requires at least one runtime input flag"));
    }

    #[test]
    fn resolve_runtime_vars_from_specs_parses_declared_types_and_defaults() {
        let cmd = matches(&[
            "cargo-ai",
            "run",
            "--run-var",
            "subject=Quarterly Review",
            "--run-var",
            "generate_images=true",
            "--run-var",
            "retry_count=3",
            "--run-var",
            "score_threshold=0.85",
        ]);
        let runtime_m = cmd
            .subcommand_matches("run")
            .expect("run subcommand should parse");

        let runtime_vars = resolve_runtime_vars_from_specs(
            runtime_m,
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
        let cmd = matches(&["cargo-ai", "run"]);
        let runtime_m = cmd
            .subcommand_matches("run")
            .expect("run subcommand should parse");

        let runtime_vars = resolve_runtime_vars_from_specs(
            runtime_m,
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
            "run",
            "--run-var",
            "subject=alpha",
            "--run-var",
            "subject=beta",
        ]);
        let runtime_m = cmd
            .subcommand_matches("run")
            .expect("run subcommand should parse");

        let error = resolve_runtime_vars_from_specs(
            runtime_m,
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
        let cmd = matches(&["cargo-ai", "run", "--run-var", "subject=alpha"]);
        let runtime_m = cmd
            .subcommand_matches("run")
            .expect("run subcommand should parse");

        let error = resolve_runtime_vars_from_specs(runtime_m, &[])
            .expect_err("undeclared runtime vars should fail");

        assert!(error.contains("is not declared in runtime_vars"));
    }

    #[test]
    fn resolve_runtime_vars_from_specs_requires_missing_non_defaulted_vars() {
        let cmd = matches(&["cargo-ai", "run"]);
        let runtime_m = cmd
            .subcommand_matches("run")
            .expect("run subcommand should parse");

        let error = resolve_runtime_vars_from_specs(
            runtime_m,
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
        let cmd = matches(&["cargo-ai", "run", "--run-var", "generate_images=yes"]);
        let runtime_m = cmd
            .subcommand_matches("run")
            .expect("run subcommand should parse");

        let error = resolve_runtime_vars_from_specs(
            runtime_m,
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
        let cmd = matches(&["cargo-ai", "run", "--run-var", "retry_count="]);
        let runtime_m = cmd
            .subcommand_matches("run")
            .expect("run subcommand should parse");

        let error = resolve_runtime_vars_from_specs(
            runtime_m,
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
        let named_inputs = Vec::new();
        let selected_inputs = Vec::new();

        let result = validate_structural_action_only_inputs(false, &named_inputs, &selected_inputs);

        assert!(result.is_ok(), "empty input set should be allowed");
    }

    #[test]
    fn validate_structural_action_only_inputs_rejects_runtime_inputs() {
        let named_inputs = Vec::new();
        let selected_inputs = vec![crate::Input {
            name: None,
            kind: crate::InputKind::Text,
            value: Some("hello".to_string()),
        }];

        let error = validate_structural_action_only_inputs(false, &named_inputs, &selected_inputs)
            .expect_err("runtime inputs should be rejected");

        assert!(error.contains("empty `agent_schema.properties`"));
        assert!(error.contains("--input-text"));
    }

    #[test]
    fn validate_structural_action_only_inputs_allows_declared_named_inputs() {
        let named_inputs = vec![crate::Input {
            name: Some("menu_image".to_string()),
            kind: crate::InputKind::Image,
            value: Some("./artifacts/menu.png".to_string()),
        }];
        let selected_inputs = named_inputs.clone();

        let result = validate_structural_action_only_inputs(false, &named_inputs, &selected_inputs);

        assert!(
            result.is_ok(),
            "declared named inputs should be allowed in structural action-only mode"
        );
    }

    #[test]
    fn validate_structural_action_only_inputs_allows_schema_backed_agents() {
        let named_inputs = Vec::new();
        let selected_inputs = vec![crate::Input {
            name: None,
            kind: crate::InputKind::Text,
            value: Some("hello".to_string()),
        }];

        let result = validate_structural_action_only_inputs(true, &named_inputs, &selected_inputs);

        assert!(
            result.is_ok(),
            "schema-backed agents should keep accepting inputs"
        );
    }

    #[tokio::test]
    async fn run_fails_closed_on_unknown_server() {
        let definition = test_runtime_definition();
        let cmd = matches(&[
            "cargo-ai",
            "run",
            "--server",
            "wat",
            "--model",
            "mistral",
            "--input-text",
            "What is 2 + 2?",
        ]);
        let runtime_m = cmd
            .subcommand_matches("run")
            .expect("run subcommand should parse");

        assert!(!super::run_with_definition(runtime_m, &definition).await);
    }

    #[tokio::test]
    async fn run_fails_closed_on_missing_openai_token() {
        let definition = test_runtime_definition();
        let cmd = matches(&[
            "cargo-ai",
            "run",
            "--server",
            "openai",
            "--model",
            "gpt-4o-mini",
            "--token",
            "",
            "--input-text",
            "Return 4",
        ]);
        let runtime_m = cmd
            .subcommand_matches("run")
            .expect("run subcommand should parse");

        assert!(!super::run_with_definition(runtime_m, &definition).await);
    }
}
