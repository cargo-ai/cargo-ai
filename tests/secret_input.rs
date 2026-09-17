//! Synthetic process checks for secret input; no hosted account is configured.
use std::{
    fs,
    io::{ErrorKind, Write},
    path::PathBuf,
    process::{Command, Output, Stdio},
    sync::atomic::{AtomicU64, Ordering},
};

const SECRET: &str = "synthetic-secret-input-sentinel";
const STORED: &str = "unrelated-provider-sentinel";
static NEXT: AtomicU64 = AtomicU64::new(0);

struct Home(PathBuf);
impl Home {
    fn new() -> Self {
        let home = Self(std::env::temp_dir().join(format!(
            "cargo-ai-secret-input-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        )));
        fs::create_dir(&home.0).unwrap();
        fs::write(home.0.join("config.toml"), "secret_store = 'file'\n[[profile]]\nname = 'example'\nserver = 'openai'\nmodel = 'synthetic-model'\nauth_mode = 'api_key'\n").unwrap();
        fs::write(home.0.join("credentials.toml"), format!("[profile_tokens]\nunrelated = '{STORED}'\n[openai_oauth]\naccess_token = 'unrelated-oauth-sentinel'\nrefresh_token = 'unrelated-refresh-sentinel'\n")).unwrap();
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
        fs::write(home.0.join("project.json"), b"project-sentinel").unwrap();
        fs::write(
            home.0.join("installed-app-state"),
            b"installed-app-sentinel",
        )
        .unwrap();
        // Let ordinary startup initialize its own metadata before preservation checks.
        assert!(home.run(&["version"], b"").status.success());
        home
    }

    fn run(&self, args: &[&str], input: &[u8]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cargo-ai"));
        command
            .arg("--no-update-check")
            .args(args)
            .current_dir(&self.0)
            .env("CARGO_AI_HOME", &self.0)
            .env("CARGO_AI_DISABLE_KEYCHAIN", "1")
            .env("CARGO_AI_SYNTHETIC_TOKEN", SECRET)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if !input.is_empty() {
            // Inspect the exact argv passed to the child, not a shell command string.
            assert!(command
                .get_args()
                .all(|arg| !arg.to_string_lossy().contains(SECRET)));
        }
        let mut child = command.spawn().expect("spawn isolated CLI directly");
        let mut stdin = child.stdin.take().unwrap();
        if let Err(error) = stdin.write_all(input) {
            // A parser or bounded reader may reject input before consuming the pipe.
            assert_eq!(error.kind(), ErrorKind::BrokenPipe);
        }
        drop(stdin);
        let output = child.wait_with_output().expect("wait for isolated CLI");
        let text = output_text(&output);
        for secret in [
            SECRET,
            STORED,
            "unrelated-oauth-sentinel",
            "unrelated-refresh-sentinel",
        ] {
            assert!(
                !text.contains(secret),
                "CLI output disclosed a synthetic secret"
            );
        }
        output
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

    fn credentials(&self) -> toml::Value {
        toml::from_str(&fs::read_to_string(self.0.join("credentials.toml")).unwrap()).unwrap()
    }

    fn assert_unrelated_preserved(&self, before: &[Vec<u8>], credentials: &toml::Value) {
        assert_eq!(fs::read(self.0.join("config.toml")).unwrap(), before[0]);
        assert_eq!(fs::read(self.0.join("project.json")).unwrap(), before[2]);
        assert_eq!(
            fs::read(self.0.join("installed-app-state")).unwrap(),
            before[3]
        );
        let after = self.credentials();
        assert_eq!(
            after["profile_tokens"]["unrelated"],
            credentials["profile_tokens"]["unrelated"]
        );
        assert_eq!(after["openai_oauth"], credentials["openai_oauth"]);
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
fn parser_rejects_conflicting_missing_and_misplaced_sources_without_disclosure() {
    let home = Home::new();
    let before = home.preserved_files();
    for args in [
        vec!["account", "confirm"],
        vec!["account", "confirm", SECRET, "--stdin"],
        vec!["account", "confirm", "--wrong", SECRET],
        vec!["account", "confirm", SECRET, "extra"],
        vec!["profile", "set", "example", "--stdin", "--token", SECRET],
        vec![
            "profile",
            "set",
            "example",
            "--stdin",
            "--env",
            "CARGO_AI_SYNTHETIC_TOKEN",
        ],
        vec!["profile", "set", "example", "--stdin", "--clear-token"],
        vec![
            "profile",
            "set",
            "example",
            "--token",
            SECRET,
            "--env",
            "CARGO_AI_SYNTHETIC_TOKEN",
        ],
        vec![
            "profile",
            "set",
            "example",
            "--token",
            SECRET,
            "--clear-token",
        ],
        vec![
            "profile",
            "set",
            "example",
            "--env",
            "CARGO_AI_SYNTHETIC_TOKEN",
            "--clear-token",
        ],
        vec!["profile", "set", "example", "--wrong", SECRET],
        vec!["profile", "set", "example", SECRET],
        vec!["profile", "set", "example", "--auth", SECRET],
        vec!["profile", "set", "example", "--temperature", SECRET],
        vec!["profile", "set", "example", "--max-output-tokens", SECRET],
    ] {
        let output = home.run(&args, b"");
        assert_eq!(output.status.code(), Some(2), "{}", output_text(&output));
        assert_eq!(home.preserved_files(), before);
    }
}

#[test]
fn malformed_stdin_fails_before_mutating_local_state() {
    let home = Home::new();
    let before = home.preserved_files();
    for (args, limit) in [
        (vec!["account", "confirm", "--stdin"], 1024),
        (vec!["profile", "set", "example", "--stdin"], 16384),
    ] {
        for input in [
            Vec::new(),
            b" \t\r\n".to_vec(),
            format!("{SECRET}\nsecond-line").into_bytes(),
            format!("{SECRET}\0").into_bytes(),
            [SECRET.as_bytes(), &[0xff]].concat(),
            [SECRET.as_bytes(), &vec![b'x'; limit + 1]].concat(),
        ] {
            let output = home.run(&args, &input);
            assert_eq!(output.status.code(), Some(1), "{}", output_text(&output));
            assert!(output_text(&output).to_ascii_lowercase().contains("secret"));
            assert_eq!(home.preserved_files(), before);
        }
    }
}

#[test]
fn confirmation_sources_reach_the_same_local_missing_email_guard() {
    let home = Home::new();
    let before = home.preserved_files();
    for (args, input) in [
        (vec!["account", "confirm", SECRET], Vec::new()),
        (
            vec!["account", "confirm", "--stdin"],
            format!(" \t{SECRET}\r\n").into_bytes(),
        ),
    ] {
        let output = home.run(&args, &input);
        assert_eq!(output.status.code(), Some(1));
        assert!(output_text(&output).contains("No configured account email"));
        assert_eq!(home.preserved_files(), before);
    }
}

#[test]
fn profile_sources_persist_and_clear_only_the_selected_token() {
    let home = Home::new();
    let before = home.preserved_files();
    let credentials = home.credentials();
    for (args, input) in [
        (
            vec!["profile", "set", "example", "--stdin"],
            SECRET.as_bytes().to_vec(),
        ),
        (
            vec!["profile", "set", "example", "--stdin"],
            format!(" \t{SECRET}\n").into_bytes(),
        ),
        (
            vec!["profile", "set", "example", "--stdin"],
            format!("{SECRET}\r\n").into_bytes(),
        ),
        (
            vec![
                "profile",
                "set",
                "example",
                "--env",
                "CARGO_AI_SYNTHETIC_TOKEN",
            ],
            Vec::new(),
        ),
        (
            vec!["profile", "set", "example", "--token", SECRET],
            Vec::new(),
        ),
    ] {
        let output = home.run(&args, &input);
        assert!(output.status.success(), "{}", output_text(&output));
        assert_eq!(
            home.credentials()["profile_tokens"]["example"].as_str(),
            Some(SECRET)
        );
        home.assert_unrelated_preserved(&before, &credentials);
        let cleared = home.run(&["profile", "set", "example", "--clear-token"], b"");
        assert!(cleared.status.success(), "{}", output_text(&cleared));
        assert!(home.credentials()["profile_tokens"]
            .get("example")
            .is_none());
        home.assert_unrelated_preserved(&before, &credentials);
    }
}

#[test]
fn profile_metadata_updates_preserve_credentials_and_unrelated_files() {
    let home = Home::new();
    let before = home.preserved_files();
    let output = home.run(
        &[
            "profile",
            "set",
            "example",
            "--description",
            "Updated metadata",
        ],
        b"",
    );
    assert!(output.status.success(), "{}", output_text(&output));
    let after = home.preserved_files();
    assert_eq!(&after[1..], &before[1..]);
    let config: toml::Value =
        toml::from_str(&String::from_utf8(after[0].clone()).unwrap()).unwrap();
    assert_eq!(
        config["profile"][0]["description"].as_str(),
        Some("Updated metadata")
    );
}

#[test]
fn profile_store_failures_exit_nonzero_without_claiming_success() {
    for corrupt in [true, false] {
        let home = Home::new();
        let path = home.0.join("credentials.toml");
        let malformed = format!("[profile_tokens]\nexample = '{SECRET}\n");
        if corrupt {
            fs::write(&path, &malformed).unwrap();
        } else {
            fs::remove_file(&path).unwrap();
            fs::create_dir(&path).unwrap();
            fs::write(path.join("sentinel"), b"preserve-directory").unwrap();
        }
        let config_before = fs::read(home.0.join("config.toml")).unwrap();
        let output = home.run(&["profile", "set", "example", "--stdin"], SECRET.as_bytes());
        assert_eq!(output.status.code(), Some(1));
        let text = output_text(&output);
        assert!(text.contains("Failed to store the profile token"), "{text}");
        assert!(!text.contains("Profile updated"));
        assert_eq!(fs::read(home.0.join("config.toml")).unwrap(), config_before);
        if corrupt {
            assert_eq!(fs::read_to_string(path).unwrap(), malformed);
        } else {
            assert_eq!(
                fs::read(path.join("sentinel")).unwrap(),
                b"preserve-directory"
            );
        }
    }
}

#[test]
fn secret_input_validation_precedes_legacy_migration_but_valid_updates_preserve_it() {
    let home = Home::new();
    let config_path = home.0.join("config.toml");
    let config = fs::read_to_string(&config_path).unwrap();
    let mut config: toml::Value = toml::from_str(&config).unwrap();
    config["profile"][0]
        .as_table_mut()
        .unwrap()
        .insert("token".to_owned(), toml::Value::String(STORED.to_owned()));
    fs::write(&config_path, toml::to_string(&config).unwrap()).unwrap();
    let before = home.preserved_files();
    for args in [
        vec!["account", "confirm", "--stdin"],
        vec!["profile", "set", "example", "--stdin"],
    ] {
        let result = home.run(&args, b"\n");
        assert!(!result.status.success());
        assert_eq!(home.preserved_files(), before);
    }
    let result = home.run(&["profile", "set", "example", "--default"], b"");
    assert!(result.status.success());
    let config = fs::read_to_string(&config_path).unwrap();
    assert!(!config.contains(STORED));
    assert!(home.credentials()["profile_tokens"]["example"].as_str() == Some(STORED));
}

#[test]
fn secret_input_help_describes_limits_without_reflecting_supplied_values() {
    let home = Home::new();
    let before = home.preserved_files();
    for (args, limit) in [
        (vec!["account", "confirm", "--stdin", "--help"], "1024"),
        (
            vec!["profile", "set", "example", "--token", SECRET, "--help"],
            "16384",
        ),
    ] {
        let output = home.run(&args, b"");
        assert!(output.status.success());
        assert!(output_text(&output).contains(limit));
        assert_eq!(home.preserved_files(), before);
    }
}
