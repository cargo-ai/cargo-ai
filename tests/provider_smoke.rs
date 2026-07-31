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
const GEMINI_TEST_TOKEN: &str = "gemini-provider-smoke-token";
const MISTRAL_TEST_TOKEN: &str = "mistral-provider-smoke-token";
const XAI_TEST_TOKEN: &str = "xai-provider-smoke-token";

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

fn gemini_success_response() -> String {
    serde_json::json!({
        "status": "completed",
        "steps": [{
            "type": "model_output",
            "content": [{"type": "text", "text": "{\"status\":\"ok\"}"}]
        }],
        "usage": {
            "total_input_tokens": 13,
            "total_output_tokens": 5,
            "total_thought_tokens": 2,
            "total_tokens": 20
        }
    })
    .to_string()
}

fn mistral_success_response(output: &str) -> String {
    serde_json::json!({
        "choices": [{"message": {"role": "assistant", "content": output}}],
        "usage": {"prompt_tokens": 14, "completion_tokens": 6, "total_tokens": 20}
    })
    .to_string()
}

fn xai_success_response(output: &str) -> String {
    serde_json::json!({
        "output": [{
            "type": "message",
            "role": "assistant",
            "content": [{"type": "output_text", "text": output}]
        }],
        "usage": {"input_tokens": 15, "output_tokens": 6, "total_tokens": 21}
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

    fn gemini_success() -> Self {
        Self::respond_after_at(
            "/v1beta/interactions",
            Duration::ZERO,
            200,
            gemini_success_response(),
        )
    }

    fn mistral_success() -> Self {
        Self::respond_after_at(
            "/v1/chat/completions",
            Duration::ZERO,
            200,
            mistral_success_response(r#"{"status":"ok"}"#),
        )
    }

    fn xai_success() -> Self {
        Self::respond_after_at(
            "/v1/responses",
            Duration::ZERO,
            200,
            xai_success_response(r#"{"status":"ok"}"#),
        )
    }

    fn respond_after(delay: Duration, status: u16, body: String) -> Self {
        Self::respond_after_at("/v1/messages", delay, status, body)
    }

    fn respond_after_at(path: &str, delay: Duration, status: u16, body: String) -> Self {
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
            url: format!("http://{address}{path}"),
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
    assert!(
        body.get("temperature").is_none(),
        "Anthropic requests must omit model-deprecated sampling controls"
    );
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

fn gemini_run_args(fixture: &Fixture, url: &str) -> Vec<String> {
    vec![
        "--server".into(),
        "gemini".into(),
        "--model".into(),
        "gemini-smoke".into(),
        "--url".into(),
        url.into(),
        "--token".into(),
        GEMINI_TEST_TOKEN.into(),
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

fn assert_gemini_success(output: &Output, request: &str, usage_path: &Path) {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "Gemini smoke failed\n{stdout}\n{stderr}"
    );
    let normalized_request = request.to_ascii_lowercase();
    assert!(normalized_request.contains("post /v1beta/interactions http/1.1"));
    assert!(normalized_request.contains("x-goog-api-key: gemini-provider-smoke-token"));
    let body = request
        .split_once("\r\n\r\n")
        .map(|(_, body)| body)
        .expect("request should contain a body");
    let body: Value = serde_json::from_str(body).expect("request body should be JSON");
    assert_eq!(body["model"], "gemini-smoke");
    assert_eq!(body["store"], false);
    assert_eq!(body["generation_config"]["max_output_tokens"], 128);
    assert_eq!(body["response_format"]["type"], "text");
    assert_eq!(body["response_format"]["mime_type"], "application/json");
    assert!(body["input"]
        .as_array()
        .expect("input should be an array")
        .iter()
        .any(|part| part["type"] == "image"));

    let events = fs::read_to_string(usage_path).expect("usage log should exist");
    let provider_event = events
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find(|event| event["event_type"] == "provider_request_completed")
        .expect("provider usage event should be recorded");
    assert_eq!(provider_event["provider"]["server"], "gemini");
    assert_eq!(provider_event["usage"]["input_tokens"], 13);
    assert_eq!(provider_event["usage"]["output_tokens"], 5);
    assert_eq!(provider_event["usage"]["total_tokens"], 20);
    assert!(!events.contains(GEMINI_TEST_TOKEN));
}

fn hosted_run_args(
    provider: &str,
    model: &str,
    token: &str,
    fixture: &Fixture,
    url: &str,
) -> Vec<String> {
    vec![
        "--server".into(),
        provider.into(),
        "--model".into(),
        model.into(),
        "--url".into(),
        url.into(),
        "--token".into(),
        token.into(),
        "--max-output-tokens".into(),
        "128".into(),
        "--usage-log".into(),
        fixture.usage.display().to_string(),
        "--render-mode".into(),
        "append-only".into(),
    ]
}

fn assert_hosted_success(
    provider: &str,
    model: &str,
    token: &str,
    output: &Output,
    request: &str,
    usage_path: &Path,
) {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "{provider} smoke failed\n{stdout}\n{stderr}"
    );
    let normalized_request = request.to_ascii_lowercase();
    assert!(normalized_request.contains(&format!("authorization: bearer {token}")));
    let body = request
        .split_once("\r\n\r\n")
        .map(|(_, body)| body)
        .expect("request should contain a body");
    let body: Value = serde_json::from_str(body).expect("request body should be JSON");
    assert_eq!(body["model"], model);
    match provider {
        "mistral" => {
            assert!(normalized_request.contains("post /v1/chat/completions http/1.1"));
            assert_eq!(body["max_tokens"], 128);
            assert_eq!(body["response_format"]["type"], "json_schema");
            assert_eq!(body["response_format"]["json_schema"]["strict"], true);
            assert_eq!(
                body["response_format"]["json_schema"]["schema"]["additionalProperties"],
                false
            );
        }
        "xai" => {
            assert!(normalized_request.contains("post /v1/responses http/1.1"));
            assert_eq!(body["store"], false);
            assert_eq!(body["max_output_tokens"], 128);
            assert_eq!(body["text"]["format"]["type"], "json_schema");
            assert_eq!(body["text"]["format"]["strict"], true);
            assert_eq!(
                body["text"]["format"]["schema"]["additionalProperties"],
                false
            );
        }
        _ => panic!("unexpected hosted provider {provider}"),
    }

    let events = fs::read_to_string(usage_path).expect("usage log should exist");
    let provider_event = events
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find(|event| event["event_type"] == "provider_request_completed")
        .expect("provider usage event should be recorded");
    assert_eq!(provider_event["provider"]["server"], provider);
    assert!(!events.contains(token));
}

fn run_interpreted_hosted_smoke(
    fixture: &Fixture,
    provider: &str,
    model: &str,
    token: &str,
    mock: MockServer,
) {
    let mut command = fixture.isolated_command(env!("CARGO_BIN_EXE_cargo-ai"));
    command
        .args(["--no-update-check", "run", "--config"])
        .arg(&fixture.definition)
        .args(hosted_run_args(provider, model, token, fixture, &mock.url));
    let output = command
        .output()
        .expect("interpreted hosted CLI should start");
    let request = mock.finish();
    assert_hosted_success(provider, model, token, &output, &request, &fixture.usage);
}

fn run_generated_hosted_smoke(
    fixture: &Fixture,
    binary_name: &str,
    provider: &str,
    model: &str,
    token: &str,
    mock: MockServer,
) {
    let output_dir = fixture.root.join("dist");
    let hatch = fixture
        .isolated_command(env!("CARGO_BIN_EXE_cargo-ai"))
        .args(["--no-update-check", "hatch", binary_name, "--config"])
        .arg(&fixture.definition)
        .args(["--output-dir"])
        .arg(&output_dir)
        .arg("--force")
        .output()
        .expect("hosted provider hatch should start");
    assert!(
        hatch.status.success(),
        "{provider} hatch failed\n{}\n{}",
        String::from_utf8_lossy(&hatch.stdout),
        String::from_utf8_lossy(&hatch.stderr)
    );
    let executable = output_dir.join(if cfg!(windows) {
        format!("{binary_name}.exe")
    } else {
        binary_name.to_string()
    });
    let output = fixture
        .isolated_command(&executable)
        .args(hosted_run_args(provider, model, token, fixture, &mock.url))
        .output()
        .expect("generated hosted agent should start");
    let request = mock.finish();
    assert_hosted_success(provider, model, token, &output, &request, &fixture.usage);
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
fn interpreted_gemini_smoke_isolated_and_deterministic() {
    let fixture = Fixture::new("gemini-interpreted");
    let mock = MockServer::gemini_success();
    let mut command = fixture.isolated_command(env!("CARGO_BIN_EXE_cargo-ai"));
    command
        .args(["--no-update-check", "run", "--config"])
        .arg(&fixture.definition)
        .args(gemini_run_args(&fixture, &mock.url));
    let output = command
        .output()
        .expect("interpreted Gemini CLI should start");
    let request = mock.finish();
    assert_gemini_success(&output, &request, &fixture.usage);
}

#[test]
#[ignore = "run explicitly in the provider smoke CI lane"]
fn generated_gemini_smoke_isolated_and_deterministic() {
    let fixture = Fixture::new("gemini-generated");
    let output_dir = fixture.root.join("dist");
    let hatch = fixture
        .isolated_command(env!("CARGO_BIN_EXE_cargo-ai"))
        .args([
            "--no-update-check",
            "hatch",
            "gemini_provider_smoke",
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
        "Gemini hatch failed\n{}\n{}",
        String::from_utf8_lossy(&hatch.stdout),
        String::from_utf8_lossy(&hatch.stderr)
    );

    let executable = output_dir.join(if cfg!(windows) {
        "gemini_provider_smoke.exe"
    } else {
        "gemini_provider_smoke"
    });
    let mock = MockServer::gemini_success();
    let output = fixture
        .isolated_command(&executable)
        .args(gemini_run_args(&fixture, &mock.url))
        .output()
        .expect("generated Gemini agent should start");
    let request = mock.finish();
    assert_gemini_success(&output, &request, &fixture.usage);
}

#[test]
fn interpreted_mistral_smoke_isolated_and_deterministic() {
    let fixture = Fixture::new("mistral-interpreted");
    run_interpreted_hosted_smoke(
        &fixture,
        "mistral",
        "mistral-smoke",
        MISTRAL_TEST_TOKEN,
        MockServer::mistral_success(),
    );
}

#[test]
#[ignore = "run explicitly in the provider smoke CI lane"]
fn generated_mistral_smoke_isolated_and_deterministic() {
    let fixture = Fixture::new("mistral-generated");
    run_generated_hosted_smoke(
        &fixture,
        "mistral_provider_smoke",
        "mistral",
        "mistral-smoke",
        MISTRAL_TEST_TOKEN,
        MockServer::mistral_success(),
    );
}

#[test]
fn interpreted_xai_smoke_isolated_and_deterministic() {
    let fixture = Fixture::new("xai-interpreted");
    run_interpreted_hosted_smoke(
        &fixture,
        "xai",
        "grok-smoke",
        XAI_TEST_TOKEN,
        MockServer::xai_success(),
    );
}

#[test]
#[ignore = "run explicitly in the provider smoke CI lane"]
fn generated_xai_smoke_isolated_and_deterministic() {
    let fixture = Fixture::new("xai-generated");
    run_generated_hosted_smoke(
        &fixture,
        "xai_provider_smoke",
        "xai",
        "grok-smoke",
        XAI_TEST_TOKEN,
        MockServer::xai_success(),
    );
}

fn marker_definition(fixture: &Fixture) -> PathBuf {
    let marker = fixture.root.join("action-ran.txt");
    let (program, args, platform) = if cfg!(windows) {
        (
            "cmd",
            vec![
                Value::String("/C".to_string()),
                Value::String(format!("echo ran>\"{}\"", marker.display())),
            ],
            "windows",
        )
    } else {
        (
            "/bin/sh",
            vec![
                Value::String("-c".to_string()),
                Value::String(format!("printf ran > '{}'", marker.display())),
            ],
            if cfg!(target_os = "macos") {
                "macos"
            } else {
                "linux"
            },
        )
    };
    let definition = fixture.root.join("fail_closed.json");
    fs::write(
        &definition,
        serde_json::to_vec_pretty(&serde_json::json!({
            "agent_definition_schema_version": "2026-03-03.r1",
            "inputs": [{"type": "text", "text": "Return status."}],
            "agent_schema": {
                "type": "object",
                "properties": {"status": {"type": "string"}},
                "required": ["status"],
                "additionalProperties": false
            },
            "actions": [{
                "name": "must_not_run",
                "logic": {"==": [1, 1]},
                "run": [{"kind": "exec", "program": program, "args": args, "platform": platform}]
            }]
        }))
        .expect("marker definition should serialize"),
    )
    .expect("marker definition should be written");
    definition
}

fn hosted_response(provider: &str, output: &str) -> String {
    match provider {
        "mistral" => mistral_success_response(output),
        "xai" => xai_success_response(output),
        _ => panic!("unexpected hosted provider {provider}"),
    }
}

fn hosted_path(provider: &str) -> &'static str {
    match provider {
        "mistral" => "/v1/chat/completions",
        "xai" => "/v1/responses",
        _ => panic!("unexpected hosted provider {provider}"),
    }
}

fn hosted_test_values(provider: &str) -> (&'static str, &'static str) {
    match provider {
        "mistral" => ("mistral-smoke", MISTRAL_TEST_TOKEN),
        "xai" => ("grok-smoke", XAI_TEST_TOKEN),
        _ => panic!("unexpected hosted provider {provider}"),
    }
}

fn run_hosted_failure(fixture: &Fixture, provider: &str, mock: MockServer) -> Output {
    let (model, token) = hosted_test_values(provider);
    let mut command = fixture.isolated_command(env!("CARGO_BIN_EXE_cargo-ai"));
    command
        .args(["--no-update-check", "run", "--config"])
        .arg(marker_definition(fixture))
        .args([
            "--server",
            provider,
            "--model",
            model,
            "--url",
            &mock.url,
            "--token",
            token,
            "--render-mode",
            "append-only",
        ]);
    let output = command.output().expect("hosted failure smoke should start");
    let _ = mock.finish();
    assert!(
        !output.status.success(),
        "invalid provider output must fail"
    );
    assert!(
        !fixture.root.join("action-ran.txt").exists(),
        "downstream action must not run after invalid output"
    );
    output
}

fn assert_hosted_fail_closed_matrix(provider: &str) {
    for (name, returned_json, expected) in [
        ("wrong-type", r#"{"status":7}"#, "required JSON schema"),
        ("missing-required", r#"{}"#, "required JSON schema"),
        (
            "unexpected-field",
            r#"{"status":"ok","extra":true}"#,
            "required JSON schema",
        ),
    ] {
        let fixture = Fixture::new(&format!("{provider}-{name}"));
        let mock = MockServer::respond_after_at(
            hosted_path(provider),
            Duration::ZERO,
            200,
            hosted_response(provider, returned_json),
        );
        let output = run_hosted_failure(&fixture, provider, mock);
        let text = format!(
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            text.contains(expected),
            "unexpected {provider} {name} diagnostic:\n{text}"
        );
    }

    let fixture = Fixture::new(&format!("{provider}-malformed"));
    let mock = MockServer::respond_after_at(
        hosted_path(provider),
        Duration::ZERO,
        200,
        hosted_response(provider, "not-json"),
    );
    let output = run_hosted_failure(&fixture, provider, mock);
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        text.contains("required JSON schema"),
        "unexpected malformed diagnostic:\n{text}"
    );

    let fixture = Fixture::new(&format!("{provider}-schema-rejected"));
    let provider_message = "selected model rejected json_schema";
    let body = if provider == "xai" {
        serde_json::json!({"error": {"message": provider_message}, "debug": "secret-debug"})
    } else {
        serde_json::json!({"message": provider_message, "debug": "secret-debug"})
    };
    let mock =
        MockServer::respond_after_at(hosted_path(provider), Duration::ZERO, 400, body.to_string());
    let output = run_hosted_failure(&fixture, provider, mock);
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        text.contains(provider_message),
        "provider rejection should be preserved:\n{text}"
    );
    assert!(
        !text.contains("secret-debug"),
        "unselected provider fields must be redacted"
    );
}

#[test]
fn mistral_invalid_returns_fail_closed_before_actions() {
    assert_hosted_fail_closed_matrix("mistral");
}

#[test]
fn xai_invalid_returns_fail_closed_before_actions() {
    assert_hosted_fail_closed_matrix("xai");
}

fn assert_hosted_timeout_and_capability_failures(provider: &str) {
    let fixture = Fixture::new(&format!("{provider}-failures"));
    let (model, token) = hosted_test_values(provider);
    let mock = MockServer::respond_after_at(
        hosted_path(provider),
        Duration::from_secs(2),
        200,
        hosted_response(provider, r#"{"status":"ok"}"#),
    );
    let timeout = fixture
        .isolated_command(env!("CARGO_BIN_EXE_cargo-ai"))
        .args(["--no-update-check", "run", "--config"])
        .arg(&fixture.definition)
        .args([
            "--server",
            provider,
            "--model",
            model,
            "--url",
            &mock.url,
            "--token",
            token,
            "--inference-timeout-in-sec",
            "1",
        ])
        .output()
        .expect("hosted timeout smoke should start");
    let _ = mock.finish();
    let timeout_text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&timeout.stdout),
        String::from_utf8_lossy(&timeout.stderr)
    );
    assert!(!timeout.status.success());
    assert!(timeout_text.to_ascii_lowercase().contains("timed out"));

    let image_failure = fixture
        .isolated_command(env!("CARGO_BIN_EXE_cargo-ai"))
        .args(["--no-update-check", "run", "--config"])
        .arg(&fixture.definition)
        .args([
            "--server",
            provider,
            "--model",
            model,
            "--token",
            token,
            "--input-image",
        ])
        .arg(&fixture.image)
        .output()
        .expect("hosted image capability smoke should start");
    let image_text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&image_failure.stdout),
        String::from_utf8_lossy(&image_failure.stderr)
    );
    assert!(!image_failure.status.success());
    assert!(
        image_text
            .to_ascii_lowercase()
            .contains("image inputs are not supported"),
        "unexpected {provider} image diagnostic:\n{image_text}"
    );

    let file = fixture.root.join("document.pdf");
    fs::write(&file, b"not a real PDF").expect("file fixture should be written");
    let file_failure = fixture
        .isolated_command(env!("CARGO_BIN_EXE_cargo-ai"))
        .args(["--no-update-check", "run", "--config"])
        .arg(&fixture.definition)
        .args([
            "--server",
            provider,
            "--model",
            model,
            "--token",
            token,
            "--input-file",
        ])
        .arg(&file)
        .output()
        .expect("hosted file capability smoke should start");
    let file_text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&file_failure.stdout),
        String::from_utf8_lossy(&file_failure.stderr)
    );
    assert!(!file_failure.status.success());
    assert!(
        file_text
            .to_ascii_lowercase()
            .contains("file inputs are not supported"),
        "unexpected {provider} file diagnostic:\n{file_text}"
    );

    let image_action_definition = fixture.root.join("unsupported_image_action.json");
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
      "model": "image-model",
      "prompt": ["Create an image."],
      "path": ["./output.png"]
    }]
  }]
}"#,
    )
    .expect("image action definition should be written");
    let generate_failure = fixture
        .isolated_command(env!("CARGO_BIN_EXE_cargo-ai"))
        .args(["--no-update-check", "run", "--config"])
        .arg(&image_action_definition)
        .args([
            "--server",
            provider,
            "--model",
            model,
            "--token",
            token,
            "--render-mode",
            "append-only",
        ])
        .output()
        .expect("hosted generate_image capability smoke should start");
    let generate_text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&generate_failure.stdout),
        String::from_utf8_lossy(&generate_failure.stderr)
    );
    assert!(!generate_failure.status.success());
    assert!(
        generate_text.contains("generate_image is not supported"),
        "unexpected {provider} generate_image diagnostic:\n{generate_text}"
    );
}

#[test]
fn mistral_timeout_and_capability_failures_are_actionable() {
    assert_hosted_timeout_and_capability_failures("mistral");
}

#[test]
fn xai_timeout_and_capability_failures_are_actionable() {
    assert_hosted_timeout_and_capability_failures("xai");
}

#[test]
fn gemini_timeout_and_file_failures_are_actionable() {
    let fixture = Fixture::new("gemini-failures");
    let mock = MockServer::respond_after_at(
        "/v1beta/interactions",
        Duration::from_secs(2),
        200,
        gemini_success_response(),
    );
    let timeout = fixture
        .isolated_command(env!("CARGO_BIN_EXE_cargo-ai"))
        .args(["--no-update-check", "run", "--config"])
        .arg(&fixture.definition)
        .args([
            "--server",
            "gemini",
            "--model",
            "gemini-smoke",
            "--url",
            &mock.url,
            "--token",
            GEMINI_TEST_TOKEN,
            "--inference-timeout-in-sec",
            "1",
        ])
        .output()
        .expect("Gemini timeout smoke should start");
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
            "gemini",
            "--model",
            "gemini-smoke",
            "--token",
            GEMINI_TEST_TOKEN,
            "--input-file",
        ])
        .arg(&file)
        .output()
        .expect("Gemini file capability smoke should start");
    let file_text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&file_failure.stdout),
        String::from_utf8_lossy(&file_failure.stderr)
    );
    assert!(!file_failure.status.success());
    assert!(
        file_text
            .to_ascii_lowercase()
            .contains("file inputs are not supported by the google gemini adapter"),
        "unexpected Gemini file-capability diagnostic:\n{file_text}"
    );
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

#[test]
#[ignore = "requires GEMINI_API_KEY and GEMINI_MODEL"]
fn live_gemini_smoke_uses_isolated_stdin_credentials() {
    let api_key = std::env::var("GEMINI_API_KEY").expect("GEMINI_API_KEY is required");
    let model = std::env::var("GEMINI_MODEL").expect("GEMINI_MODEL is required");
    let fixture = Fixture::new("gemini-live");
    let cli = env!("CARGO_BIN_EXE_cargo-ai");
    let add = fixture
        .isolated_command(cli)
        .args([
            "--no-update-check",
            "profile",
            "add",
            "gemini-live",
            "--server",
            "gemini",
            "--model",
            &model,
            "--auth",
            "api_key",
            "--max-output-tokens",
            "128",
        ])
        .output()
        .expect("Gemini profile add should start");
    assert!(add.status.success(), "Gemini profile add should succeed");

    let mut set = fixture.isolated_command(cli);
    let mut child = set
        .args([
            "--no-update-check",
            "profile",
            "set",
            "gemini-live",
            "--stdin",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Gemini profile set should start");
    child
        .stdin
        .take()
        .expect("profile set stdin should exist")
        .write_all(api_key.as_bytes())
        .expect("Gemini API key should be written to isolated profile stdin");
    let set_output = child.wait_with_output().expect("profile set should finish");
    assert!(
        set_output.status.success(),
        "Gemini token setup should succeed"
    );

    let run = fixture
        .isolated_command(cli)
        .args(["--no-update-check", "run", "--config"])
        .arg(&fixture.definition)
        .args(["--profile", "gemini-live", "--usage-log"])
        .arg(&fixture.usage)
        .output()
        .expect("live Gemini smoke should start");
    assert!(
        run.status.success(),
        "live Gemini smoke failed\n{}\n{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    let events = fs::read_to_string(&fixture.usage).expect("usage log should exist");
    assert!(events.contains("\"server\":\"gemini\""));
    assert!(!events.contains(&api_key));
}

fn run_live_hosted_smoke(provider: &str, key_env: &str, model_env: &str) {
    let api_key = std::env::var(key_env).unwrap_or_else(|_| panic!("{key_env} is required"));
    let model = std::env::var(model_env).unwrap_or_else(|_| panic!("{model_env} is required"));
    let fixture = Fixture::new(&format!("{provider}-live"));
    let cli = env!("CARGO_BIN_EXE_cargo-ai");
    let profile = format!("{provider}-api");
    let add = fixture
        .isolated_command(cli)
        .args([
            "--no-update-check",
            "profile",
            "add",
            &profile,
            "--server",
            provider,
            "--model",
            &model,
            "--auth",
            "api_key",
            "--max-output-tokens",
            "128",
        ])
        .output()
        .expect("hosted profile add should start");
    assert!(
        add.status.success(),
        "{provider} profile add should succeed"
    );

    let mut set = fixture.isolated_command(cli);
    let mut child = set
        .args(["--no-update-check", "profile", "set", &profile, "--stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("hosted profile set should start");
    child
        .stdin
        .take()
        .expect("profile set stdin should exist")
        .write_all(api_key.as_bytes())
        .expect("hosted API key should be written to isolated profile stdin");
    let set_output = child.wait_with_output().expect("profile set should finish");
    assert!(
        set_output.status.success(),
        "{provider} token setup should succeed"
    );

    let run = fixture
        .isolated_command(cli)
        .args(["--no-update-check", "run", "--config"])
        .arg(&fixture.definition)
        .args(["--profile", &profile, "--usage-log"])
        .arg(&fixture.usage)
        .output()
        .expect("live hosted smoke should start");
    assert!(
        run.status.success(),
        "live {provider} smoke failed\n{}\n{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    let events = fs::read_to_string(&fixture.usage).expect("usage log should exist");
    assert!(events.contains(&format!("\"server\":\"{provider}\"")));
    assert!(!events.contains(&api_key));
}

#[test]
#[ignore = "requires MISTRAL_API_KEY and MISTRAL_MODEL"]
fn live_mistral_smoke_uses_isolated_stdin_credentials() {
    run_live_hosted_smoke("mistral", "MISTRAL_API_KEY", "MISTRAL_MODEL");
}

#[test]
#[ignore = "requires XAI_API_KEY and XAI_MODEL"]
fn live_xai_smoke_uses_isolated_stdin_credentials() {
    run_live_hosted_smoke("xai", "XAI_API_KEY", "XAI_MODEL");
}
