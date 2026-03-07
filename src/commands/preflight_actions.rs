//! Action execution helpers for preflight/test flows.
use crate::config::adder::set_account_tokens;
use crate::config::loader::{config_path, load_config};
use crate::credentials::store;
use crate::infra_api;
use crate::ui;
use jsonlogic::apply;

const INFRA_BASE_URL: &str = "https://api.cargo-ai.org";

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

                    for step in matching_steps {
                        if step.kind.eq_ignore_ascii_case("exec") {
                            run_exec_step(step, &data, &action.name)?;
                        } else if step.kind.eq_ignore_ascii_case("email_me") {
                            run_email_me_step(step, &data, &action.name).await?;
                        } else {
                            eprintln!(
                                "⚠️ Skipping action '{}' with unsupported step kind '{}'.",
                                action.name, step.kind
                            );
                        }
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

    Ok(())
}

fn run_exec_step(
    step: &crate::RunStep,
    data: &serde_json::Value,
    action_name: &str,
) -> Result<(), String> {
    let program = step.program.as_deref().ok_or_else(|| {
        format!(
            "Action '{}' exec step is missing required `program`.",
            action_name
        )
    })?;

    let resolved_args = resolve_run_args(&step.args, data, action_name)?;
    println!("Running '{}': {} {:?}", action_name, program, resolved_args);

    let status = std::process::Command::new(program)
        .args(&resolved_args)
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

    Ok(())
}

async fn run_email_me_step(
    step: &crate::RunStep,
    data: &serde_json::Value,
    action_name: &str,
) -> Result<(), String> {
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
    println!("Running '{}': email_me {:?}", action_name, subject);

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
                render_backend_ui_or_json(&refresh_response);
                return Err(
                    "Session refresh did not return a new access token. Cannot retry email_me action."
                        .to_string(),
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

    render_backend_ui_or_json(&response);

    let succeeded = response
        .get("status")
        .and_then(|v| v.as_str())
        .map(|status| status.eq_ignore_ascii_case("success"))
        .unwrap_or(false);

    if succeeded {
        Ok(())
    } else {
        Err(format!("Action '{}' email_me request failed.", action_name))
    }
}

fn render_backend_ui_or_json(response: &serde_json::Value) {
    if !ui::account_status::render_backend_ui(response) {
        match serde_json::to_string_pretty(response) {
            Ok(pretty) => println!("{pretty}"),
            Err(_) => println!("{response:?}"),
        }
    }
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
        matching_run_steps, resolve_run_args, resolve_string_parts, step_matches_platform,
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
            args,
            subject: None,
            text: None,
            platforms: platforms.map(|platforms| {
                platforms
                    .iter()
                    .map(|platform| platform.to_string())
                    .collect()
            }),
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
}
