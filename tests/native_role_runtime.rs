//! Real CLI and emitted executable acceptance using only isolated state and loopback providers.
//!
//! Compatible evidence enters through the private native-parent pipe. This fixture does not
//! add a public discovery override or treat a caller's evidence as native authority.
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const CLI: &str = env!("CARGO_BIN_EXE_cargo-ai");
const PNG_B64: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAAAXNSR0IArs4c6QAAAERlWElmTU0AKgAAAAgAAYdpAAQAAAABAAAAGgAAAAAAA6ABAAMAAAABAAEAAKACAAQAAAABAAAAAaADAAQAAAABAAAAAQAAAAD5Ip3+AAAADElEQVQIHWNISXcGAAJBAQ/t+dCDAAAAAElFTkSuQmCC";
const WAV: &[u8] = b"RIFF\x26\x00\x00\x00WAVEfmt \x10\x00\x00\x00\x01\x00\x01\x00\x40\x1f\x00\x00\x40\x1f\x00\x00\x01\x00\x08\x00data\x02\x00\x00\x00\x00\x00";

struct Provider {
    url: String,
    requests: Arc<Mutex<Vec<(String, Value)>>>,
    stop: mpsc::Sender<()>,
    worker: Option<thread::JoinHandle<()>>,
}
impl Provider {
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!(
            "http://{}/v1/chat/completions",
            listener.local_addr().unwrap()
        );
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured = requests.clone();
        let (stop, stopped) = mpsc::channel();
        let worker = thread::spawn(move || {
            while stopped.try_recv().is_err() {
                let (mut socket, _) = match listener.accept() {
                    Ok(value) => value,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                    Err(e) => panic!("mock provider accept: {e}"),
                };
                socket.set_nonblocking(false).unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut bytes = Vec::new();
                let (headers, body) = loop {
                    let mut chunk = [0; 4096];
                    let n = socket.read(&mut chunk).unwrap();
                    assert!(n > 0, "provider request truncated");
                    bytes.extend_from_slice(&chunk[..n]);
                    if let Some(end) = bytes.windows(4).position(|v| v == b"\r\n\r\n") {
                        let headers = String::from_utf8(bytes[..end].to_vec()).unwrap();
                        let length = headers
                            .lines()
                            .find_map(|line| {
                                let (key, value) = line.split_once(':')?;
                                key.eq_ignore_ascii_case("content-length")
                                    .then(|| value.trim().parse::<usize>().unwrap())
                            })
                            .unwrap_or(0);
                        if bytes.len() >= end + 4 + length {
                            break (headers, bytes[end + 4..end + 4 + length].to_vec());
                        }
                    }
                    assert!(bytes.len() < 1024 * 1024);
                };
                let speech = headers.starts_with("POST /v1/audio/speech ");
                let image = headers.starts_with("POST /v1/images/generations ");
                captured
                    .lock()
                    .unwrap()
                    .push((headers, serde_json::from_slice(&body).unwrap()));
                let (kind, response) = if speech {
                    ("audio/wav", WAV.to_vec())
                } else if image {
                    (
                        "application/json",
                        serde_json::to_vec(&json!({"data":[{"b64_json":PNG_B64}]})).unwrap(),
                    )
                } else {
                    ("application/json", serde_json::to_vec(&json!({
                        "id":"isolated-role-fixture","object":"chat.completion","created":1,
                        "model":"mock","choices":[{"index":0,"message":{"role":"assistant","content":"{\"status\":\"ok\"}"},"finish_reason":"stop"}],
                        "usage":{"prompt_tokens":2,"completion_tokens":2,"total_tokens":4}
                    })).unwrap())
                };
                write!(socket, "HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", response.len()).unwrap();
                socket.write_all(&response).unwrap();
            }
        });
        Self {
            url,
            requests,
            stop,
            worker: Some(worker),
        }
    }
    fn take(&self) -> Vec<(String, Value)> {
        std::mem::take(&mut *self.requests.lock().unwrap())
    }
}
impl Drop for Provider {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
fn fixture_base_dir(runner_temp: Option<std::ffi::OsString>) -> PathBuf {
    runner_temp
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
}

#[test]
fn fixture_base_prefers_nonempty_runner_temp() {
    assert_eq!(
        fixture_base_dir(Some("runner-temp".into())),
        PathBuf::from("runner-temp")
    );
    assert_eq!(fixture_base_dir(Some("".into())), std::env::temp_dir());
    assert_eq!(fixture_base_dir(None), std::env::temp_dir());
}

struct Fixture {
    root: PathBuf,
    home: PathBuf,
    contexts: [Value; 2],
    build_warnings: Mutex<Vec<String>>,
}
impl Fixture {
    fn new(provider: &Provider) -> Self {
        let id = uuid::Uuid::new_v4().simple().to_string();
        let root = fixture_base_dir(std::env::var_os("RUNNER_TEMP")).join(format!(
            "nrr-{:x}-{}",
            std::process::id(),
            &id[..12]
        ));
        fs::create_dir(&root).unwrap();
        let home = root.join("h");
        fs::create_dir(&home).unwrap();
        let root = root.canonicalize().unwrap();
        fs::write(home.join("config.toml"), format!(
            "secret_store='file'\ndefault_profile='a'\n[[profile]]\nname='a'\nserver='openai'\nmodel='wrong-profile-default-a'\nauth_mode='api_key'\nurl='{}'\ntemperature=0.9\n[[profile]]\nname='b'\nserver='openai'\nmodel='wrong-profile-default-b'\nauth_mode='api_key'\nurl='{}'\ntemperature=0.8\n", provider.url, provider.url)).unwrap();
        fs::write(
            home.join("credentials.toml"),
            "[profile_tokens]\na='synthetic-private-token-a'\nb='synthetic-private-token-b'\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&home, fs::Permissions::from_mode(0o700)).unwrap();
            fs::set_permissions(
                home.join("credentials.toml"),
                fs::Permissions::from_mode(0o600),
            )
            .unwrap();
        }
        let mut fixture = Self {
            root,
            home,
            contexts: [Value::Null, Value::Null],
            build_warnings: Mutex::new(Vec::new()),
        };
        fixture.contexts = [fixture.refresh("a"), fixture.refresh("b")];
        let prior = fixture.contexts[0].clone();
        fixture.contexts[0] = fixture.refresh("a");
        assert_eq!(
            prior["profile_uuid"], fixture.contexts[0]["profile_uuid"],
            "fresh process preserves profile UUID"
        );
        assert_eq!(
            prior["connection_generation"], fixture.contexts[0]["connection_generation"],
            "unchanged explicit refresh preserves reviewed connection generation"
        );
        fixture
    }
    fn command(&self, executable: impl AsRef<std::ffi::OsStr>) -> Command {
        let mut command = Command::new(executable);
        let mut paths = vec![Path::new(CLI).parent().unwrap().to_path_buf()];
        paths.extend(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        ));
        command.env("PATH", std::env::join_paths(paths).unwrap());
        command
            .current_dir(&self.root)
            .env("CARGO_AI_HOME", &self.home)
            .env("CARGO_AI_DISABLE_KEYCHAIN", "1")
            .env("CODEX_HOME", self.root.join("codex"))
            .env("CARGO_NET_OFFLINE", "true")
            .env_remove("CARGO_TARGET_DIR")
            .env_remove("CARGO_AI_EXECUTION_POLICY_V1")
            .env_remove("CARGO_AI_EXECUTION_POLICY_REQUIRED_V1");
        command
    }
    fn refresh(&self, profile: &str) -> Value {
        let output = self
            .command(CLI)
            .args(["--no-update-check", "profile", "refresh-context", profile])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "context refresh: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }
    fn definitions(&self, child: &Path) {
        #[cfg(unix)]
        self.bridge_tool();
        fs::write(
            self.root.join("pixel.png"),
            BASE64_STANDARD.decode(PNG_B64).unwrap(),
        )
        .unwrap();
        let schema = json!({"type":"object","properties":{"status":{"type":"string"}}});
        fs::write(self.root.join("child.json"), serde_json::to_vec(&json!({
            "agent_definition_schema_version":"2026-10-06.r1","inputs":[{"type":"text","text":"Child business input"}],"agent_schema":schema,"actions":[]
        })).unwrap()).unwrap();
        let mut root = json!({
            "agent_definition_schema_version":"2026-10-06.r1","inputs":[{"type":"text","text":"Root business input"},{"type":"image","path":"./pixel.png"}],"agent_schema":schema,
            "actions":[{"name":"speak-and-review","logic":{"==":[1,1]},"run":[
                {"kind":"generate_image","prompt":"A single blue square.","path":"./completed.png"},
                {"kind":"generate_audio","text":"Only public business text.","path":"./completed.wav"},
                { "kind":"agent","artifact":format!("./{}",child.file_name().unwrap().to_str().unwrap())},
                {"kind":"tool","name":"bridge","params":{}}
            ]}]
        });
        if !cfg!(unix) {
            root["actions"][0]["run"].as_array_mut().unwrap().pop();
        }
        fs::write(
            self.root.join("root.json"),
            serde_json::to_vec(&root).unwrap(),
        )
        .unwrap();
    }
    #[cfg(unix)]
    fn bridge_tool(&self) {
        use std::os::unix::fs::PermissionsExt;
        let directory = self.root.join(".cargo-ai/tools/bridge");
        fs::create_dir_all(directory.join("bin")).unwrap();
        fs::write(
            self.root.join(".cargo-ai/project.toml"),
            "[project]\nname='native-pipeline-fixture'\n",
        )
        .unwrap();
        let version = Command::new("rustc").arg("-vV").output().unwrap();
        let version = String::from_utf8(version.stdout).unwrap();
        let target = version
            .lines()
            .find_map(|line| line.strip_prefix("host: "))
            .unwrap();
        let describe = json!({"protocol_version":1,"supported_protocol_versions":[1,2],"name":"bridge","description":"Native child bridge fixture","params":{},"result":{"type":"string","nullable":true},
            "resource_profile":{"network":"none","filesystem_read":"none","filesystem_write":"required","subprocess":"none","env_read":"none","credential_access":"none"},"self_test":{"supported":false,"safe":false},
            "examples":{"minimal_invoke":{"protocol_version":1,"params":{}},"full_invoke":{"protocol_version":1,"params":{}}}});
        fs::write(directory.join("tool.json"),serde_json::to_vec(&json!({"schema_version":1,"tool_id":"bridge","artifacts":{target:{"path":"bin/bridge"}}})).unwrap()).unwrap();
        let script=format!("#!/bin/sh\nif [ \"$1\" = describe ]; then printf '%s\\n' '{}'; exit 0; fi\nIFS= read -r invocation || exit 2\nprintf '%s\\n' \"$invocation\" > bridge-invoke.json\nprintf '%s\\n' '{{\"protocol_version\":2,\"type\":\"child_request\",\"request_id\":\"one\",\"call_site\":\"tool-review\",\"inputs\":{{}}}}'\nIFS= read -r reply || exit 3\nprintf '%s\\n' \"$reply\" > bridge-child-result.json\ncase \"$reply\" in *'\"error\":null'*) ;; *) :;; esac\nprintf '%s\\n' '{{\"protocol_version\":2,\"type\":\"result\",\"result\":\"done\"}}'\n",describe);
        fs::write(directory.join("bin/bridge"), script).unwrap();
        fs::set_permissions(
            directory.join("bin/bridge"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
    }
    fn hatch(&self, name: &str, definition: &str) -> PathBuf {
        let output = self
            .command(CLI)
            .args([
                "--no-update-check",
                "hatch",
                name,
                "--config",
                definition,
                "--output-dir",
                "dist",
                "--force",
                "--keep-project",
            ])
            .output()
            .unwrap();
        let diagnostics = format!(
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            output.status.success(),
            "actual hatch failed: {diagnostics}"
        );
        if diagnostics.lines().any(|s| {
            s.trim_start().starts_with("warning:") || s.trim_start().starts_with("warning[")
        }) {
            self.build_warnings.lock().unwrap().push(diagnostics);
        }
        self.root
            .join("dist")
            .join(format!("{name}{}", std::env::consts::EXE_SUFFIX))
    }
    fn bootstrap(&self, mapping: usize) -> Value {
        let profile = ["a", "b"][mapping];
        let context = &self.contexts[mapping];
        let text_model = ["role-model-a", "role-model-b"][mapping];
        let audio_model = ["speech-model-a", "speech-model-b"][mapping];
        let image_model = ["image-model-a", "image-model-b"][mapping];
        let settings = json!({"temperature": if mapping == 0 {0.2} else {0.4}});
        let voice = if mapping == 0 { "alloy" } else { "coral" };
        let bindings = json!({"version":1,"revision":"revision-a","bindings":[
            {"role":"writer","profile":profile,"profile_uuid":context["profile_uuid"],"connection_generation":context["connection_generation"],"provider":"openai","model":text_model,"settings":settings},
            {"role":"illustrator","profile":profile,"profile_uuid":context["profile_uuid"],"connection_generation":context["connection_generation"],"provider":"openai","model":image_model,"settings":{"format":"png"}},
            {"role":"narrator","profile":profile,"profile_uuid":context["profile_uuid"],"connection_generation":context["connection_generation"],"provider":"openai","model":audio_model,"settings":{"voice":voice,"format":"wav"}}
        ]});
        let fixed_identity = format!(
            "{:x}",
            Sha256::digest(
                serde_json::to_vec(&json!({"version":1,"fixed_connections":[]})).unwrap()
            )
        );
        let references: Vec<_> = bindings["bindings"].as_array().unwrap().iter().map(|binding| json!({"role":binding["role"],"profile_uuid":binding["profile_uuid"],"connection_generation":binding["connection_generation"],"provider":binding["provider"]})).collect();
        let connection_identity = format!(
            "{:x}",
            Sha256::digest(
                serde_json::to_vec(
                    &json!({"role_connections":references,"fixed_connections":fixed_identity})
                )
                .unwrap()
            )
        );
        let text = json!({"profile":profile,"request_kind":"text","model":{"kind":"named","value":text_model},"thinking":{"mode":"provider_default"}});
        let image = json!({"profile":profile,"request_kind":"image","model":{"kind":"named","value":image_model},"thinking":{"mode":"provider_default"}});
        let audio = json!({"profile":profile,"request_kind":"audio","model":{"kind":"named","value":audio_model},"thinking":{"mode":"provider_default"}});
        let call = |id: &str,
                    definition: &str,
                    site: &str,
                    kind: &str,
                    role: &str,
                    selection: &Value,
                    settings: Value,
                    target: Option<&str>| {
            let operation = match kind {
                "audio" => "speech_generation",
                "image" => "image_generation",
                _ => "text_generation",
            };
            json!({"call_site":{"id":id,"locator":{"definition":definition,"site":site},"kind":kind,"role":role,"target":target,
                    "requirements":{"operation":operation,"input_modalities":["text"],"structured_output":matches!(kind,"root"|"child"),"settings":{}}},
                "selection":selection,"settings":settings,"compatibility":"compatible","reason":"trusted native fixture evidence",
                "evidence":[{"profile_uuid":context["profile_uuid"],"connection_generation":context["connection_generation"],"provider":"openai","model":selection["model"]["value"],"operation":operation,"input_modalities":["text","image"],"structured_output":true,"settings":settings,"status":"compatible","evidence_revision":"native-test-only","provenance":["isolated native parent"],"valid_until_unix_secs":4102444800u64}]})
        };
        let mut calls = vec![
            call(
                "root",
                "root.json",
                "root",
                "root",
                "writer",
                &text,
                settings.clone(),
                None,
            ),
            call(
                "image",
                "root.json",
                "actions.0.run.0",
                "image",
                "illustrator",
                &image,
                json!({"format":"png"}),
                None,
            ),
            call(
                "speech",
                "root.json",
                "actions.0.run.1",
                "audio",
                "narrator",
                &audio,
                json!({"voice":voice,"format":"wav"}),
                None,
            ),
            call(
                "review-launch",
                "root.json",
                "actions.0.run.2",
                "child",
                "writer",
                &text,
                settings.clone(),
                Some("child.json"),
            ),
            call(
                "review-root",
                "child.json",
                "root",
                "root",
                "writer",
                &text,
                settings,
                None,
            ),
        ];
        let mut tool_call = call(
            "tool-review",
            "root.json",
            "tools.bridge.review",
            "tool_child",
            "writer",
            &text,
            json!({"temperature":if mapping==0 {0.2} else {0.4}}),
            Some("child.json"),
        );
        tool_call["call_site"]["input_schema"] =
            json!({"type":"object","properties":{},"required":[],"additionalProperties":false});
        let source: Value =
            serde_json::from_slice(&fs::read(self.root.join("root.json")).unwrap()).unwrap();
        let artifact = source["actions"][0]["run"][2]["artifact"]
            .as_str()
            .unwrap()
            .trim_start_matches("./");
        if !artifact.ends_with(".json") {
            calls[3]["call_site"]["artifact"] = json!(artifact);
            tool_call["call_site"]["artifact"] = json!(artifact);
        }
        if cfg!(unix) {
            calls.push(tool_call);
        }
        let sites: Vec<_> = calls
            .iter()
            .map(|call| call["call_site"]["id"].clone())
            .collect();
        let limits = json!({"max_runtime_secs":30,"max_output_tokens":256,"max_agent_depth":4});
        let mut contents = serde_json::Map::new();
        let mut names = vec!["root.json", "child.json", "pixel.png"];
        if cfg!(unix) {
            names.extend([
                ".cargo-ai/tools/bridge/tool.json",
                ".cargo-ai/tools/bridge/bin/bridge",
            ]);
        }
        if !artifact.ends_with(".json") {
            names.push(artifact);
        }
        for name in names {
            let path = self.root.join(name);
            contents.insert(
                path.to_str().unwrap().into(),
                json!(format!("{:x}", Sha256::digest(fs::read(path).unwrap()))),
            );
        }
        json!({"version":1,"resolution":{"version":1,"identity":"trusted-native-fixture","binding_revision":"revision-a",
            "binding_identity":format!("{:x}",Sha256::digest(serde_json::to_vec(&bindings).unwrap())),
            "context":{"project_identity":"isolated","environment_identity":"isolated","package_identity":"isolated","runtime_contract":"native-role-v1","connection_context_identity":connection_identity,"consent_identity":"isolated"},
            "scope":{"key":{"action":"review","interface":"fixture","mode":"default"},"call_sites":sites,"resources":[],"data_scopes":[],"limits":limits},
            "calls":calls,"required_selections":[text,image,audio],"ready":true,"execution_authorized":false,"invocation_access":"unverified"},
            "bindings":bindings,"policy":{"version":1,"allowed":[text,image,audio],"limits":limits},"definition":"root.json","package_root":self.root,"content_identities":contents,"invocation_id":"isolated-native-invocation","binding_revision":"revision-a","parent_permit_id":"fixture-parent"})
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[derive(Clone, Copy)]
enum Control {
    Allow,
    DenyAfter(usize),
    EofAfter(usize),
    OverlapChildren,
}
struct Run {
    success: bool,
    frames: Vec<Value>,
    diagnostics: String,
}
fn invoke(fixture: &Fixture, executable: &Path, bootstrap: Value, control: Control) -> Run {
    invoke_reference(fixture, executable, bootstrap, control, None)
}
fn invoke_reference(
    fixture: &Fixture,
    executable: &Path,
    bootstrap: Value,
    control: Control,
    reference: Option<&str>,
) -> Run {
    invoke_reference_in_dir(
        fixture,
        executable,
        bootstrap,
        control,
        reference,
        &fixture.root,
    )
}
fn invoke_reference_in_dir(
    fixture: &Fixture,
    executable: &Path,
    bootstrap: Value,
    control: Control,
    reference: Option<&str>,
    cwd: &Path,
) -> Run {
    let mut command = fixture.command(executable);
    command.current_dir(cwd);
    if executable == Path::new(CLI) {
        command.args(["--no-update-check", "run"]);
        if let Some(reference) = reference {
            command.arg(reference);
        } else {
            command.args(["--config", "root.json"]);
        }
    }
    let mut child = command
        .arg("--native-role-child")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take();
    writeln!(stdin.as_mut().unwrap(), "{}", json!({"protocol":"cargo_ai_native_role","version":1,"type":"bootstrap","bootstrap":bootstrap})).unwrap();
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let (tx, rx) = mpsc::channel();
    let reader = thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            if tx.send(line.unwrap()).is_err() {
                break;
            }
        }
    });
    let errors = thread::spawn(move || {
        let mut value = String::new();
        BufReader::new(stderr).read_to_string(&mut value).unwrap();
        value
    });
    let mut frames = Vec::new();
    let mut diagnostics = String::new();
    let mut admitted = 0;
    let mut launch_sites = std::collections::BTreeMap::<String, String>::new();
    let mut pending_siblings = Vec::new();
    let mut overlapped = false;
    let started = Instant::now();
    loop {
        if started.elapsed() > Duration::from_secs(45) {
            let _ = child.kill();
            panic!("native fixture timed out: {diagnostics}\n{frames:?}");
        }
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(line) => {
                let value = serde_json::from_str::<Value>(&line).ok();
                if let Some(value) = value.filter(|v| v["protocol"] == "cargo_ai_native_role") {
                    if value["type"] == "admit" {
                        let permit_id = format!("permit-{admitted}");
                        if matches!(control, Control::OverlapChildren) {
                            launch_sites.insert(
                                permit_id.clone(),
                                value["boundary"]["call_site"].as_str().unwrap().to_owned(),
                            );
                            if value["boundary"]["call_site"] == "review-root" {
                                let parent =
                                    value["boundary"]["parent_permit_id"].as_str().unwrap();
                                let site = launch_sites
                                    .get(parent)
                                    .expect("child admission has its known launch parent");
                                let allowed = site == "first-launch";
                                assert!(allowed || site == "second-launch");
                                pending_siblings.push(json!({"protocol":"cargo_ai_native_role","version":1,"type":"admission","request_id":value["request_id"],"allowed":allowed,"permit_id":permit_id}));
                                admitted += 1;
                                frames.push(value);
                                if pending_siblings.len() == 2 {
                                    overlapped = true;
                                    for reply in pending_siblings.drain(..) {
                                        writeln!(stdin.as_mut().unwrap(), "{reply}").unwrap();
                                    }
                                }
                                continue;
                            }
                        }
                        let allowed = match control {
                            Control::Allow | Control::OverlapChildren => true,
                            Control::DenyAfter(n) | Control::EofAfter(n) => admitted < n,
                        };
                        if !allowed && matches!(control, Control::EofAfter(_)) {
                            stdin.take();
                        } else if let Some(writer) = stdin.as_mut() {
                            writeln!(writer,"{}",json!({"protocol":"cargo_ai_native_role","version":1,"type":"admission","request_id":value["request_id"],"allowed":allowed,"permit_id":if allowed {json!(format!("permit-{admitted}"))} else {Value::Null}})).unwrap();
                        }
                        admitted += 1;
                    }
                    frames.push(value);
                } else {
                    diagnostics.push_str(&line);
                    diagnostics.push('\n');
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    stdin.take();
    let status = child.wait().unwrap();
    reader.join().unwrap();
    diagnostics.push_str(&errors.join().unwrap());
    if matches!(control, Control::OverlapChildren) {
        assert!(overlapped, "both identical child sources must be live before either provider admission completes: {frames:?}");
    }
    Run {
        success: status.success(),
        frames,
        diagnostics,
    }
}
fn assert_success(run: &Run) {
    assert!(
        run.success,
        "native process failed: {}\n{:?}",
        run.diagnostics, run.frames
    );
    let terminal = run
        .frames
        .iter()
        .find(|v| v["type"] == "result")
        .expect("native terminal result");
    assert!(terminal["error"].is_null(), "native error: {terminal}");
}
fn exercise(executable: &Path, fixture: &Fixture, provider: &Provider) {
    let config_before = fs::read(fixture.home.join("config.toml")).unwrap();
    let credential_path = fixture.home.join("credentials.toml");
    let mut credentials: toml::Value =
        toml::from_str(&fs::read_to_string(&credential_path).unwrap()).unwrap();
    let mut records: Value =
        serde_json::from_str(credentials["role_contexts"].as_str().unwrap()).unwrap();
    for (name, reference) in ["a", "b"].into_iter().zip(&fixture.contexts) {
        assert_eq!(&records["profiles"][name]["reference"], reference);
        records["profiles"][name]["refreshed_at"] = json!(0);
    }
    credentials["role_contexts"] = toml::Value::String(serde_json::to_string(&records).unwrap());
    fs::write(&credential_path, toml::to_string(&credentials).unwrap()).unwrap();
    for mapping in 0..2 {
        let run = invoke(
            fixture,
            executable,
            fixture.bootstrap(mapping),
            Control::Allow,
        );
        assert_success(&run);
        let requests = provider.take();
        assert_eq!(
            requests.len(),
            if cfg!(unix) { 5 } else { 4 },
            "root, image, audio, direct child and tool child must each dispatch once: {requests:?}"
        );
        for (headers, body) in &requests {
            assert!(headers.to_lowercase().contains(&format!(
                "authorization: bearer synthetic-private-token-{}",
                ["a", "b"][mapping]
            )));
            let serialized = body.to_string();
            for private in [
                "synthetic-private-token",
                "profile_uuid",
                "connection_generation",
                "revision-a",
                "trusted-native-fixture",
            ] {
                assert!(
                    !serialized.contains(private),
                    "private context leaked into provider business data"
                );
            }
        }
        assert_eq!(
            requests[0].1["model"],
            ["role-model-a", "role-model-b"][mapping]
        );
        assert_eq!(
            requests[0].1["temperature"],
            if mapping == 0 { 0.2 } else { 0.4 }
        );
        assert_eq!(
            requests[2].1["model"],
            ["speech-model-a", "speech-model-b"][mapping]
        );
        assert_eq!(
            requests[2].1["voice"],
            if mapping == 0 { "alloy" } else { "coral" }
        );
        assert_eq!(requests[3].1["model"], requests[0].1["model"]);
        #[cfg(unix)]
        {
            assert_eq!(requests[4].1["model"], requests[0].1["model"]);
            assert_eq!(requests[4].1["temperature"], requests[0].1["temperature"]);
            let invocation: Value =
                serde_json::from_slice(&fs::read(fixture.root.join("bridge-invoke.json")).unwrap())
                    .unwrap();
            assert_eq!(
                invocation,
                json!({"protocol_version":2,"type":"invoke","params":{}}),
                "tool sees business params only"
            );
            let reply: Value = serde_json::from_slice(
                &fs::read(fixture.root.join("bridge-child-result.json")).unwrap(),
            )
            .unwrap();
            assert_eq!(reply["result"], json!({"status":"ok"}));
            assert!(reply["error"].is_null());
        }
        assert_eq!(
            requests[1].1["model"],
            ["image-model-a", "image-model-b"][mapping]
        );
        assert_eq!(requests[1].1["output_format"], "png");
        assert_eq!(requests[2].1["response_format"], "wav");
        assert_eq!(
            fs::read(fixture.root.join("completed.png")).unwrap(),
            BASE64_STANDARD.decode(PNG_B64).unwrap()
        );
        assert!(requests[0].1["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["content"]
                .as_array()
                .is_some_and(|parts| parts.iter().any(|p| p["type"] == "image_url"
                    && p["image_url"]["url"] == format!("data:image/png;base64,{PNG_B64}")))));
        assert_eq!(requests[0].1["response_format"]["type"], "json_schema");
        assert_eq!(
            requests[0].1["response_format"]["json_schema"]["schema"]["additionalProperties"],
            false
        );
        assert_eq!(fs::read(fixture.root.join("completed.wav")).unwrap(), WAV);
        let admissions: Vec<_> = run.frames.iter().filter(|v| v["type"] == "admit").collect();
        assert!(
            admissions
                .iter()
                .any(|v| v["boundary"]["agent"] == "root.json"
                    && v["boundary"]["call_site"] == "root")
        );
        assert!(admissions
            .iter()
            .any(|v| v["boundary"]["agent"] == "child.json"
                && v["boundary"]["call_site"] == "review-root"));
    }
    #[cfg(unix)]
    {
        let mut denied = fixture.bootstrap(0);
        let calls = denied["resolution"]["calls"].as_array_mut().unwrap();
        let call = calls
            .iter_mut()
            .find(|c| c["call_site"]["id"] == "tool-review")
            .unwrap();
        call["selection"]["model"]["value"] = json!("denied-tool-model");
        let selection = call["selection"].clone();
        denied["resolution"]["required_selections"]
            .as_array_mut()
            .unwrap()
            .push(selection);
        let run = invoke(fixture, executable, denied, Control::Allow);
        let terminal = run
            .frames
            .iter()
            .find(|f| f["type"] == "result")
            .expect("native terminal after outer tool success");
        assert_eq!(
            terminal["error"]["code"], "action.execution_selection_denied",
            "outer tool success cannot hide declared child denial: {:?}",
            run.frames
        );
        assert_eq!(
            provider.take().len(),
            4,
            "root/media/direct child complete; denied tool child dispatches zero requests"
        );
        let denial = run
            .frames
            .iter()
            .find(|f| f["type"] == "denied" && f["code"] == "action.execution_selection_denied")
            .expect("retained policy denial");
        assert_eq!(denial["boundary"]["call_site"], "tool-review");
        assert_eq!(denial["boundary"]["agent"], "root.json");
        assert_eq!(denial["boundary"]["target_agent"], "child.json");
        assert_eq!(denial["boundary"]["kind"], "native_child");
        let terminal = run.frames.iter().find(|f| f["type"] == "result").unwrap();
        let outcomes = terminal["native_outcomes"]["outcomes"].as_array().unwrap();
        let parent = outcomes
            .iter()
            .find(|o| o["permit_id"] == denial["boundary"]["parent_permit_id"])
            .expect("denial belongs to actual tool occurrence");
        assert_eq!(parent["boundary"]["kind"], "tool");
        assert_eq!(
            parent["state"], "completed",
            "outer tool really returned success"
        );
        assert!(!outcomes
            .iter()
            .any(|o| o["boundary"]["call_site"] == "tool-review" && o["state"] == "completed"));
    }
    for control in [Control::DenyAfter(0), Control::EofAfter(0)] {
        let run = invoke(fixture, executable, fixture.bootstrap(0), control);
        assert!(!run.success, "denial/EOF cannot succeed");
        assert!(provider.take().is_empty());
        assert_eq!(
            fs::read(fixture.root.join("completed.wav")).unwrap(),
            WAV,
            "earlier completed effect survives"
        );
    }
    let run = invoke(
        fixture,
        executable,
        fixture.bootstrap(0),
        Control::DenyAfter(3),
    );
    assert!(!run.success, "later child revocation cannot succeed");
    assert_eq!(
        provider.take().len(),
        3,
        "completed root/image/audio retained, unadmitted child never dispatched"
    );
    assert!(run
        .frames
        .iter()
        .any(|v| v["type"] == "settled" && v["state"] == "completed"));
    assert_eq!(fs::read(fixture.root.join("completed.wav")).unwrap(), WAV);
    let canceled_child = invoke(
        fixture,
        executable,
        fixture.bootstrap(0),
        Control::EofAfter(4),
    );
    assert!(
        !canceled_child.success,
        "control loss during descendant admission cancels invocation"
    );
    assert_eq!(
        provider.take().len(),
        3,
        "already completed root/media persist, unadmitted descendant inference never dispatches"
    );
    assert!(
        canceled_child
            .frames
            .iter()
            .any(|v| v["type"] == "settled" && v["state"] == "completion_unknown"),
        "owned child cancellation retains completion uncertainty: {:?}",
        canceled_child.frames
    );
    assert_eq!(fs::read(fixture.root.join("completed.wav")).unwrap(), WAV);
    let mut unknown = fixture.bootstrap(0);
    unknown["resolution"]["calls"][0]["evidence"][0]["status"] = json!("unknown");
    assert!(!invoke(fixture, executable, unknown, Control::Allow).success);
    assert!(
        provider.take().is_empty(),
        "unknown evidence must never dispatch"
    );
    let mut policy = fixture.bootstrap(0);
    policy["policy"]["allowed"] = json!([]);
    assert!(!invoke(fixture, executable, policy, Control::Allow).success);
    assert!(
        provider.take().is_empty(),
        "denied selection must never dispatch"
    );
    assert_eq!(
        fs::read(fixture.home.join("config.toml")).unwrap(),
        config_before,
        "native invocation never initializes or rewrites private config"
    );
    let old = fixture.bootstrap(0);
    assert_eq!(fixture.refresh("a"), fixture.contexts[0]);
    let unchanged = invoke(fixture, executable, old.clone(), Control::Allow);
    assert_success(&unchanged);
    assert_eq!(provider.take().len(), if cfg!(unix) { 5 } else { 4 });
    fs::write(
        fixture.home.join("credentials.toml"),
        fs::read_to_string(fixture.home.join("credentials.toml"))
            .unwrap()
            .replace(
                "synthetic-private-token-a",
                "synthetic-private-token-a-rotated",
            ),
    )
    .unwrap();
    assert!(!invoke(fixture, executable, old.clone(), Control::Allow).success);
    assert!(
        provider.take().is_empty(),
        "unreviewed changed credentials never dispatch"
    );
    let changed = fixture.refresh("a");
    assert_eq!(changed["profile_uuid"], fixture.contexts[0]["profile_uuid"]);
    assert_ne!(
        changed["connection_generation"],
        fixture.contexts[0]["connection_generation"]
    );
    assert!(!invoke(fixture, executable, old, Control::Allow).success);
    assert!(
        provider.take().is_empty(),
        "rotated context must never dispatch"
    );
}

#[test]
fn cli_native_role_root_media_and_child_dispatch_are_scoped() {
    let provider = Provider::new();
    let fixture = Fixture::new(&provider);
    fixture.definitions(&fixture.root.join("child.json"));
    exercise(Path::new(CLI), &fixture, &provider);
}

#[test]
#[ignore = "builds and executes two actual generated agents; run explicitly in native qualification"]
fn emitted_native_role_root_media_and_child_dispatch_are_scoped() {
    let provider = Provider::new();
    let mut fixture = Fixture::new(&provider);
    fixture.definitions(&fixture.root.join("child.json"));
    let child = fixture.hatch("nrc", "child.json");
    fixture.definitions(&child);
    let root = fixture.hatch("nrr", "root.json");
    fixture.contexts = [fixture.refresh("a"), fixture.refresh("b")];
    fs::copy(&child, fixture.root.join(child.file_name().unwrap())).unwrap();
    let original = fs::read(fixture.root.join("child.json")).unwrap();
    let mut changed: Value = serde_json::from_slice(&original).unwrap();
    changed["inputs"][0]["text"] = json!("Changed passive source after hatch");
    fs::write(
        fixture.root.join("child.json"),
        serde_json::to_vec(&changed).unwrap(),
    )
    .unwrap();
    let drift = invoke(&fixture, &root, fixture.bootstrap(0), Control::Allow);
    assert!(
        !drift.success,
        "stale emitted child source must fail before child spawn"
    );
    assert_eq!(
        provider.take().len(),
        3,
        "earlier root and both media operations finish, stale generated child never dispatches"
    );
    assert!(!drift
        .frames
        .iter()
        .any(|v| v["type"] == "admit" && v["boundary"]["kind"] == "native_child"));
    fs::write(fixture.root.join("child.json"), original).unwrap();
    exercise(&root, &fixture, &provider);
    let warnings = fixture.build_warnings.lock().unwrap();
    assert!(
        warnings.is_empty(),
        "generated builds must be warning-free: {warnings:?}"
    );
}

fn exercise_child_lineage(emitted: bool) {
    let provider = Provider::new();
    let mut fixture = Fixture::new(&provider);
    fixture.definitions(&fixture.root.join("child.json"));
    let mut bootstrap = fixture.bootstrap(0);
    let schema = json!({"type":"object","properties":{"status":{"type":"string"}}});
    let leaf = json!({"agent_definition_schema_version":"2026-10-06.r1","inputs":[{"type":"text","text":"Nested business input"}],"agent_schema":schema,"actions":[]});
    fs::write(fixture.root.join("leaf.json"), leaf.to_string()).unwrap();
    let leaf_artifact = if emitted {
        let path = fixture.hatch("nll", "leaf.json");
        fs::copy(&path, fixture.root.join(path.file_name().unwrap())).unwrap();
        path.file_name().unwrap().to_str().unwrap().to_owned()
    } else {
        "leaf.json".into()
    };
    let nested_step = if cfg!(unix) {
        json!({"kind":"tool","name":"bridge","params":{}})
    } else {
        json!({"kind":"agent","artifact":format!("./{leaf_artifact}")})
    };
    let child = json!({"agent_definition_schema_version":"2026-10-06.r1","inputs":[{"type":"text","text":"Repeated child business input"}],"agent_schema":schema,"actions":[{"name":"nested","logic":{"==":[1,1]},"run":[nested_step]}]});
    fs::write(fixture.root.join("child.json"), child.to_string()).unwrap();
    let child_artifact = if emitted {
        let path = fixture.hatch("nlc", "child.json");
        fs::copy(&path, fixture.root.join(path.file_name().unwrap())).unwrap();
        path.file_name().unwrap().to_str().unwrap().to_owned()
    } else {
        "child.json".into()
    };
    let root = json!({"agent_definition_schema_version":"2026-10-06.r1","action_execution":"parallel","agent_schema":{"type":"object","properties":{}},"actions":[
        {"name":"first","logic":{"==":[1,1]},"run":[{"kind":"agent","artifact":format!("./{child_artifact}")}]},
        {"name":"second","logic":{"==":[1,1]},"run":[{"kind":"agent","artifact":format!("./{child_artifact}")}]}
    ]});
    fs::write(fixture.root.join("root.json"), root.to_string()).unwrap();
    let executable = if emitted {
        fixture.hatch("nlr", "root.json")
    } else {
        PathBuf::from(CLI)
    };
    // Builds can refresh native metadata; bind only after the exact executables exist.
    fixture.contexts = [fixture.refresh("a"), fixture.refresh("b")];
    for binding in bootstrap["bindings"]["bindings"].as_array_mut().unwrap() {
        binding["profile_uuid"] = fixture.contexts[0]["profile_uuid"].clone();
        binding["connection_generation"] = fixture.contexts[0]["connection_generation"].clone();
    }
    bootstrap["resolution"]["binding_identity"] = json!(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&bootstrap["bindings"]).unwrap())
    ));
    let fixed = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&json!({"version":1,"fixed_connections":[]})).unwrap())
    );
    let references: Vec<_> = bootstrap["bindings"]["bindings"].as_array().unwrap().iter().map(|b| json!({"role":b["role"],"profile_uuid":b["profile_uuid"],"connection_generation":b["connection_generation"],"provider":b["provider"]})).collect();
    bootstrap["resolution"]["context"]["connection_context_identity"] = json!(format!(
        "{:x}",
        Sha256::digest(
            serde_json::to_vec(&json!({"role_connections":references,"fixed_connections":fixed}))
                .unwrap()
        )
    ));
    for call in bootstrap["resolution"]["calls"].as_array_mut().unwrap() {
        call["evidence"][0]["profile_uuid"] = fixture.contexts[0]["profile_uuid"].clone();
        call["evidence"][0]["connection_generation"] =
            fixture.contexts[0]["connection_generation"].clone();
    }
    let original = bootstrap["resolution"]["calls"].as_array().unwrap().clone();
    let mut first = original[3].clone();
    first["call_site"]["id"] = json!("first-launch");
    first["call_site"]["locator"]["site"] = json!("actions.0.run.0");
    if emitted {
        first["call_site"]["artifact"] = json!(child_artifact);
    }
    let mut second = first.clone();
    second["call_site"]["id"] = json!("second-launch");
    second["call_site"]["locator"]["site"] = json!("actions.1.run.0");
    let mut nested = if cfg!(unix) {
        original.last().unwrap().clone()
    } else {
        first.clone()
    };
    nested["call_site"]["id"] = json!(if cfg!(unix) {
        "tool-review"
    } else {
        "nested-launch"
    });
    nested["call_site"]["locator"]["definition"] = json!("child.json");
    nested["call_site"]["target"] = json!("leaf.json");
    if emitted {
        nested["call_site"]["artifact"] = json!(leaf_artifact);
    }
    let mut leaf_call = original[4].clone();
    leaf_call["call_site"]["id"] = json!("leaf-root");
    leaf_call["call_site"]["locator"]["definition"] = json!("leaf.json");
    bootstrap["resolution"]["calls"] = json!([first, second, original[4], nested, leaf_call]);
    let sites: Vec<Value> = bootstrap["resolution"]["calls"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["call_site"]["id"].clone())
        .collect();
    bootstrap["resolution"]["scope"]["call_sites"] = json!(sites);
    let mut names = vec![
        "root.json".to_owned(),
        "child.json".into(),
        "leaf.json".into(),
    ];
    if cfg!(unix) {
        names.extend([
            ".cargo-ai/tools/bridge/tool.json".into(),
            ".cargo-ai/tools/bridge/bin/bridge".into(),
        ]);
    }
    if emitted {
        names.extend([leaf_artifact, child_artifact]);
    }
    let identities = names
        .into_iter()
        .map(|name| {
            let path = fixture.root.join(name);
            (
                path.to_str().unwrap().to_owned(),
                json!(format!("{:x}", Sha256::digest(fs::read(path).unwrap()))),
            )
        })
        .collect::<serde_json::Map<String, Value>>();
    bootstrap["content_identities"] = json!(identities);
    for _ in 0..2 {
        let run = invoke(
            &fixture,
            &executable,
            bootstrap.clone(),
            Control::OverlapChildren,
        );
        assert!(!run.success, "mixed child outcome cannot be root success");
        assert_eq!(
            provider.take().len(),
            2,
            "one successful child plus its nested leaf; denied sibling dispatches zero"
        );
        let terminal = run
            .frames
            .iter()
            .find(|f| f["type"] == "result")
            .expect("terminal outcomes");
        let outcomes = terminal["native_outcomes"]["outcomes"].as_array().unwrap();
        let find = |site: &str| {
            outcomes
                .iter()
                .find(|o| o["boundary"]["call_site"] == site && o["permit_id"] != "")
                .unwrap()
        };
        let first = find("first-launch");
        let second = find("second-launch");
        assert_ne!(first["permit_id"], second["permit_id"]);
        assert_eq!(first["state"], "completed");
        assert_eq!(second["state"], "failed");
        let completed = outcomes
            .iter()
            .find(|o| o["boundary"]["call_site"] == "review-root" && o["state"] == "completed")
            .unwrap();
        let denied = outcomes
            .iter()
            .find(|o| o["boundary"]["call_site"] == "review-root" && o["state"] == "not_dispatched")
            .unwrap();
        assert_eq!(
            completed["boundary"]["parent_permit_id"],
            first["permit_id"]
        );
        assert_eq!(denied["boundary"]["parent_permit_id"], second["permit_id"]);
        let nested = find(if cfg!(unix) {
            "tool-review"
        } else {
            "nested-launch"
        });
        if cfg!(unix) {
            let tool = outcomes
                .iter()
                .find(|o| o["permit_id"] == nested["boundary"]["parent_permit_id"])
                .unwrap();
            assert_eq!(tool["boundary"]["kind"], "tool");
            assert_eq!(tool["boundary"]["parent_permit_id"], first["permit_id"]);
        } else {
            assert_eq!(nested["boundary"]["parent_permit_id"], first["permit_id"]);
        }
        let leaf = find("leaf-root");
        assert_eq!(leaf["boundary"]["parent_permit_id"], nested["permit_id"]);
        for outcome in outcomes {
            assert_eq!(
                outcome["boundary"]["invocation_id"],
                "isolated-native-invocation"
            );
            assert_eq!(outcome["boundary"]["binding_revision"], "revision-a");
        }
    }
    let warnings = fixture.build_warnings.lock().unwrap();
    assert!(
        warnings.is_empty(),
        "generated builds must be warning-free: {warnings:?}"
    );
}

#[test]
fn cli_native_role_overlapping_children_retain_nested_permit_lineage() {
    exercise_child_lineage(false);
}

#[test]
#[ignore = "builds and executes three native agents; run explicitly in native qualification"]
fn emitted_native_role_overlapping_children_retain_nested_permit_lineage() {
    exercise_child_lineage(true);
}

fn machine(fixture: &Fixture, args: &[&str], request: Option<&Value>) -> Value {
    let mut command = fixture.command(CLI);
    command
        .args(["--no-update-check"])
        .args(args)
        .args(["--output-format", "json"]);
    let output = if let Some(request) = request {
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(request.to_string().as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    } else {
        command.output().unwrap()
    };
    let value: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|_| {
        panic!(
            "machine output: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    });
    assert!(output.status.success(), "machine request: {value}");
    value["data"].clone()
}

#[cfg(unix)]
#[test]
fn public_role_session_keeps_input_open_acknowledges_revoke_and_cancels_lost_control() {
    let provider = Provider::new();
    let fixture = Fixture::new(&provider);
    let definition = |steps: Value| json!({"agent_definition_schema_version":"2026-10-06.r1","agent_schema":{"type":"object","properties":{}},"actions":[{"name":"work","logic":{"==":[1,1]},"run":steps}]});
    fs::write(
        fixture.root.join("root.json"),
        serde_json::to_vec(&definition(json!([
            {"kind":"agent","artifact":"./first.json"},{"kind":"agent","artifact":"./second.json"}
        ])))
        .unwrap(),
    )
    .unwrap();
    fs::write(fixture.root.join("first.json"),serde_json::to_vec(&definition(json!([
        {"kind":"exec","program":"/bin/sh","args":["-c","printf '%s' $$ > owned-pid; printf started > started; while [ ! -f release ]; do sleep 0.02; done; printf retained > completed"]}
    ]))).unwrap()).unwrap();
    fs::write(
        fixture.root.join("second.json"),
        serde_json::to_vec(&definition(json!([
            {"kind":"exec","program":"/bin/sh","args":["-c","printf forbidden > forbidden"]}
        ])))
        .unwrap(),
    )
    .unwrap();
    let limits = json!({"max_runtime_secs":20,"max_output_tokens":256,"max_agent_depth":4});
    let call = |id: &str, index: usize, target: &str| json!({"id":id,"locator":{"definition":"root.json","site":format!("actions.0.run.{index}")},"kind":"child","target":target,"requirements":{"operation":"text_generation","input_modalities":[],"structured_output":false,"settings":{}}});
    fs::write(fixture.root.join("cargo-ai-actions.json"),serde_json::to_vec(&json!({
        "schema_version":3,"actions":[{"id":"work","target":"root.json","input_schema":{"type":"object","properties":{},"required":[],"additionalProperties":false},"mappings":{}}],
        "interfaces":[{"id":"fixture","actions":["work"]}],
        "role_registry":{"version":1,"roles":[],"call_sites":[call("first",0,"first.json"),call("second",1,"second.json")],
            "contexts":[{"key":{"action":"work","interface":"fixture","mode":"default"},"call_sites":["first","second"],"resources":[],"data_scopes":[],"limits":limits}],"tool_content":{}}
    })).unwrap()).unwrap();
    fs::create_dir(fixture.root.join(".cargo-ai")).unwrap();
    fs::write(
        fixture.root.join(".cargo-ai/project.toml"),
        "[project]\nname='native-session-fixture'\n",
    )
    .unwrap();
    let config_before = fs::read(fixture.home.join("config.toml")).unwrap();
    let discovered = machine(&fixture, &["actions", "list", "--project", "."], None);
    let mut request = json!({"schema_version":3,"interface":"fixture","action":"work","inputs":{},"expected_binding":discovered["binding"],
        "execution_policy":{"version":1,"allowed":[],"limits":limits},"attachment_grants":{},
        "role_execution":{"mode":"default","binding_revision":{"version":1,"revision":"public-revision","bindings":[]}}});
    let resolved = machine(
        &fixture,
        &[
            "actions",
            "resolve",
            "--project",
            ".",
            "--interface",
            "fixture",
            "--action",
            "work",
            "--request-stdin",
        ],
        Some(&request),
    );
    assert_eq!(resolved["ready"], true, "{resolved}");
    assert_eq!(resolved["execution_authorized"], false);
    request["role_execution"]["resolution_id"] = resolved["identity"].clone();
    for mode in ["revoke", "eof", "malformed"] {
        for name in ["started", "release", "completed", "forbidden"] {
            let _ = fs::remove_file(fixture.root.join(name));
        }
        let mut child = fixture
            .command(CLI)
            .args([
                "--no-update-check",
                "run",
                "--project",
                ".",
                "--interface",
                "fixture",
                "--action",
                "work",
                "--action-request-stdin",
                "--role-session",
                "--output-format",
                "ndjson",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut stdin = child.stdin.take();
        writeln!(
            stdin.as_mut().unwrap(),
            "{}",
            json!({"protocol":"cargo_ai_role_session","version":1,"type":"start","request":request})
        )
        .unwrap();
        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();
        let (tx, rx) = mpsc::channel();
        let reader = thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                if tx.send(line.unwrap()).is_err() {
                    break;
                }
            }
        });
        let errors = thread::spawn(move || {
            let mut value = String::new();
            BufReader::new(stderr).read_to_string(&mut value).unwrap();
            value
        });
        let start = Instant::now();
        let mut sent = false;
        let mut ack = false;
        let mut frames = Vec::new();
        loop {
            if start.elapsed() > Duration::from_secs(15) {
                let _ = child.kill();
                panic!("public session {mode} timed out: {frames:?}");
            }
            if !sent && fixture.root.join("started").exists() {
                sent = true;
                match mode {
                    "revoke"=>writeln!(stdin.as_mut().unwrap(),"{}",json!({"protocol":"cargo_ai_role_session","version":1,"type":"revoke","binding_revision":"public-revision"})).unwrap(),
                    "eof"=>{stdin.take();},
                    _=>writeln!(stdin.as_mut().unwrap(),"{{malformed control").unwrap(),
                }
            }
            match rx.recv_timeout(Duration::from_millis(10)) {
                Ok(line) => {
                    let frame: Value =
                        serde_json::from_str(&line).expect("public session stdout is NDJSON");
                    if frame["protocol"] == "cargo_ai_role_session" && frame["type"] == "revoked" {
                        assert!(sent);
                        assert_eq!(frame["binding_revision"], "public-revision");
                        ack = true;
                        fs::write(
                            fixture.root.join("release"),
                            b"release only after acknowledgment",
                        )
                        .unwrap();
                    }
                    frames.push(frame);
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        stdin.take();
        let status = child.wait().unwrap();
        reader.join().unwrap();
        let diagnostics = errors.join().unwrap();
        assert!(
            sent,
            "session exited before first admitted child: {frames:?}\n{diagnostics}"
        );
        assert!(
            !status.success(),
            "withdrawn/lost session cannot succeed: {frames:?}"
        );
        assert!(
            !fixture.root.join("forbidden").exists(),
            "second unadmitted child must not run"
        );
        if mode == "revoke" {
            assert!(ack);
            assert_eq!(
                fs::read(fixture.root.join("completed")).unwrap(),
                b"retained"
            );
        } else {
            assert!(
                !fixture.root.join("completed").exists(),
                "lost control cancels admitted owned work"
            );
        }
        assert!(
            provider.take().is_empty(),
            "structural session must perform no inference"
        );
        assert_eq!(
            fs::read(fixture.home.join("config.toml")).unwrap(),
            config_before,
            "public role sessions preserve uninitialized private attribution state"
        );
    }
    public_blocked_output_cancels_owned_work(&fixture, &request);
    assert!(provider.take().is_empty());
    assert_eq!(
        fs::read(fixture.home.join("config.toml")).unwrap(),
        config_before
    );
}

#[cfg(unix)]
fn public_blocked_output_cancels_owned_work(fixture: &Fixture, request: &Value) {
    // The longest valid revision makes the bounded acknowledgment set exceed a 64 KiB pipe.
    let mut request = request.clone();
    request["role_execution"]["binding_revision"]["revision"] = json!("r".repeat(128));
    request["role_execution"]
        .as_object_mut()
        .unwrap()
        .remove("resolution_id");
    let resolved = machine(
        fixture,
        &[
            "actions",
            "resolve",
            "--project",
            ".",
            "--interface",
            "fixture",
            "--action",
            "work",
            "--request-stdin",
        ],
        Some(&request),
    );
    assert_eq!(resolved["ready"], true);
    request["role_execution"]["resolution_id"] = resolved["identity"].clone();
    for name in ["started", "release", "completed", "forbidden", "owned-pid"] {
        let _ = fs::remove_file(fixture.root.join(name));
    }
    let mut child = fixture
        .command(CLI)
        .args([
            "--no-update-check",
            "run",
            "--project",
            ".",
            "--interface",
            "fixture",
            "--action",
            "work",
            "--action-request-stdin",
            "--role-session",
            "--output-format",
            "ndjson",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    writeln!(
        stdin,
        "{}",
        json!({"protocol":"cargo_ai_role_session","version":1,"type":"start","request":request})
    )
    .unwrap();
    // Keep the read end open but deliberately never drain the host's acknowledgment output.
    let stdout = child.stdout.take().unwrap();
    let deadline = Instant::now();
    while !fixture.root.join("started").exists() {
        if deadline.elapsed() > Duration::from_secs(10) {
            let _ = child.kill();
            let _ = child.wait();
            panic!("blocked-output fixture did not start its owned descendant");
        }
        assert!(
            child.try_wait().unwrap().is_none(),
            "host exited before owned work started"
        );
        thread::sleep(Duration::from_millis(10));
    }
    let owned_pid = fs::read_to_string(fixture.root.join("owned-pid")).unwrap();
    assert!(owned_pid.parse::<u32>().unwrap() > 1);
    let flood = thread::spawn(move || {
        let frame = json!({"protocol":"cargo_ai_role_session","version":1,"type":"revoke","binding_revision":"different-revision"}).to_string();
        for _ in 0..100_000 {
            if writeln!(stdin, "{frame}").is_err() {
                break;
            }
        }
    });
    let blocked_at = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if blocked_at.elapsed() > Duration::from_secs(15) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = Command::new("kill")
                .args(["-KILL", owned_pid.trim()])
                .status();
            panic!("undrained public output failed to close authority and owned work before the session deadline");
        }
        thread::sleep(Duration::from_millis(10));
    };
    drop(stdout);
    flood.join().unwrap();
    assert!(
        !status.success(),
        "blocked control output cannot claim successful completion"
    );
    let liveness_deadline = Instant::now();
    loop {
        let alive = Command::new("kill")
            .args(["-0", owned_pid.trim()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap()
            .success();
        if !alive {
            break;
        }
        assert!(
            liveness_deadline.elapsed() < Duration::from_secs(2),
            "owned descendant survived blocked control-output cleanup"
        );
        thread::sleep(Duration::from_millis(10));
    }
    assert!(
        !fixture.root.join("completed").exists(),
        "cancelled owned work must not claim completion"
    );
    assert!(
        !fixture.root.join("forbidden").exists(),
        "later native dispatch must remain closed"
    );
}

fn successful_command(fixture: &Fixture, args: &[&str]) {
    let output = fixture
        .command(CLI)
        .args(["--no-update-check"])
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "command {args:?}: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn public_installed_noop(fixture: &Fixture, request: &Value) {
    let mut child = fixture
        .command(CLI)
        .args([
            "--no-update-check",
            "run",
            "--package",
            "native-fixture",
            "--interface",
            "fixture",
            "--action",
            "status",
            "--action-request-stdin",
            "--role-session",
            "--output-format",
            "ndjson",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    writeln!(
        stdin,
        "{}",
        json!({"protocol":"cargo_ai_role_session","version":1,"type":"start","request":request})
    )
    .unwrap();
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let (tx, rx) = mpsc::channel();
    let reader = thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            if tx.send(line.unwrap()).is_err() {
                break;
            }
        }
    });
    let errors = thread::spawn(move || {
        let mut text = String::new();
        BufReader::new(stderr).read_to_string(&mut text).unwrap();
        text
    });
    let started = Instant::now();
    let mut frames = Vec::new();
    loop {
        if started.elapsed() > Duration::from_secs(15) {
            let _ = child.kill();
            panic!("installed public session timed out: {frames:?}");
        }
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(line) => frames.push(serde_json::from_str::<Value>(&line).unwrap()),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    // Finishing without closing stdin proves the persistent reader did not require EOF.
    let status = child.wait().unwrap();
    drop(stdin);
    reader.join().unwrap();
    let diagnostics = errors.join().unwrap();
    assert!(
        status.success(),
        "installed public role session failed: {frames:?}\n{diagnostics}"
    );
    assert!(frames.iter().any(|frame|frame["outcome"]=="succeeded" || frame["data"]["outcome"]=="succeeded"),"missing successful terminal response: {frames:?}");
}

#[test]
fn installed_native_roles_preserve_private_mapping_and_public_structural_execution() {
    let provider = Provider::new();
    let mut fixture = Fixture::new(&provider);
    fixture.definitions(&fixture.root.join("child.json"));
    let mut bootstraps = [fixture.bootstrap(0), fixture.bootstrap(1)];
    let root = json!({"agent_definition_schema_version":"2026-10-06.r1","inputs":[{"type":"text","text":"Installed public business input"}],"agent_schema":{"type":"object","properties":{"status":{"type":"string"}}},"actions":[]});
    let status = json!({"agent_definition_schema_version":"2026-10-06.r1","agent_schema":{"type":"object","properties":{}},"actions":[]});
    fs::write(
        fixture.root.join("root.json"),
        serde_json::to_vec(&root).unwrap(),
    )
    .unwrap();
    fs::write(
        fixture.root.join("status.json"),
        serde_json::to_vec(&status).unwrap(),
    )
    .unwrap();
    let requirements = json!({"operation":"text_generation","input_modalities":["text"],"structured_output":true,"settings":{}});
    let limits = json!({"max_runtime_secs":30,"max_output_tokens":256,"max_agent_depth":4});
    let action = |id: &str, target: &str| json!({"id":id,"target":target,"input_schema":{"type":"object","properties":{},"required":[],"additionalProperties":false},"mappings":{}});
    let context = |action: &str, sites: Value| json!({"key":{"action":action,"interface":"fixture","mode":"default"},"call_sites":sites,"resources":[],"data_scopes":[],"limits":limits});
    let catalog = json!({"schema_version":3,"actions":[action("review","root.json"),action("status","status.json")],"interfaces":[{"id":"fixture","actions":["review","status"]}],
        "role_registry":{"version":1,"roles":[{"id":"writer","label":"Writer","purpose":"Review installed business content","requirements":requirements,"different_model_from":[]}],
        "call_sites":[{"id":"root","locator":{"definition":"root.json","site":"root"},"kind":"root","role":"writer","requirements":requirements}],
        "contexts":[context("review",json!(["root"])),context("status",json!([]))],"tool_content":{}}});
    fs::write(
        fixture.root.join("cargo-ai-actions.json"),
        serde_json::to_vec(&catalog).unwrap(),
    )
    .unwrap();
    fs::write(
        fixture.root.join("host-role-bindings.json"),
        b"{\"profile_uuid\":\"unselected-private-canary\"}",
    )
    .unwrap();
    fs::create_dir_all(fixture.root.join(".cargo-ai")).unwrap();
    fs::write(fixture.root.join(".cargo-ai/project.toml"),"format_version=1\n[project]\nname='native_installed_fixture'\nversion='0.1.0'\n[build.default]\nagent_definitions=['root.json','status.json']\nhatched_agents=[]\ntools=[]\nassets=['cargo-ai-actions.json']\n").unwrap();
    successful_command(
        &fixture,
        &["package", "default", "--output-dir", "export", "--force"],
    );
    successful_command(
        &fixture,
        &["packages", "install", "export", "--as", "native-fixture"],
    );
    fixture.contexts = [fixture.refresh("a"), fixture.refresh("b")];
    let installed = fixture
        .home
        .join("packages/native-fixture/package")
        .canonicalize()
        .unwrap();
    for name in ["root.json", "status.json", "cargo-ai-actions.json"] {
        assert_eq!(
            fs::read(installed.join(name)).unwrap(),
            fs::read(fixture.root.join(name)).unwrap()
        );
    }
    assert!(!installed.join("host-role-bindings.json").exists());
    assert!(!installed.join(fixture.home.file_name().unwrap()).exists());
    let discovered = machine(
        &fixture,
        &["actions", "list", "--package", "native-fixture"],
        None,
    );
    assert_eq!(
        discovered["role_registry"], catalog["role_registry"],
        "installed discovery preserves portable role declarations"
    );
    let config_before = fs::read(fixture.home.join("config.toml")).unwrap();
    for (mapping, bootstrap) in bootstraps.iter_mut().enumerate() {
        let mut writer = bootstrap["bindings"]["bindings"][0].clone();
        writer["profile_uuid"] = fixture.contexts[mapping]["profile_uuid"].clone();
        writer["connection_generation"] =
            fixture.contexts[mapping]["connection_generation"].clone();
        bootstrap["bindings"]["bindings"] = json!([writer]);
        bootstrap["resolution"]["calls"]
            .as_array_mut()
            .unwrap()
            .truncate(1);
        bootstrap["resolution"]["calls"][0]["evidence"][0]["profile_uuid"] =
            fixture.contexts[mapping]["profile_uuid"].clone();
        bootstrap["resolution"]["calls"][0]["evidence"][0]["connection_generation"] =
            fixture.contexts[mapping]["connection_generation"].clone();
        let selection = bootstrap["resolution"]["calls"][0]["selection"].clone();
        bootstrap["resolution"]["required_selections"] = json!([selection]);
        bootstrap["policy"]["allowed"] = json!([selection]);
        bootstrap["resolution"]["scope"]["call_sites"] = json!(["root"]);
        bootstrap["package_root"] = json!(installed);
        let mut identities = serde_json::Map::new();
        for name in [
            "root.json",
            "status.json",
            "cargo-ai-actions.json",
            "cargo-ai-package.toml",
        ] {
            let path = installed.join(name);
            identities.insert(
                path.to_str().unwrap().into(),
                json!(format!("{:x}", Sha256::digest(fs::read(path).unwrap()))),
            );
        }
        bootstrap["content_identities"] = Value::Object(identities);
        bootstrap["resolution"]["binding_identity"] = json!(format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&bootstrap["bindings"]).unwrap())
        ));
        let references = json!([{"role":writer["role"],"profile_uuid":writer["profile_uuid"],"connection_generation":writer["connection_generation"],"provider":writer["provider"]}]);
        let fixed = format!(
            "{:x}",
            Sha256::digest(
                serde_json::to_vec(&json!({"version":1,"fixed_connections":[]})).unwrap()
            )
        );
        bootstrap["resolution"]["context"]["connection_context_identity"] = json!(format!(
            "{:x}",
            Sha256::digest(
                serde_json::to_vec(
                    &json!({"role_connections":references,"fixed_connections":fixed})
                )
                .unwrap()
            )
        ));
        let mut request = json!({"schema_version":3,"interface":"fixture","action":"review","inputs":{},"expected_binding":discovered["binding"],"execution_policy":bootstrap["policy"],"attachment_grants":{},
            "role_execution":{"mode":"default","binding_revision":bootstrap["bindings"]}});
        let resolved = machine(
            &fixture,
            &[
                "actions",
                "resolve",
                "--package",
                "native-fixture",
                "--interface",
                "fixture",
                "--action",
                "review",
                "--request-stdin",
            ],
            Some(&request),
        );
        assert_eq!(
            resolved["ready"], false,
            "custom endpoint must not invent production compatibility"
        );
        assert_eq!(
            resolved["calls"][0]["selection"]["profile"],
            ["a", "b"][mapping]
        );
        assert_eq!(
            resolved["calls"][0]["selection"]["model"]["value"],
            ["role-model-a", "role-model-b"][mapping]
        );
        assert!(
            provider.take().is_empty(),
            "installed resolution is passive"
        );
        // Only the internal native test parent supplies compatible mock operation evidence.
        let run = invoke_reference(
            &fixture,
            Path::new(CLI),
            bootstrap.clone(),
            Control::Allow,
            Some("native-fixture::root"),
        );
        assert_success(&run);
        let requests = provider.take();
        assert_eq!(requests.len(), 1);
        assert_eq!(
            requests[0].1["model"],
            ["role-model-a", "role-model-b"][mapping]
        );
        assert_eq!(
            requests[0].1["temperature"],
            if mapping == 0 { 0.2 } else { 0.4 }
        );
        assert!(requests[0].0.to_lowercase().contains(&format!(
            "authorization: bearer synthetic-private-token-{}",
            ["a", "b"][mapping]
        )));
        request["action"] = json!("status");
        request["execution_policy"]["allowed"] = json!([]);
        let ready = machine(
            &fixture,
            &[
                "actions",
                "resolve",
                "--package",
                "native-fixture",
                "--interface",
                "fixture",
                "--action",
                "status",
                "--request-stdin",
            ],
            Some(&request),
        );
        assert_eq!(
            ready["ready"], true,
            "structural installed action needs no fabricated model capability"
        );
        assert_eq!(ready["required_selections"], json!([]));
        request["role_execution"]["resolution_id"] = ready["identity"].clone();
        public_installed_noop(&fixture, &request);
        assert!(provider.take().is_empty());
        assert_eq!(
            fs::read(fixture.home.join("config.toml")).unwrap(),
            config_before
        );
    }
}

fn structural_request(fixture: &Fixture, project: &Path) -> (Value, Value) {
    let project = project.to_str().unwrap();
    let discovered = machine(fixture, &["actions", "list", "--project", project], None);
    let limits = json!({"max_runtime_secs":30,"max_output_tokens":256,"max_agent_depth":4});
    let mut request = json!({"schema_version":3,"interface":"fixture","action":"work","inputs":{},"expected_binding":discovered["binding"],
        "execution_policy":{"version":1,"allowed":[],"limits":limits},"attachment_grants":{},
        "role_execution":{"mode":"default","binding_revision":{"version":1,"revision":"structural-revision","bindings":[]}}});
    let resolved = machine(
        fixture,
        &[
            "actions",
            "resolve",
            "--project",
            project,
            "--interface",
            "fixture",
            "--action",
            "work",
            "--request-stdin",
        ],
        Some(&request),
    );
    assert_eq!(resolved["ready"], true, "{resolved}");
    assert_eq!(resolved["required_selections"], json!([]));
    assert_eq!(resolved["execution_authorized"], false);
    request["role_execution"]["resolution_id"] = resolved["identity"].clone();
    (request, resolved)
}

fn machine_failure(fixture: &Fixture, args: &[&str], request: Option<&Value>) -> Value {
    let mut command = fixture.command(CLI);
    command
        .args(["--no-update-check"])
        .args(args)
        .args(["--output-format", "json"]);
    let output = if let Some(request) = request {
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(request.to_string().as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    } else {
        command.output().unwrap()
    };
    assert!(
        !output.status.success(),
        "unexpected success: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    serde_json::from_slice(&output.stdout).unwrap_or_else(|_| {
        panic!(
            "invalid machine failure: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

fn public_structural_run(fixture: &Fixture, project: &Path, cwd: &Path, request: &Value) -> Run {
    let mut child = fixture
        .command(CLI)
        .current_dir(cwd)
        .args([
            "--no-update-check",
            "run",
            "--project",
            project.to_str().unwrap(),
            "--interface",
            "fixture",
            "--action",
            "work",
            "--action-request-stdin",
            "--role-session",
            "--output-format",
            "ndjson",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    writeln!(
        input,
        "{}",
        json!({"protocol":"cargo_ai_role_session","version":1,"type":"start","request":request})
    )
    .unwrap();
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let (tx, rx) = mpsc::channel();
    let reader = thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            if tx.send(line.unwrap()).is_err() {
                break;
            }
        }
    });
    let errors = thread::spawn(move || {
        let mut text = String::new();
        BufReader::new(stderr).read_to_string(&mut text).unwrap();
        text
    });
    let start = Instant::now();
    let mut frames = Vec::new();
    loop {
        if start.elapsed() > Duration::from_secs(45) {
            let _ = child.kill();
            panic!("structural session timeout: {frames:?}");
        }
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(line) => frames.push(serde_json::from_str::<Value>(&line).unwrap()),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    let status = child.wait().unwrap();
    drop(input);
    reader.join().unwrap();
    Run {
        success: status.success(),
        frames,
        diagnostics: errors.join().unwrap(),
    }
}

fn exercise_nested_child_paths(emitted: bool) {
    let provider = Provider::new();
    let fixture = Fixture::new(&provider);
    fs::create_dir_all(fixture.root.join(".cargo-ai")).unwrap();
    fs::create_dir(fixture.root.join("agents")).unwrap();
    fs::create_dir(fixture.root.join("mutable-data")).unwrap();
    fs::write(
        fixture.root.join(".cargo-ai/project.toml"),
        "[project]\nname='nested-child-fixture'\n",
    )
    .unwrap();
    let child = json!({"agent_definition_schema_version":"2026-10-06.r1","agent_schema":{"type":"object","properties":{}},"actions":[]});
    fs::write(
        fixture.root.join("agents/child.json"),
        serde_json::to_vec(&child).unwrap(),
    )
    .unwrap();
    let emitted_child = if cfg!(windows) {
        "nested-child.exe"
    } else {
        "nested-child"
    };
    let artifact = if emitted {
        format!("./{emitted_child}")
    } else {
        "./child.json".into()
    };
    let parent = json!({"agent_definition_schema_version":"2026-10-06.r1","agent_schema":{"type":"object","properties":{}},"actions":[{"name":"work","logic":{"==":[1,1]},"run":[{"kind":"agent","artifact":artifact}]}]});
    fs::write(
        fixture.root.join("agents/parent.json"),
        serde_json::to_vec(&parent).unwrap(),
    )
    .unwrap();
    let limits = json!({"max_runtime_secs":30,"max_output_tokens":256,"max_agent_depth":4});
    let mut call = json!({"id":"nested-child","locator":{"definition":"agents/parent.json","site":"actions.0.run.0"},"kind":"child","target":"agents/child.json","requirements":{"operation":"text_generation","input_modalities":[],"structured_output":false,"settings":{}}});
    if emitted {
        call["artifact"] = json!(format!("agents/{emitted_child}"));
        let executable = fixture.hatch("nested-child", "agents/child.json");
        fs::copy(executable, fixture.root.join("agents").join(emitted_child)).unwrap();
    }
    let catalog = json!({"schema_version":3,"actions":[{"id":"work","target":"agents/parent.json","input_schema":{"type":"object","properties":{},"required":[],"additionalProperties":false},"mappings":{}}],"interfaces":[{"id":"fixture","actions":["work"]}],"role_registry":{"version":1,"roles":[],"call_sites":[call],"contexts":[{"key":{"action":"work","interface":"fixture","mode":"default"},"call_sites":["nested-child"],"resources":[],"data_scopes":[],"limits":limits}],"tool_content":{}}});
    fs::write(
        fixture.root.join("cargo-ai-actions.json"),
        serde_json::to_vec(&catalog).unwrap(),
    )
    .unwrap();
    let (request, resolution) = structural_request(&fixture, &fixture.root);
    let data = fixture.root.join("mutable-data");
    fs::write(data.join("child.json"), b"counterfeit-data-definition").unwrap();
    let public = public_structural_run(&fixture, &fixture.root, &data, &request);
    assert!(
        public.success,
        "nested public child failed: {}\n{:?}",
        public.diagnostics, public.frames
    );
    let mut paths = vec![
        "agents/parent.json".to_owned(),
        "agents/child.json".to_owned(),
        "cargo-ai-actions.json".to_owned(),
    ];
    if emitted {
        paths.push(format!("agents/{emitted_child}"));
    }
    let originals: Vec<_> = paths
        .iter()
        .map(|path| (path.clone(), fs::read(fixture.root.join(path)).unwrap()))
        .collect();
    let content: serde_json::Map<_, _> = originals
        .iter()
        .map(|(path, bytes)| {
            (
                fixture.root.join(path).to_str().unwrap().to_owned(),
                json!(format!("{:x}", Sha256::digest(bytes))),
            )
        })
        .collect();
    let bootstrap = json!({"version":1,"resolution":resolution,"bindings":request["role_execution"]["binding_revision"],"policy":request["execution_policy"],"definition":"agents/parent.json","package_root":fixture.root,"content_identities":content,"invocation_id":"nested-invocation","binding_revision":"structural-revision","parent_permit_id":"nested-parent"});
    let executable = if emitted {
        fixture.hatch("nested-parent", "agents/parent.json")
    } else {
        PathBuf::from(CLI)
    };
    let definition = fixture.root.join("agents/parent.json");
    let run = invoke_reference_in_dir(
        &fixture,
        &executable,
        bootstrap,
        Control::Allow,
        Some(definition.to_str().unwrap()),
        &data,
    );
    assert_success(&run);
    assert!(
        run.frames.iter().any(|frame| frame["type"] == "admit"
            && frame["boundary"]["target_agent"] == "agents/child.json"
            && frame["boundary"]["parent_permit_id"] == "nested-parent"),
        "portable nested child admission missing: {:?}",
        run.frames
    );
    for (path, bytes) in originals {
        assert_eq!(fs::read(fixture.root.join(path)).unwrap(), bytes);
    }
    assert_eq!(
        fs::read(data.join("child.json")).unwrap(),
        b"counterfeit-data-definition"
    );
    assert!(!data.join("agents").exists());
    assert!(
        provider.take().is_empty(),
        "structural nested child must not invoke providers"
    );
}

#[test]
fn cli_native_nested_child_paths_are_portable() {
    exercise_nested_child_paths(false);
}

#[test]
#[ignore = "builds and executes actual nested generated parent and child"]
fn emitted_native_nested_child_paths_are_portable() {
    exercise_nested_child_paths(true);
}

#[cfg(unix)]
fn structural_child_fixture(fixture: &Fixture) -> (Value, Value) {
    fs::create_dir_all(fixture.root.join(".cargo-ai")).unwrap();
    fs::write(
        fixture.root.join(".cargo-ai/project.toml"),
        "[project]\nname='structural-child-fixture'\n",
    )
    .unwrap();
    let definition = |steps: Value| json!({"agent_definition_schema_version":"2026-10-06.r1","agent_schema":{"type":"object","properties":{}},"actions":[{"name":"work","logic":{"==":[1,1]},"run":steps}]});
    fs::write(
        fixture.root.join("root.json"),
        serde_json::to_vec(&definition(
            json!([{"kind":"agent","artifact":"./child.json"}]),
        ))
        .unwrap(),
    )
    .unwrap();
    fs::write(fixture.root.join("child.json"),serde_json::to_vec(&definition(json!([{"kind":"exec","program":"/bin/sh","args":["-c","printf declared > child-completed"]}]))).unwrap()).unwrap();
    let limits = json!({"max_runtime_secs":30,"max_output_tokens":256,"max_agent_depth":4});
    fs::write(fixture.root.join("cargo-ai-actions.json"),serde_json::to_vec(&json!({"schema_version":3,
        "actions":[{"id":"work","target":"root.json","input_schema":{"type":"object","properties":{},"required":[],"additionalProperties":false},"mappings":{}}],"interfaces":[{"id":"fixture","actions":["work"]}],
        "role_registry":{"version":1,"roles":[],"call_sites":[{"id":"child-launch","locator":{"definition":"root.json","site":"actions.0.run.0"},"kind":"child","target":"child.json","requirements":{"operation":"text_generation","input_modalities":[],"structured_output":false,"settings":{}}}],
        "contexts":[{"key":{"action":"work","interface":"fixture","mode":"default"},"call_sites":["child-launch"],"resources":[],"data_scopes":[],"limits":limits}],"tool_content":{}}})).unwrap()).unwrap();
    structural_request(fixture, &fixture.root)
}

#[cfg(unix)]
fn exercise_structural_child_from_data_cwd(emitted: bool) {
    let provider = Provider::new();
    let fixture = Fixture::new(&provider);
    let (mut request, mut resolution) = structural_child_fixture(&fixture);
    if emitted {
        let child = fixture.hatch("structural-child", "child.json");
        fs::copy(child, fixture.root.join("structural-child")).unwrap();
        let mut definition: Value =
            serde_json::from_slice(&fs::read(fixture.root.join("root.json")).unwrap()).unwrap();
        definition["actions"][0]["run"][0]["artifact"] = json!("./structural-child");
        fs::write(
            fixture.root.join("root.json"),
            serde_json::to_vec(&definition).unwrap(),
        )
        .unwrap();
        let mut catalog: Value =
            serde_json::from_slice(&fs::read(fixture.root.join("cargo-ai-actions.json")).unwrap())
                .unwrap();
        catalog["role_registry"]["call_sites"][0]["artifact"] = json!("structural-child");
        fs::write(
            fixture.root.join("cargo-ai-actions.json"),
            serde_json::to_vec(&catalog).unwrap(),
        )
        .unwrap();
        (request, resolution) = structural_request(&fixture, &fixture.root);
    }
    let data = fixture.root.join("mutable-data");
    fs::create_dir(&data).unwrap();
    let mut originals: Vec<_> = ["root.json", "child.json", "cargo-ai-actions.json"]
        .into_iter()
        .map(|name| (name, fs::read(fixture.root.join(name)).unwrap()))
        .collect();
    if emitted {
        originals.push((
            "structural-child",
            fs::read(fixture.root.join("structural-child")).unwrap(),
        ));
    }
    let executable = if emitted {
        fixture.hatch("structural-parent", "root.json")
    } else {
        PathBuf::from(CLI)
    };
    for counterfeit in [false, true] {
        if counterfeit {
            fs::write(data.join("child.json"),br#"{"agent_schema":{"type":"object","properties":{}},"actions":[{"name":"counterfeit","logic":{"==":[1,1]},"run":[{"kind":"exec","program":"/bin/sh","args":["-c","printf counterfeit > counterfeit-completed"]}]}]}"#).unwrap();
        }
        let public = public_structural_run(&fixture, &fixture.root, &data, &request);
        assert!(
            public.success,
            "public child from data cwd failed: {}\n{:?}",
            public.diagnostics, public.frames
        );
        fs::remove_file(fixture.root.join("child-completed")).unwrap();
        let content: serde_json::Map<_, _> = originals
            .iter()
            .map(|(name, bytes)| {
                (
                    fixture.root.join(name).to_str().unwrap().to_owned(),
                    json!(format!("{:x}", Sha256::digest(bytes))),
                )
            })
            .collect();
        let bootstrap = json!({"version":1,"resolution":resolution,"bindings":request["role_execution"]["binding_revision"],"policy":request["execution_policy"],"definition":"root.json","package_root":fixture.root,"content_identities":content,"invocation_id":"structural-invocation","binding_revision":"structural-revision","parent_permit_id":"structural-parent"});
        let root = fixture.root.join("root.json");
        let run = invoke_reference_in_dir(
            &fixture,
            &executable,
            bootstrap.clone(),
            Control::Allow,
            Some(root.to_str().unwrap()),
            &data,
        );
        assert_success(&run);
        assert_eq!(
            fs::read(fixture.root.join("child-completed")).unwrap(),
            b"declared"
        );
        assert!(!data.join("root.json").exists());
        assert!(!data.join("child-completed").exists());
        assert!(!data.join("counterfeit-completed").exists());
        let admissions: Vec<_> = run
            .frames
            .iter()
            .filter(|frame| frame["type"] == "admit")
            .collect();
        assert_eq!(admissions.len(), 1, "{admissions:?}");
        assert_eq!(admissions[0]["boundary"]["call_site"], "child-launch");
        assert_eq!(admissions[0]["boundary"]["target_agent"], "child.json");
        assert_eq!(
            admissions[0]["boundary"]["parent_permit_id"],
            "structural-parent"
        );
        let denied = invoke_reference_in_dir(
            &fixture,
            &executable,
            bootstrap.clone(),
            Control::DenyAfter(0),
            Some(root.to_str().unwrap()),
            &data,
        );
        assert!(
            !denied.success,
            "independent child admission must be enforced"
        );
        let mut undeclared = bootstrap.clone();
        undeclared["resolution"]["calls"] = json!([]);
        let rejected = invoke_reference_in_dir(
            &fixture,
            &executable,
            undeclared,
            Control::Allow,
            Some(root.to_str().unwrap()),
            &data,
        );
        assert!(!rejected.success, "undeclared child must fail");
        let mut escaped = bootstrap.clone();
        escaped["resolution"]["calls"][0]["call_site"]["target"] = json!("../escape.json");
        let rejected = invoke_reference_in_dir(
            &fixture,
            &executable,
            escaped,
            Control::Allow,
            Some(root.to_str().unwrap()),
            &data,
        );
        assert!(!rejected.success, "escaping child must fail");
        fs::write(fixture.root.join("child.json"), b"{}").unwrap();
        let stale = invoke_reference_in_dir(
            &fixture,
            &executable,
            bootstrap,
            Control::Allow,
            Some(root.to_str().unwrap()),
            &data,
        );
        assert!(!stale.success, "stale child must fail");
        fs::write(fixture.root.join("child.json"), &originals[1].1).unwrap();
        assert!(
            provider.take().is_empty(),
            "structural child must issue zero HTTP requests"
        );
        for (name, bytes) in &originals {
            assert_eq!(&fs::read(fixture.root.join(name)).unwrap(), bytes);
        }
    }
}

#[cfg(unix)]
#[test]
fn cli_structural_child_uses_package_definition_from_mutable_data_cwd() {
    exercise_structural_child_from_data_cwd(false);
}

#[cfg(unix)]
#[test]
#[ignore = "builds and executes a fresh generated parent; run explicitly in native qualification"]
fn emitted_structural_child_uses_package_definition_from_mutable_data_cwd() {
    exercise_structural_child_from_data_cwd(true);
}

#[test]
fn native_target_build_preserves_source_backed_role_tool_inventory() {
    let provider = Provider::new();
    let fixture = Fixture::new(&provider);
    let tools = fixture.root.join("tools/portable");
    fs::create_dir_all(tools.join("src")).unwrap();
    fs::create_dir_all(fixture.root.join(".cargo-ai/tools/portable")).unwrap();
    fs::write(
        tools.join("Cargo.toml"),
        "[package]\nname='portable'\nversion='0.1.0'\nedition='2021'\n[workspace]\n",
    )
    .unwrap();
    fs::write(
        tools.join("Cargo.lock"),
        "version = 3\n\n[[package]]\nname = \"portable\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    let describe = json!({"protocol_version":1,"supported_protocol_versions":[1,2],"name":"portable","description":"Portable structural fixture","params":{},"result":{"type":"string","nullable":true},"resource_profile":{"network":"none","filesystem_read":"none","filesystem_write":"none","subprocess":"none","env_read":"none","credential_access":"none"},"self_test":{"supported":false,"safe":false},"examples":{"minimal_invoke":{"protocol_version":1,"params":{}},"full_invoke":{"protocol_version":1,"params":{}}}});
    fs::write(tools.join("src/main.rs"),format!("use std::io::{{self, BufRead}};\nfn main() {{ if std::env::args().nth(1).as_deref() == Some(\"describe\") {{ println!(\"{{}}\",r#\"{describe}\"#); }} else {{ let mut line = String::new(); io::stdin().lock().read_line(&mut line).unwrap(); println!(\"{{}}\",r#\"{{\"protocol_version\":2,\"type\":\"result\",\"result\":\"portable\"}}\"#); }} }}\n")).unwrap();
    fs::write(fixture.root.join(".cargo-ai/tools/portable/tool.json"),serde_json::to_vec(&json!({"schema_version":1,"tool_id":"portable","source":{"manifest_path":"tools/portable/Cargo.toml"},"binary":{"default_name":"portable"},"artifacts":{}})).unwrap()).unwrap();
    fs::write(fixture.root.join("root.json"),serde_json::to_vec(&json!({"agent_definition_schema_version":"2026-10-06.r1","agent_schema":{"type":"object","properties":{}},"actions":[{"name":"work","logic":{"==":[1,1]},"run":[{"kind":"tool","name":"portable","params":{}}]}]})).unwrap()).unwrap();
    let limits = json!({"max_runtime_secs":30,"max_output_tokens":256,"max_agent_depth":4});
    let inventory = [
        "tools/portable/Cargo.toml",
        "tools/portable/Cargo.lock",
        "tools/portable/src/main.rs",
    ];
    fs::write(fixture.root.join("cargo-ai-actions.json"),serde_json::to_vec(&json!({"schema_version":3,"actions":[{"id":"work","target":"root.json","input_schema":{"type":"object","properties":{},"required":[],"additionalProperties":false},"mappings":{}}],"interfaces":[{"id":"fixture","actions":["work"]}],"role_registry":{"version":1,"roles":[],"call_sites":[],"contexts":[{"key":{"action":"work","interface":"fixture","mode":"default"},"call_sites":[],"resources":[],"data_scopes":[],"limits":limits}],"tool_content":{"portable":inventory}}})).unwrap()).unwrap();
    fs::write(fixture.root.join(".cargo-ai/project.toml"),"format_version=1\n[project]\nname='portable-built-fixture'\nversion='0.1.0'\n[build.default]\nagent_definitions=['root.json']\nhatched_agents=[]\ntools=['portable']\nassets=['cargo-ai-actions.json','tools/portable/Cargo.toml','tools/portable/Cargo.lock','tools/portable/src/main.rs']\n").unwrap();
    fs::write(
        fixture.root.join("host-role-bindings.json"),
        b"private-unselected-canary",
    )
    .unwrap();
    let version = Command::new("rustc").arg("-vV").output().unwrap();
    let version = String::from_utf8(version.stdout).unwrap();
    let target = version
        .lines()
        .find_map(|line| line.strip_prefix("host: "))
        .unwrap();
    successful_command(
        &fixture,
        &[
            "build",
            "default",
            "--target",
            target,
            "--output-dir",
            "built",
            "--force",
        ],
    );
    let built = fixture.root.join("built");
    let manifest: Value = serde_json::from_slice(
        &fs::read(built.join(".cargo-ai/tools/portable/tool.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        manifest["source"]["manifest_path"], "tools/portable/Cargo.toml",
        "target-built tools must retain portable source metadata: {manifest}"
    );
    let artifact = manifest["artifacts"][target]["path"].as_str().unwrap();
    assert!(
        !artifact.contains('\\'),
        "portable artifact path: {artifact}"
    );
    let source_manifest: Value = serde_json::from_slice(
        &fs::read(fixture.root.join(".cargo-ai/tools/portable/tool.json")).unwrap(),
    )
    .unwrap();
    let source_artifact = source_manifest["artifacts"][target]["path"]
        .as_str()
        .unwrap();
    assert!(
        !source_artifact.contains('\\'),
        "portable source-project artifact path: {source_artifact}"
    );
    machine(&fixture, &["actions", "list", "--project", "."], None);
    let binary = built
        .join(".cargo-ai/tools/portable")
        .join(manifest["artifacts"][target]["path"].as_str().unwrap());
    assert!(binary.is_file());
    for path in inventory {
        assert_eq!(
            fs::read(built.join(path)).unwrap(),
            fs::read(fixture.root.join(path)).unwrap()
        );
    }
    assert!(!built.join("host-role-bindings.json").exists());
    assert!(!built.join("tools/portable/target").exists());
    assert!(!built.join("h").exists());
    assert!(
        !manifest
            .to_string()
            .contains(fixture.root.to_str().unwrap()),
        "no author-machine paths in portable tool manifest"
    );
    let project_metadata = fixture.root.join(".cargo-ai/project.toml");
    let metadata = fs::read_to_string(&project_metadata).unwrap();
    let preserved_manifest = fs::read(built.join(".cargo-ai/tools/portable/tool.json")).unwrap();
    fs::write(
        &project_metadata,
        metadata.replace(",'tools/portable/src/main.rs'", ""),
    )
    .unwrap();
    let rejected = fixture
        .command(CLI)
        .args([
            "--no-update-check",
            "build",
            "default",
            "--target",
            target,
            "--output-dir",
            "built",
            "--force",
        ])
        .output()
        .unwrap();
    assert!(
        !rejected.status.success(),
        "catalog tool source omission must fail before output replacement"
    );
    let diagnostic = String::from_utf8_lossy(&rejected.stderr);
    assert!(
        diagnostic.contains("Native role tool source")
            && diagnostic.contains("not explicitly selected"),
        "source-selection rejection must not be masked by invalid catalog: {diagnostic}"
    );
    assert_eq!(
        fs::read(built.join(".cargo-ai/tools/portable/tool.json")).unwrap(),
        preserved_manifest
    );
    fs::write(&project_metadata,format!("{metadata}\n[build.legacy]\nagent_definitions=['root.json']\nhatched_agents=[]\ntools=['portable']\nassets=[]\n")).unwrap();
    successful_command(
        &fixture,
        &[
            "build",
            "legacy",
            "--target",
            target,
            "--output-dir",
            "legacy-built",
            "--force",
        ],
    );
    let legacy = fixture.root.join("legacy-built");
    let legacy_manifest: Value = serde_json::from_slice(
        &fs::read(legacy.join(".cargo-ai/tools/portable/tool.json")).unwrap(),
    )
    .unwrap();
    assert!(
        legacy_manifest.get("source").is_none(),
        "unselected sources must not change legacy binary-only manifests"
    );
    assert!(!legacy.join("tools").exists());
    fs::write(&project_metadata, metadata).unwrap();
    fs::rename(&tools, fixture.root.join("detached-original-source")).unwrap();
    let (request, _) = structural_request(&fixture, &built);
    let discovered = machine(
        &fixture,
        &[
            "actions",
            "validate",
            "--project",
            built.to_str().unwrap(),
            "--interface",
            "fixture",
            "--action",
            "work",
            "--request-stdin",
        ],
        Some(&request),
    );
    assert!(discovered.is_object());
    let source = built.join("tools/portable/src/main.rs");
    let original = fs::read(&source).unwrap();
    fs::write(&source, b"fn main() {}\n").unwrap();
    let changed = machine(
        &fixture,
        &["actions", "list", "--project", built.to_str().unwrap()],
        None,
    );
    assert_ne!(
        changed["binding"], request["expected_binding"],
        "built source digest participates in native inventory"
    );
    machine_failure(
        &fixture,
        &[
            "actions",
            "resolve",
            "--project",
            built.to_str().unwrap(),
            "--interface",
            "fixture",
            "--action",
            "work",
            "--request-stdin",
        ],
        Some(&request),
    );
    fs::write(&source, original).unwrap();
    let native = fs::read(&binary).unwrap();
    fs::write(&binary, b"changed-native-artifact").unwrap();
    let changed = machine(
        &fixture,
        &["actions", "list", "--project", built.to_str().unwrap()],
        None,
    );
    assert_ne!(
        changed["binding"], request["expected_binding"],
        "built binary digest participates in native inventory"
    );
    fs::write(&binary, native).unwrap();
    fs::write(
        built.join("tools/portable/src/unlisted.rs"),
        b"unlisted source",
    )
    .unwrap();
    machine_failure(
        &fixture,
        &["actions", "list", "--project", built.to_str().unwrap()],
        None,
    );
    assert!(
        provider.take().is_empty(),
        "target build/discovery/resolution must issue zero provider HTTP requests"
    );
}
