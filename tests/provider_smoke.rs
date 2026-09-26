//! Reusable process-level provider smoke coverage.

#[path = "support/provider_cache.rs"]
mod provider_cache;
#[path = "support/qualification_policy.rs"]
#[allow(dead_code)]
mod qualification_policy;
#[path = "support/qualification_report.rs"]
mod qualification_report;
#[path = "support/typesafe_smoke.rs"]
mod typesafe_smoke;

#[path = "../templates/definition_validation.rs"]
mod definition_validation;

use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

static NEXT_FIXTURE_ID: AtomicU64 = AtomicU64::new(0);
const TEST_TOKEN: &str = "anthropic-provider-smoke-token";
const GEMINI_TEST_TOKEN: &str = "gemini-provider-smoke-token";
const MISTRAL_TEST_TOKEN: &str = "mistral-provider-smoke-token";
const XAI_TEST_TOKEN: &str = "xai-provider-smoke-token";
const OPENAI_TEST_TOKEN: &str = "openai-provider-smoke-token";

fn fixture_base_dir(runner_temp: Option<std::ffi::OsString>) -> PathBuf {
    runner_temp
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
}

fn fixture_dir_name(process_id: u32, sequence: u64) -> String {
    format!("cps-{process_id:x}-{sequence:x}")
}

fn create_fixture_root(base: &Path) -> PathBuf {
    fs::create_dir_all(base).expect("provider smoke fixture base should be created");
    loop {
        let sequence = NEXT_FIXTURE_ID.fetch_add(1, Ordering::Relaxed);
        let root = base.join(fixture_dir_name(std::process::id(), sequence));
        match fs::create_dir(&root) {
            Ok(()) => return root,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => panic!(
                "provider smoke fixture root {} should be created: {error}",
                root.display()
            ),
        }
    }
}

struct Fixture {
    root: PathBuf,
    home: PathBuf,
    definition: PathBuf,
    image: PathBuf,
    usage: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let base = fixture_base_dir(std::env::var_os("RUNNER_TEMP"));
        let root = create_fixture_root(&base);
        let home = root.join("h");
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
            .env("CARGO_AI_DISABLE_KEYCHAIN", "1")
            // Generated workspaces own their outputs independently of the test runner.
            .env_remove("CARGO_TARGET_DIR");
        command
    }
}

#[test]
fn fixture_base_prefers_nonempty_runner_temp() {
    let runner_temp = std::ffi::OsString::from("runner-temp");
    assert_eq!(
        fixture_base_dir(Some(runner_temp.clone())),
        PathBuf::from(runner_temp)
    );
    assert_eq!(
        fixture_base_dir(Some(std::ffi::OsString::new())),
        std::env::temp_dir()
    );
    assert_eq!(fixture_base_dir(None), std::env::temp_dir());
}

#[test]
fn fixture_directory_name_is_compact_and_deterministic() {
    assert_eq!(fixture_dir_name(0x2a, 0xff), "cps-2a-ff");
    assert!(fixture_dir_name(u32::MAX, u64::MAX).len() <= 29);
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

fn openai_success_response(output: &str) -> String {
    serde_json::json!({
        "id": "chatcmpl-provider-smoke",
        "object": "chat.completion",
        "created": 1,
        "model": "openai-smoke",
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": output},
            "finish_reason": "stop"
        }],
        "usage": {"prompt_tokens": 16, "completion_tokens": 6, "total_tokens": 22}
    })
    .to_string()
}

fn media_responses(transcript: &str, include_child: bool) -> Vec<MediaResponse> {
    let mut responses = vec![
        MediaResponse {
            path: "/v1/audio/speech",
            content_type: "audio/wav",
            body: MEDIA_WAV.to_vec(),
        },
        MediaResponse {
            path: "/v1/audio/transcriptions",
            content_type: "application/json",
            body: serde_json::json!({"text": transcript})
                .to_string()
                .into_bytes(),
        },
    ];
    if include_child {
        responses.push(MediaResponse {
            path: "/v1/chat/completions",
            content_type: "application/json",
            body: openai_success_response(r#"{"status":"ok"}"#).into_bytes(),
        });
    }
    responses
}

fn write_media_definitions(fixture: &Fixture) {
    let definition = serde_json::json!({
        "agent_definition_schema_version": "2026-09-09.r1",
        "agent_schema": {"type":"object","properties":{}},
        "runtime_vars": {"audio_path":{"type":"string","default":"./speech.wav"}},
        "actions": [{
            "name":"speak_then_listen",
            "logic":{"==":[1,1]},
            "run":[
                {"kind":"generate_audio","model":"tts-model","text":"The quarterly report is ready.","voice":"coral","path":"./speech.wav"},
                {"kind":"transcribe_audio","model":"transcriber-model","audio":{"path":{"var":"runtime.audio_path"}},"output_variable":"transcript"},
                {"kind":"agent","artifact":"./child.json","inputs":[{"type":"text","text":["Transcript: ",{"var":"transcript"}]}]}
            ]
        }]
    });
    fs::write(
        &fixture.definition,
        serde_json::to_vec(&definition).unwrap(),
    )
    .unwrap();
    let child = serde_json::json!({
        "agent_definition_schema_version": "2026-09-09.r1",
        "agent_schema": {"type":"object","properties":{"status":{"type":"string"}}},
        "actions": []
    });
    fs::write(
        fixture.root.join("child.json"),
        serde_json::to_vec(&child).unwrap(),
    )
    .unwrap();
}

fn configure_media_profile(fixture: &Fixture, url: &str) {
    let cli = env!("CARGO_BIN_EXE_cargo-ai");
    let added = fixture
        .isolated_command(cli)
        .args([
            "--no-update-check",
            "profile",
            "add",
            "media-child",
            "--server",
            "openai",
            "--model",
            "child-model",
            "--url",
            url,
            "--auth",
            "api_key",
            "--default",
        ])
        .output()
        .expect("isolated media profile add should start");
    assert!(
        added.status.success(),
        "profile add failed: {}",
        String::from_utf8_lossy(&added.stderr)
    );
    let mut set = fixture.isolated_command(cli);
    let mut child = set
        .args([
            "--no-update-check",
            "profile",
            "set",
            "media-child",
            "--stdin",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("isolated media profile set should start");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(OPENAI_TEST_TOKEN.as_bytes())
        .unwrap();
    let saved = child.wait_with_output().unwrap();
    assert!(
        saved.status.success(),
        "profile set failed: {}",
        String::from_utf8_lossy(&saved.stderr)
    );
}

fn configure_media_fixture(fixture: &Fixture, url: &str) {
    write_media_definitions(fixture);
    configure_media_profile(fixture, url);
}

fn media_command(fixture: &Fixture, program: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut command = fixture.isolated_command(program);
    let cli_parent = Path::new(env!("CARGO_BIN_EXE_cargo-ai")).parent().unwrap();
    let mut search_paths = vec![cli_parent.to_path_buf()];
    search_paths.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    command.env("PATH", std::env::join_paths(search_paths).unwrap());
    command
}

fn media_run_args(url: &str) -> [&str; 14] {
    [
        "--server",
        "openai",
        "--url",
        url,
        "--token",
        OPENAI_TEST_TOKEN,
        "--model",
        "child-model",
        "--max-output-tokens",
        "128",
        "--render-mode",
        "append-only",
        "--inference-timeout-in-sec",
        "10",
    ]
}

fn assert_media_chain(fixture: &Fixture, output: &Output, requests: &[String]) {
    assert!(
        output.status.success(),
        "media chain failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(MEDIA_WAV.len(), 46);
    assert_eq!(
        fs::read(fixture.root.join("speech.wav")).unwrap(),
        MEDIA_WAV
    );
    assert_eq!(
        requests.len(),
        3,
        "expected speech, transcription and child inference only"
    );
    let speech: Value =
        serde_json::from_str(requests[0].split_once("\r\n\r\n").unwrap().1).unwrap();
    assert_eq!(speech["model"], "tts-model");
    assert_eq!(speech["input"], MEDIA_TRANSCRIPT);
    assert_eq!(speech["voice"], "coral");
    assert_eq!(speech["response_format"], "wav");
    assert!(requests[1].contains("name=\"model\"\r\n\r\ntranscriber-model"));
    assert!(requests[1].contains("filename=\"speech.wav\""));
    assert!(requests[1]
        .as_bytes()
        .windows(MEDIA_WAV.len())
        .any(|window| window == MEDIA_WAV));
    let child: Value = serde_json::from_str(requests[2].split_once("\r\n\r\n").unwrap().1).unwrap();
    assert_eq!(child["model"], "child-model");
    assert!(
        child["messages"][0]["content"]
            .as_array()
            .unwrap()
            .iter()
            .any(|part| part["text"] == format!("Transcript: {MEDIA_TRANSCRIPT}")),
        "child did not receive exact transcript: {child}"
    );
}

fn ollama_success_response(output: &str) -> String {
    serde_json::json!({
        "choices": [{"message": {"role": "assistant", "content": output}}],
        "prompt_eval_count": 8,
        "eval_count": 3
    })
    .to_string()
}

struct MockServer {
    url: String,
    request: thread::JoinHandle<String>,
}

const MEDIA_WAV: &[u8] = b"RIFF\x26\x00\x00\x00WAVEfmt \x10\x00\x00\x00\x01\x00\x01\x00\x40\x1f\x00\x00\x40\x1f\x00\x00\x01\x00\x08\x00data\x02\x00\x00\x00\x00\x00";
const MEDIA_TRANSCRIPT: &str = "The quarterly report is ready.";

struct MediaResponse {
    path: &'static str,
    content_type: &'static str,
    body: Vec<u8>,
}

struct MediaServer {
    url: String,
    requests: thread::JoinHandle<Vec<String>>,
}

impl MediaServer {
    fn new(responses: Vec<MediaResponse>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("media mock listener should bind");
        listener
            .set_nonblocking(true)
            .expect("media mock should be nonblocking");
        let address = listener
            .local_addr()
            .expect("media mock address should resolve");
        let requests = thread::spawn(move || {
            let mut requests = Vec::new();
            'responses: for response in responses {
                let started = Instant::now();
                let mut stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(error)
                            if error.kind() == std::io::ErrorKind::WouldBlock
                                && started.elapsed() < Duration::from_secs(30) =>
                        {
                            thread::sleep(Duration::from_millis(10));
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            break 'responses
                        }
                        Err(error) => panic!("media mock expected another request: {error}"),
                    }
                };
                let request = read_http_request(&mut stream);
                assert!(
                    request.starts_with(&format!("POST {} HTTP/1.1", response.path)),
                    "unexpected media request: {request}"
                );
                let header = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    response.content_type,
                    response.body.len()
                );
                stream
                    .write_all(header.as_bytes())
                    .expect("media response header should write");
                stream
                    .write_all(&response.body)
                    .expect("media response body should write");
                requests.push(request);
            }
            requests
        });
        Self {
            url: format!("http://{address}/v1/chat/completions"),
            requests,
        }
    }

    fn finish(self) -> Vec<String> {
        self.requests.join().expect("media mock should finish")
    }
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

    fn openai_success() -> Self {
        Self::respond_after_at(
            "/v1/chat/completions",
            Duration::ZERO,
            200,
            openai_success_response(r#"{"status":"ok"}"#),
        )
    }

    fn ollama_success() -> Self {
        Self::respond_after_at(
            "/v1/chat/completions",
            Duration::ZERO,
            200,
            ollama_success_response(r#"{"status":"ok"}"#),
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
        .set_read_timeout(Some(Duration::from_secs(1)))
        .expect("read timeout should configure");
    let started = Instant::now();
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 4096];
    let mut expected_len = None;
    loop {
        let count = match stream.read(&mut buffer) {
            Ok(count) => count,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) && started.elapsed() < Duration::from_secs(30) =>
            {
                continue;
            }
            Err(error) => panic!("request should be readable: {error}"),
        };
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
    if fixture.home.join("batch-seed-marker").exists() {
        assert!(
            String::from_utf8_lossy(&hatch.stdout).contains("Reused warmed template"),
            "batch case should reuse its copied seed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&hatch.stdout),
            String::from_utf8_lossy(&hatch.stderr)
        );
        eprintln!(
            "generated-provider hatch: {}",
            String::from_utf8_lossy(&hatch.stderr)
        );
    }
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

fn openai_compatible_run_args(
    provider: &str,
    model: &str,
    token: Option<&str>,
    fixture: &Fixture,
    url: &str,
) -> Vec<String> {
    let mut args = vec![
        "--server".into(),
        provider.into(),
        "--model".into(),
        model.into(),
        "--url".into(),
        url.into(),
        "--max-output-tokens".into(),
        "128".into(),
        "--usage-log".into(),
        fixture.usage.display().to_string(),
        "--render-mode".into(),
        "append-only".into(),
    ];
    if let Some(token) = token {
        args.extend(["--token".into(), token.into()]);
    }
    args
}

fn assert_openai_compatible_success(
    provider: &str,
    model: &str,
    token: Option<&str>,
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
    assert!(normalized_request.contains("post /v1/chat/completions http/1.1"));
    match token {
        Some(token) => {
            assert!(normalized_request.contains(&format!("authorization: bearer {token}")))
        }
        None => assert!(!normalized_request.contains("authorization:")),
    }
    let body = request
        .split_once("\r\n\r\n")
        .map(|(_, body)| body)
        .expect("request should contain a body");
    let body: Value = serde_json::from_str(body).expect("request body should be JSON");
    assert_eq!(body["model"], model);
    assert!(
        body.get("temperature").is_none(),
        "unset profiles must use the provider default temperature"
    );
    assert_eq!(body["response_format"]["type"], "json_schema");
    assert_eq!(body["response_format"]["json_schema"]["strict"], true);
    assert_eq!(
        body["response_format"]["json_schema"]["schema"]["additionalProperties"],
        false
    );

    let events = fs::read_to_string(usage_path).expect("usage log should exist");
    let provider_event = events
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find(|event| event["event_type"] == "provider_request_completed")
        .expect("provider usage event should be recorded");
    assert_eq!(provider_event["provider"]["server"], provider);
    if provider == "ollama" {
        assert_eq!(provider_event["usage"]["input_tokens"], 8);
        assert_eq!(provider_event["usage"]["output_tokens"], 3);
        assert_eq!(provider_event["usage"]["total_tokens"], 11);
    } else {
        assert_eq!(provider_event["usage"]["input_tokens"], 16);
        assert_eq!(provider_event["usage"]["output_tokens"], 6);
        assert_eq!(provider_event["usage"]["total_tokens"], 22);
    }
    if let Some(token) = token {
        assert!(!events.contains(token));
    }
}

fn run_interpreted_openai_compatible_smoke(
    fixture: &Fixture,
    provider: &str,
    model: &str,
    token: Option<&str>,
    mock: MockServer,
) {
    let output = fixture
        .isolated_command(env!("CARGO_BIN_EXE_cargo-ai"))
        .args(["--no-update-check", "run", "--config"])
        .arg(&fixture.definition)
        .args(openai_compatible_run_args(
            provider, model, token, fixture, &mock.url,
        ))
        .output()
        .expect("interpreted OpenAI-compatible CLI should start");
    let request = mock.finish();
    assert_openai_compatible_success(provider, model, token, &output, &request, &fixture.usage);
}

fn run_generated_openai_compatible_smoke(
    fixture: &Fixture,
    binary_name: &str,
    provider: &str,
    model: &str,
    token: Option<&str>,
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
        .expect("OpenAI-compatible hatch should start");
    if fixture.home.join("batch-seed-marker").exists() {
        assert!(
            String::from_utf8_lossy(&hatch.stdout).contains("Reused warmed template"),
            "batch case should reuse its copied seed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&hatch.stdout),
            String::from_utf8_lossy(&hatch.stderr)
        );
        eprintln!(
            "generated-provider hatch: {}",
            String::from_utf8_lossy(&hatch.stderr)
        );
    }
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
        .args(openai_compatible_run_args(
            provider, model, token, fixture, &mock.url,
        ))
        .output()
        .expect("generated OpenAI-compatible agent should start");
    let request = mock.finish();
    assert_openai_compatible_success(provider, model, token, &output, &request, &fixture.usage);
    if provider == "ollama" {
        assert_endpoint_diagnostics_exclude_secrets(fixture, Some(&executable));
        assert_inflight_attempt_survives_termination(Some(&executable));
    }
}

fn assert_inflight_attempt_survives_termination(executable: Option<&Path>) {
    let fixture = Fixture::new();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!(
        "http://{}/v1/chat/completions",
        listener.local_addr().unwrap()
    );
    let (received_tx, received_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let request = read_http_request(&mut stream);
        received_tx.send(request).unwrap();
        let _ = release_rx.recv_timeout(Duration::from_secs(30));
    });
    let mut command = match executable {
        Some(executable) => fixture.isolated_command(executable),
        None => {
            let mut command = fixture.isolated_command(env!("CARGO_BIN_EXE_cargo-ai"));
            command
                .args(["--no-update-check", "run", "--config"])
                .arg(&fixture.definition);
            command
        }
    };
    let mut child = command
        .env("CARGO_AI_USAGE_TRACKING", "on")
        .args([
            "--server",
            "ollama",
            "--model",
            "interrupted-model",
            "--url",
            &url,
            "--inference-timeout-in-sec",
            "30",
            "--render-mode",
            "append-only",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let request = received_rx.recv_timeout(Duration::from_secs(20));
    let query = || {
        fixture
            .isolated_command(env!("CARGO_BIN_EXE_cargo-ai"))
            .args(["usage", "export", "--format", "ndjson"])
            .output()
            .unwrap()
    };
    let inflight = query();
    child.kill().unwrap();
    child.wait().unwrap();
    let _ = release_tx.send(());
    server.join().unwrap();
    let interrupted = query();
    assert!(
        request.is_ok(),
        "provider request should reach the held endpoint"
    );
    assert!(inflight.status.success());
    assert!(interrupted.status.success());
    assert_eq!(
        inflight.stdout, interrupted.stdout,
        "termination must preserve committed start facts"
    );
    let events: Vec<Value> = String::from_utf8(inflight.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let starts: Vec<_> = events
        .iter()
        .filter(|event| event["event_type"] == "provider_request_started")
        .collect();
    assert_eq!(starts.len(), 1);
    let start = starts[0];
    assert!(start["attempt_id"]
        .as_str()
        .unwrap()
        .starts_with("cai_attempt_"));
    assert!(start["operation_id"]
        .as_str()
        .unwrap()
        .starts_with("cai_operation_"));
    assert_eq!(start["provider"]["requested_model"], "interrupted-model");
    assert_eq!(start["started_at"], start["timestamp"]);
    assert!(start.get("usage").is_none());
    assert_eq!(start["retry_count"], 0);
    assert!(!events
        .iter()
        .any(|event| event["event_type"] == "provider_request_completed"));
    let summary = fixture
        .isolated_command(env!("CARGO_BIN_EXE_cargo-ai"))
        .args(["usage", "summary", "--json"])
        .output()
        .unwrap();
    assert!(summary.status.success());
    let summary: Value = serde_json::from_slice(&summary.stdout).unwrap();
    assert_eq!(summary["summary"]["request_count"], 1);
    assert_eq!(summary["summary"]["unfinished_request_count"], 1);
    for counter in ["input_tokens", "output_tokens", "total_tokens"] {
        assert_eq!(summary["summary"]["unknown_request_counts"][counter], 1);
    }
}

#[test]
fn interpreted_inflight_attempt_survives_termination() {
    assert_inflight_attempt_survives_termination(None);
}

fn assert_endpoint_diagnostics_exclude_secrets(fixture: &Fixture, executable: Option<&Path>) {
    let profile = fixture
        .isolated_command(env!("CARGO_BIN_EXE_cargo-ai"))
        .args([
            "--no-update-check",
            "profile",
            "add",
            "diagnostic-profile",
            "--server",
            "ollama",
            "--model",
            "synthetic-model",
        ])
        .output()
        .expect("create isolated diagnostic profile");
    assert!(
        profile.status.success(),
        "diagnostic profile should be created"
    );
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind local endpoint");
    let addr = listener.local_addr().expect("read local address");
    drop(listener);
    for (url, reason) in [
        (
            format!("http://synthetic-user:synthetic-password@{addr}/synthetic-path?key=synthetic-query#synthetic-fragment"),
            "Issue communicating with the AI server (Ollama)",
        ),
        (
            "ftp://synthetic-user:synthetic-password@localhost/synthetic-path?key=synthetic-query#synthetic-fragment".to_string(),
            "Invalid URL",
        ),
        (
            "http://[synthetic-user:synthetic-password/synthetic-path?key=synthetic-query#synthetic-fragment".to_string(),
            "Issue communicating with the AI server (Ollama)",
        ),
    ] {
        let mut command = match executable {
            Some(executable) => fixture.isolated_command(executable),
            None => {
                let mut command = fixture.isolated_command(env!("CARGO_BIN_EXE_cargo-ai"));
                command
                    .args(["--no-update-check", "run", "--config"])
                    .arg(&fixture.definition);
                command
            }
        };
        let output = command
            .env("NO_PROXY", "127.0.0.1,localhost")
            .env("no_proxy", "127.0.0.1,localhost")
            .args(openai_compatible_run_args(
                "ollama", "synthetic-model", None, fixture, &url,
            ))
            .args(["--profile", "diagnostic-profile", "--inference-timeout-in-sec", "2"])
            .output()
            .expect("diagnostic fixture should start");
        let diagnostics = format!(
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!output.status.success(), "invalid endpoint should fail");
        assert!(diagnostics.contains(reason), "missing failure reason: {diagnostics}");
        for secret in [
            "synthetic-user",
            "synthetic-password",
            "synthetic-path",
            "synthetic-query",
            "synthetic-fragment",
        ] {
            assert!(!diagnostics.contains(secret), "endpoint leaked: {diagnostics}");
        }
    }
}

#[test]
fn interpreted_explicit_profile_selection_fails_closed() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind unused default endpoint");
    listener.set_nonblocking(true).unwrap();
    let url = format!(
        "http://{}/v1/chat/completions",
        listener.local_addr().unwrap()
    );
    let valid_config = format!(
        r#"default_profile = "fallback"
[[profile]]
name = "fallback"
server = "ollama"
model = "fallback-model"
url = "{url}"
[[profile]]
name = "empty-server"
server = ""
model = "selected-model"
[[profile]]
name = "unknown-server"
server = "unsupported-provider"
model = "selected-model"
[[profile]]
name = "missing-key"
server = "typesafe"
model = "jev-1.13.0"
auth_mode = "api_key"
"#
    );
    for (name, config, expected) in [
        (
            "missing",
            Some(valid_config.as_str()),
            "Profile 'missing' not found",
        ),
        ("", Some(valid_config.as_str()), "Profile '' not found"),
        (" ", Some(valid_config.as_str()), "Profile ' ' not found"),
        (
            "Fallback",
            Some(valid_config.as_str()),
            "Profile 'Fallback' not found",
        ),
        (
            "empty-server",
            Some(valid_config.as_str()),
            "Unknown AI server",
        ),
        (
            "unknown-server",
            Some(valid_config.as_str()),
            "Unknown AI server",
        ),
        (
            "missing-key",
            Some(valid_config.as_str()),
            "Missing API token",
        ),
        ("missing", None, "Profile 'missing' not found"),
        (
            "missing",
            Some("not valid TOML = ["),
            "Profile 'missing' not found",
        ),
    ] {
        let fixture = Fixture::new();
        if let Some(config) = config {
            fs::write(fixture.home.join("config.toml"), config).unwrap();
        }
        let output = fixture
            .isolated_command(env!("CARGO_BIN_EXE_cargo-ai"))
            .args(["--no-update-check", "run", "--config"])
            .arg(&fixture.definition)
            .args([
                "--profile",
                name,
                "--url",
                &url,
                "--inference-timeout-in-sec",
                "1",
            ])
            .env("NO_PROXY", "127.0.0.1,localhost")
            .env("no_proxy", "127.0.0.1,localhost")
            .output()
            .expect("isolated profile selection should run");
        let diagnostics = format!(
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            !output.status.success(),
            "{name:?} must fail: {diagnostics}"
        );
        assert!(diagnostics.contains(expected), "{name:?}: {diagnostics}");
        assert!(
            !diagnostics.contains("loaded profile: fallback"),
            "{diagnostics}"
        );
        match listener.accept() {
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            other => panic!("{name:?} must not contact the endpoint: {other:?}"),
        }
    }

    for (profile, override_model, expected_model) in [
        (None, None, "fallback-model"),
        (Some("selected"), None, "selected-model"),
        (Some("selected"), Some("override-model"), "override-model"),
    ] {
        let fixture = Fixture::new();
        let mock = MockServer::ollama_success();
        let config = format!(
            r#"default_profile = "fallback"
[[profile]]
name = "fallback"
server = "ollama"
model = "fallback-model"
url = "{url}"
[[profile]]
name = "selected"
server = "ollama"
model = "selected-model"
url = "{url}"
"#,
            url = mock.url
        );
        fs::write(fixture.home.join("config.toml"), config).unwrap();
        let mut command = fixture.isolated_command(env!("CARGO_BIN_EXE_cargo-ai"));
        command
            .args(["--no-update-check", "run", "--config"])
            .arg(&fixture.definition)
            .args(["--inference-timeout-in-sec", "2"])
            .env("NO_PROXY", "127.0.0.1,localhost")
            .env("no_proxy", "127.0.0.1,localhost");
        if let Some(profile) = profile {
            command.args(["--profile", profile]);
        }
        if let Some(model) = override_model {
            command.args(["--model", model]);
        }
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "profile control failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let request = mock.finish();
        let body: Value = serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap();
        assert_eq!(body["model"], expected_model);
    }

    // A caller can still select a provider and model without any saved profile.
    let fixture = Fixture::new();
    run_interpreted_openai_compatible_smoke(
        &fixture,
        "ollama",
        "manual-model",
        None,
        MockServer::ollama_success(),
    );
}

#[test]
fn interpreted_endpoint_diagnostics_exclude_secrets() {
    assert_endpoint_diagnostics_exclude_secrets(&Fixture::new(), None);
}

#[test]
fn interpreted_anthropic_smoke_isolated_and_deterministic() {
    let fixture = Fixture::new();
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
fn interpreted_audio_chain_writes_transcribes_and_forwards_exact_capture() {
    let fixture = Fixture::new();
    let mock = MediaServer::new(media_responses(MEDIA_TRANSCRIPT, true));
    configure_media_fixture(&fixture, &mock.url);
    let output = media_command(&fixture, env!("CARGO_BIN_EXE_cargo-ai"))
        .args(["--no-update-check", "run", "--config"])
        .arg(&fixture.definition)
        .args(media_run_args(&mock.url))
        .output()
        .expect("interpreted audio chain should start");
    let requests = mock.finish();
    assert_media_chain(&fixture, &output, &requests);
}

#[test]
fn interpreted_audio_chain_does_not_forward_empty_transcript() {
    let fixture = Fixture::new();
    let mock = MediaServer::new(media_responses("   ", false));
    configure_media_fixture(&fixture, &mock.url);
    let output = media_command(&fixture, env!("CARGO_BIN_EXE_cargo-ai"))
        .args(["--no-update-check", "run", "--config"])
        .arg(&fixture.definition)
        .args(media_run_args(&mock.url))
        .output()
        .expect("interpreted empty-transcript chain should start");
    assert!(
        !output.status.success(),
        "empty transcript must fail the action"
    );
    let requests = mock.finish();
    assert_eq!(
        requests.len(),
        2,
        "child must not be invoked without a transcript capture"
    );
    assert_eq!(
        fs::read(fixture.root.join("speech.wav")).unwrap(),
        MEDIA_WAV
    );
    let diagnostic = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        diagnostic.contains("no transcript"),
        "missing empty-transcript diagnostic: {diagnostic}"
    );
}

#[test]
#[ignore = "run explicitly in the provider smoke CI lane"]
fn generated_audio_chain_writes_transcribes_and_forwards_exact_capture() {
    generated_audio_chain_case(&Fixture::new());
}

fn generated_audio_chain_case(fixture: &Fixture) {
    write_media_definitions(fixture);
    let output_dir = fixture.root.join("dist");
    let hatch = media_command(&fixture, env!("CARGO_BIN_EXE_cargo-ai"))
        .args([
            "--no-update-check",
            "hatch",
            "media_chain_smoke",
            "--config",
        ])
        .arg(&fixture.definition)
        .arg("--output-dir")
        .arg(&output_dir)
        .arg("--force")
        .output()
        .expect("media chain hatch should start");
    assert!(
        hatch.status.success(),
        "media hatch failed:\n{}\n{}",
        String::from_utf8_lossy(&hatch.stdout),
        String::from_utf8_lossy(&hatch.stderr)
    );
    if fixture.home.join("batch-seed-marker").exists() {
        assert!(
            String::from_utf8_lossy(&hatch.stdout).contains("Reused warmed template"),
            "batch media case should reuse its copied seed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&hatch.stdout),
            String::from_utf8_lossy(&hatch.stderr)
        );
    }
    let executable = output_dir.join(if cfg!(windows) {
        "media_chain_smoke.exe"
    } else {
        "media_chain_smoke"
    });
    let mock = MediaServer::new(media_responses(MEDIA_TRANSCRIPT, true));
    configure_media_profile(&fixture, &mock.url);
    let output = media_command(&fixture, &executable)
        .args(media_run_args(&mock.url))
        .output()
        .expect("generated audio chain should start");
    let requests = mock.finish();
    assert_media_chain(&fixture, &output, &requests);
}

#[test]
#[ignore = "run explicitly in the provider smoke CI lane"]
fn generated_anthropic_smoke_isolated_and_deterministic() {
    generated_anthropic_case(&Fixture::new());
}

fn generated_anthropic_case(fixture: &Fixture) {
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
    if fixture.home.join("batch-seed-marker").exists() {
        assert!(
            String::from_utf8_lossy(&hatch.stdout).contains("Reused warmed template"),
            "batch case should reuse its copied seed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&hatch.stdout),
            String::from_utf8_lossy(&hatch.stderr)
        );
        eprintln!(
            "generated-provider hatch: {}",
            String::from_utf8_lossy(&hatch.stderr)
        );
    }
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
    let fixture = Fixture::new();
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
    generated_gemini_case(&Fixture::new());
}

fn generated_gemini_case(fixture: &Fixture) {
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
    if fixture.home.join("batch-seed-marker").exists() {
        assert!(
            String::from_utf8_lossy(&hatch.stdout).contains("Reused warmed template"),
            "batch case should reuse its copied seed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&hatch.stdout),
            String::from_utf8_lossy(&hatch.stderr)
        );
        eprintln!(
            "generated-provider hatch: {}",
            String::from_utf8_lossy(&hatch.stderr)
        );
    }
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
    let fixture = Fixture::new();
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
    generated_mistral_case(&Fixture::new());
}

fn generated_mistral_case(fixture: &Fixture) {
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
    let fixture = Fixture::new();
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
    generated_xai_case(&Fixture::new());
}

fn generated_xai_case(fixture: &Fixture) {
    run_generated_hosted_smoke(
        &fixture,
        "xai_provider_smoke",
        "xai",
        "grok-smoke",
        XAI_TEST_TOKEN,
        MockServer::xai_success(),
    );
}

#[test]
fn interpreted_openai_smoke_isolated_and_deterministic() {
    let fixture = Fixture::new();
    run_interpreted_openai_compatible_smoke(
        &fixture,
        "openai",
        "openai-smoke",
        Some(OPENAI_TEST_TOKEN),
        MockServer::openai_success(),
    );
}

#[test]
#[ignore = "run explicitly in the provider smoke CI lane"]
fn generated_openai_smoke_isolated_and_deterministic() {
    generated_openai_case(&Fixture::new());
}

fn generated_openai_case(fixture: &Fixture) {
    run_generated_openai_compatible_smoke(
        &fixture,
        "openai_provider_smoke",
        "openai",
        "openai-smoke",
        Some(OPENAI_TEST_TOKEN),
        MockServer::openai_success(),
    );
}

#[test]
fn interpreted_ollama_smoke_isolated_and_deterministic() {
    let fixture = Fixture::new();
    run_interpreted_openai_compatible_smoke(
        &fixture,
        "ollama",
        "ollama-smoke",
        None,
        MockServer::ollama_success(),
    );
}

#[test]
#[ignore = "run explicitly in the provider smoke CI lane"]
fn generated_ollama_smoke_isolated_and_deterministic() {
    generated_ollama_case(&Fixture::new());
}

fn generated_ollama_case(fixture: &Fixture) {
    run_generated_openai_compatible_smoke(
        &fixture,
        "ollama_provider_smoke",
        "ollama",
        "ollama-smoke",
        None,
        MockServer::ollama_success(),
    );
}

#[test]
#[ignore = "run explicitly in the provider smoke CI lane"]
fn generated_provider_batch_isolated_and_deterministic() {
    let seed = Fixture::new();
    let identity =
        provider_cache::CacheIdentity::current(Path::new(env!("CARGO_BIN_EXE_cargo-ai")));
    let started = Instant::now();
    let hatch = seed
        .isolated_command(env!("CARGO_BIN_EXE_cargo-ai"))
        .args([
            "--no-update-check",
            "hatch",
            "neutral_provider_seed",
            "--config",
        ])
        .arg(&seed.definition)
        .arg("--output-dir")
        .arg(seed.root.join("dist"))
        .arg("--force")
        .output()
        .unwrap();
    assert!(
        hatch.status.success(),
        "neutral seed failed: {}\n{}",
        String::from_utf8_lossy(&hatch.stdout),
        String::from_utf8_lossy(&hatch.stderr)
    );
    let cache = provider_cache::SeedCache::capture(&seed.home, identity.clone()).unwrap();
    eprintln!(
        "generated-provider neutral-seed: {:.2}s",
        started.elapsed().as_secs_f64()
    );
    let cases: [(&str, fn(&Fixture)); 8] = [
        ("anthropic", generated_anthropic_case),
        ("gemini", generated_gemini_case),
        ("mistral", generated_mistral_case),
        ("xai", generated_xai_case),
        ("openai", generated_openai_case),
        ("ollama", generated_ollama_case),
        ("typesafe", typesafe_smoke::generated_typesafe_case),
        ("media-chain", generated_audio_chain_case),
    ];
    let mut completed = Vec::new();
    for (provider, case) in cases {
        assert_eq!(
            provider_cache::CacheIdentity::current(Path::new(env!("CARGO_BIN_EXE_cargo-ai"))),
            identity,
            "generated-provider {provider}: CLI/cache identity changed during the batch; rerun with no concurrent CLI builds"
        );
        let fixture = Fixture::new();
        let copied = Instant::now();
        cache.copy_into(&fixture.home, &identity).unwrap();
        eprintln!(
            "generated-provider {provider} copy: {:.2}s",
            copied.elapsed().as_secs_f64()
        );
        fs::write(fixture.home.join("batch-seed-marker"), "neutral").unwrap();
        let mut definition: Value = serde_json::from_str(definition_json()).unwrap();
        definition["inputs"][0]["text"] =
            Value::String(format!("Return a short status. provider_case_{provider}"));
        fs::write(
            &fixture.definition,
            serde_json::to_vec(&definition).unwrap(),
        )
        .unwrap();
        let execution = Instant::now();
        case(&fixture);
        fs::write(fixture.home.join("provider-case-output"), provider).unwrap();
        cache.unchanged().unwrap();
        completed.push(provider);
        eprintln!(
            "generated-provider {provider}: passed, assembly/execution {:.2}s",
            execution.elapsed().as_secs_f64()
        );
    }
    assert_eq!(
        completed,
        [
            "anthropic",
            "gemini",
            "mistral",
            "xai",
            "openai",
            "ollama",
            "typesafe",
            "media-chain"
        ]
    );
    eprintln!(
        "generated-provider batch: 8/8 passed in {:.2}s",
        started.elapsed().as_secs_f64()
    );
}

fn marker_definition(fixture: &Fixture) -> PathBuf {
    let marker = fixture.root.join("action-ran.txt");
    let (program, args, platform) = if cfg!(windows) {
        (
            "cmd",
            vec![
                Value::String("/C".to_string()),
                Value::String("echo".to_string()),
                Value::String("ran>action-ran.txt".to_string()),
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
        let fixture = Fixture::new();
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

    let fixture = Fixture::new();
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

    let fixture = Fixture::new();
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
    let fixture = Fixture::new();
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

    let image_action_definition = fixture.root.join("hosted_image_action.json");
    let image_extension = if provider == "xai" { "jpg" } else { "png" };
    let image_model = if provider == "xai" {
        "grok-imagine-image-2.0"
    } else {
        "mistral-small-latest"
    };
    fs::write(
        &image_action_definition,
        format!(
            r#"{{
  "agent_definition_schema_version": "2026-03-03.r1",
  "inputs": [{{"name":"request","type":"text","text":"Create an image."}}],
  "agent_schema": {{"type":"object","properties":{{}}}},
  "actions": [{{
    "name": "hosted_image_generation",
    "logic": {{"==":[1,1]}},
    "run": [{{
      "kind": "generate_image",
      "model": "{image_model}",
      "prompt": ["Create an image."],
      "path": ["./output.{image_extension}"]
    }}]
  }}]
}}"#
        ),
    )
    .expect("image action definition should be written");
    let image_path = if provider == "xai" {
        "/v1/images/generations"
    } else {
        "/v1/chat/completions"
    };
    let image_rejection = "image model is unavailable";
    let image_error = if provider == "xai" {
        serde_json::json!({"error": {"message": image_rejection}})
    } else {
        serde_json::json!({"message": image_rejection})
    };
    let image_mock =
        MockServer::respond_after_at(image_path, Duration::ZERO, 400, image_error.to_string());
    let generate_failure = fixture
        .isolated_command(env!("CARGO_BIN_EXE_cargo-ai"))
        .args(["--no-update-check", "run", "--config"])
        .arg(&image_action_definition)
        .args([
            "--server",
            provider,
            "--model",
            model,
            "--url",
            &image_mock.url,
            "--token",
            token,
            "--render-mode",
            "append-only",
        ])
        .output()
        .expect("hosted generate_image capability smoke should start");
    let image_request = image_mock.finish();
    let generate_text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&generate_failure.stdout),
        String::from_utf8_lossy(&generate_failure.stderr)
    );
    assert!(!generate_failure.status.success());
    assert!(
        generate_text.contains(image_rejection),
        "unexpected {provider} generate_image diagnostic:\n{generate_text}"
    );
    assert!(
        image_request.starts_with(&format!("POST {image_path} HTTP/1.1")),
        "unexpected {provider} image request: {image_request}"
    );
    let image_body: Value = serde_json::from_str(image_request.split_once("\r\n\r\n").unwrap().1)
        .expect("image request body should be JSON");
    assert_eq!(image_body["model"], image_model);
    if provider == "xai" {
        assert_eq!(image_body["prompt"], "Create an image.");
        assert_eq!(image_body["response_format"], "b64_json");
    } else {
        assert_eq!(image_body["messages"][0]["content"], "Create an image.");
        assert_eq!(image_body["tools"][0]["type"], "image_generation");
    }
    assert!(!fixture
        .root
        .join(format!("output.{image_extension}"))
        .exists());
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
    let fixture = Fixture::new();
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
    let fixture = Fixture::new();
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
    run_live_hosted_smoke("anthropic", "ANTHROPIC_API_KEY", "ANTHROPIC_MODEL");
}

#[test]
#[ignore = "requires GEMINI_API_KEY and GEMINI_MODEL"]
fn live_gemini_smoke_uses_isolated_stdin_credentials() {
    run_live_hosted_smoke("gemini", "GEMINI_API_KEY", "GEMINI_MODEL");
}

fn run_live_hosted_smoke(provider: &str, key_env: &str, model_env: &str) {
    let api_key = std::env::var(key_env).unwrap_or_else(|_| panic!("{key_env} is required"));
    let model = std::env::var(model_env).unwrap_or_else(|_| panic!("{model_env} is required"));
    let report =
        qualification_report::Context::from_environment().expect("valid qualification mode");
    let fixture = Fixture::new();
    let run = run_profile_probe(&fixture, provider, &model, &api_key, None);
    if let Some(report) = report {
        report
            .write(
                provider,
                &model,
                &format!("{provider}-api"),
                &api_key,
                run.status.code(),
                &fixture.usage,
            )
            .expect("complete sanitized qualification report");
    } else {
        strict_live_probe(&run).expect("live provider smoke failed");
        let events = fs::read_to_string(&fixture.usage).expect("usage log should exist");
        assert!(events.contains(&format!("\"server\":\"{provider}\"")));
        assert!(!events.contains(&api_key));
    }
}

fn strict_live_probe(run: &Output) -> Result<(), &'static str> {
    if run.status.success() {
        Ok(())
    } else {
        Err("live provider smoke failed")
    }
}

fn run_profile_probe(
    fixture: &Fixture,
    provider: &str,
    model: &str,
    api_key: &str,
    url: Option<&str>,
) -> Output {
    assert!(
        !api_key.is_empty() && !model.trim().is_empty(),
        "probe credentials/model must be configured"
    );
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

    let mut command = fixture.isolated_command(cli);
    command
        .env_remove("CARGO_AI_USAGE_ROOT_RUN_ID")
        .env_remove("CARGO_AI_USAGE_PARENT_AGENT_RUN_ID");
    command
        .args(["--no-update-check", "run", "--config"])
        .arg(&fixture.definition)
        .args(["--profile", &profile, "--usage-log"])
        .arg(&fixture.usage);
    if let Some(url) = url {
        command.args(["--url", url]);
    }
    command.output().expect("profile probe should start")
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

#[test]
#[ignore = "requires OPENAI_API_KEY and OPENAI_MODEL"]
fn live_openai_smoke_uses_isolated_stdin_credentials() {
    run_live_hosted_smoke("openai", "OPENAI_API_KEY", "OPENAI_MODEL");
}

// This selector is deliberately manual. Its proof directory is a private, persistent
// attempt ledger; reruns against the same directory cannot silently reset the budget.
#[test]
#[ignore = "requires explicit isolated media proof directory and provider API keys"]
fn live_media_bundle_uses_isolated_stdin_credentials() {
    let provider = std::env::var("CARGO_AI_MEDIA_PROVIDER")
        .expect("CARGO_AI_MEDIA_PROVIDER must select openai, gemini, mistral, or xai");
    assert!(
        matches!(provider.as_str(), "openai" | "gemini" | "mistral" | "xai"),
        "unsupported media proof provider"
    );
    let proof_root = PathBuf::from(
        std::env::var_os("CARGO_AI_MEDIA_PROOF_DIR")
            .expect("CARGO_AI_MEDIA_PROOF_DIR must name an explicit private directory"),
    );
    assert!(
        proof_root.is_absolute(),
        "media proof directory must be absolute"
    );
    let metadata = fs::symlink_metadata(&proof_root).expect("media proof directory must exist");
    assert!(metadata.is_dir() && !metadata.file_type().is_symlink());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            metadata.permissions().mode() & 0o077,
            0,
            "proof directory must be private (mode 0700)"
        );
    }
    let key_name = match provider.as_str() {
        "openai" => "OPENAI_API_KEY",
        "gemini" => "GEMINI_API_KEY",
        "mistral" => "MISTRAL_API_KEY",
        _ => "XAI_API_KEY",
    };
    let key = std::env::var(key_name).expect("selected provider API key is required");
    let child_key =
        std::env::var("OPENAI_API_KEY").expect("OPENAI_API_KEY is required for the text child");
    let child_model =
        std::env::var("OPENAI_MODEL").expect("OPENAI_MODEL is required for the text child");
    assert!(
        !key.trim().is_empty() && !child_key.trim().is_empty() && !child_model.trim().is_empty()
    );
    let cases = if provider == "gemini" {
        vec!["speech-wav", "chain", "image", "image-reference"]
    } else if provider == "openai" {
        vec![
            "speech-wav",
            "chain",
            "speech-mp3",
            "image",
            "image-reference",
        ]
    } else if provider == "mistral" {
        vec!["speech-wav", "chain", "speech-mp3", "image"]
    } else {
        vec![
            "speech-wav",
            "chain",
            "speech-mp3",
            "image",
            "image-reference",
        ]
    };
    let selected_case = std::env::var("CARGO_AI_MEDIA_CASE").ok();
    assert!(
        selected_case
            .as_deref()
            .is_none_or(|case| cases.contains(&case)),
        "media case must be one of the selected provider's supported cases"
    );
    let skip_speech = std::env::var("CARGO_AI_MEDIA_SKIP_SPEECH").as_deref() == Ok("1");
    assert!(
        !skip_speech || provider == "mistral",
        "speech skipping is only supported for the Mistral media checkpoint"
    );
    assert!(
        !skip_speech || !matches!(selected_case.as_deref(), Some("speech-wav" | "speech-mp3")),
        "a speech case cannot be selected while Mistral speech is skipped"
    );
    if skip_speech && selected_case.as_deref() != Some("image") {
        assert!(
            std::env::var_os("CARGO_AI_MEDIA_INPUT_WAV").is_some(),
            "Mistral transcription without speech requires CARGO_AI_MEDIA_INPUT_WAV"
        );
    }
    let voice = media_live_voice(
        &provider,
        selected_case.as_deref(),
        skip_speech,
        std::env::var("CARGO_AI_MISTRAL_VOICE_ID").ok().as_deref(),
    );
    for program in ["ffmpeg", "ffprobe"] {
        let output = Command::new(program)
            .arg("-version")
            .output()
            .expect("media decoder is required before provider calls");
        assert!(output.status.success(), "media decoder preflight failed");
        let version = String::from_utf8_lossy(&output.stdout);
        eprintln!(
            "media proof decoder: {}",
            version.lines().next().unwrap_or(program)
        );
    }

    let selected_runtime = std::env::var("CARGO_AI_MEDIA_RUNTIME").ok();
    assert!(
        selected_runtime
            .as_deref()
            .is_none_or(|value| matches!(value, "current" | "hatched")),
        "media runtime must be current or hatched"
    );
    let fixture = Fixture::new();
    let reference = fixture.root.join("reference.png");
    let reference_result = Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=c=blue:s=64x64",
            "-frames:v",
            "1",
            "-y",
        ])
        .arg(&reference)
        .output()
        .expect("reference image generator should start");
    assert!(
        reference_result.status.success(),
        "reference image generation failed before provider calls"
    );
    let reference_bytes = fs::read(&reference).unwrap();
    assert!(!reference_bytes.is_empty() && reference_bytes.len() <= 10 * 1024 * 1024);
    media_live_profile(
        &fixture,
        "media-source",
        &provider,
        media_profile_model(&provider),
        &key,
    );
    media_live_profile(&fixture, "media-child", "openai", &child_model, &child_key);
    let definition = media_live_definition(&provider, &voice);
    let child_definition = media_live_child_definition();
    definition_validation::validate_definition(&definition)
        .expect("media fixture must validate before provider calls");
    definition_validation::validate_definition(&child_definition)
        .expect("media child fixture must validate before provider calls");
    fs::write(
        &fixture.definition,
        serde_json::to_vec(&definition).unwrap(),
    )
    .unwrap();
    fs::write(
        fixture.root.join("child.json"),
        serde_json::to_vec(&child_definition).unwrap(),
    )
    .unwrap();
    let output_dir = fixture.root.join("dist");
    if selected_runtime.as_deref() != Some("current") {
        let hatch = fixture
            .isolated_command(env!("CARGO_BIN_EXE_cargo-ai"))
            .args(["--no-update-check", "hatch", "live_media_proof", "--config"])
            .arg(&fixture.definition)
            .arg("--output-dir")
            .arg(&output_dir)
            .arg("--force")
            .output()
            .expect("media proof hatch should start");
        assert!(
            hatch.status.success(),
            "media proof hatch failed before provider calls"
        );
    }
    let generated = output_dir.join(if cfg!(windows) {
        "live_media_proof.exe"
    } else {
        "live_media_proof"
    });
    let current = Path::new(env!("CARGO_BIN_EXE_cargo-ai"));
    for (runtime, executable) in [("current", current), ("hatched", generated.as_path())] {
        if selected_runtime
            .as_deref()
            .is_some_and(|selected| selected != runtime)
        {
            continue;
        }
        if selected_case.as_deref() == Some("chain") || (skip_speech && selected_case.is_none()) {
            let source = if let Some(explicit) = std::env::var_os("CARGO_AI_MEDIA_INPUT_WAV") {
                media_live_explicit_wav(&proof_root, Path::new(&explicit))
            } else {
                let retry = proof_root.join(format!("{provider}-{runtime}-speech-wav-retry.wav"));
                if retry.exists() {
                    retry
                } else {
                    proof_root.join(format!("{provider}-{runtime}-speech-wav.wav"))
                }
            };
            let input = fixture.root.join("speech.wav");
            fs::copy(source, &input)
                .expect("a selected chain case requires a private WAV proof artifact");
            if std::env::var_os("CARGO_AI_MEDIA_INPUT_WAV").is_some() {
                media_live_verify_input_wav(&input);
            }
        }
        for case in &cases {
            if skip_speech && matches!(*case, "speech-wav" | "speech-mp3") {
                continue;
            }
            if selected_case
                .as_deref()
                .is_some_and(|selected| selected != *case)
            {
                continue;
            }
            let calls = media_live_case_calls(&provider, case);
            let stem = media_live_reserve(
                &proof_root,
                &provider,
                runtime,
                case,
                calls,
                std::env::var("CARGO_AI_MEDIA_RETRY_FAILED").as_deref() == Ok("1"),
            );
            let mut command = media_command(&fixture, executable);
            if runtime == "current" {
                command
                    .args(["--no-update-check", "run", "--config"])
                    .arg(&fixture.definition);
            }
            let output = command
                .args(["--profile", "media-child", "--run-var"])
                .arg(format!("case={case}"))
                .env_remove("OPENAI_API_KEY")
                .env_remove("GEMINI_API_KEY")
                .env_remove("MISTRAL_API_KEY")
                .env_remove("XAI_API_KEY")
                .output()
                .expect("media case process should start; reserved attempts remain charged if it cannot");
            media_live_record(
                &proof_root,
                &fixture,
                &provider,
                runtime,
                case,
                &stem,
                &output,
            );
            assert!(
                output.status.success(),
                "media proof case failed; inspect private result and ledger without blindly rerunning"
            );
        }
    }
}

fn media_live_voice(
    provider: &str,
    selected_case: Option<&str>,
    skip_speech: bool,
    mistral_voice: Option<&str>,
) -> String {
    match provider {
        "openai" => "coral".into(),
        "gemini" => "Kore".into(),
        "xai" => "eve".into(),
        "mistral" if skip_speech || matches!(selected_case, Some("chain" | "image")) => "fixture-voice".into(),
        "mistral" => mistral_voice
            .filter(|voice| !voice.trim().is_empty())
            .expect("BLOCKED: an existing permitted CARGO_AI_MISTRAL_VOICE_ID is required for Mistral speech; no Mistral request was started")
            .into(),
        _ => unreachable!(),
    }
}

fn media_live_explicit_wav(proof_root: &Path, source: &Path) -> PathBuf {
    assert!(
        source.is_absolute(),
        "explicit media input WAV must be absolute"
    );
    let source_metadata =
        fs::symlink_metadata(source).expect("explicit media input WAV must exist");
    assert!(
        source_metadata.is_file() && !source_metadata.file_type().is_symlink(),
        "explicit media input WAV must be a regular file, not a symlink"
    );
    assert!(
        source
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("wav")),
        "explicit media input must be a WAV file"
    );
    assert!(
        source_metadata.len() > 0 && source_metadata.len() <= 10 * 1024 * 1024,
        "explicit media input WAV must be at most 10 MiB"
    );
    let canonical_root = proof_root
        .canonicalize()
        .expect("private proof root must exist");
    let canonical_source = source
        .canonicalize()
        .expect("explicit media input WAV must resolve");
    assert!(
        canonical_source.starts_with(&canonical_root),
        "explicit media input WAV must belong to the private proof directory"
    );
    canonical_source
}

#[test]
fn media_live_mistral_voice_is_required_only_for_selected_speech() {
    assert_eq!(
        media_live_voice("mistral", Some("image"), false, None),
        "fixture-voice"
    );
    assert_eq!(
        media_live_voice("mistral", Some("chain"), false, None),
        "fixture-voice"
    );
    assert_eq!(
        media_live_voice("mistral", None, true, None),
        "fixture-voice"
    );
    assert_eq!(
        media_live_voice("mistral", None, false, Some("saved-voice")),
        "saved-voice"
    );
    assert!(std::panic::catch_unwind(|| media_live_voice("mistral", None, false, None)).is_err());
    assert!(std::panic::catch_unwind(|| media_live_voice(
        "mistral",
        Some("speech-wav"),
        false,
        None
    ))
    .is_err());
}

#[test]
fn media_live_explicit_wav_must_be_a_regular_file_in_private_proof_root() {
    let fixture = Fixture::new();
    let proof_root = fixture.root.join("private-proof");
    fs::create_dir(&proof_root).unwrap();
    let input = proof_root.join("input.wav");
    fs::write(&input, b"RIFFfixture").unwrap();
    assert_eq!(
        media_live_explicit_wav(&proof_root, &input),
        input.canonicalize().unwrap()
    );
    assert!(std::panic::catch_unwind(|| media_live_explicit_wav(
        &proof_root,
        Path::new("input.wav")
    ))
    .is_err());
    let outside = fixture.root.join("outside.wav");
    fs::write(&outside, b"RIFFfixture").unwrap();
    assert!(std::panic::catch_unwind(|| media_live_explicit_wav(&proof_root, &outside)).is_err());
    #[cfg(unix)]
    {
        let link = proof_root.join("link.wav");
        std::os::unix::fs::symlink(&input, &link).unwrap();
        assert!(std::panic::catch_unwind(|| media_live_explicit_wav(&proof_root, &link)).is_err());
    }
}

fn media_live_verify_input_wav(input: &Path) {
    let probe = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "a:0",
            "-show_entries",
            "stream=codec_name,duration:format=duration",
            "-of",
            "json",
        ])
        .arg(input)
        .output()
        .expect("explicit media input WAV probe should start");
    assert!(
        probe.status.success(),
        "explicit media input WAV must be probeable"
    );
    let facts: Value = serde_json::from_slice(&probe.stdout).expect("valid WAV probe JSON");
    assert!(
        facts["streams"][0]["codec_name"]
            .as_str()
            .is_some_and(|codec| codec.starts_with("pcm_")),
        "explicit media input WAV must contain PCM audio"
    );
    let duration = facts["format"]["duration"]
        .as_str()
        .or_else(|| facts["streams"][0]["duration"].as_str())
        .and_then(|value| value.parse::<f64>().ok())
        .expect("explicit media input WAV must expose a duration");
    assert!(
        duration.is_finite() && duration > 0.0 && duration <= 20.0,
        "explicit media input WAV must be at most 20 seconds"
    );
    let decoded = Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(input)
        .args([
            "-t",
            "21",
            "-f",
            "s16le",
            "-acodec",
            "pcm_s16le",
            "-ac",
            "1",
            "-ar",
            "8000",
            "-",
        ])
        .output()
        .expect("explicit media input WAV decoder should start");
    assert!(
        decoded.status.success()
            && !decoded.stdout.is_empty()
            && decoded.stdout.len() <= 20 * 8000 * 2
            && decoded
                .stdout
                .chunks_exact(2)
                .any(|sample| sample != [0, 0]),
        "explicit media input WAV must decode to bounded non-silent audio"
    );
}

fn media_profile_model(provider: &str) -> &'static str {
    match provider {
        "openai" => "gpt-4o-mini-tts",
        "gemini" => "gemini-3.8-flash-tts",
        "mistral" => "mistral-small-latest",
        _ => "grok-imagine-image-2.0",
    }
}

fn media_live_profile(fixture: &Fixture, name: &str, provider: &str, model: &str, key: &str) {
    let cli = env!("CARGO_BIN_EXE_cargo-ai");
    let added = fixture
        .isolated_command(cli)
        .args([
            "--no-update-check",
            "profile",
            "add",
            name,
            "--server",
            provider,
            "--model",
            model,
            "--auth",
            "api_key",
        ])
        .output()
        .expect("media profile add should start");
    assert!(
        added.status.success(),
        "media profile add failed before provider calls"
    );
    let mut child = fixture
        .isolated_command(cli)
        .args(["--no-update-check", "profile", "set", name, "--stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("media profile credential setup should start");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(key.as_bytes())
        .unwrap();
    assert!(
        child.wait().unwrap().success(),
        "media profile credential setup failed before provider calls"
    );
}

fn media_live_child_definition() -> Value {
    serde_json::json!({
        "agent_definition_schema_version":"2026-09-09.r1",
        "inputs":[{"type":"text","text":"Read the supplied transcript and obey its verification instruction."}],
        "agent_schema":{"type":"object","properties":{"status":{"type":"string","enum":["heard-blue-lantern-silver-compass"]}}},
        "actions":[]
    })
}

#[test]
fn media_live_fixtures_validate_before_provider_requests() {
    definition_validation::validate_definition(&media_live_child_definition()).unwrap();
    for provider in ["openai", "gemini", "mistral", "xai"] {
        definition_validation::validate_definition(&media_live_definition(
            provider,
            "fixture-voice",
        ))
        .unwrap();
    }
}

fn media_live_definition(provider: &str, voice: &str) -> Value {
    let speech_model = match provider {
        "openai" => Some("gpt-4o-mini-tts"),
        "gemini" => Some("gemini-3.8-flash-tts"),
        "mistral" => Some("voxtral-mini-tts-2603"),
        _ => None,
    };
    let transcription_model = match provider {
        "openai" => "gpt-transcribe",
        "gemini" => "gemini-3.5-transcribe",
        "mistral" => "voxtral-mini-latest",
        _ => "grok-voice-transcribe-2.0",
    };
    let image_model = match provider {
        "openai" => "gpt-image-2",
        "gemini" => "gemini-3.1-flash-image",
        "mistral" => "mistral-small-latest",
        _ => "grok-imagine-image-2.0",
    };
    let script = "The blue lantern rests beside the silver compass.";
    let mut speech = serde_json::json!({"kind":"generate_audio","profile":"media-source","text":script,"voice":voice,"path":"./speech.wav"});
    if let Some(model) = speech_model {
        speech["model"] = Value::String(model.into());
    }
    let mut mp3 = speech.clone();
    mp3["path"] = Value::String("./speech.mp3".into());
    let chain = vec![
        serde_json::json!({"kind":"transcribe_audio","profile":"media-source","model":transcription_model,"audio":{"path":"./speech.wav"},"output_variable":"transcript"}),
        serde_json::json!({"kind":"exec","program":"printf","args":["TRANSCRIPT_PROOF=%s\\n",{"var":"transcript"}]}),
        serde_json::json!({"kind":"agent","artifact":"./child.json","profile":"media-child","inputs":[{"type":"text","text":["Read the transcript after this colon. If and only if it says blue lantern and silver compass, return exactly {\"status\":\"heard-blue-lantern-silver-compass\"}; otherwise return {\"status\":\"missing-content\"}. Transcript: ",{"var":"transcript"}]}]}),
    ];
    let image_extension = if matches!(provider, "gemini" | "xai") {
        "jpg"
    } else {
        "png"
    };
    let image = serde_json::json!({"kind":"generate_image","profile":"media-source","model":image_model,"prompt":"One prominent solid red circle centered on a plain white background.","path":format!("./image.{image_extension}")});
    let mut image_reference = image.clone();
    image_reference["path"] = Value::String(format!("./image-reference.{image_extension}"));
    image_reference["reference_images"] = serde_json::json!([{"path":"./reference.png"}]);
    if provider == "gemini" {
        image_reference["prompt"] = Value::String("Use the supplied blue reference image to add a small blue square in the upper-left corner, while keeping one prominent solid red circle centered on a plain white background.".into());
    }
    serde_json::json!({
        "agent_definition_schema_version":"2026-09-09.r1",
        "agent_schema":{"type":"object","properties":{}},
        "runtime_vars":{"case":{"type":"string","default":"chain"}},
        "actions":[
            {"name":"speech_wav","logic":{"==":[{"var":"runtime.case"},"speech-wav"]},"run":[speech]},
            {"name":"chain","logic":{"==":[{"var":"runtime.case"},"chain"]},"run":chain},
            {"name":"speech_mp3","logic":{"==":[{"var":"runtime.case"},"speech-mp3"]},"run":[mp3]},
            {"name":"image","logic":{"==":[{"var":"runtime.case"},"image"]},"run":[image]},
            {"name":"image_reference","logic":{"==":[{"var":"runtime.case"},"image-reference"]},"run":[image_reference]}
        ]
    })
}

fn media_live_case_calls(provider: &str, case: &str) -> [u32; 5] {
    match case {
        "speech-wav" => [1, 0, 0, 0, 0],
        "chain" => [0, 1, 0, 1, 0],
        "speech-mp3" => [1, 0, 0, 0, 0],
        "image" | "image-reference" => [0, 0, 1, 0, u32::from(provider == "mistral")],
        _ => unreachable!(),
    }
}

fn media_live_reserve(
    root: &Path,
    provider: &str,
    runtime: &str,
    case: &str,
    calls: [u32; 5],
    retry_failed: bool,
) -> String {
    let ledger = root.join("attempts.json");
    let mut value: Value = if ledger.exists() {
        serde_json::from_slice(&fs::read(&ledger).unwrap())
            .expect("valid persistent attempt ledger")
    } else {
        serde_json::json!({"tts":0,"stt":0,"image":0,"child":0,"mistral_image":0,"cases":[]})
    };
    let prior_attempts = value["cases"]
        .as_array()
        .expect("valid attempt cases")
        .iter()
        .filter(|entry| {
            entry["provider"] == provider && entry["runtime"] == runtime && entry["case"] == case
        })
        .count();
    let original_stem = format!("{provider}-{runtime}-{case}");
    if prior_attempts != 0 {
        assert!(prior_attempts == 1 && retry_failed, "media case requires an explicit single diagnosed retry; passed or uncertain cases cannot be replayed");
        let previous: Value = serde_json::from_slice(
            &fs::read(root.join(format!("{original_stem}.json")))
                .expect("retry requires a recorded failed result"),
        )
        .unwrap();
        assert!(
            previous["exit_code"].as_i64().is_some_and(|code| code != 0),
            "only a recorded failure can be retried"
        );
    }
    let stem = if prior_attempts == 0 {
        original_stem
    } else {
        format!("{original_stem}-retry")
    };
    for (index, (name, limit)) in [
        ("tts", 16),
        ("stt", 12),
        ("image", 24),
        ("child", 12),
        ("mistral_image", 6),
    ]
    .iter()
    .enumerate()
    {
        let prior = value[name].as_u64().expect("valid attempt counter");
        let next = prior + u64::from(calls[index]);
        assert!(
            next <= *limit,
            "media proof attempt budget exhausted before dispatch"
        );
        value[name] = Value::from(next);
    }
    let total = ["tts", "stt", "image", "child"]
        .iter()
        .map(|name| value[name].as_u64().unwrap())
        .sum::<u64>();
    assert!(
        total <= 64,
        "media proof total budget exhausted before dispatch"
    );
    value["cases"].as_array_mut().unwrap().push(serde_json::json!({"provider":provider,"runtime":runtime,"case":case,"attempt":prior_attempts + 1,"evidence_stem":stem,"reserved":calls,"state":"started_or_uncertain"}));
    let temporary = root.join("attempts.json.tmp");
    fs::write(&temporary, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
    fs::rename(temporary, ledger).unwrap();
    stem
}

#[test]
fn media_live_retry_preserves_failure_and_allows_only_one_diagnosed_attempt() {
    let fixture = Fixture::new();
    let root = fixture.root.as_path();
    let reserve = |retry| {
        media_live_reserve(
            root,
            "openai",
            "current",
            "speech-wav",
            [1, 0, 0, 0, 0],
            retry,
        )
    };
    assert_eq!(reserve(false), "openai-current-speech-wav");
    let ledger_before = fs::read(root.join("attempts.json")).unwrap();
    assert!(std::panic::catch_unwind(|| reserve(true)).is_err());
    assert_eq!(fs::read(root.join("attempts.json")).unwrap(), ledger_before);
    let report = root.join("openai-current-speech-wav.json");
    fs::write(&report, br#"{"exit_code":0}"#).unwrap();
    assert!(std::panic::catch_unwind(|| reserve(true)).is_err());
    fs::write(&report, br#"{"exit_code":1}"#).unwrap();
    assert!(std::panic::catch_unwind(|| reserve(false)).is_err());
    assert_eq!(reserve(true), "openai-current-speech-wav-retry");
    assert_eq!(fs::read(&report).unwrap(), br#"{"exit_code":1}"#);
    assert!(std::panic::catch_unwind(|| reserve(true)).is_err());
    let ledger: Value =
        serde_json::from_slice(&fs::read(root.join("attempts.json")).unwrap()).unwrap();
    assert_eq!(ledger["tts"], 2);
    assert_eq!(ledger["cases"].as_array().unwrap().len(), 2);
}

#[test]
fn media_live_retry_cannot_reset_the_existing_request_budget() {
    let fixture = Fixture::new();
    let ledger = fixture.root.join("attempts.json");
    let exhausted = br#"{"tts":16,"stt":0,"image":2,"child":0,"mistral_image":2,"cases":[]}"#;
    fs::write(&ledger, exhausted).unwrap();
    assert!(std::panic::catch_unwind(|| media_live_reserve(
        &fixture.root,
        "openai",
        "current",
        "speech-wav",
        [1, 0, 0, 0, 0],
        true
    ))
    .is_err());
    assert_eq!(fs::read(&ledger).unwrap(), exhausted);
}

fn media_live_record(
    root: &Path,
    fixture: &Fixture,
    provider: &str,
    runtime: &str,
    case: &str,
    stem: &str,
    output: &Output,
) {
    let path = match case {
        "chain" | "speech-wav" => fixture.root.join("speech.wav"),
        "speech-mp3" => fixture.root.join("speech.mp3"),
        "image" => fixture.root.join(if matches!(provider, "gemini" | "xai") {
            "image.jpg"
        } else {
            "image.png"
        }),
        "image-reference" => fixture.root.join(if matches!(provider, "gemini" | "xai") {
            "image-reference.jpg"
        } else {
            "image-reference.png"
        }),
        _ => unreachable!(),
    };
    let mut evidence = serde_json::json!({
        "provider":provider,"runtime":runtime,"case":case,
        "profile":"media-source",
        "models":match case {
            "speech-wav" | "speech-mp3" => if provider == "xai" { serde_json::json!([null]) } else { serde_json::json!([if provider == "openai" { "gpt-4o-mini-tts" } else if provider == "gemini" { "gemini-3.8-flash-tts" } else { "voxtral-mini-tts-2603" }]) },
            "chain" => serde_json::json!([match provider { "openai" => "gpt-transcribe", "gemini" => "gemini-3.5-transcribe", "mistral" => "voxtral-mini-latest", _ => "grok-voice-transcribe-2.0" }, std::env::var("OPENAI_MODEL").unwrap_or_default()]),
            _ => serde_json::json!([if provider == "openai" { "gpt-image-2" } else if provider == "gemini" { "gemini-3.1-flash-image" } else if provider == "mistral" { "mistral-small-latest" } else { "grok-imagine-image-2.0" }]),
        },
        "time_unix_seconds":std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs(),
        "candidate":std::env::var("CARGO_AI_SHA").ok(),
        "exit_code":output.status.code(),
        "stdout_sha256":format!("{:x}", Sha256::digest(&output.stdout)),
        "stderr_sha256":format!("{:x}", Sha256::digest(&output.stderr))
    });
    let selected_key = match provider {
        "openai" => "OPENAI_API_KEY",
        "gemini" => "GEMINI_API_KEY",
        "mistral" => "MISTRAL_API_KEY",
        _ => "XAI_API_KEY",
    };
    let secrets = [
        std::env::var(selected_key).unwrap_or_default(),
        std::env::var("OPENAI_API_KEY").unwrap_or_default(),
        std::env::var("CARGO_AI_MISTRAL_VOICE_ID").unwrap_or_default(),
    ];
    fs::write(
        root.join(format!("{stem}.stdout.txt")),
        media_live_private_diagnostic(&output.stdout, &secrets),
    )
    .expect("private sanitized stdout should be retained");
    fs::write(
        root.join(format!("{stem}.stderr.txt")),
        media_live_private_diagnostic(&output.stderr, &secrets),
    )
    .expect("private sanitized stderr should be retained");
    if output.status.success() {
        let bytes = fs::read(&path).expect("successful media case must write its file");
        let extension = path.extension().unwrap().to_string_lossy();
        let private_copy = root.join(format!("{stem}.{extension}"));
        fs::write(&private_copy, &bytes).expect("private media artifact should be retained");
        evidence["artifact_sha256"] = Value::String(format!("{:x}", Sha256::digest(&bytes)));
        evidence["artifact_bytes"] = Value::from(bytes.len());
        fs::write(
            root.join(format!("{stem}.json")),
            serde_json::to_vec_pretty(&evidence).unwrap(),
        )
        .unwrap();
        let stream = if matches!(case, "chain" | "speech-wav" | "speech-mp3") {
            "a:0"
        } else {
            "v:0"
        };
        let probe = Command::new("ffprobe")
            .args([
                "-v",
                "error",
                "-select_streams",
                stream,
                "-show_entries",
                "stream=codec_name,width,height,duration:format=duration,format_name",
                "-of",
                "json",
            ])
            .arg(&path)
            .output()
            .expect("ffprobe should start");
        assert!(probe.status.success(), "media file must be probeable");
        let facts: Value = serde_json::from_slice(&probe.stdout).expect("valid ffprobe JSON");
        let codec = facts["streams"][0]["codec_name"]
            .as_str()
            .expect("decoded codec must be identified");
        match extension.as_ref() {
            "wav" => assert!(
                codec.starts_with("pcm_"),
                "WAV must contain decoded PCM audio"
            ),
            "mp3" => assert_eq!(codec, "mp3", "MP3 must decode as MP3"),
            "png" => assert_eq!(codec, "png", "PNG must decode as PNG"),
            "jpg" => assert_eq!(codec, "mjpeg", "JPEG must decode as JPEG"),
            _ => unreachable!(),
        }
        if matches!(case, "chain" | "speech-wav" | "speech-mp3") {
            let duration = facts["format"]["duration"]
                .as_str()
                .or_else(|| facts["streams"][0]["duration"].as_str())
                .and_then(|value| value.parse::<f64>().ok())
                .expect("speech file must expose a finite duration");
            assert!(
                duration.is_finite() && duration > 0.0 && duration <= 20.0,
                "speech proof audio must be at most 20 seconds"
            );
        }
        let decoded = if matches!(case, "chain" | "speech-wav" | "speech-mp3") {
            Command::new("ffmpeg")
                .args(["-v", "error", "-i"])
                .arg(&path)
                .args([
                    "-t",
                    "21",
                    "-f",
                    "s16le",
                    "-acodec",
                    "pcm_s16le",
                    "-ac",
                    "1",
                    "-ar",
                    "8000",
                    "-",
                ])
                .output()
                .expect("audio decoder should start")
        } else {
            Command::new("ffmpeg")
                .args(["-v", "error", "-i"])
                .arg(&path)
                .args([
                    "-frames:v",
                    "1",
                    "-vf",
                    "scale=256:256",
                    "-f",
                    "rawvideo",
                    "-pix_fmt",
                    "rgb24",
                    "-",
                ])
                .output()
                .expect("image decoder should start")
        };
        assert!(
            decoded.status.success() && !decoded.stdout.is_empty(),
            "media file must actually decode"
        );
        if matches!(case, "chain" | "speech-wav" | "speech-mp3") {
            assert!(
                decoded
                    .stdout
                    .chunks_exact(2)
                    .any(|sample| sample != [0, 0]),
                "speech file must not be silent"
            );
            assert!(
                decoded.stdout.len() <= 20 * 8000 * 2,
                "speech proof fixture must be at most 20 seconds"
            );
            assert!(
                bytes.len() <= 10 * 1024 * 1024,
                "transcription proof audio must be at most 10 MiB"
            );
        } else {
            assert!(
                decoded.stdout.chunks_exact(3).any(|pixel| pixel[0] > 150
                    && pixel[0] > pixel[1].saturating_add(40)
                    && pixel[0] > pixel[2].saturating_add(40)),
                "decoded image must contain the requested prominent red content"
            );
        }
        if case == "chain" {
            let text = String::from_utf8_lossy(&output.stdout);
            let transcript = text.lines().find_map(|line| {
                line.split_once("TRANSCRIPT_PROOF=")
                    .map(|(_, value)| value.trim())
            });
            let transcript =
                transcript.expect("captured transcript must be available as local proof");
            let normalized = transcript.to_ascii_lowercase();
            assert!(
                normalized.contains("blue lantern") && normalized.contains("silver compass"),
                "transcript must contain the supplied speech content"
            );
            assert!(
                text.contains("child: completed successfully"),
                "native text child must complete after transcript capture"
            );
            evidence["transcript_sha256"] =
                Value::String(format!("{:x}", Sha256::digest(transcript.as_bytes())));
        }
        if case == "speech-mp3" {
            evidence["spoken_content_manual_listen_pending"] = Value::Bool(true);
        }
        if matches!(case, "image" | "image-reference") {
            evidence["visual_content_review_pending"] = Value::Bool(true);
        }
        evidence["decoded_bytes"] = Value::from(decoded.stdout.len());
        evidence["probe"] = facts;
    }
    let report = root.join(format!("{stem}.json"));
    fs::write(report, serde_json::to_vec_pretty(&evidence).unwrap()).unwrap();
}

fn media_live_private_diagnostic(raw: &[u8], secrets: &[String]) -> String {
    let mut result = String::new();
    for line in String::from_utf8_lossy(raw).lines() {
        let lower = line.to_ascii_lowercase();
        if lower.contains("://") || lower.contains("url") || lower.contains('?') {
            result.push_str("[URL-bearing diagnostic omitted]\n");
            continue;
        }
        let mut safe = line.to_string();
        for secret in secrets.iter().filter(|secret| !secret.is_empty()) {
            safe = safe.replace(secret, "[redacted]");
        }
        result.push_str(&safe);
        result.push('\n');
    }
    result
}

#[test]
fn qualification_reports_validate_real_probes_and_keep_strict_diagnostics() {
    let successes: [(&str, fn() -> MockServer); 5] = [
        ("anthropic", MockServer::success),
        ("gemini", MockServer::gemini_success),
        ("mistral", MockServer::mistral_success),
        ("xai", MockServer::xai_success),
        ("openai", MockServer::openai_success),
    ];
    for (provider, server) in successes {
        qualification_probe_case(provider, server(), "pass", "none");
    }
    for (status, expected, diagnostic) in [
        (429, "rate_limited", "rate_limited"),
        (401, "failure", "unauthorized"),
        (400, "failure", "invalid_request"),
        (500, "failure", "server_error"),
    ] {
        qualification_probe_case(
            "mistral",
            MockServer::respond_after_at(
                "/v1/chat/completions",
                Duration::ZERO,
                status,
                r#"{"message":"private-response-sentinel"}"#.into(),
            ),
            expected,
            diagnostic,
        );
    }
    qualification_probe_case(
        "mistral",
        MockServer::respond_after_at(
            "/v1/chat/completions",
            Duration::ZERO,
            200,
            mistral_success_response(r#"{"status":false}"#),
        ),
        "failure",
        "execution_failure",
    );
}

fn qualification_probe_case(provider: &str, mock: MockServer, expected: &str, diagnostic: &str) {
    let fixture = Fixture::new();
    let model = "qualification-mock-model";
    let token = "qualification-fake-secret-sentinel";
    let output = run_profile_probe(&fixture, provider, model, token, Some(&mock.url));
    let request = mock.finish();
    assert!(request.contains(model));
    assert!(request.contains(token));
    let report = qualification_report::Context {
        path: fixture.root.join("report.json"),
        candidate: "a".repeat(40),
        run_id: "123".into(),
        run_attempt: "2".into(),
        probe_id: "b".repeat(32),
    };
    assert_eq!(
        report
            .write(
                provider,
                model,
                &format!("{provider}-api"),
                token,
                output.status.code(),
                &fixture.usage
            )
            .unwrap(),
        expected
    );
    let saved = fs::read(&report.path).unwrap();
    let detail: Value = serde_json::from_slice(&saved).unwrap();
    assert_eq!(detail["diagnostic"], diagnostic);
    for forbidden in [token, model, "private-response-sentinel", "cps-"] {
        assert!(!String::from_utf8_lossy(&saved).contains(forbidden));
    }
    assert!(report
        .write(
            provider,
            model,
            &format!("{provider}-api"),
            token,
            output.status.code(),
            &fixture.usage
        )
        .is_err());
    assert_eq!(fs::read(&report.path).unwrap(), saved);
    if expected == "pass" {
        assert!(strict_live_probe(&output).is_ok());
    } else {
        assert!(strict_live_probe(&output).is_err());
    }
    // Exercise the actual producer/consumer boundary without a real workflow or service.
    let raw = fs::read(&report.path).unwrap();
    let decision = qualification_policy::evaluate(
        provider,
        &raw,
        &qualification_policy::Identity {
            candidate: &"a".repeat(40),
            run_id: "123",
            run_attempt: "2",
            probe_id: Some(&"b".repeat(32)),
        },
        "success",
        "true",
    )
    .unwrap();
    assert_eq!(
        decision.accepted,
        expected == "pass" || expected == "rate_limited"
    );

    if expected == "pass" && provider == "mistral" {
        let raw = fs::read_to_string(&fixture.usage).unwrap();
        let events: Vec<Value> = raw
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let serialize = |items: &[Value]| {
            items
                .iter()
                .map(Value::to_string)
                .collect::<Vec<_>>()
                .join("\n")
        };
        let classify = |value: &str, code| {
            qualification_report::classify(value, provider, model, &format!("{provider}-api"), code)
        };
        assert!(classify(&raw, None).is_err());
        assert!(classify(&raw, Some(2)).is_err());
        assert!(classify(&raw, Some(1)).is_err());
        assert!(classify(&serialize(&events[..4]), Some(0)).is_err());
        assert!(classify(&(raw.clone() + &raw), Some(0)).is_err());
        assert!(classify("invalid", Some(0)).is_err());
        for (index, field, value) in [
            (2, "root_run_id", Value::String("stale".into())),
            (2, "agent_run_id", Value::String("stale".into())),
            (2, "status", Value::String("failed".into())),
            (4, "status", Value::String("failed".into())),
        ] {
            let mut changed = events.clone();
            changed[index][field] = value;
            assert!(classify(&serialize(&changed), Some(0)).is_err());
        }
        let mut changed = events.clone();
        changed[2]["provider"]["server"] = Value::String("xai".into());
        assert!(classify(&serialize(&changed), Some(0)).is_err());
    }
}
