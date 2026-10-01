//! Discovery fixtures use disposable homes and synthetic loopback servers only.
use serde_json::Value;
use std::{
    fs,
    io::{ErrorKind, Write},
    path::PathBuf,
    process::{Command, Output, Stdio},
};

struct Home(PathBuf);
impl Home {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("cargo-ai-discovery-cli-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        fs::write(root.join("sentinel"), "application-state").unwrap();
        Self(root)
    }
    fn configure(&self, mode: Option<&str>, auth: &str, url: &str, token: &str) {
        let mode = mode
            .map(|mode| format!("secret_store='{mode}'\n"))
            .unwrap_or_default();
        fs::write(self.0.join("config.toml"), format!("{mode}default_profile='fixture'\n[[profile]]\nname='fixture'\nserver='ollama'\nmodel='obsolete-never-invoked'\nauth_mode='{auth}'\nurl='{url}/v1/chat/completions'\ntoken='legacy-inline-never-migrate'\n")).unwrap();
        fs::write(
            self.0.join("credentials.toml"),
            format!("[profile_tokens]\nfixture='{token}'\n"),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&self.0, fs::Permissions::from_mode(0o700)).unwrap();
            fs::set_permissions(
                self.0.join("credentials.toml"),
                fs::Permissions::from_mode(0o600),
            )
            .unwrap();
        }
    }
    fn run(&self, args: &[&str], input: &[u8]) -> Output {
        let mut child = Command::new(env!("CARGO_BIN_EXE_cargo-ai"))
            .arg("--no-update-check")
            .args(args)
            .current_dir(&self.0)
            .env("CARGO_AI_HOME", &self.0)
            .env("CARGO_AI_DISABLE_KEYCHAIN", "1")
            .env("CODEX_HOME", &self.0)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        if let Err(error) = child.stdin.take().unwrap().write_all(input) {
            assert_eq!(error.kind(), ErrorKind::BrokenPipe);
        }
        child.wait_with_output().unwrap()
    }
    fn snapshot(&self) -> Vec<(String, Vec<u8>)> {
        let mut files: Vec<_> = fs::read_dir(&self.0)
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                (
                    entry.file_name().to_string_lossy().into_owned(),
                    fs::read(entry.path()).unwrap(),
                )
            })
            .collect();
        files.sort_by(|a, b| a.0.cmp(&b.0));
        files
    }
}
impl Drop for Home {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn body(output: &Output) -> Value {
    assert!(
        output.stderr.is_empty(),
        "unexpected stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}
const FORMAT: &[&str] = &["--output-format", "json", "--output-schema-version", "1"];
fn args(base: &[&'static str]) -> Vec<&'static str> {
    [base, FORMAT].concat()
}

#[tokio::test]
async fn saved_discovery_is_home_scoped_read_only_and_ignores_model() {
    let mut server = mockito::Server::new_async().await;
    for token in ["synthetic-first-key", "synthetic-second-key"] {
        let home = Home::new();
        home.configure(Some("file"), "api_key", &server.url(), token);
        let before = home.snapshot();
        let mock = server
            .mock("GET", "/api/tags")
            .match_header("authorization", format!("Bearer {token}").as_str())
            .with_status(200)
            .with_body(r#"{"models":[{"name":"custom:exact"}]}"#)
            .create_async()
            .await;
        let output = home.run(&args(&["models", "list", "--profile", "fixture"]), b"");
        assert!(output.status.success());
        let response = body(&output);
        assert_eq!(response["data"]["models"][0]["id"], "custom:exact");
        assert_eq!(response["data"]["complete"], true);
        assert!(response["data"].get("compatibility").is_none());
        assert!(!String::from_utf8_lossy(&output.stdout).contains(token));
        assert_eq!(home.snapshot(), before);
        mock.assert_async().await;
    }
}

#[tokio::test]
async fn discovery_rejects_unscoped_stores_and_no_auth_never_reads_credentials() {
    let mut server = mockito::Server::new_async().await;
    for mode in [None, Some("keychain")] {
        let home = Home::new();
        home.configure(mode, "api_key", &server.url(), "unrelated-key");
        let before = home.snapshot();
        let output = home.run(&args(&["models", "list", "--profile", "fixture"]), b"");
        assert!(!output.status.success());
        assert_eq!(
            body(&output)["error"]["code"],
            "discovery.unsupported_secret_store"
        );
        assert_eq!(home.snapshot(), before);
    }
    let home = Home::new();
    home.configure(Some("keychain"), "none", &server.url(), "unused");
    fs::write(
        home.0.join("credentials.toml"),
        "malformed credential sentinel",
    )
    .unwrap();
    let before = home.snapshot();
    let mock = server
        .mock("GET", "/api/tags")
        .match_header("authorization", mockito::Matcher::Missing)
        .with_status(200)
        .with_body(r#"{"models":[]}"#)
        .create_async()
        .await;
    let output = home.run(&args(&["models", "list", "--profile", "fixture"]), b"");
    assert!(output.status.success());
    assert_eq!(body(&output)["data"]["models"], serde_json::json!([]));
    assert!(body(&output)["data"].get("compatibility").is_none());
    assert_eq!(home.snapshot(), before);
    mock.assert_async().await;
}

#[tokio::test]
async fn draft_discovery_consumes_bounded_stdin_and_never_creates_home() {
    let mut server = mockito::Server::new_async().await;
    let home = Home::new();
    let selected = home.0.join("absent");
    let mock = server
        .mock("GET", "/api/tags")
        .match_header("authorization", "Bearer synthetic-draft-key")
        .with_status(200)
        .with_body(r#"{"models":[]}"#)
        .expect(1)
        .create_async()
        .await;
    let mut command = Command::new(env!("CARGO_BIN_EXE_cargo-ai"));
    command
        .args([
            "models",
            "list",
            "--server",
            "ollama",
            "--auth",
            "api_key",
            "--stdin",
            "--url",
            &format!("{}/api/tags", server.url()),
        ])
        .args(FORMAT)
        .env("CARGO_AI_HOME", &selected)
        .env("CARGO_AI_DISABLE_KEYCHAIN", "1")
        .env("CODEX_HOME", &home.0)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"synthetic-draft-key\n")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    assert!(!selected.exists());
    assert!(!String::from_utf8_lossy(&output.stdout).contains("synthetic-draft-key"));
    mock.assert_async().await;
    for input in [
        vec![b'x'; 16 * 1024 + 1],
        b"one\nsecond".to_vec(),
        vec![0xff],
        vec![],
    ] {
        let output = home.run(
            &[
                "models",
                "list",
                "--server",
                "ollama",
                "--auth",
                "api_key",
                "--stdin",
                "--url",
                &format!("{}/api/tags", server.url()),
                "--output-format",
                "json",
            ],
            &input,
        );
        assert!(!output.status.success());
        assert_eq!(
            body(&output)["error"]["code"],
            "discovery.invalid_credentials"
        );
    }
}

#[cfg(unix)]
#[test]
fn cancellation_stops_pending_catalog_and_preserves_home() {
    use std::{io::Read, net::TcpListener, sync::mpsc, time::Duration};
    let home = Home::new();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let (sent, received) = mpsc::channel();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut request = [0u8; 4096];
        let size = stream.read(&mut request).unwrap();
        assert!(request[..size].starts_with(b"GET /api/tags HTTP/1.1"));
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 1000\r\n\r\n{")
            .unwrap();
        sent.send(()).unwrap();
        let mut byte = [0u8; 1];
        // Dropping the pending request closes this stream without another request.
        assert!(stream.read(&mut byte).is_err() || byte[0] == 0);
    });
    home.configure(
        Some("file"),
        "none",
        &format!("http://{address}"),
        "unused-key",
    );
    let before = home.snapshot();
    let child = Command::new(env!("CARGO_BIN_EXE_cargo-ai"))
        .args(args(&["models", "list", "--profile", "fixture"]))
        .env("CARGO_AI_HOME", &home.0)
        .env("CARGO_AI_DISABLE_KEYCHAIN", "1")
        .env("CODEX_HOME", &home.0)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    received.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(Command::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status()
        .unwrap()
        .success());
    let output = child.wait_with_output().unwrap();
    assert!(!output.status.success());
    let response = body(&output);
    assert_eq!(response["outcome"], "canceled");
    assert_eq!(home.snapshot(), before);
    server.join().unwrap();
}

fn account_config(home: &Home, disabled: bool) {
    fs::write(home.0.join("config.toml"), format!("default_profile='fixture'\n[[profile]]\nname='fixture'\nserver='openai'\nmodel='invalid-placeholder-never-invoked'\nauth_mode='openai_account'\n[openai_auth]\nlocally_disabled={disabled}\n")).unwrap();
}
fn synthetic_jwt(claims: Value) -> String {
    use base64::Engine;
    format!(
        "e30.{}.synthetic-signature",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(claims.to_string())
    )
}
#[test]
fn account_discovery_missing_expired_invalid_and_local_logout_are_safe_and_read_only() {
    let home = Home::new();
    account_config(&home, false);
    let commands = [
        args(&["models", "list", "--profile", "fixture"]),
        args(&[
            "models",
            "list",
            "--server",
            "openai",
            "--auth",
            "openai_account",
        ]),
    ];
    for command in &commands {
        let before = home.snapshot();
        let output = home.run(command, b"");
        assert_eq!(
            body(&output)["error"]["code"],
            "discovery.credentials_required"
        );
        assert_eq!(home.snapshot(), before);
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let expired = serde_json::json!({"auth_mode":"chatgpt","tokens":{
        "access_token":synthetic_jwt(serde_json::json!({"exp":now-1})),
        "id_token":synthetic_jwt(serde_json::json!({"https://api.openai.com/auth":{"chatgpt_account_id":"synthetic-cli-context"}})),
        "account_id":"synthetic-cli-context"}}).to_string();
    for (raw, code) in [
        (expired.as_str(), "discovery.credentials_expired"),
        (
            "{malformed-synthetic-private-auth",
            "discovery.invalid_credentials",
        ),
        (
            "{\"tokens\":{\"access_token\":\"synthetic-token\"}}",
            "discovery.unsupported_connection",
        ),
    ] {
        fs::write(home.0.join("auth.json"), raw).unwrap();
        for command in &commands {
            let before = home.snapshot();
            let output = home.run(command, b"");
            assert_eq!(body(&output)["error"]["code"], code);
            for secret in [
                "synthetic-cli-context",
                "synthetic-token",
                "malformed-synthetic-private-auth",
            ] {
                assert!(!String::from_utf8_lossy(&output.stdout).contains(secret));
                assert!(!String::from_utf8_lossy(&output.stderr).contains(secret));
            }
            assert_eq!(home.snapshot(), before);
        }
    }
    account_config(&home, true);
    for command in &commands {
        let before = home.snapshot();
        let output = home.run(command, b"");
        assert_eq!(body(&output)["error"]["code"], "discovery.locally_disabled");
        assert_eq!(home.snapshot(), before);
    }
}
#[test]
fn account_draft_selectors_reject_unsafe_routes_before_auth_and_do_not_initialize_home() {
    let home = Home::new();
    fs::write(home.0.join("auth.json"), "synthetic-auth-must-not-be-read").unwrap();
    for options in [
        vec!["--server", "ollama", "--auth", "openai_account"],
        vec![
            "--server",
            "openai",
            "--auth",
            "openai_account",
            "--url",
            "https://fixture.invalid/backend-api/codex/responses",
        ],
        vec![
            "--server",
            "openai",
            "--auth",
            "openai_account",
            "--url",
            "https://chatgpt.com/backend-api/codex/responses?secret=sentinel",
        ],
    ] {
        let output = home.run(
            &[vec!["models", "list"], options, FORMAT.to_vec()].concat(),
            b"",
        );
        assert_eq!(
            body(&output)["error"]["code"],
            "discovery.unsupported_connection"
        );
    }
    let output = home.run(
        &args(&[
            "models",
            "list",
            "--server",
            "openai",
            "--auth",
            "openai_account",
            "--stdin",
        ]),
        b"synthetic-private-stdin",
    );
    assert_eq!(body(&output)["error"]["code"], "cli.invalid_input");
    let selected = home.0.join("absent-account-home");
    let output = Command::new(env!("CARGO_BIN_EXE_cargo-ai"))
        .args(args(&[
            "models",
            "list",
            "--server",
            "openai",
            "--auth",
            "openai_account",
        ]))
        .env("CARGO_AI_HOME", &selected)
        .env("CODEX_HOME", &home.0)
        .env("CARGO_AI_DISABLE_KEYCHAIN", "1")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(
        body(&output)["error"]["code"],
        "discovery.invalid_credentials"
    );
    assert!(!selected.exists());
}
