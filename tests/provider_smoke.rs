//! Reusable process-level provider smoke coverage.

use serde_json::Value;
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

static NEXT_FIXTURE_ID: AtomicU64 = AtomicU64::new(0);
const TEST_TOKEN: &str = "anthropic-provider-smoke-token";

struct Fixture {
    root: PathBuf,
    home: PathBuf,
    definition: PathBuf,
    image: PathBuf,
    usage: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let sequence = NEXT_FIXTURE_ID.fetch_add(1, Ordering::Relaxed);
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "cargo-ai-provider-smoke-{name}-{}-{timestamp}-{sequence}",
            std::process::id()
        ));
        let home = root.join("home");
        fs::create_dir_all(&home).expect("isolated Cargo AI Home should be created");
        let definition = root.join("anthropic_smoke.json");
        fs::write(&definition, definition_json()).expect("definition should be written");
        let image = root.join("pixel.png");
        fs::write(&image, [137, 80, 78, 71, 13, 10, 26, 10])
            .expect("image fixture should be written");
        let usage = root.join("usage.ndjson");
        Self {
            root,
            home,
            definition,
            image,
            usage,
        }
    }

    fn isolated_command(&self, program: impl AsRef<std::ffi::OsStr>) -> Command {
        let mut command = Command::new(program);
        command
            .current_dir(&self.root)
            .env("CARGO_AI_HOME", &self.home)
            .env("CARGO_AI_DISABLE_KEYCHAIN", "1");
        command
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn definition_json() -> &'static str {
    r#"{
  "agent_definition_schema_version": "2026-03-03.r1",
  "inputs": [{"type":"text","text":"Return a short status."}],
  "agent_schema": {
    "type": "object",
    "properties": {"status":{"type":"string"}},
    "required": ["status"],
    "additionalProperties": false
  },
  "actions": []
}"#
}

fn success_response() -> String {
    serde_json::json!({
        "content": [{"type": "text", "text": "{\"status\":\"ok\"}"}],
        "usage": {"input_tokens": 12, "output_tokens": 5}
    })
    .to_string()
}

struct MockServer {
    url: String,
    request: thread::JoinHandle<String>,
}

impl MockServer {
    fn success() -> Self {
        Self::respond_after(Duration::ZERO, 200, success_response())
    }

    fn respond_after(delay: Duration, status: u16, body: String) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("mock listener should bind");
        let address = listener.local_addr().expect("mock address should resolve");
        let request = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("mock should accept one request");
            let request = read_http_request(&mut stream);
            thread::sleep(delay);
            let reason = if status == 200 { "OK" } else { "Error" };
            let response = format!(
                "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes());
            request
        });
        Self {
            url: format!("http://{address}/v1/messages"),
            request,
        }
    }

    fn finish(self) -> String {
        self.request.join().expect("mock server should finish")
    }
}

fn read_http_request(stream: &mut TcpStream) -> String {
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("read timeout should configure");
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 4096];
    let mut expected_len = None;
    loop {
        let count = stream
            .read(&mut buffer)
            .expect("request should be readable");
        if count == 0 {
            break;
        }
        bytes.extend_from_slice(&buffer[..count]);
        if let Some(header_end) = find_header_end(&bytes) {
            let headers = String::from_utf8_lossy(&bytes[..header_end]);
            let content_length = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())
                        .flatten()
                })
                .unwrap_or(0);
            expected_len = Some(header_end + 4 + content_length);
        }
        if expected_len.is_some_and(|length| bytes.len() >= length) {
            break;
        }
    }
    String::from_utf8(bytes).expect("request should be UTF-8")
}

fn find_header_end(bytes: &[u8]) -> Option<usize> {
    bytes.windows(4).position(|window| window == b"\r\n\r\n")
}

fn run_args(fixture: &Fixture, url: &str) -> Vec<String> {
    vec![
        "--server".into(),
        "anthropic".into(),
        "--model".into(),
        "claude-smoke".into(),
        "--url".into(),
        url.into(),
        "--token".into(),
        TEST_TOKEN.into(),
        "--max-output-tokens".into(),
        "128".into(),
        "--input-text".into(),
        "Describe the image and return status ok.".into(),
        "--input-image".into(),
        fixture.image.display().to_string(),
        "--usage-log".into(),
        fixture.usage.display().to_string(),
        "--render-mode".into(),
        "append-only".into(),
    ]
}

fn assert_success(output: &Output, request: &str, usage_path: &Path) {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "provider smoke failed\n{stdout}\n{stderr}"
    );
    let normalized_request = request.to_ascii_lowercase();
    assert!(normalized_request.contains("post /v1/messages http/1.1"));
    assert!(normalized_request.contains("x-api-key: anthropic-provider-smoke-token"));
    assert!(normalized_request.contains("anthropic-version: 2023-06-01"));
    let body = request
        .split_once("\r\n\r\n")
        .map(|(_, body)| body)
        .expect("request should contain a body");
    let body: Value = serde_json::from_str(body).expect("request body should be JSON");
    assert_eq!(body["model"], "claude-smoke");
    assert_eq!(body["max_tokens"], 128);
    assert_eq!(body["output_config"]["format"]["type"], "json_schema");
    assert!(body["messages"][0]["content"]
        .as_array()
        .expect("content should be an array")
        .iter()
        .any(|part| part["type"] == "image"));

    let events = fs::read_to_string(usage_path).expect("usage log should exist");
    let provider_event = events
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find(|event| event["event_type"] == "provider_request_completed")
        .expect("provider usage event should be recorded");
    assert_eq!(provider_event["provider"]["server"], "anthropic");
    assert_eq!(provider_event["usage"]["input_tokens"], 12);
    assert_eq!(provider_event["usage"]["output_tokens"], 5);
    assert_eq!(provider_event["usage"]["total_tokens"], 17);
    assert!(!events.contains(TEST_TOKEN));
}

#[test]
fn interpreted_anthropic_smoke_isolated_and_deterministic() {
    let fixture = Fixture::new("interpreted");
    let mock = MockServer::success();
    let mut command = fixture.isolated_command(env!("CARGO_BIN_EXE_cargo-ai"));
    command
        .args(["--no-update-check", "run", "--config"])
        .arg(&fixture.definition)
        .args(run_args(&fixture, &mock.url));
    let output = command.output().expect("interpreted CLI should start");
    let request = mock.finish();
    assert_success(&output, &request, &fixture.usage);
}

#[test]
#[ignore = "run explicitly in the provider smoke CI lane"]
fn generated_anthropic_smoke_isolated_and_deterministic() {
    let fixture = Fixture::new("generated");
    let output_dir = fixture.root.join("dist");
    let hatch = fixture
        .isolated_command(env!("CARGO_BIN_EXE_cargo-ai"))
        .args([
            "--no-update-check",
            "hatch",
            "anthropic_provider_smoke",
            "--config",
        ])
        .arg(&fixture.definition)
        .args(["--output-dir"])
        .arg(&output_dir)
        .arg("--force")
        .output()
        .expect("hatch should start");
    assert!(
        hatch.status.success(),
        "hatch failed\n{}\n{}",
        String::from_utf8_lossy(&hatch.stdout),
        String::from_utf8_lossy(&hatch.stderr)
    );

    let executable = output_dir.join(if cfg!(windows) {
        "anthropic_provider_smoke.exe"
    } else {
        "anthropic_provider_smoke"
    });
    let mock = MockServer::success();
    let output = fixture
        .isolated_command(&executable)
        .args(run_args(&fixture, &mock.url))
        .output()
        .expect("generated agent should start");
    let request = mock.finish();
    assert_success(&output, &request, &fixture.usage);
}

#[test]
fn anthropic_timeout_and_file_failures_are_actionable() {
    let fixture = Fixture::new("failures");
    let mock = MockServer::respond_after(Duration::from_secs(2), 200, success_response());
    let timeout = fixture
        .isolated_command(env!("CARGO_BIN_EXE_cargo-ai"))
        .args(["--no-update-check", "run", "--config"])
        .arg(&fixture.definition)
        .args([
            "--server",
            "anthropic",
            "--model",
            "claude-smoke",
            "--url",
            &mock.url,
            "--token",
            TEST_TOKEN,
            "--inference-timeout-in-sec",
            "1",
        ])
        .output()
        .expect("timeout smoke should start");
    let _ = mock.finish();
    let timeout_text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&timeout.stdout),
        String::from_utf8_lossy(&timeout.stderr)
    );
    assert!(!timeout.status.success());
    assert!(timeout_text.to_ascii_lowercase().contains("timed out"));

    let file = fixture.root.join("document.pdf");
    fs::write(&file, b"not a real PDF").expect("file fixture should be written");
    let file_failure = fixture
        .isolated_command(env!("CARGO_BIN_EXE_cargo-ai"))
        .args(["--no-update-check", "run", "--config"])
        .arg(&fixture.definition)
        .args([
            "--server",
            "anthropic",
            "--model",
            "claude-smoke",
            "--token",
            TEST_TOKEN,
            "--input-file",
        ])
        .arg(&file)
        .output()
        .expect("file capability smoke should start");
    let file_text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&file_failure.stdout),
        String::from_utf8_lossy(&file_failure.stderr)
    );
    assert!(!file_failure.status.success());
    assert!(
        file_text
            .to_ascii_lowercase()
            .contains("file inputs are not supported by the anthropic adapter"),
        "unexpected file-capability diagnostic:\n{file_text}"
    );

    let image_action_definition = fixture.root.join("anthropic_image_action.json");
    fs::write(
        &image_action_definition,
        r#"{
  "agent_definition_schema_version": "2026-03-03.r1",
  "inputs": [{"name":"request","type":"text","text":"Create an image."}],
  "agent_schema": {"type":"object","properties":{}},
  "actions": [{
    "name": "unsupported_image_generation",
    "logic": {"==":[1,1]},
    "run": [{
      "kind": "generate_image",
      "model": "claude-smoke",
      "prompt": ["Create an image."],
      "path": ["./output.png"]
    }]
  }]
}"#,
    )
    .expect("image action definition should be written");
    let image_failure = fixture
        .isolated_command(env!("CARGO_BIN_EXE_cargo-ai"))
        .args(["--no-update-check", "run", "--config"])
        .arg(&image_action_definition)
        .args([
            "--server",
            "anthropic",
            "--model",
            "claude-smoke",
            "--token",
            TEST_TOKEN,
            "--render-mode",
            "append-only",
        ])
        .output()
        .expect("image capability smoke should start");
    let image_text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&image_failure.stdout),
        String::from_utf8_lossy(&image_failure.stderr)
    );
    assert!(!image_failure.status.success());
    assert!(
        image_text.contains("generate_image is not supported by the Anthropic adapter"),
        "unexpected image-capability diagnostic:\n{image_text}"
    );
}

#[test]
#[ignore = "requires ANTHROPIC_API_KEY and ANTHROPIC_MODEL"]
fn live_anthropic_smoke_uses_isolated_stdin_credentials() {
    let api_key = std::env::var("ANTHROPIC_API_KEY").expect("ANTHROPIC_API_KEY is required");
    let model = std::env::var("ANTHROPIC_MODEL").expect("ANTHROPIC_MODEL is required");
    let fixture = Fixture::new("live");
    let cli = env!("CARGO_BIN_EXE_cargo-ai");
    let add = fixture
        .isolated_command(cli)
        .args([
            "--no-update-check",
            "profile",
            "add",
            "anthropic-live",
            "--server",
            "anthropic",
            "--model",
            &model,
            "--auth",
            "api_key",
            "--max-output-tokens",
            "128",
        ])
        .output()
        .expect("profile add should start");
    assert!(add.status.success(), "profile add should succeed");

    let mut set = fixture.isolated_command(cli);
    let mut child = set
        .args([
            "--no-update-check",
            "profile",
            "set",
            "anthropic-live",
            "--stdin",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("profile set should start");
    child
        .stdin
        .take()
        .expect("profile set stdin should exist")
        .write_all(api_key.as_bytes())
        .expect("API key should be written to isolated profile stdin");
    let set_output = child.wait_with_output().expect("profile set should finish");
    assert!(
        set_output.status.success(),
        "profile token setup should succeed"
    );

    let run = fixture
        .isolated_command(cli)
        .args(["--no-update-check", "run", "--config"])
        .arg(&fixture.definition)
        .args(["--profile", "anthropic-live", "--usage-log"])
        .arg(&fixture.usage)
        .output()
        .expect("live Anthropic smoke should start");
    assert!(
        run.status.success(),
        "live Anthropic smoke failed\n{}\n{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    let events = fs::read_to_string(&fixture.usage).expect("usage log should exist");
    assert!(events.contains("\"server\":\"anthropic\""));
    assert!(!events.contains(&api_key));
}
