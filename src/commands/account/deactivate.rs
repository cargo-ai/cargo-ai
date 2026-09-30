//! Stop hosted access, optionally recording a request for manual deletion.
use clap::ArgMatches;
use std::io::{self, IsTerminal};

use super::{
    consent,
    helpers::{load_account_auth, INFRA_BASE_URL},
};
use crate::{config::loader::load_config, infra_api};

pub async fn run(args: &ArgMatches) -> bool {
    execute(args, &mut super::machine::Report::new(false)).await
}
pub(crate) async fn machine_run(
    args: &ArgMatches,
) -> Result<serde_json::Value, crate::commands::machine::Failure> {
    let mut report = super::machine::Report::new(true);
    execute(args, &mut report).await;
    report.result
}
async fn execute(args: &ArgMatches, report: &mut super::machine::Report) -> bool {
    execute_at(args, report, INFRA_BASE_URL).await
}
async fn execute_at(
    args: &ArgMatches,
    report: &mut super::machine::Report,
    base_url: &str,
) -> bool {
    let request_deletion = args.get_flag("request-deletion");
    let confirmation_email = args.get_one::<String>("confirm-email");
    if report.active
        && ((request_deletion && confirmation_email.is_none())
            || (!request_deletion && !args.get_flag("yes")))
    {
        report.fail(
            "cli.interaction_required",
            "Explicit deactivation or deletion confirmation is required.",
            serde_json::json!({"remote_effect":"not_attempted"}),
        );
        return false;
    }
    if !io::stdin().is_terminal()
        && ((request_deletion && confirmation_email.is_none())
            || (!request_deletion && !args.get_flag("yes")))
    {
        eprintln!("x Noninteractive deactivation requires --yes, or --request-deletion --confirm-email <EMAIL> for a deletion request.");
        return false;
    }
    report.prerequisites();
    let auth = match load_account_auth() {
        Ok(auth) => auth,
        Err(error) => {
            eprintln!("{error}");
            return false;
        }
    };
    let email = match load_config()
        .and_then(|config| config.account)
        .and_then(|account| account.email)
    {
        Some(email) => email,
        None => {
            eprintln!("x No account email is configured. Reactivate with account register and confirm first.");
            return false;
        }
    };
    println!("Hosted access will stop and your public packages and agents will be hidden.");
    println!("Local projects, installed applications and provider credentials are preserved.");
    let confirmed_email = if request_deletion {
        println!("An administrator will later remove your account and live hosted data. Completed deletion is permanent.");
        println!("Restricted copies and operational records remain until expiry or separate retirement, without a universal deadline.");
        println!("Restored copies must exclude completed deletions before returning to service.");
        let entered = match confirmation_email {
            Some(value) => value.trim().to_string(),
            None => match consent::read_answer("Type your account email to confirm: ") {
                Ok(value) => value,
                Err(error) => {
                    eprintln!("x {error}");
                    return false;
                }
            },
        };
        if !entered.eq_ignore_ascii_case(&email) {
            report.fail(
                "input.confirmation_mismatch",
                "Email confirmation did not match the configured account.",
                serde_json::json!({"remote_effect":"not_attempted"}),
            );
            eprintln!("x Email confirmation did not match the configured account. No deactivation was requested.");
            return false;
        }
        Some(entered)
    } else {
        println!("Your account and hosted data will be retained.");
        match consent::confirm("Deactivate your account? [y/N]: ", args.get_flag("yes")) {
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
        None
    };

    // The server revokes hosted sessions. Keeping local state avoids stale cleanup
    // overwriting credentials from a concurrent successful reactivation.
    report.transmitting(true);
    let response = match infra_api::account::deactivate::deactivate(
        base_url,
        &auth.access_token,
        request_deletion,
        confirmed_email.as_deref(),
    )
    .await
    {
        Ok(response) => response,
        Err(_) => {
            eprintln!("x The deactivation result could not be confirmed. Hosted access may already have stopped. Check account status before taking another action; an incomplete transition may require administrator assistance.");
            return false;
        }
    };
    if response["status"] != "success" || response["type"] != "account_deactivated" {
        report.result = super::machine::response_ok(&response, "account_deactivated")
            .map(|()| serde_json::Value::Null);
        let message = response.get("message").and_then(|value| value.as_str())
            .unwrap_or("Deactivation was not confirmed. Check account status; an incomplete transition may require administrator assistance.");
        eprintln!("x {message}");
        return false;
    }
    match response
        .get("deletion_requested")
        .and_then(|value| value.as_bool())
    {
        Some(true) => {
            println!("Account deactivated. Permanent deletion requested.");
            println!("Deletion is processed manually and may take several days.");
            println!("Your data has not yet been permanently deleted.");
            println!("Reactivating before deletion begins will cancel this request.");
        }
        Some(false) if !request_deletion => println!("Account deactivated."),
        _ => {
            report.fail("operation.partial", "Deactivation was confirmed but the requested deletion state was not.", serde_json::json!({"deactivated":true,"deletion_requested":null,"local_state":"preserved"}));
            eprintln!("x The server did not confirm the requested deletion state. Check account status or contact the administrator.");
            return false;
        }
    }
    report.accepted(serde_json::json!({"deactivated":true,"deletion_requested":response["deletion_requested"],"deletion_completed":false,"local_state":"preserved"}));
    println!("To reactivate: cargo ai account register {email}");
    println!("Then confirm using the new code sent to your email.");
    true
}

#[cfg(test)]
mod machine_tests {
    use super::*;
    use serde_json::json;
    #[tokio::test]
    async fn machine_account_deactivate_distinguishes_deletion_request_from_completed_deletion() {
        let home = crate::commands::secret_input::test_support::Home::new();
        crate::config::adder::set_account_tokens(
            "fixture-access".into(),
            "fixture-refresh".into(),
            3600,
        )
        .unwrap();
        let before = std::fs::read(home.path.join("credentials.toml")).unwrap();
        let mut server = mockito::Server::new_async().await;
        let request=server.mock("POST","/account").with_status(200)
            .with_body(json!({"status":"success","type":"account_deactivated","deletion_requested":true,"message":"private-secret"}).to_string()).expect(1).create_async().await;
        let args = super::super::machine::fixture_args(&[
            "cargo-ai",
            "account",
            "deactivate",
            "--yes",
            "--request-deletion",
            "--confirm-email",
            "owner@example.test",
        ]);
        let mut report = super::super::machine::Report::new(true);
        assert!(
            execute_at(
                args.subcommand_matches("account")
                    .unwrap()
                    .subcommand_matches("deactivate")
                    .unwrap(),
                &mut report,
                &server.url()
            )
            .await
        );
        let data = report.result.unwrap();
        assert_eq!(data["deletion_requested"], true);
        assert_eq!(data["deletion_completed"], false);
        assert_eq!(data["local_state"], "preserved");
        assert_eq!(
            std::fs::read(home.path.join("credentials.toml")).unwrap(),
            before
        );
        assert!(!data.to_string().contains("private-secret"));
        request.assert_async().await;
    }
}
