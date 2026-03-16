//! Action execution helpers for preflight/test flows.
use crate::config::adder::set_account_tokens;
use crate::config::loader::{config_path, load_config};
use crate::credentials::store;
use crate::infra_api;
use jsonlogic::apply;
use std::path::{Component, Path};
use std::process::Stdio;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const INFRA_BASE_URL: &str = "https://api.cargo-ai.org";
const AGENT_ACTION_DEPTH_ENV: &str = "CARGO_AI_AGENT_ACTION_DEPTH";
const AGENT_ACTION_MAX_DEPTH_ENV: &str = "CARGO_AI_AGENT_ACTION_MAX_DEPTH";
const AGENT_ACTION_MAX_RUNTIME_SECS_ENV: &str = "CARGO_AI_AGENT_MAX_RUNTIME_SECS";
const AGENT_ACTION_RUNTIME_STARTED_AT_MS_ENV: &str = "CARGO_AI_AGENT_RUNTIME_STARTED_AT_MS";
const AGENT_ACTION_RUNTIME_DEADLINE_MS_ENV: &str = "CARGO_AI_AGENT_RUNTIME_DEADLINE_MS";
const DEFAULT_AGENT_ACTION_MAX_RUNTIME_SECS: u64 = 600;
const SUPPORTED_FILE_EXTENSIONS_MESSAGE: &str = "pdf, docx, csv, xla, xlb, xlc, xlm, xls, xlsx, xlt, xlw, tsv, iif, doc, dot, odt, rtf, pot, ppa, pps, ppt, pptx, pwz, wiz";

#[derive(Debug, Clone, Copy)]
pub(crate) struct InvocationRuntimeBudget {
    pub(crate) max_runtime_secs: u64,
    pub(crate) started_at_ms: u64,
    pub(crate) deadline_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StepExecutionOutcome {
    Completed,
    SoftFailureLogged,
    SuccessAlreadyPrinted,
}

#[derive(Debug, Clone)]
struct AccountAuth {
    access_token: String,
    refresh_token: Option<String>,
}

#[derive(Debug)]
enum RefreshAccessError {
    MissingRefreshToken,
    RequestFailed(String),
    MissingRefreshedToken(serde_json::Value),
}

/// Applies configured action rules to model output and executes matching steps.
pub(crate) async fn apply_actions(
    output: &crate::Output,
    actions: &[crate::Action],
    max_agent_depth: u32,
    runtime_budget: InvocationRuntimeBudget,
) -> Result<(), String> {
    // println!("DEBUG: Applying actions -> {:?}", actions);

    let data = match serde_json::to_value(output) {
        Ok(data) => data,
        Err(error) => {
            eprintln!("❌ Failed to serialize output for action evaluation: {error}");
            return Err(format!(
                "Failed to serialize output for action evaluation: {error}"
            ));
        }
    };
    let current_platform = current_action_platform();

    for action in actions {
        match apply(&action.logic, &data) {
            Ok(result) => {
                // println!("Action Loop: {:?}", action);
                if result.as_bool() == Some(true) {
                    let matching_steps = matching_run_steps(&action.run, current_platform);
                    if matching_steps.is_empty() {
                        eprintln!(
                            "⚠️ No run steps matched the current platform for action '{}' (current platform: {}).",
                            action.name,
                            current_platform.unwrap_or("unsupported")
                        );
                        continue;
                    }

                    print_action_start(&action.name);
                    let single_step_action = matching_steps.len() == 1;
                    let mut outcomes = Vec::with_capacity(matching_steps.len());
                    let mut action_data = data.clone();

                    for step in matching_steps {
                        if !should_run_step(step, &action_data, &action.name)? {
                            continue;
                        }

                        let step_result = if step.kind.eq_ignore_ascii_case("exec") {
                            run_exec_step(step, &action_data, &action.name, runtime_budget)
                                .await
                                .map(|captured_output| {
                                    (StepExecutionOutcome::Completed, captured_output)
                                })
                        } else if step.kind.eq_ignore_ascii_case("email_me") {
                            run_email_me_step(
                                step,
                                &action_data,
                                &action.name,
                                runtime_budget,
                                single_step_action,
                            )
                            .await
                            .map(|outcome| (outcome, None))
                        } else if step.kind.eq_ignore_ascii_case("agent") {
                            run_agent_step(
                                step,
                                &action_data,
                                &action.name,
                                max_agent_depth,
                                runtime_budget,
                            )
                            .await
                            .map(|outcome| (outcome, None))
                        } else {
                            eprintln!(
                                "⚠️ Skipping action '{}' with unsupported step kind '{}'.",
                                action.name, step.kind
                            );
                            outcomes.push(StepExecutionOutcome::SoftFailureLogged);
                            continue;
                        };

                        match step_result {
                            Ok((outcome, captured_output)) => {
                                if let Some((name, value)) = captured_output {
                                    insert_action_output_variable(
                                        &mut action_data,
                                        name.as_str(),
                                        value,
                                        action.name.as_str(),
                                    )?;
                                }
                                insert_step_status_variable(
                                    &mut action_data,
                                    step,
                                    "succeeded",
                                    action.name.as_str(),
                                )?;
                                outcomes.push(outcome);
                            }
                            Err(error) => {
                                insert_step_status_variable(
                                    &mut action_data,
                                    step,
                                    "failed",
                                    action.name.as_str(),
                                )?;
                                insert_step_error_variable(
                                    &mut action_data,
                                    step,
                                    error.as_str(),
                                    action.name.as_str(),
                                )?;

                                if matches!(step_failure_mode(step), crate::FailureMode::Continue) {
                                    println!("{error}");
                                    outcomes.push(StepExecutionOutcome::SoftFailureLogged);
                                } else {
                                    return Err(error);
                                }
                            }
                        }
                    }

                    if let Some(summary) = action_completion_summary(&outcomes) {
                        print_action_success(&action.name, summary);
                    }
                }
            }
            Err(error) => {
                println!(
                    "Failed to evaluate logic for action '{}': {}",
                    action.name, error
                );
            }
        }
    }

    if let Some(message) = root_run_completion_message() {
        println!("{message}");
    }

    Ok(())
}

async fn run_exec_step(
    step: &crate::RunStep,
    data: &serde_json::Value,
    action_name: &str,
    runtime_budget: InvocationRuntimeBudget,
) -> Result<Option<(String, String)>, String> {
    let program = step.program.as_deref().ok_or_else(|| {
        format!(
            "Action '{}' exec step is missing required `program`.",
            action_name
        )
    })?;

    let resolved_args = resolve_run_args(&step.args, data, action_name)
        .map_err(|error| format!("Action '{}': {error}", action_name))?;

    let remaining = remaining_runtime_duration(
        runtime_budget,
        &format!("before starting command '{}'", program),
    )
    .map_err(|context| {
        action_runtime_timeout_message(action_name, runtime_budget, context.as_str())
    })?;

    if let Some(output_variable) = step.output_variable.as_deref() {
        let child = tokio::process::Command::new(program)
            .args(&resolved_args)
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|error| format!("{action_name}: failed to execute command: {error}."))?;

        match tokio::time::timeout(remaining, child.wait_with_output()).await {
            Ok(Ok(output)) if output.status.success() => {
                let captured_output = String::from_utf8_lossy(&output.stdout)
                    .trim_end_matches(['\r', '\n'])
                    .to_string();
                println!(
                    "ℹ️ Action '{}' stored exec output in variable '{}'.",
                    action_name, output_variable
                );
                Ok(Some((output_variable.to_string(), captured_output)))
            }
            Ok(Ok(output)) => Err(format!(
                "Action '{}' exec step command '{}' exited with status {}.",
                action_name, program, output.status
            )),
            Ok(Err(error)) => Err(format!(
                "Action '{}' exec step failed while waiting for command '{}': {}",
                action_name, program, error
            )),
            Err(_) => Err(action_runtime_timeout_message(
                action_name,
                runtime_budget,
                &format!("while waiting for command '{}'", program),
            )),
        }
    } else {
        let mut child = tokio::process::Command::new(program)
            .args(&resolved_args)
            .spawn()
            .map_err(|error| format!("{action_name}: failed to execute command: {error}."))?;

        match tokio::time::timeout(remaining, child.wait()).await {
            Ok(Ok(status)) if status.success() => Ok(None),
            Ok(Ok(status)) => Err(format!(
                "Action '{}' exec step command '{}' exited with status {}.",
                action_name, program, status
            )),
            Ok(Err(error)) => Err(format!(
                "Action '{}' exec step failed while waiting for command '{}': {}",
                action_name, program, error
            )),
            Err(_) => {
                let _ = child.kill().await;
                Err(action_runtime_timeout_message(
                    action_name,
                    runtime_budget,
                    &format!("while waiting for command '{}'", program),
                ))
            }
        }
    }
}

fn insert_action_output_variable(
    data: &mut serde_json::Value,
    name: &str,
    value: String,
    action_name: &str,
) -> Result<(), String> {
    insert_action_string_variable(data, name, value, action_name)
}

fn insert_action_string_variable(
    data: &mut serde_json::Value,
    name: &str,
    value: String,
    action_name: &str,
) -> Result<(), String> {
    let Some(object) = data.as_object_mut() else {
        return Err(format!(
            "Action '{}' could not store captured variable '{}' because the action data context is not an object.",
            action_name, name
        ));
    };

    object.insert(name.to_string(), serde_json::Value::String(value));
    Ok(())
}

fn insert_step_status_variable(
    data: &mut serde_json::Value,
    step: &crate::RunStep,
    status: &str,
    action_name: &str,
) -> Result<(), String> {
    let Some(name) = step.status_variable.as_deref() else {
        return Ok(());
    };

    insert_action_string_variable(data, name, status.to_string(), action_name)
}

fn insert_step_error_variable(
    data: &mut serde_json::Value,
    step: &crate::RunStep,
    error: &str,
    action_name: &str,
) -> Result<(), String> {
    let Some(name) = step.error_variable.as_deref() else {
        return Ok(());
    };

    insert_action_string_variable(data, name, error.to_string(), action_name)
}

fn step_failure_mode(step: &crate::RunStep) -> crate::FailureMode {
    step.failure_mode
        .clone()
        .unwrap_or(crate::FailureMode::Stop)
}

fn should_run_step(
    step: &crate::RunStep,
    data: &serde_json::Value,
    action_name: &str,
) -> Result<bool, String> {
    let Some(condition) = step.when.as_ref() else {
        return Ok(true);
    };

    apply(condition, data)
        .map(|result| result.as_bool() == Some(true))
        .map_err(|error| {
            format!(
                "Action '{}' failed to evaluate step `when` for kind '{}': {}",
                action_name, step.kind, error
            )
        })
}

async fn run_email_me_step(
    step: &crate::RunStep,
    data: &serde_json::Value,
    action_name: &str,
    runtime_budget: InvocationRuntimeBudget,
    single_step_action: bool,
) -> Result<StepExecutionOutcome, String> {
    let subject_parts = step.subject.as_deref().ok_or_else(|| {
        format!(
            "Action '{}' email_me step is missing required `subject`.",
            action_name
        )
    })?;
    let text_parts = step.text.as_deref().ok_or_else(|| {
        format!(
            "Action '{}' email_me step is missing required `text`.",
            action_name
        )
    })?;

    let subject = resolve_string_parts(subject_parts, data, action_name, "subject")?;
    let text = resolve_string_parts(text_parts, data, action_name, "text")?;

    let remaining =
        remaining_runtime_duration(runtime_budget, "before sending email").map_err(|context| {
            action_runtime_timeout_message(action_name, runtime_budget, context.as_str())
        })?;

    let response = tokio::time::timeout(remaining, async {
        let auth = load_account_auth()?;
        let access_token_owned = auth.access_token;
        let refresh_token = auth.refresh_token;

        let mut response = infra_api::account::send_mail::send_test_mail(
            INFRA_BASE_URL,
            access_token_owned.as_str(),
            subject.as_str(),
            text.as_str(),
        )
        .await
        .map_err(|error| format!("Request failed: {error:?}"))?;

        let is_expired_error = response
            .get("type")
            .and_then(|v| v.as_str())
            .map(|t| t == "access_token_expired")
            .unwrap_or(false);

        if is_expired_error {
            response = match refresh_access_token_for_retry(
                access_token_owned.as_str(),
                refresh_token.as_deref(),
            )
            .await
            {
                Err(RefreshAccessError::MissingRefreshToken) => {
                    return Err(
                        "Access token expired, and no refresh token exists in credential store. Run `cargo ai account status` or re-confirm account."
                            .to_string(),
                    );
                }
                Err(RefreshAccessError::RequestFailed(error)) => {
                    return Err(format!("Request failed while refreshing session: {error}"));
                }
                Err(RefreshAccessError::MissingRefreshedToken(refresh_response)) => {
                    return Err(
                        format_backend_error_message(&refresh_response).unwrap_or_else(|| {
                            "Session refresh did not return a new access token. Cannot retry email_me action."
                                .to_string()
                        }),
                    );
                }
                Ok((retry_access_token, refreshed_expires_in)) => {
                    if let Some(rt) = refresh_token.as_deref() {
                        persist_refreshed_access_token(
                            retry_access_token.as_str(),
                            rt,
                            refreshed_expires_in,
                        );
                    }

                    infra_api::account::send_mail::send_test_mail(
                        INFRA_BASE_URL,
                        retry_access_token.as_str(),
                        subject.as_str(),
                        text.as_str(),
                    )
                    .await
                    .map_err(|error| format!("Request failed after session refresh: {error:?}"))?
                }
            };
        }

        let succeeded = response
            .get("status")
            .and_then(|v| v.as_str())
            .map(|status| status.eq_ignore_ascii_case("success"))
            .unwrap_or(false);

        if succeeded {
            Ok(response)
        } else {
            Err(format_backend_error_message(&response).unwrap_or_else(|| {
                format!(
                    "Action '{}' email_me request failed.\n{}",
                    action_name,
                    pretty_backend_json(&response)
                )
            }))
        }
    })
    .await;

    let response = match response {
        Ok(Ok(response)) => response,
        Ok(Err(error)) => return Err(error),
        Err(_) => {
            return Err(action_runtime_timeout_message(
                action_name,
                runtime_budget,
                "while sending email",
            ));
        }
    };

    if single_step_action {
        print_action_success(action_name, "email sent");
    }
    render_backend_ui_or_json(&response);
    Ok(if single_step_action {
        StepExecutionOutcome::SuccessAlreadyPrinted
    } else {
        StepExecutionOutcome::Completed
    })
}

async fn run_agent_step(
    step: &crate::RunStep,
    data: &serde_json::Value,
    action_name: &str,
    max_agent_depth: u32,
    runtime_budget: InvocationRuntimeBudget,
) -> Result<StepExecutionOutcome, String> {
    let agent = step.agent.as_deref().ok_or_else(|| {
        format!(
            "Action '{}' agent step is missing required `agent`.",
            action_name
        )
    })?;

    let current_depth = current_agent_action_depth();
    validate_agent_action_depth(current_depth, max_agent_depth, action_name)?;

    validate_agent_step_target(agent, action_name)?;
    let agent_path = Path::new(agent);
    if !agent_path.exists() {
        return Err(format!(
            "Action '{}' agent step target '{}' was not found relative to the current working directory.",
            action_name, agent
        ));
    }

    let mut command = tokio::process::Command::new(agent_path);
    let (child_args, resolution_notes) =
        child_input_args(step.input_mode, step.inputs.as_deref(), data, action_name)?;
    for note in resolution_notes {
        println!("ℹ️ {note}");
    }
    for argument in child_args {
        command.arg(argument);
    }
    command.env(AGENT_ACTION_DEPTH_ENV, (current_depth + 1).to_string());
    command.env(AGENT_ACTION_MAX_DEPTH_ENV, max_agent_depth.to_string());
    command.env(
        AGENT_ACTION_MAX_RUNTIME_SECS_ENV,
        runtime_budget.max_runtime_secs.to_string(),
    );
    command.env(
        AGENT_ACTION_RUNTIME_STARTED_AT_MS_ENV,
        runtime_budget.started_at_ms.to_string(),
    );
    command.env(
        AGENT_ACTION_RUNTIME_DEADLINE_MS_ENV,
        runtime_budget.deadline_ms.to_string(),
    );

    let remaining = remaining_runtime_duration(
        runtime_budget,
        &format!("before starting child agent '{}'", agent),
    )
    .map_err(|context| {
        action_runtime_timeout_message(action_name, runtime_budget, context.as_str())
    })?;

    let mut child = command.spawn().map_err(|error| {
        format!(
            "Action '{}' failed to start child agent '{}': {}",
            action_name, agent, error
        )
    })?;

    match tokio::time::timeout(remaining, child.wait()).await {
        Ok(Ok(status)) if status.success() => Ok(StepExecutionOutcome::Completed),
        Ok(Ok(status)) => Err(format!(
            "Action '{}' child agent '{}' exited with status {} at depth {}.",
            action_name,
            agent,
            status,
            current_depth + 1
        )),
        Ok(Err(error)) => Err(format!(
            "Action '{}' failed while waiting for child agent '{}' at depth {}: {}",
            action_name,
            agent,
            current_depth + 1,
            error
        )),
        Err(_) => {
            let _ = child.kill().await;
            Err(action_runtime_timeout_message(
                action_name,
                runtime_budget,
                &format!(
                    "while waiting for child agent '{}' at depth {}",
                    agent,
                    current_depth + 1
                ),
            ))
        }
    }
}

fn action_completion_summary(outcomes: &[StepExecutionOutcome]) -> Option<&'static str> {
    if outcomes.is_empty()
        || outcomes.iter().any(|outcome| {
            matches!(
                outcome,
                StepExecutionOutcome::SoftFailureLogged
                    | StepExecutionOutcome::SuccessAlreadyPrinted
            )
        })
    {
        None
    } else {
        Some("completed")
    }
}

fn run_completion_message_for_depth(depth: u32) -> Option<&'static str> {
    if depth == 0 {
        Some("✅ Run complete.")
    } else {
        None
    }
}

fn root_run_completion_message() -> Option<&'static str> {
    run_completion_message_for_depth(current_agent_action_depth())
}

fn print_action_start(action_name: &str) {
    println!("Running action: {}", action_name);
}

fn print_action_success(action_name: &str, summary: &str) {
    println!("{action_name}: {summary}.");
}

fn render_backend_ui_or_json(response: &serde_json::Value) {
    if let Some(message) = format_backend_ui_message(response, true) {
        println!("{message}");
    } else {
        println!("{}", pretty_backend_json(response));
    }
}

fn format_backend_error_message(response: &serde_json::Value) -> Option<String> {
    format_backend_ui_message(response, false)
}

fn format_backend_ui_message(
    response: &serde_json::Value,
    include_kind_prefix: bool,
) -> Option<String> {
    let ui = response.get("ui")?;
    if ui.get("schema").and_then(|value| value.as_str()) != Some("1.0") {
        return None;
    }

    let kind = ui
        .get("kind")
        .and_then(|value| value.as_str())
        .unwrap_or("info");
    let title = ui
        .get("title")
        .and_then(|value| value.as_str())
        .unwrap_or("Status");
    let summary = ui
        .get("summary")
        .and_then(|value| value.as_str())
        .unwrap_or("Status response received.");

    let mut lines = Vec::new();
    if include_kind_prefix {
        let kind_prefix = match kind {
            "success" => "✅",
            "error" => "⚠️",
            "failure" => "❌",
            _ => "ℹ️",
        };
        lines.push(format!("{kind_prefix} {title}"));
    } else {
        lines.push(title.to_string());
    }
    lines.push(summary.to_string());

    if let Some(variant) = ui.get("variant").and_then(|value| value.as_str()) {
        if !variant.trim().is_empty() {
            lines.push(format!("Variant: {variant}"));
        }
    }

    if let Some(sections) = ui.get("sections").and_then(|value| value.as_array()) {
        for section in sections {
            append_backend_section_lines(section, &mut lines);
        }
    }

    if let Some(actions) = ui.get("actions").and_then(|value| value.as_array()) {
        let action_lines = actions
            .iter()
            .filter_map(|action| {
                let label = action
                    .get("label")
                    .and_then(|value| value.as_str())
                    .unwrap_or("");
                let command = action
                    .get("command")
                    .and_then(|value| value.as_str())
                    .unwrap_or("");

                if label.is_empty() && command.is_empty() {
                    None
                } else if !label.is_empty() && !command.is_empty() {
                    Some(format!("- {}: {}", label, command))
                } else if !label.is_empty() {
                    Some(format!("- {}", label))
                } else {
                    Some(format!("- {}", command))
                }
            })
            .collect::<Vec<_>>();

        if !action_lines.is_empty() {
            lines.push(String::new());
            lines.push("Actions:".to_string());
            lines.extend(action_lines);
        }
    }

    if let Some(next_steps) = ui.get("next_steps").and_then(|value| value.as_array()) {
        let step_lines = next_steps
            .iter()
            .filter_map(|step| step.as_str())
            .map(str::trim)
            .filter(|step| !step.is_empty())
            .map(|step| format!("- {}", step))
            .collect::<Vec<_>>();

        if !step_lines.is_empty() {
            lines.push(String::new());
            lines.push("Next steps:".to_string());
            lines.extend(step_lines);
        }
    }

    Some(lines.join("\n"))
}

fn append_backend_section_lines(section: &serde_json::Value, lines: &mut Vec<String>) {
    let section_type = section
        .get("type")
        .and_then(|value| value.as_str())
        .unwrap_or("");
    let title = section
        .get("title")
        .and_then(|value| value.as_str())
        .unwrap_or("");

    if !title.is_empty() {
        lines.push(String::new());
        lines.push(format!("{title}:"));
    }

    match section_type {
        "kv" => {
            if let Some(items) = section.get("items").and_then(|value| value.as_array()) {
                for item in items {
                    let label = item
                        .get("label")
                        .and_then(|value| value.as_str())
                        .unwrap_or("");
                    let value = item
                        .get("value")
                        .map(backend_ui_value_to_string)
                        .unwrap_or_default();

                    if label.is_empty() && value.is_empty() {
                        continue;
                    }

                    if label.is_empty() {
                        lines.push(format!("- {}", value));
                    } else {
                        lines.push(format!("- {}: {}", label, value));
                    }
                }
            }
        }
        "list" => {
            if let Some(items) = section.get("items").and_then(|value| value.as_array()) {
                for item in items {
                    let value = backend_ui_value_to_string(item);
                    if !value.is_empty() {
                        lines.push(format!("- {}", value));
                    }
                }
            }
        }
        "notice" => {
            if let Some(message) = section.get("message").and_then(|value| value.as_str()) {
                if !message.trim().is_empty() {
                    lines.push(message.to_string());
                }
            }
        }
        "json" => {
            if let Some(data) = section.get("data") {
                match serde_json::to_string_pretty(data) {
                    Ok(pretty) => lines.extend(pretty.lines().map(str::to_string)),
                    Err(_) => lines.push(backend_ui_value_to_string(data)),
                }
            }
        }
        _ => {}
    }
}

fn backend_ui_value_to_string(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Null => "null".to_string(),
        serde_json::Value::Bool(boolean) => boolean.to_string(),
        serde_json::Value::Number(number) => number.to_string(),
        serde_json::Value::String(text) => text.to_string(),
        serde_json::Value::Array(_) | serde_json::Value::Object(_) => {
            serde_json::to_string(value).unwrap_or_default()
        }
    }
}

fn pretty_backend_json(response: &serde_json::Value) -> String {
    serde_json::to_string_pretty(response).unwrap_or_else(|_| format!("{response:?}"))
}

fn load_account_auth() -> Result<AccountAuth, String> {
    let cfg = load_config().ok_or_else(|| {
        format!(
            "❌ No local config file found at '{}'. Run `cargo ai account register <email>` on this machine, or copy your config from another machine.",
            config_path().display()
        )
    })?;

    let acct = cfg.account.as_ref().ok_or_else(|| {
        "❌ No account found in config. You must confirm your account first.".to_string()
    })?;

    if let Some(account_tokens) = store::load_account_tokens()
        .map_err(|error| format!("❌ Failed to load account credentials: {error}"))?
    {
        return Ok(AccountAuth {
            access_token: account_tokens.access_token,
            refresh_token: account_tokens.refresh_token,
        });
    }

    let access_token = acct.access_token.as_ref().cloned().ok_or_else(|| {
        "❌ No access token found in credentials store or legacy config. Run `cargo ai account confirm <code>` first."
            .to_string()
    })?;

    Ok(AccountAuth {
        access_token,
        refresh_token: acct.refresh_token.clone(),
    })
}

async fn refresh_access_token_for_retry(
    access_token: &str,
    refresh_token: Option<&str>,
) -> Result<(String, Option<i32>), RefreshAccessError> {
    let rt = refresh_token.ok_or(RefreshAccessError::MissingRefreshToken)?;

    let refresh_response =
        infra_api::account::status::fetch_status(INFRA_BASE_URL, access_token, Some(rt))
            .await
            .map_err(|error| RefreshAccessError::RequestFailed(format!("{error:?}")))?;

    let refreshed_access_token = refresh_response
        .get("session")
        .and_then(|session| session.get("access_token"))
        .and_then(|value| value.as_str())
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string());

    let refreshed_expires_in = refresh_response
        .get("session")
        .and_then(|session| session.get("expires_in_seconds"))
        .and_then(|value| value.as_i64())
        .and_then(|value| i32::try_from(value).ok());

    match refreshed_access_token {
        Some(token) => Ok((token, refreshed_expires_in)),
        None => Err(RefreshAccessError::MissingRefreshedToken(refresh_response)),
    }
}

fn persist_refreshed_access_token(
    refreshed_access_token: &str,
    refresh_token: &str,
    refreshed_expires_in: Option<i32>,
) {
    if let Some(expires_in) = refreshed_expires_in {
        if let Err(error) = set_account_tokens(
            refreshed_access_token.to_string(),
            refresh_token.to_string(),
            expires_in,
        ) {
            eprintln!("⚠️ Failed to update account tokens in credential store: {error}");
        }
    }
}

fn current_time_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

fn new_runtime_budget(max_runtime_secs: u64) -> InvocationRuntimeBudget {
    let started_at_ms = current_time_millis();
    InvocationRuntimeBudget {
        max_runtime_secs,
        started_at_ms,
        deadline_ms: started_at_ms.saturating_add(max_runtime_secs.saturating_mul(1000)),
    }
}

fn inherited_agent_action_runtime_budget() -> Option<InvocationRuntimeBudget> {
    let max_runtime_secs = std::env::var(AGENT_ACTION_MAX_RUNTIME_SECS_ENV)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())?;
    let started_at_ms = std::env::var(AGENT_ACTION_RUNTIME_STARTED_AT_MS_ENV)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())?;
    let deadline_ms = std::env::var(AGENT_ACTION_RUNTIME_DEADLINE_MS_ENV)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())?;

    Some(InvocationRuntimeBudget {
        max_runtime_secs,
        started_at_ms,
        deadline_ms,
    })
}

pub(crate) fn configured_agent_action_runtime_budget(
    cli_override: Option<u64>,
) -> InvocationRuntimeBudget {
    cli_override
        .map(new_runtime_budget)
        .or_else(inherited_agent_action_runtime_budget)
        .unwrap_or_else(|| new_runtime_budget(DEFAULT_AGENT_ACTION_MAX_RUNTIME_SECS))
}

fn remaining_runtime_duration(
    runtime_budget: InvocationRuntimeBudget,
    exhausted_context: &str,
) -> Result<Duration, String> {
    let now = current_time_millis();
    if now >= runtime_budget.deadline_ms {
        return Err(exhausted_context.to_string());
    }

    Ok(Duration::from_millis(
        runtime_budget.deadline_ms.saturating_sub(now),
    ))
}

fn elapsed_runtime_secs(runtime_budget: InvocationRuntimeBudget) -> u64 {
    current_time_millis()
        .saturating_sub(runtime_budget.started_at_ms)
        .div_ceil(1000)
}

fn action_runtime_timeout_message(
    action_name: &str,
    runtime_budget: InvocationRuntimeBudget,
    context: &str,
) -> String {
    format!(
        "Action '{}' exceeded max-runtime-in-sec {} after {} seconds {}.",
        action_name,
        runtime_budget.max_runtime_secs,
        elapsed_runtime_secs(runtime_budget),
        context
    )
}

fn validate_agent_step_target(agent: &str, action_name: &str) -> Result<(), String> {
    let agent_path = Path::new(agent);
    if agent.trim().is_empty() {
        return Err(format!(
            "Action '{}' agent step target '{}' must use explicit same-level './childagent' form.",
            action_name, agent
        ));
    }

    if agent_path.is_absolute() {
        return Err(format!(
            "Action '{}' agent step target '{}' must use explicit same-level './childagent' form; absolute paths are not allowed.",
            action_name, agent
        ));
    }

    if agent_path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(format!(
            "Action '{}' agent step target '{}' must use explicit same-level './childagent' form; parent traversal (`..`) is not allowed.",
            action_name, agent
        ));
    }

    if !agent.starts_with("./") {
        let message = if contains_explicit_path_separator(agent) {
            "must use explicit same-level './childagent' form; nested child-agent paths are not allowed."
        } else {
            "must use explicit same-level './childagent' form; bare child-agent names are not allowed."
        };
        return Err(format!(
            "Action '{}' agent step target '{}' {}",
            action_name, agent, message
        ));
    }

    let sibling = &agent[2..];
    if sibling.is_empty() || !is_single_normal_path_component(sibling) {
        return Err(format!(
            "Action '{}' agent step target '{}' must stay at the same level; nested child-agent paths such as './agents/childagent' are not allowed.",
            action_name, agent
        ));
    }

    Ok(())
}

fn contains_explicit_path_separator(path: &str) -> bool {
    path.contains('/') || path.contains('\\')
}

fn is_single_normal_path_component(path: &str) -> bool {
    let mut components = Path::new(path).components();
    matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none()
}

fn validate_agent_action_depth(
    current_depth: u32,
    max_agent_depth: u32,
    action_name: &str,
) -> Result<(), String> {
    if current_depth >= max_agent_depth {
        return Err(format!(
            "Action '{}' cannot invoke another agent because current depth {} has reached max-agent-depth {}.",
            action_name, current_depth, max_agent_depth
        ));
    }

    Ok(())
}

fn current_action_platform() -> Option<&'static str> {
    if cfg!(target_os = "macos") {
        Some("macos")
    } else if cfg!(target_os = "linux") {
        Some("linux")
    } else if cfg!(target_os = "windows") {
        Some("windows")
    } else {
        None
    }
}

fn matching_run_steps<'a>(
    run_steps: &'a [crate::RunStep],
    current_platform: Option<&str>,
) -> Vec<&'a crate::RunStep> {
    run_steps
        .iter()
        .filter(|step| step_matches_platform(step.platforms.as_deref(), current_platform))
        .collect()
}

fn child_input_args(
    input_mode: Option<crate::ActionInputMode>,
    inputs: Option<&[crate::ActionInput]>,
    data: &serde_json::Value,
    action_name: &str,
) -> Result<(Vec<String>, Vec<String>), String> {
    let mut args = Vec::new();
    let mut notes = Vec::new();

    if let Some(input_mode) = input_mode {
        if inputs.is_none() {
            return Err(format!(
                "Action '{}' child-agent `input_mode` requires `inputs`.",
                action_name
            ));
        }

        args.push("--input-mode".to_string());
        args.push(
            match input_mode {
                crate::ActionInputMode::Replace => "replace",
                crate::ActionInputMode::Append => "append",
                crate::ActionInputMode::Prepend => "prepend",
            }
            .to_string(),
        );
    }

    if let Some(inputs) = inputs {
        for (index, input) in inputs.iter().enumerate() {
            match input {
                crate::ActionInput::Text { text } => {
                    let resolved = resolve_string_parts(
                        text,
                        data,
                        action_name,
                        &format!("child-agent text input {}", index + 1),
                    )?;
                    args.push("--input-text".to_string());
                    args.push(resolved);
                    if child_input_uses_dynamic_parts(text) {
                        notes.push(format!(
                            "Action '{}' resolved dynamic child-agent text input {}.",
                            action_name,
                            index + 1
                        ));
                    }
                }
                crate::ActionInput::Url { url } => {
                    let resolved = resolve_string_parts(
                        url,
                        data,
                        action_name,
                        &format!("child-agent url input {}", index + 1),
                    )?;
                    validate_child_input_url(&resolved, action_name, index + 1)?;
                    args.push("--input-url".to_string());
                    args.push(resolved.clone());
                    if child_input_uses_dynamic_parts(url) {
                        notes.push(format!(
                            "Action '{}' resolved dynamic child-agent url input {} -> {}.",
                            action_name,
                            index + 1,
                            resolved
                        ));
                    }
                }
                crate::ActionInput::Image { path } => {
                    let resolved = resolve_string_parts(
                        path,
                        data,
                        action_name,
                        &format!("child-agent image path input {}", index + 1),
                    )?;
                    validate_child_input_path(&resolved, action_name, index + 1, "image")?;
                    args.push("--input-image".to_string());
                    args.push(resolved.clone());
                    if child_input_uses_dynamic_parts(path) {
                        notes.push(format!(
                            "Action '{}' resolved dynamic child-agent image path input {} -> {}.",
                            action_name,
                            index + 1,
                            resolved
                        ));
                    }
                }
                crate::ActionInput::File { path } => {
                    let resolved = resolve_string_parts(
                        path,
                        data,
                        action_name,
                        &format!("child-agent file path input {}", index + 1),
                    )?;
                    validate_child_input_path(&resolved, action_name, index + 1, "file")?;
                    validate_child_file_extension(&resolved, action_name, index + 1)?;
                    args.push("--input-file".to_string());
                    args.push(resolved.clone());
                    if child_input_uses_dynamic_parts(path) {
                        notes.push(format!(
                            "Action '{}' resolved dynamic child-agent file path input {} -> {}.",
                            action_name,
                            index + 1,
                            resolved
                        ));
                    }
                }
            }
        }
    }

    Ok((args, notes))
}

fn child_input_uses_dynamic_parts(parts: &[crate::RunArg]) -> bool {
    parts
        .iter()
        .any(|part| matches!(part, crate::RunArg::Variable(_)))
}

fn validate_child_input_url(
    url: &str,
    action_name: &str,
    input_index: usize,
) -> Result<(), String> {
    if url.starts_with("http://") || url.starts_with("https://") {
        Ok(())
    } else {
        Err(format!(
            "Action '{}' child-agent url input {} must resolve to an http(s) URL.",
            action_name, input_index
        ))
    }
}

fn validate_child_input_path(
    path: &str,
    action_name: &str,
    input_index: usize,
    input_kind: &str,
) -> Result<(), String> {
    if path.trim().is_empty() {
        return Err(format!(
            "Action '{}' child-agent {} input {} must resolve to a non-empty relative path.",
            action_name, input_kind, input_index
        ));
    }

    let candidate = Path::new(path);
    if candidate.is_absolute() {
        return Err(format!(
            "Action '{}' child-agent {} input {} must stay at the current level or below; absolute paths are not allowed.",
            action_name, input_kind, input_index
        ));
    }

    if candidate
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(format!(
            "Action '{}' child-agent {} input {} must stay at the current level or below; parent traversal (`..`) is not allowed.",
            action_name, input_kind, input_index
        ));
    }

    Ok(())
}

fn validate_child_file_extension(
    path: &str,
    action_name: &str,
    input_index: usize,
) -> Result<(), String> {
    let extension = Path::new(path)
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.to_ascii_lowercase());

    match extension.as_deref() {
        Some(
            "pdf" | "docx" | "csv" | "xla" | "xlb" | "xlc" | "xlm" | "xls" | "xlsx" | "xlt" | "xlw"
            | "tsv" | "iif" | "doc" | "dot" | "odt" | "rtf" | "pot" | "ppa" | "pps" | "ppt"
            | "pptx" | "pwz" | "wiz",
        ) => Ok(()),
        _ => Err(format!(
            "Action '{}' child-agent file input {} must use a supported extension: {}.",
            action_name, input_index, SUPPORTED_FILE_EXTENSIONS_MESSAGE
        )),
    }
}

fn current_agent_action_depth() -> u32 {
    std::env::var(AGENT_ACTION_DEPTH_ENV)
        .ok()
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(0)
}

fn step_matches_platform(platforms: Option<&[String]>, current_platform: Option<&str>) -> bool {
    match platforms {
        None => true,
        Some(platforms) => current_platform
            .is_some_and(|platform| platforms.iter().any(|candidate| candidate == platform)),
    }
}

fn resolve_run_args(
    args: &[crate::RunArg],
    data: &serde_json::Value,
    action_name: &str,
) -> Result<Vec<String>, String> {
    args.iter()
        .enumerate()
        .map(|(index, arg)| resolve_run_arg(arg, data, action_name, index))
        .collect()
}

fn resolve_string_parts(
    parts: &[crate::RunArg],
    data: &serde_json::Value,
    action_name: &str,
    field_name: &str,
) -> Result<String, String> {
    let mut resolved = String::new();

    for (index, part) in parts.iter().enumerate() {
        let value = resolve_run_arg(part, data, action_name, index)?;
        resolved.push_str(&value);
    }

    if resolved.trim().is_empty() {
        return Err(format!(
            "Action '{}' {} resolved to an empty string.",
            action_name, field_name
        ));
    }

    Ok(resolved)
}

fn resolve_run_arg(
    arg: &crate::RunArg,
    data: &serde_json::Value,
    action_name: &str,
    index: usize,
) -> Result<String, String> {
    match arg {
        crate::RunArg::Literal(literal) => Ok(literal.clone()),
        crate::RunArg::Variable(variable) => {
            let Some(value) = data.get(variable) else {
                return Err(format!(
                    "Action '{}' arg {} references missing output field '{}'.",
                    action_name, index, variable
                ));
            };

            match value {
                serde_json::Value::String(text) => Ok(text.clone()),
                serde_json::Value::Bool(boolean) => Ok(boolean.to_string()),
                serde_json::Value::Number(number) => Ok(number.to_string()),
                serde_json::Value::Array(_) => Err(format!(
                    "Action '{}' arg {} references array-valued field '{}', which is unsupported for arg substitution.",
                    action_name, index, variable
                )),
                serde_json::Value::Object(_) => Err(format!(
                    "Action '{}' arg {} references object-valued field '{}', which is unsupported for arg substitution.",
                    action_name, index, variable
                )),
                serde_json::Value::Null => Err(format!(
                    "Action '{}' arg {} references null field '{}', which is unsupported for arg substitution.",
                    action_name, index, variable
                )),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        action_completion_summary, apply_actions, child_input_args,
        configured_agent_action_runtime_budget, format_backend_error_message,
        format_backend_ui_message, insert_action_output_variable, matching_run_steps,
        resolve_run_args, resolve_string_parts, run_agent_step, run_completion_message_for_depth,
        run_exec_step, step_matches_platform, validate_agent_action_depth, StepExecutionOutcome,
    };
    use serde_json::json;

    fn run_step(
        program: &str,
        platforms: Option<&[&str]>,
        args: Vec<crate::RunArg>,
    ) -> crate::RunStep {
        crate::RunStep {
            kind: "exec".to_string(),
            program: Some(program.to_string()),
            output_variable: None,
            status_variable: None,
            error_variable: None,
            failure_mode: None,
            when: None,
            args,
            subject: None,
            text: None,
            agent: None,
            inputs: None,
            input_mode: None,
            platforms: platforms.map(|platforms| {
                platforms
                    .iter()
                    .map(|platform| platform.to_string())
                    .collect()
            }),
        }
    }

    fn action(run: Vec<crate::RunStep>) -> crate::Action {
        crate::Action {
            name: "demo".to_string(),
            logic: json!({ "==": [{ "var": "answer" }, 4] }),
            run,
        }
    }

    #[test]
    fn platformless_steps_match_supported_platforms() {
        assert!(step_matches_platform(None, Some("macos")));
        assert!(step_matches_platform(None, Some("linux")));
        assert!(step_matches_platform(None, None));
    }

    #[test]
    fn explicit_platforms_match_only_listed_platforms() {
        let platforms = vec!["macos".to_string(), "linux".to_string()];
        assert!(step_matches_platform(Some(&platforms), Some("macos")));
        assert!(step_matches_platform(Some(&platforms), Some("linux")));
        assert!(!step_matches_platform(Some(&platforms), Some("windows")));
        assert!(!step_matches_platform(Some(&platforms), None));
    }

    #[test]
    fn matching_run_steps_preserve_declared_order() {
        let run_steps = vec![
            run_step("first", Some(&["windows"]), vec![]),
            run_step("second", None, vec![]),
            run_step("third", Some(&["macos", "linux"]), vec![]),
            run_step("fourth", None, vec![]),
        ];

        let matching = matching_run_steps(&run_steps, Some("macos"));
        let programs = matching
            .iter()
            .map(|step| {
                step.program
                    .as_deref()
                    .expect("exec test steps have a program")
            })
            .collect::<Vec<_>>();

        assert_eq!(programs, vec!["second", "third", "fourth"]);
    }

    #[test]
    fn resolves_literal_and_variable_args() {
        let resolved = resolve_run_args(
            &[
                crate::RunArg::Literal("value=".to_string()),
                crate::RunArg::Variable("answer".to_string()),
                crate::RunArg::Variable("raining".to_string()),
            ],
            &json!({
                "answer": 4,
                "raining": true
            }),
            "demo",
        )
        .expect("args should resolve");

        assert_eq!(resolved, vec!["value=", "4", "true"]);
    }

    #[test]
    fn rejects_missing_variable_args() {
        let error = resolve_run_args(
            &[crate::RunArg::Variable("answer".to_string())],
            &json!({}),
            "demo",
        )
        .unwrap_err();

        assert!(error.contains("missing output field 'answer'"));
    }

    #[test]
    fn rejects_array_valued_variable_args() {
        let error = resolve_run_args(
            &[crate::RunArg::Variable("numbers".to_string())],
            &json!({
                "numbers": [1, 2, 3]
            }),
            "demo",
        )
        .unwrap_err();

        assert!(error.contains("array-valued field 'numbers'"));
    }

    #[test]
    fn resolves_string_parts_without_implicit_spaces() {
        let resolved = resolve_string_parts(
            &[
                crate::RunArg::Literal("raining=".to_string()),
                crate::RunArg::Variable("raining".to_string()),
            ],
            &json!({
                "raining": true
            }),
            "demo",
            "text",
        )
        .expect("string parts should resolve");

        assert_eq!(resolved, "raining=true");
    }

    #[test]
    fn child_input_args_map_to_runtime_flags() {
        let (args, notes) = child_input_args(
            None,
            Some(&[
                crate::ActionInput::Text {
                    text: vec![crate::RunArg::Literal("hello".to_string())],
                },
                crate::ActionInput::Url {
                    url: vec![crate::RunArg::Literal("https://example.com".to_string())],
                },
                crate::ActionInput::Image {
                    path: vec![crate::RunArg::Literal("./diagram.png".to_string())],
                },
                crate::ActionInput::File {
                    path: vec![crate::RunArg::Literal("./report.pdf".to_string())],
                },
            ]),
            &json!({}),
            "demo",
        )
        .expect("child input args should resolve");

        assert_eq!(
            args,
            vec![
                "--input-text",
                "hello",
                "--input-url",
                "https://example.com",
                "--input-image",
                "./diagram.png",
                "--input-file",
                "./report.pdf",
            ]
        );
        assert!(notes.is_empty());
    }

    #[test]
    fn child_input_args_include_explicit_input_mode() {
        let (args, notes) = child_input_args(
            Some(crate::ActionInputMode::Prepend),
            Some(&[crate::ActionInput::Text {
                text: vec![crate::RunArg::Literal("hello".to_string())],
            }]),
            &json!({}),
            "demo",
        )
        .expect("child input args should resolve");

        assert_eq!(
            args,
            vec!["--input-mode", "prepend", "--input-text", "hello"]
        );
        assert!(notes.is_empty());
    }

    #[test]
    fn child_input_args_reject_input_mode_without_inputs() {
        let error = child_input_args(
            Some(crate::ActionInputMode::Append),
            None,
            &json!({}),
            "demo",
        )
        .unwrap_err();

        assert!(error.contains("child-agent `input_mode` requires `inputs`"));
    }

    #[test]
    fn child_input_args_resolve_dynamic_text_and_file_path() {
        let (args, notes) = child_input_args(
            None,
            Some(&[
                crate::ActionInput::Text {
                    text: vec![
                        crate::RunArg::Literal("hello ".to_string()),
                        crate::RunArg::Variable("customer".to_string()),
                    ],
                },
                crate::ActionInput::File {
                    path: vec![
                        crate::RunArg::Literal("./reports/".to_string()),
                        crate::RunArg::Variable("report_filename".to_string()),
                    ],
                },
            ]),
            &json!({
                "customer": "Acme",
                "report_filename": "q1.pdf"
            }),
            "demo",
        )
        .expect("dynamic child input args should resolve");

        assert_eq!(
            args,
            vec![
                "--input-text",
                "hello Acme",
                "--input-file",
                "./reports/q1.pdf"
            ]
        );
        assert_eq!(notes.len(), 2);
        assert!(notes[0].contains("dynamic child-agent text input 1"));
        assert!(notes[1].contains("./reports/q1.pdf"));
    }

    #[test]
    fn child_input_args_reject_invalid_dynamic_url() {
        let error = child_input_args(
            None,
            Some(&[crate::ActionInput::Url {
                url: vec![crate::RunArg::Variable("source_url".to_string())],
            }]),
            &json!({
                "source_url": "ftp://example.com/report"
            }),
            "demo",
        )
        .unwrap_err();

        assert!(error.contains("must resolve to an http(s) URL"));
    }

    #[test]
    fn child_input_args_reject_invalid_dynamic_file_extension() {
        let error = child_input_args(
            None,
            Some(&[crate::ActionInput::File {
                path: vec![
                    crate::RunArg::Literal("./reports/".to_string()),
                    crate::RunArg::Variable("report_filename".to_string()),
                ],
            }]),
            &json!({
                "report_filename": "q1.exe"
            }),
            "demo",
        )
        .unwrap_err();

        assert!(error.contains("supported extension"));
    }

    #[test]
    fn child_input_args_reject_parent_traversal_in_dynamic_path() {
        let error = child_input_args(
            None,
            Some(&[crate::ActionInput::Image {
                path: vec![crate::RunArg::Variable("image_path".to_string())],
            }]),
            &json!({
                "image_path": "../diagram.png"
            }),
            "demo",
        )
        .unwrap_err();

        assert!(error.contains("parent traversal"));
    }

    #[test]
    fn action_completion_summary_uses_completed_for_clean_runs() {
        let summary = action_completion_summary(&[StepExecutionOutcome::Completed]);
        assert_eq!(summary, Some("completed"));
    }

    #[test]
    fn action_completion_summary_suppresses_duplicate_single_step_email_success() {
        let summary = action_completion_summary(&[StepExecutionOutcome::SuccessAlreadyPrinted]);
        assert_eq!(summary, None);
    }

    #[test]
    fn action_completion_summary_suppresses_final_success_after_soft_failure() {
        let summary = action_completion_summary(&[
            StepExecutionOutcome::Completed,
            StepExecutionOutcome::SoftFailureLogged,
        ]);
        assert_eq!(summary, None);
    }

    #[test]
    fn run_completion_message_for_depth_prints_for_root_runs_only() {
        assert_eq!(
            run_completion_message_for_depth(0),
            Some("✅ Run complete.")
        );
        assert_eq!(run_completion_message_for_depth(1), None);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn exec_step_captures_output_variable_on_success() {
        let step = crate::RunStep {
            kind: "exec".to_string(),
            program: Some("/bin/sh".to_string()),
            output_variable: Some("report_listing".to_string()),
            status_variable: None,
            error_variable: None,
            failure_mode: None,
            when: None,
            args: vec![
                crate::RunArg::Literal("-lc".to_string()),
                crate::RunArg::Literal("printf 'alpha\\nbeta\\n'".to_string()),
            ],
            subject: None,
            text: None,
            agent: None,
            inputs: None,
            input_mode: None,
            platforms: None,
        };

        let runtime_budget = configured_agent_action_runtime_budget(Some(600));
        let captured_output = run_exec_step(&step, &json!({}), "capture_exec", runtime_budget)
            .await
            .expect("exec capture should succeed");

        assert_eq!(
            captured_output,
            Some(("report_listing".to_string(), "alpha\nbeta".to_string()))
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn agent_step_invokes_child_with_forwarded_inputs() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;

        let current_dir = std::env::current_dir().expect("current dir should resolve");
        let script_name = format!(".tmp-cai2032-agent-child-{}.sh", std::process::id());
        let script_path = current_dir.join(&script_name);
        let output_path = std::env::temp_dir().join(format!(
            "cai2032-agent-child-args-{}.txt",
            std::process::id()
        ));

        let script_body = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"{}\"\n",
            output_path.display()
        );

        fs::write(&script_path, script_body).expect("script should be written");
        let mut permissions = fs::metadata(&script_path)
            .expect("script metadata should load")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&script_path, permissions).expect("script should be executable");

        let step = crate::RunStep {
            kind: "agent".to_string(),
            program: None,
            output_variable: None,
            status_variable: None,
            error_variable: None,
            failure_mode: None,
            when: None,
            args: Vec::new(),
            subject: None,
            text: None,
            agent: Some(format!("./{}", script_name)),
            inputs: Some(vec![
                crate::ActionInput::Text {
                    text: vec![
                        crate::RunArg::Literal("hello ".to_string()),
                        crate::RunArg::Variable("customer".to_string()),
                    ],
                },
                crate::ActionInput::Url {
                    url: vec![crate::RunArg::Literal("https://example.com".to_string())],
                },
                crate::ActionInput::Image {
                    path: vec![crate::RunArg::Literal("./diagram.png".to_string())],
                },
                crate::ActionInput::File {
                    path: vec![
                        crate::RunArg::Literal("./reports/".to_string()),
                        crate::RunArg::Variable("report_filename".to_string()),
                    ],
                },
            ]),
            input_mode: Some(crate::ActionInputMode::Append),
            platforms: None,
        };

        let runtime_budget = configured_agent_action_runtime_budget(Some(600));
        let result = run_agent_step(
            &step,
            &json!({
                "customer": "world",
                "report_filename": "report.pdf"
            }),
            "invoke_child",
            5,
            runtime_budget,
        )
        .await;

        let _ = fs::remove_file(&script_path);

        assert!(
            result.is_ok(),
            "child agent invocation should succeed: {result:?}"
        );

        let args = fs::read_to_string(&output_path).expect("child output should be captured");
        let _ = fs::remove_file(&output_path);

        assert_eq!(
            args.lines().collect::<Vec<_>>(),
            vec![
                "--input-mode",
                "append",
                "--input-text",
                "hello world",
                "--input-url",
                "https://example.com",
                "--input-image",
                "./diagram.png",
                "--input-file",
                "./reports/report.pdf",
            ]
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn captured_exec_output_can_flow_into_later_agent_step() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;

        let current_dir = std::env::current_dir().expect("current dir should resolve");
        let script_name = format!(".tmp-cai2036-phase5-child-{}.sh", std::process::id());
        let script_path = current_dir.join(&script_name);
        let output_path = std::env::temp_dir().join(format!(
            "cai2036-phase5-child-args-{}.txt",
            std::process::id()
        ));

        let script_body = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"{}\"\n",
            output_path.display()
        );

        fs::write(&script_path, script_body).expect("script should be written");
        let mut permissions = fs::metadata(&script_path)
            .expect("script metadata should load")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&script_path, permissions).expect("script should be executable");

        let exec_step = crate::RunStep {
            kind: "exec".to_string(),
            program: Some("/bin/sh".to_string()),
            output_variable: Some("report_listing".to_string()),
            status_variable: None,
            error_variable: None,
            failure_mode: None,
            when: None,
            args: vec![
                crate::RunArg::Literal("-lc".to_string()),
                crate::RunArg::Literal("printf 'q1.pdf | q2.pdf\\n'".to_string()),
            ],
            subject: None,
            text: None,
            agent: None,
            inputs: None,
            input_mode: None,
            platforms: None,
        };
        let agent_step = crate::RunStep {
            kind: "agent".to_string(),
            program: None,
            output_variable: None,
            status_variable: None,
            error_variable: None,
            failure_mode: None,
            when: None,
            args: Vec::new(),
            subject: None,
            text: None,
            agent: Some(format!("./{}", script_name)),
            inputs: Some(vec![crate::ActionInput::Text {
                text: vec![
                    crate::RunArg::Literal("Files:\n".to_string()),
                    crate::RunArg::Variable("report_listing".to_string()),
                ],
            }]),
            input_mode: None,
            platforms: None,
        };

        let runtime_budget = configured_agent_action_runtime_budget(Some(600));
        let mut action_data = json!({});
        let captured_output = run_exec_step(
            &exec_step,
            &action_data,
            "capture_then_agent",
            runtime_budget,
        )
        .await
        .expect("exec capture should succeed");
        let (name, value) = captured_output.expect("captured output should be present");
        insert_action_output_variable(&mut action_data, name.as_str(), value, "capture_then_agent")
            .expect("captured output should be inserted");

        let result = run_agent_step(
            &agent_step,
            &action_data,
            "capture_then_agent",
            5,
            runtime_budget,
        )
        .await;

        let _ = fs::remove_file(&script_path);

        assert!(
            result.is_ok(),
            "child agent invocation should succeed: {result:?}"
        );

        let args = fs::read_to_string(&output_path).expect("child output should be captured");
        let _ = fs::remove_file(&output_path);

        assert_eq!(
            args.lines().collect::<Vec<_>>(),
            vec!["--input-text", "Files:", "q1.pdf | q2.pdf"]
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn agent_step_inherits_max_depth_and_runtime_budget_for_child_processes() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;

        let current_dir = std::env::current_dir().expect("current dir should resolve");
        let script_name = format!(".tmp-cai2032-agent-depth-child-{}.sh", std::process::id());
        let script_path = current_dir.join(&script_name);
        let output_path =
            std::env::temp_dir().join(format!("cai2032-agent-depth-{}.txt", std::process::id()));

        let script_body = format!(
            "#!/bin/sh\nprintf '%s\\n%s' \"$CARGO_AI_AGENT_ACTION_MAX_DEPTH\" \"$CARGO_AI_AGENT_MAX_RUNTIME_SECS\" > \"{}\"\n",
            output_path.display()
        );

        fs::write(&script_path, script_body).expect("script should be written");
        let mut permissions = fs::metadata(&script_path)
            .expect("script metadata should load")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&script_path, permissions).expect("script should be executable");

        let step = crate::RunStep {
            kind: "agent".to_string(),
            program: None,
            output_variable: None,
            status_variable: None,
            error_variable: None,
            failure_mode: None,
            when: None,
            args: Vec::new(),
            subject: None,
            text: None,
            agent: Some(format!("./{}", script_name)),
            inputs: None,
            input_mode: None,
            platforms: None,
        };

        let runtime_budget = configured_agent_action_runtime_budget(Some(600));
        let result = run_agent_step(&step, &json!({}), "invoke_child", 7, runtime_budget).await;

        let _ = fs::remove_file(&script_path);

        assert!(
            result.is_ok(),
            "child agent invocation should succeed: {result:?}"
        );

        let inherited_values =
            fs::read_to_string(&output_path).expect("child output should be captured");
        let _ = fs::remove_file(&output_path);

        assert_eq!(
            inherited_values.lines().collect::<Vec<_>>(),
            vec!["7", "600"]
        );
    }

    #[tokio::test]
    async fn agent_step_rejects_bare_child_name() {
        let step = crate::RunStep {
            kind: "agent".to_string(),
            program: None,
            output_variable: None,
            status_variable: None,
            error_variable: None,
            failure_mode: None,
            when: None,
            args: Vec::new(),
            subject: None,
            text: None,
            agent: Some("child_agent".to_string()),
            inputs: None,
            input_mode: None,
            platforms: None,
        };

        let runtime_budget = configured_agent_action_runtime_budget(Some(600));
        let error = run_agent_step(&step, &json!({}), "invoke_child", 5, runtime_budget)
            .await
            .expect_err("bare child agent names should be rejected");

        assert!(error.contains("bare child-agent names are not allowed"));
    }

    #[tokio::test]
    async fn agent_step_rejects_parent_traversal_path() {
        let step = crate::RunStep {
            kind: "agent".to_string(),
            program: None,
            output_variable: None,
            status_variable: None,
            error_variable: None,
            failure_mode: None,
            when: None,
            args: Vec::new(),
            subject: None,
            text: None,
            agent: Some("./../child_agent".to_string()),
            inputs: None,
            input_mode: None,
            platforms: None,
        };

        let runtime_budget = configured_agent_action_runtime_budget(Some(600));
        let error = run_agent_step(&step, &json!({}), "invoke_child", 5, runtime_budget)
            .await
            .expect_err("parent traversal should be rejected");

        assert!(error.contains("parent traversal"));
    }

    #[tokio::test]
    async fn agent_step_rejects_nested_child_path() {
        let step = crate::RunStep {
            kind: "agent".to_string(),
            program: None,
            output_variable: None,
            status_variable: None,
            error_variable: None,
            failure_mode: None,
            when: None,
            args: Vec::new(),
            subject: None,
            text: None,
            agent: Some("./agents/child_agent".to_string()),
            inputs: None,
            input_mode: None,
            platforms: None,
        };

        let runtime_budget = configured_agent_action_runtime_budget(Some(600));
        let error = run_agent_step(&step, &json!({}), "invoke_child", 5, runtime_budget)
            .await
            .expect_err("nested child agent paths should be rejected");

        assert!(error.contains("nested child-agent paths"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn agent_step_times_out_against_invocation_budget() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;

        let current_dir = std::env::current_dir().expect("current dir should resolve");
        let script_name = format!(".tmp-cai2032-agent-timeout-child-{}.sh", std::process::id());
        let script_path = current_dir.join(&script_name);

        let script_body = "#!/bin/sh\nsleep 2\n";

        fs::write(&script_path, script_body).expect("script should be written");
        let mut permissions = fs::metadata(&script_path)
            .expect("script metadata should load")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&script_path, permissions).expect("script should be executable");

        let step = crate::RunStep {
            kind: "agent".to_string(),
            program: None,
            output_variable: None,
            status_variable: None,
            error_variable: None,
            failure_mode: None,
            when: None,
            args: Vec::new(),
            subject: None,
            text: None,
            agent: Some(format!("./{}", script_name)),
            inputs: None,
            input_mode: None,
            platforms: None,
        };

        let runtime_budget = configured_agent_action_runtime_budget(Some(1));
        let error = run_agent_step(&step, &json!({}), "invoke_child", 5, runtime_budget)
            .await
            .expect_err("runtime budget should time out the child");

        let _ = fs::remove_file(&script_path);

        assert!(error.contains("max-runtime-in-sec 1"));
        assert!(error.contains("while waiting for child agent"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn apply_actions_stops_on_failed_exec_by_default() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;

        let current_dir = std::env::current_dir().expect("current dir should resolve");
        let script_name = format!(".tmp-cai2040-stop-child-{}.sh", std::process::id());
        let script_path = current_dir.join(&script_name);
        let output_path =
            std::env::temp_dir().join(format!("cai2040-stop-output-{}.txt", std::process::id()));

        let script_body = format!("#!/bin/sh\nprintf 'ran' > \"{}\"\n", output_path.display());
        fs::write(&script_path, script_body).expect("script should be written");
        let mut permissions = fs::metadata(&script_path)
            .expect("script metadata should load")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&script_path, permissions).expect("script should be executable");

        let failing_step = crate::RunStep {
            kind: "exec".to_string(),
            program: Some("/bin/sh".to_string()),
            output_variable: None,
            status_variable: Some("step_status".to_string()),
            error_variable: Some("step_error".to_string()),
            failure_mode: None,
            when: None,
            args: vec![
                crate::RunArg::Literal("-lc".to_string()),
                crate::RunArg::Literal("exit 7".to_string()),
            ],
            subject: None,
            text: None,
            agent: None,
            inputs: None,
            input_mode: None,
            platforms: None,
        };
        let second_step = crate::RunStep {
            kind: "agent".to_string(),
            program: None,
            output_variable: None,
            status_variable: None,
            error_variable: None,
            failure_mode: None,
            when: None,
            args: Vec::new(),
            subject: None,
            text: None,
            agent: Some(format!("./{}", script_name)),
            inputs: None,
            input_mode: None,
            platforms: None,
        };

        let runtime_budget = configured_agent_action_runtime_budget(Some(600));
        let result = apply_actions(
            &crate::Output { answer: 4 },
            &[action(vec![failing_step, second_step])],
            5,
            runtime_budget,
        )
        .await;

        let _ = fs::remove_file(&script_path);
        let _ = fs::remove_file(&output_path);

        let error = result.expect_err("failed exec should stop by default");
        assert!(error.contains("exited with status"));
        assert!(
            !output_path.exists(),
            "later step should not run after default stop"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn apply_actions_continue_mode_exposes_failed_status_to_later_when() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;

        let current_dir = std::env::current_dir().expect("current dir should resolve");
        let script_name = format!(".tmp-cai2040-continue-child-{}.sh", std::process::id());
        let script_path = current_dir.join(&script_name);
        let output_path = std::env::temp_dir().join(format!(
            "cai2040-continue-output-{}.txt",
            std::process::id()
        ));

        let script_body = format!("#!/bin/sh\nprintf 'ran' > \"{}\"\n", output_path.display());
        fs::write(&script_path, script_body).expect("script should be written");
        let mut permissions = fs::metadata(&script_path)
            .expect("script metadata should load")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&script_path, permissions).expect("script should be executable");

        let failing_step = crate::RunStep {
            kind: "exec".to_string(),
            program: Some("/bin/sh".to_string()),
            output_variable: None,
            status_variable: Some("step_status".to_string()),
            error_variable: Some("step_error".to_string()),
            failure_mode: Some(crate::FailureMode::Continue),
            when: None,
            args: vec![
                crate::RunArg::Literal("-lc".to_string()),
                crate::RunArg::Literal("exit 9".to_string()),
            ],
            subject: None,
            text: None,
            agent: None,
            inputs: None,
            input_mode: None,
            platforms: None,
        };
        let second_step = crate::RunStep {
            kind: "agent".to_string(),
            program: None,
            output_variable: None,
            status_variable: None,
            error_variable: None,
            failure_mode: None,
            when: Some(json!({ "==": [{ "var": "step_status" }, "failed"] })),
            args: Vec::new(),
            subject: None,
            text: None,
            agent: Some(format!("./{}", script_name)),
            inputs: None,
            input_mode: None,
            platforms: None,
        };

        let runtime_budget = configured_agent_action_runtime_budget(Some(600));
        let result = apply_actions(
            &crate::Output { answer: 4 },
            &[action(vec![failing_step, second_step])],
            5,
            runtime_budget,
        )
        .await;

        let _ = fs::remove_file(&script_path);

        assert!(
            result.is_ok(),
            "continue-mode failure should not stop the action"
        );

        let file_contents =
            fs::read_to_string(&output_path).expect("later step should have executed");
        let _ = fs::remove_file(&output_path);

        assert_eq!(file_contents, "ran");
    }

    #[test]
    fn validate_agent_action_depth_allows_nested_calls_below_limit() {
        let result = validate_agent_action_depth(2, 5, "invoke_child");

        assert!(result.is_ok(), "depth below limit should be allowed");
    }

    #[test]
    fn validate_agent_action_depth_rejects_when_limit_is_reached() {
        let error = validate_agent_action_depth(5, 5, "invoke_child")
            .expect_err("depth at limit should be rejected");

        assert!(error.contains("current depth 5"));
        assert!(error.contains("max-agent-depth 5"));
    }

    #[test]
    fn validate_agent_action_depth_rejects_zero_depth_limit() {
        let error = validate_agent_action_depth(0, 0, "invoke_child")
            .expect_err("zero max depth should disable child invocation");

        assert!(error.contains("current depth 0"));
        assert!(error.contains("max-agent-depth 0"));
    }

    #[test]
    fn formats_backend_ui_success_with_kind_prefix() {
        let response = json!({
            "ui": {
                "schema": "1.0",
                "kind": "success",
                "title": "Email sent",
                "summary": "Test email sent to sales@analyzer1.com.",
                "next_steps": ["Check your inbox and spam folder for the message."]
            }
        });

        let rendered =
            format_backend_ui_message(&response, true).expect("success ui should format");

        assert!(rendered.contains("✅ Email sent"));
        assert!(rendered.contains("Test email sent to sales@analyzer1.com."));
        assert!(rendered.contains("Next steps:"));
    }

    #[test]
    fn formats_backend_ui_failure_without_kind_prefix() {
        let response = json!({
            "ui": {
                "schema": "1.0",
                "kind": "failure",
                "title": "Request failed",
                "summary": "Email sending is disabled for this account.",
                "next_steps": ["Enable mail and retry."]
            }
        });

        let rendered = format_backend_error_message(&response).expect("failure ui should format");

        assert!(rendered.starts_with("Request failed"));
        assert!(!rendered.contains("❌ Request failed"));
        assert!(rendered.contains("Email sending is disabled for this account."));
        assert!(rendered.contains("Next steps:"));
    }
}
