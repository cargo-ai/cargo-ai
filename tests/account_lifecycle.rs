//! Account consent boundaries run without contacting hosted services.
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Home(PathBuf);
impl Home {
    fn new(with_credentials: bool) -> Self {
        let home = Self(std::env::temp_dir().join(format!(
            "cargo-ai-lifecycle-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        )));
        fs::create_dir(&home.0).expect("create isolated home");
        fs::write(
            home.0.join("config.toml"),
            "profile = []\nsecret_store = 'file'\n[account]\nemail = 'owner@example.test'\n",
        )
        .unwrap();
        let account = if with_credentials {
            "\n[account]\naccess_token = 'synthetic-hosted-token'\nrefresh_token = 'synthetic-hosted-refresh'\n"
        } else {
            ""
        };
        fs::write(home.0.join("credentials.toml"), format!("[profile_tokens]\nexample = 'provider-sentinel'\n[openai_oauth]\naccess_token = 'oauth-sentinel'\nrefresh_token = 'oauth-refresh-sentinel'\n{account}")).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&home.0, fs::Permissions::from_mode(0o700)).unwrap();
            fs::set_permissions(
                home.0.join("credentials.toml"),
                fs::Permissions::from_mode(0o600),
            )
            .unwrap();
        }
        fs::write(home.0.join("project.json"), b"local-project-sentinel").unwrap();
        fs::write(
            home.0.join("installed-app-state"),
            b"installed-app-sentinel",
        )
        .unwrap();
        assert!(home.run(&["version"]).status.success());
        home
    }
    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_cargo-ai"))
            .arg("--no-update-check")
            .args(args)
            .current_dir(&self.0)
            .env("CARGO_AI_HOME", &self.0)
            .env("CARGO_AI_DISABLE_KEYCHAIN", "1")
            .output()
            .expect("run isolated CLI")
    }
    fn preserved_files(&self) -> Vec<Vec<u8>> {
        [
            "config.toml",
            "credentials.toml",
            "project.json",
            "installed-app-state",
        ]
        .iter()
        .map(|name| fs::read(self.0.join(name)).unwrap())
        .collect()
    }
}
impl Drop for Home {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn output_text(output: &Output) -> String {
    format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn noninteractive_deactivation_requires_explicit_consent_and_preserves_local_state() {
    let home = Home::new(true);
    let before = home.preserved_files();
    for args in [
        vec!["account", "deactivate"],
        vec!["account", "deactivate", "--request-deletion"],
        vec!["account", "deactivate", "--request-deletion", "--yes"],
    ] {
        let output = home.run(&args);
        assert_eq!(output.status.code(), Some(1), "{}", output_text(&output));
        assert!(output_text(&output).contains("Noninteractive deactivation requires"));
        assert_eq!(home.preserved_files(), before);
    }
}

#[test]
fn deletion_email_is_confirmation_and_cannot_select_another_account() {
    let home = Home::new(true);
    let before = home.preserved_files();
    let output = home.run(&[
        "account",
        "deactivate",
        "--request-deletion",
        "--confirm-email",
        "someone-else@example.test",
    ]);
    assert_eq!(output.status.code(), Some(1));
    let text = output_text(&output);
    assert!(text.contains("No deactivation was requested."), "{text}");
    assert!(!text.contains("synthetic-hosted-token"));
    assert!(!text.contains("provider-sentinel"));
    assert_eq!(home.preserved_files(), before);
}

#[test]
fn explicit_confirmation_still_requires_hosted_authentication() {
    let home = Home::new(false);
    let before = home.preserved_files();
    for args in [
        vec!["account", "deactivate", "--yes"],
        vec![
            "account",
            "deactivate",
            "--request-deletion",
            "--confirm-email",
            "owner@example.test",
        ],
    ] {
        let output = home.run(&args);
        assert_eq!(output.status.code(), Some(1));
        assert!(
            output_text(&output).contains("No access token found"),
            "{}",
            output_text(&output)
        );
        assert_eq!(home.preserved_files(), before);
    }
}

#[test]
fn registration_discloses_cancellation_and_refuses_unconfirmed_noninteractive_recovery() {
    let home = Home::new(false);
    let before = home.preserved_files();
    let output = home.run(&["account", "register", "owner@example.test"]);
    assert_eq!(output.status.code(), Some(1));
    let text = output_text(&output);
    assert!(text.contains("cancel pending deletion"), "{text}");
    assert!(
        text.contains("Noninteractive account changes require explicit confirmation"),
        "{text}"
    );
    assert_eq!(home.preserved_files(), before);
}

#[test]
fn deactivation_help_explains_manual_deletion_and_confirmation_flags() {
    let home = Home::new(false);
    let output = home.run(&["account", "deactivate", "--help"]);
    assert!(output.status.success());
    let text = output_text(&output);
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    for expected in [
        "--request-deletion",
        "--confirm-email",
        "--yes",
        "retention policy",
        "provider credentials",
        "no universal expiry deadline",
        "exclude completed deletions before returning to service",
    ] {
        assert!(normalized.contains(expected), "Missing {expected}: {text}");
    }
    let invalid = home.run(&[
        "account",
        "deactivate",
        "--confirm-email",
        "owner@example.test",
    ]);
    assert_eq!(invalid.status.code(), Some(2));
}
