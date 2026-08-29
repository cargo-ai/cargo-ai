use std::ffi::OsStr;
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

static NEXT_FIXTURE_ID: AtomicU64 = AtomicU64::new(0);

pub struct Fixture {
    pub root: PathBuf,
    pub cargo_ai_home: PathBuf,
    pub fallback_home: PathBuf,
}

impl Fixture {
    pub fn new(_label: &str) -> Self {
        let base = std::env::var_os("RUNNER_TEMP")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        fs::create_dir_all(&base).expect("fixture base should be created");
        let sequence = NEXT_FIXTURE_ID.fetch_add(1, Ordering::Relaxed);
        let root = base.join(format!("q-{:x}-{sequence:x}", std::process::id()));
        fs::create_dir(&root).expect("fixture root should be unique");
        let cargo_ai_home = root.join("h");
        let fallback_home = root.join("u");
        fs::create_dir_all(&cargo_ai_home).expect("isolated Cargo AI Home should be created");
        fs::create_dir_all(&fallback_home).expect("fallback user home should be created");
        Self {
            root,
            cargo_ai_home,
            fallback_home,
        }
    }

    pub fn cargo_ai_command(&self, current_dir: &Path) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cargo-ai"));
        command
            .current_dir(current_dir)
            .env("CARGO_AI_HOME", &self.cargo_ai_home)
            .env("CARGO_AI_DISABLE_KEYCHAIN", "1")
            .env("HOME", &self.fallback_home);
        command
    }

    #[allow(dead_code)]
    pub fn command(&self, program: impl AsRef<OsStr>, current_dir: &Path) -> Command {
        let mut command = Command::new(program);
        command
            .current_dir(current_dir)
            .env("CARGO_AI_HOME", &self.cargo_ai_home)
            .env("CARGO_AI_DISABLE_KEYCHAIN", "1")
            .env("HOME", &self.fallback_home);
        command
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

pub fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

#[allow(dead_code)]
pub fn copy_tree(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).expect("fixture destination should be created");
    for entry in fs::read_dir(source).expect("fixture source should be readable") {
        let entry = entry.expect("fixture entry should be readable");
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        if entry
            .file_type()
            .expect("fixture type should be readable")
            .is_dir()
        {
            copy_tree(&source_path, &destination_path);
        } else {
            fs::copy(&source_path, &destination_path).expect("fixture file should be copied");
        }
    }
}

pub fn output_text(output: &Output) -> String {
    format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

pub fn assert_success(output: &Output, label: &str) {
    assert!(
        output.status.success(),
        "{label} failed with {}\n{}",
        output.status,
        output_text(output)
    );
}

pub struct OneShotHttpServer {
    pub url: String,
    request: thread::JoinHandle<Option<String>>,
}

impl OneShotHttpServer {
    pub fn json(path: &str, body: serde_json::Value) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("loopback listener should bind");
        listener
            .set_nonblocking(true)
            .expect("loopback listener should be nonblocking");
        let address = listener
            .local_addr()
            .expect("loopback address should resolve");
        let path = path.to_string();
        let response_path = path.clone();
        let body = body.to_string();
        let request = thread::spawn(move || {
            let started = Instant::now();
            let (mut stream, _) = loop {
                match listener.accept() {
                    Ok(connection) => break connection,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        if started.elapsed() > Duration::from_secs(15) {
                            return None;
                        }
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => panic!("loopback accept failed: {error}"),
                }
            };
            let request = read_http_request(&mut stream);
            let status_line = if request
                .lines()
                .next()
                .is_some_and(|line| line.contains(response_path.as_str()))
            {
                "HTTP/1.1 200 OK"
            } else {
                "HTTP/1.1 404 Not Found"
            };
            let response = format!(
                "{status_line}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream
                .write_all(response.as_bytes())
                .expect("loopback response should be written");
            Some(request)
        });
        Self {
            url: format!("http://{address}{path}"),
            request,
        }
    }

    pub fn finish(self) -> String {
        self.request
            .join()
            .expect("loopback server should finish")
            .expect("loopback server should receive a request")
    }
}

fn read_http_request(stream: &mut TcpStream) -> String {
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("request timeout should configure");
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 4096];
    let mut expected_length = None;
    loop {
        let count = stream
            .read(&mut buffer)
            .expect("request should be readable");
        if count == 0 {
            break;
        }
        bytes.extend_from_slice(&buffer[..count]);
        if let Some(header_end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
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
            expected_length = Some(header_end + 4 + content_length);
        }
        if expected_length.is_some_and(|length| bytes.len() >= length) {
            break;
        }
    }
    String::from_utf8(bytes).expect("request should be UTF-8")
}

pub fn openai_success_response(status: &str) -> serde_json::Value {
    serde_json::json!({
        "id": "chatcmpl-qualification",
        "object": "chat.completion",
        "created": 1,
        "model": "qualification-model",
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": serde_json::json!({"status": status}).to_string()
            },
            "finish_reason": "stop"
        }],
        "usage": {
            "prompt_tokens": 3,
            "completion_tokens": 2,
            "total_tokens": 5
        }
    })
}
