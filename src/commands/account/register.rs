//! Runtime behavior for `cargo ai account register`.
use clap::ArgMatches;

use crate::config::adder::set_account_email;
use crate::config::loader::load_config;
use crate::config::setup::{config_path, ensure_config_file_exists};
use crate::infra_api;
use crate::ui;

use std::io::{self, Write};

use super::helpers::{
    extract_status_account_email, fetch_status_for_register_guard, load_account_auth,
    INFRA_BASE_URL,
};

/// Registers an account email and persists the active email on success.
pub async fn run(reg_m: &ArgMatches) -> bool {
    execute(reg_m, &mut super::machine::Report::new(false)).await
}
pub(crate) async fn machine_run(
    args: &ArgMatches,
) -> Result<serde_json::Value, crate::commands::machine::Failure> {
    let mut report = super::machine::Report::new(true);
    execute(args, &mut report).await;
    report.result
}
async fn execute(reg_m: &ArgMatches, report: &mut super::machine::Report) -> bool {
    execute_at(reg_m, report, INFRA_BASE_URL).await
}
async fn execute_at(
    reg_m: &ArgMatches,
    report: &mut super::machine::Report,
    base_url: &str,
) -> bool {
    let Some(email) = reg_m.get_one::<String>("email") else {
        eprintln!("x Missing email. Use `cargo ai account register <email>`.");
        return false;
    };

    if report.active {
        if crate::config::loader::load_config_strict().is_err() {
            report.fail(
                "config.invalid",
                "The existing account configuration could not be validated.",
                serde_json::json!({"remote_effect":"not_attempted"}),
            );
            return false;
        }
        if !reg_m.get_flag("yes") {
            report.fail(
                "cli.interaction_required",
                "Registration requires explicit confirmation.",
                serde_json::json!({"remote_effect":"not_attempted"}),
            );
            return false;
        }
        if load_config()
            .and_then(|cfg| cfg.account)
            .and_then(|account| account.email)
            .is_some_and(|existing| !existing.eq_ignore_ascii_case(email))
        {
            report.fail(
                "cli.interaction_required",
                "Switching the configured account requires the existing interactive confirmation.",
                serde_json::json!({"remote_effect":"not_attempted"}),
            );
            return false;
        }
    }
    report.fail(
        "persistence.failed",
        "Local registration configuration could not be initialized.",
        serde_json::json!({"remote_effect":"not_attempted"}),
    );
    if let Err(e) = ensure_config_file_exists() {
        eprintln!(
            "x Failed to initialize local config at '{}': {e}",
            config_path().display()
        );
        return false;
    }

    // Guard: skip register when local session is already valid for the requested email.
    if let Some(cfg) = load_config() {
        if let Some(acct) = cfg.account.as_ref() {
            if acct.email.as_deref().is_some() {
                if let Ok(auth) = load_account_auth() {
                    report.protect(&auth.access_token);
                    if let Some(secret) = auth.refresh_token.as_deref() {
                        report.protect(secret);
                    }
                    let status_response = fetch_status_for_register_guard(
                        auth.access_token.as_str(),
                        auth.refresh_token.as_deref(),
                    )
                    .await;

                    if let Some(active_email) = extract_status_account_email(&status_response) {
                        if active_email.eq_ignore_ascii_case(email) {
                            report.accepted(serde_json::json!({"email":active_email,"already_authenticated":true,"code_request":"not_needed","email_delivery":"not_checked","local_email_persistence":"unchanged"}));
                            println!(
                                "✓ You are already signed in as '{}'. Registration is not needed.",
                                active_email
                            );
                            return true;
                        }
                    }
                }
            }

            // If an account email is already configured and differs, confirm before proceeding.
            if let Some(existing_email) = acct.email.as_ref() {
                if !existing_email.eq_ignore_ascii_case(email) {
                    println!(
                        "! Local account is currently configured as '{}'.",
                        existing_email
                    );
                    println!(
                        "Continuing will replace local account email and tokens on this machine."
                    );
                    println!(
                        "If you need the current local state, back up '{}'.",
                        config_path().display()
                    );
                    print!("Continue and switch to '{}'? [y/N]: ", email);
                    if let Err(e) = io::stdout().flush() {
                        eprintln!("! Failed to flush stdout: {e}");
                        return false;
                    }

                    let mut input = String::new();
                    if let Err(e) = io::stdin().read_line(&mut input) {
                        eprintln!("! Failed to read input: {e}");
                        return false;
                    }

                    let input = input.trim();
                    if !(input.eq_ignore_ascii_case("y") || input.eq_ignore_ascii_case("yes")) {
                        println!("Operation canceled.");
                        return true;
                    }
                }
            }
        }
    }

    println!("Confirming a new email code will reactivate a deactivated account and cancel pending deletion, provided deletion has not begun.");
    match super::consent::confirm(
        "Request a new confirmation code? [y/N]: ",
        reg_m.get_flag("yes"),
    ) {
        Ok(true) => {}
        Ok(false) => {
            println!("Operation canceled.");
            return true;
        }
        Err(error) => {
            eprintln!("x {error}");
            return false;
        }
    }

    report.transmitting(true);
    match infra_api::account::register::register_email(base_url, email).await {
        Ok(json) => {
            if report.active {
                if let Err(error) =
                    super::machine::response_ok(&json, "register_account_email_succeeded")
                {
                    report.result = Err(error);
                    return false;
                }
            }
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
                    eprintln!("! Failed to save account email to config: {e}");
                    report.fail("operation.partial", "The code request was accepted but local email persistence failed.", serde_json::json!({"email":email,"code_request":"accepted","email_delivery":"not_checked","local_email_persistence":"failed","credential_persistence":"unknown"}));
                    return true;
                }
                report.result = super::machine::response_ok(&json, "register_account_email_succeeded").map(|()|serde_json::json!({"email":email,"code_request":"accepted","email_delivery":"not_checked","local_email_persistence":"persisted"}));
                if let Ok(data) = &report.result {
                    report.accepted(data.clone());
                }
                true
            } else {
                report.result =
                    super::machine::response_ok(&json, "register_account_email_succeeded")
                        .map(|()| serde_json::Value::Null);
                false
            }
        }
        Err(e) => {
            eprintln!("x Request failed: {e}");
            false
        }
    }
}

#[cfg(test)]
mod machine_tests {
    use super::*;
    use serde_json::json;
    #[tokio::test]
    async fn machine_account_register_requires_consent_then_reports_acceptance_not_delivery() {
        let home = crate::commands::secret_input::test_support::Home::new();
        let before = std::fs::read(home.path.join("config.toml")).unwrap();
        let mut server = mockito::Server::new_async().await;
        let request=server.mock("POST","/account").match_body(mockito::Matcher::PartialJson(json!({"action":"register","email":"owner@example.test"})))
            .with_status(200).with_body(json!({"status":"success","type":"register_account_email_succeeded","message":"private-secret","ui":{"title":"private-secret"}}).to_string()).expect(1).create_async().await;
        let args = super::super::machine::fixture_args(&[
            "cargo-ai",
            "account",
            "register",
            "owner@example.test",
        ]);
        let mut report = super::super::machine::Report::new(true);
        assert!(
            !execute_at(
                args.subcommand_matches("account")
                    .unwrap()
                    .subcommand_matches("register")
                    .unwrap(),
                &mut report,
                &server.url()
            )
            .await
        );
        assert_eq!(report.result.unwrap_err().code, "cli.interaction_required");
        assert_eq!(
            std::fs::read(home.path.join("config.toml")).unwrap(),
            before
        );
        let args = super::super::machine::fixture_args(&[
            "cargo-ai",
            "account",
            "register",
            "owner@example.test",
            "--yes",
        ]);
        let mut report = super::super::machine::Report::new(true);
        assert!(
            execute_at(
                args.subcommand_matches("account")
                    .unwrap()
                    .subcommand_matches("register")
                    .unwrap(),
                &mut report,
                &server.url()
            )
            .await
        );
        let data = report.result.unwrap();
        assert_eq!(data["code_request"], "accepted");
        assert_eq!(data["email_delivery"], "not_checked");
        assert!(!data.to_string().contains("private-secret"));
        request.assert_async().await;
    }
}
