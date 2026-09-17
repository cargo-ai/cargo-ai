//! Runtime behavior for `cargo ai account confirm`.
use clap::ArgMatches;
use serde_json::Value;
use std::io::{self, Write};

use super::helpers::INFRA_BASE_URL;
use crate::commands::secret_input;
use crate::config::adder::set_account_tokens;
use crate::config::loader::load_config;
use crate::infra_api;

const RECOVERY: &str = "Check the selected CARGO_AI_HOME and credential-store/config write access, then check account status. If credentials are unavailable, request a fresh code through account registration with the existing consent; do not assume this code is reusable.";
const UNKNOWN: &str = "Could not determine the confirmation outcome from the service response. Check connectivity and account status before requesting a fresh code; do not automatically retry the same code.";

struct Outcome {
    success: bool,
    message: &'static str,
    recovery: bool,
    reactivated: bool,
    deletion_cancelled: bool,
}

impl Outcome {
    fn failure(message: &'static str) -> Self {
        Self {
            success: false,
            message,
            recovery: false,
            reactivated: false,
            deletion_cancelled: false,
        }
    }

    // Only locally authored text and boolean lifecycle notices reach output.
    fn write(&self, stdout: &mut impl Write, stderr: &mut impl Write) -> io::Result<()> {
        let out: &mut dyn Write = if self.success { stdout } else { stderr };
        writeln!(out, "{}", self.message)?;
        if self.reactivated {
            writeln!(out, "Account reactivated.")?;
        }
        if self.deletion_cancelled {
            writeln!(out, "Your pending deletion request has been canceled.")?;
        }
        if self.recovery {
            writeln!(out, "{RECOVERY}")?;
        }
        Ok(())
    }
}

fn apply_response<E>(
    json: &Value,
    persist: impl FnOnce(String, String, i32) -> Result<(), E>,
) -> Outcome {
    let status = json
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !status.eq_ignore_ascii_case("success") {
        return Outcome::failure(
            if status.eq_ignore_ascii_case("failure") || status.eq_ignore_ascii_case("error") {
                "Confirmation was not accepted. Check the latest code and configured account email. If the code is invalid or expired, request a fresh code through account registration with the existing consent."
            } else {
                UNKNOWN
            },
        );
    }
    let mut outcome = Outcome {
        success: false,
        message: "The account was confirmed, but the service returned invalid or missing required credentials; local setup is incomplete.",
        recovery: true,
        reactivated: json.get("reactivated").and_then(Value::as_bool) == Some(true),
        deletion_cancelled: json.get("deletion_cancelled").and_then(Value::as_bool) == Some(true),
    };
    let credentials = &json["credentials"];
    let token = |key| {
        credentials
            .get(key)
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
    };
    let expiry = credentials
        .get("expires_in")
        .and_then(Value::as_i64)
        .and_then(|value| i32::try_from(value).ok())
        .filter(|value| *value > 0);
    if let (Some(access), Some(refresh), Some(expiry)) =
        (token("access_token"), token("refresh_token"), expiry)
    {
        if persist(access.to_owned(), refresh.to_owned(), expiry).is_ok() {
            outcome.success = true;
            outcome.message =
                "Account confirmed. Required credentials and local account metadata saved.";
            outcome.recovery = false;
        } else {
            outcome.message = "The account was confirmed, but required credentials or local account metadata could not be saved; local setup is incomplete.";
        }
    }
    outcome
}

async fn confirm_with<E>(
    base_url: &str,
    email: &str,
    code: &str,
    persist: impl FnOnce(String, String, i32) -> Result<(), E>,
) -> Outcome {
    match infra_api::account::confirm::confirm_email(base_url, email, code).await {
        Ok(json) => apply_response(&json, persist),
        Err(_) => Outcome::failure(UNKNOWN),
    }
}

/// Confirms a registration code and reports success only after persistence.
pub async fn run(conf_m: &ArgMatches) -> bool {
    let code = if conf_m.get_flag("stdin") {
        match secret_input::read_stdin(secret_input::CONFIRMATION_LIMIT) {
            Ok(code) => code,
            Err(message) => {
                eprintln!("x {message}");
                return false;
            }
        }
    } else if let Some(code) = conf_m.get_one::<String>("code") {
        code.clone()
    } else {
        eprintln!("x Supply exactly one confirmation source: CODE or --stdin.");
        return false;
    };
    let Some(email) = load_config()
        .and_then(|cfg| cfg.account)
        .and_then(|account| account.email)
    else {
        eprintln!("x No configured account email is available. Check the selected CARGO_AI_HOME and config, or register the account first.");
        return false;
    };
    if crate::credentials::migration::run_legacy_credential_migration().is_err() {
        eprintln!("x Could not prepare existing credentials. Check the selected home, config and credential-store access; no confirmation request was sent.");
        return false;
    }
    let outcome = confirm_with(INFRA_BASE_URL, &email, &code, set_account_tokens).await;
    outcome
        .write(&mut io::stdout().lock(), &mut io::stderr().lock())
        .is_ok()
        && outcome.success
}

#[cfg(test)]
mod secret_input_tests {
    use super::*;
    use crate::commands::secret_input::test_support::Home;
    use crate::credentials::store;
    use serde_json::json;
    use std::fs;

    const CODE: &str = "synthetic-confirm-code";
    const ACCESS: &str = "synthetic-access-secret";
    const REFRESH: &str = "synthetic-refresh-secret";
    const ID: &str = "synthetic-id-secret";

    fn response() -> Value {
        json!({"status":"success", "reactivated":true, "deletion_cancelled":true,
            "credentials":{"access_token":ACCESS,"refresh_token":REFRESH,"id_token":ID,"expires_in":3600},
            "ui":{"title":CODE,"summary":ACCESS}, "message":REFRESH})
    }
    fn captured(outcome: &Outcome) -> (String, String) {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        outcome.write(&mut out, &mut err).unwrap();
        let out = String::from_utf8(out).unwrap();
        let err = String::from_utf8(err).unwrap();
        for secret in [CODE, ACCESS, REFRESH, ID] {
            assert!(
                !out.contains(secret) && !err.contains(secret),
                "credential reached output"
            );
        }
        if outcome.success {
            assert!(err.is_empty());
        } else {
            assert!(out.is_empty());
        }
        (out, err)
    }

    #[test]
    fn secret_input_rejects_malformed_credentials_and_untrusted_backend_text() {
        for (key, value) in [
            ("access_token", json!(null)),
            ("access_token", json!(" ")),
            ("refresh_token", json!(false)),
            ("refresh_token", json!("")),
            ("expires_in", json!(0)),
            ("expires_in", json!(-1)),
            ("expires_in", json!(i64::MAX)),
            ("expires_in", json!("3600")),
        ] {
            let mut data = response();
            data["credentials"][key] = value;
            let result = apply_response(&data, |_, _, _| -> Result<(), ()> {
                panic!("invalid credentials must not be saved")
            });
            assert!(!result.success);
            assert!(captured(&result).1.contains("local setup is incomplete"));
        }
        for status in ["failure", "error", CODE] {
            let result = apply_response(
                &json!({"status":status,"message":CODE,"credentials":response()["credentials"],"ui":{"summary":ACCESS}}),
                |_, _, _| -> Result<(), ()> { panic!("failed response must not be saved") },
            );
            assert!(!result.success);
            captured(&result);
        }
        let result = apply_response(&response(), |_, _, _| Err(format!("{ACCESS} {REFRESH}")));
        assert!(!result.success);
        assert!(captured(&result).1.contains("account was confirmed"));
    }

    #[tokio::test]
    async fn secret_input_confirms_persists_and_uses_saved_credentials_for_status() {
        let home = Home::new();
        store::store_profile_token("unrelated", "synthetic-unrelated").unwrap();
        fs::write(home.path.join("sentinel"), b"preserve-local-state").unwrap();
        let mut server = mockito::Server::new_async().await;
        let confirmation = server
            .mock("POST", "/account")
            .match_body(mockito::Matcher::PartialJson(
                json!({"action":"confirm","email":"owner@example.test","code":CODE}),
            ))
            .with_status(200)
            .with_body(response().to_string())
            .expect(1)
            .create_async()
            .await;
        let result = confirm_with(
            &server.url(),
            "owner@example.test",
            CODE,
            set_account_tokens,
        )
        .await;
        assert!(result.success);
        let (out, _) = captured(&result);
        assert!(
            out.contains("Account reactivated.")
                && out.contains("pending deletion request has been canceled")
        );
        confirmation.assert_async().await;
        let auth = super::super::helpers::load_account_auth().unwrap();
        assert!(auth.access_token == ACCESS && auth.refresh_token.as_deref() == Some(REFRESH));
        let cfg = load_config().unwrap();
        let account = cfg.account.unwrap();
        assert_eq!(account.access_token_expires_in, Some(3600));
        assert!(account.access_token_issued_at.unwrap() > 0);
        let raw = fs::read_to_string(home.path.join("config.toml")).unwrap();
        for secret in [CODE, ACCESS, REFRESH, ID] {
            assert!(!raw.contains(secret));
        }
        let status = server.mock("POST", "/account")
            .match_body(mockito::Matcher::PartialJson(json!({"action":"status","credentials":{"access_token":ACCESS,"refresh_token":REFRESH}})))
            .with_status(200).with_body(r#"{"status":"success","account":{"status":"active"}}"#).expect(1).create_async().await;
        let result = infra_api::account::status::fetch_status(
            &server.url(),
            &auth.access_token,
            auth.refresh_token.as_deref(),
        )
        .await
        .unwrap();
        assert_eq!(result["status"], "success");
        status.assert_async().await;
        assert!(
            store::load_profile_token("unrelated").unwrap().as_deref()
                == Some("synthetic-unrelated")
        );
        assert_eq!(
            fs::read(home.path.join("sentinel")).unwrap(),
            b"preserve-local-state"
        );
    }

    #[test]
    fn secret_input_persistence_failure_never_completes_setup() {
        let home = Home::new();
        fs::create_dir(home.path.join("credentials.toml")).unwrap();
        let result = apply_response(&response(), set_account_tokens);
        assert!(!result.success);
        captured(&result);
        fs::remove_dir(home.path.join("credentials.toml")).unwrap();
        let original = fs::read(home.path.join("config.toml")).unwrap();
        // Inject the metadata-stage failure after the real credential store succeeds.
        let result = apply_response(&response(), |access, refresh, _| {
            store::store_account_tokens(&access, Some(&refresh)).unwrap();
            Err(format!("metadata write failed: {ACCESS} {REFRESH}"))
        });
        assert!(!result.success);
        assert!(captured(&result).1.contains("local setup is incomplete"));
        assert!(store::load_account_tokens().unwrap().unwrap().access_token == ACCESS);
        assert_eq!(fs::read(home.path.join("config.toml")).unwrap(), original);
    }

    #[tokio::test]
    async fn secret_input_http_failures_are_redacted_and_never_retried() {
        let _home = Home::new();
        let mut server = mockito::Server::new_async().await;
        for (status, body) in [
            (
                400,
                json!({"status":"failure","error":CODE,"message":ACCESS}).to_string(),
            ),
            (200, ACCESS.to_owned()),
            (503, REFRESH.to_owned()),
        ] {
            let mock = server
                .mock("POST", "/account")
                .with_status(status)
                .with_body(body)
                .expect(1)
                .create_async()
                .await;
            let result = confirm_with(
                &server.url(),
                "owner@example.test",
                CODE,
                |_, _, _| -> Result<(), ()> { panic!("error response must not be persisted") },
            )
            .await;
            assert!(!result.success);
            captured(&result);
            mock.assert_async().await;
            mock.remove_async().await;
        }
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        drop(listener);
        let result = confirm_with(
            &url,
            "owner@example.test",
            CODE,
            |_, _, _| -> Result<(), ()> { panic!("transport failure must not persist") },
        )
        .await;
        assert!(!result.success);
        assert!(captured(&result).1.contains("do not automatically retry"));
    }
}
