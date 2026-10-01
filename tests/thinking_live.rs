//! Explicit, bounded hosted-provider thinking qualification.

use serde_json::{json, Value};
#[path = "../src/providers/thinking.rs"]
mod thinking;
#[path = "../src/providers/thinking_image.rs"]
mod thinking_image;
use std::{
    env, fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};

const PROVIDERS: [&str; 5] = ["openai", "anthropic", "gemini", "xai", "mistral"];
const KEYS: [&str; 6] = [
    "OPENAI_API_KEY",
    "ANTHROPIC_API_KEY",
    "GEMINI_API_KEY",
    "XAI_API_KEY",
    "MISTRAL_API_KEY",
    "TYPESAFE_API_KEY",
];
const UNAVAILABLE: &str = "cargo_ai_qualification_unavailable_choice";

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let base = env::var_os("RUNNER_TEMP")
            .map(PathBuf::from)
            .unwrap_or_else(env::temp_dir);
        let root = base.join(format!("thinking-{}", uuid::Uuid::new_v4().simple()));
        fs::create_dir(&root).expect("private fixture creation failed");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&root, fs::Permissions::from_mode(0o700))
                .expect("private fixture permissions failed");
        }
        fs::create_dir(root.join("home")).expect("isolated home creation failed");
        fs::create_dir(root.join("user")).expect("isolated user creation failed");
        fs::write(
            root.join("home/config.toml"),
            "secret_store='file'\nprofile=[]\n",
        )
        .expect("isolated config creation failed");
        fs::write(root.join("answer.json"), json!({
            "agent_definition_schema_version":"2026-03-03.r1",
            "inputs":[{"type":"text","text":"Return status ok."}],
            "agent_schema":{"type":"object","properties":{"status":{"type":"string","enum":["ok"]}},"required":["status"],"additionalProperties":false},
            "actions":[]
        }).to_string()).expect("definition creation failed");
        Self(root)
    }
    fn command(&self, program: impl AsRef<std::ffi::OsStr>) -> Command {
        let mut command = Command::new(program);
        command
            .current_dir(&self.0)
            .env("CARGO_AI_HOME", self.0.join("home"))
            .env("HOME", self.0.join("user"))
            .env("CODEX_HOME", self.0.join("codex"))
            .env("CARGO_AI_DISABLE_KEYCHAIN", "1")
            .env_remove("CARGO_TARGET_DIR")
            .env_remove("CARGO_AI_USAGE_ROOT_RUN_ID")
            .env_remove("CARGO_AI_USAGE_PARENT_AGENT_RUN_ID");
        for name in KEYS {
            command.env_remove(name);
        }
        command
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn input(name: &str) -> String {
    let value = env::var(name).expect("missing explicit thinking qualification input");
    assert!(
        !value.trim().is_empty() && value.len() <= 1024 && !value.chars().any(char::is_control),
        "invalid thinking qualification input"
    );
    value
}

fn invoke(mut command: Command, token: Option<&str>) -> Output {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    if token.is_some() {
        command.stdin(Stdio::piped());
    } else {
        command.stdin(Stdio::null());
    }
    let mut child = command
        .spawn()
        .expect("qualification process failed to start");
    if let Some(token) = token {
        child
            .stdin
            .take()
            .expect("credential stdin unavailable")
            .write_all(token.as_bytes())
            .expect("credential stdin failed");
    }
    let output = child
        .wait_with_output()
        .expect("qualification process failed to finish");
    if let Some(token) = token {
        safe_output(&output, token);
    }
    output
}

fn safe_output(output: &Output, token: &str) {
    assert!(
        output.stdout.len() <= 1024 * 1024 && output.stderr.len() <= 1024 * 1024,
        "oversized qualification output"
    );
    for bytes in [&output.stdout, &output.stderr] {
        assert!(
            !bytes
                .windows(token.len())
                .any(|part| part == token.as_bytes()),
            "qualification output redaction failed"
        );
    }
}

fn terminal(output: &Output) -> Value {
    assert!(
        output.status.success() && output.stderr.is_empty(),
        "machine qualification invocation failed"
    );
    let value: Value =
        serde_json::from_slice(&output.stdout).expect("invalid machine qualification response");
    assert!(
        value["schema_version"] == 1
            && value["outcome"] == "succeeded"
            && value["completion"]["complete"] == true
            && value["completion"]["terminal"] == true
            && value["error"].is_null(),
        "incomplete machine qualification response"
    );
    value
}

fn hatch(fixture: &Fixture, name: &str, definition: &str) -> PathBuf {
    let mut command = fixture.command(env!("CARGO_BIN_EXE_cargo-ai"));
    command.args([
        "--no-update-check",
        "hatch",
        name,
        "--config",
        definition,
        "--output-dir",
        "dist",
        "--force",
    ]);
    let build = invoke(command, None);
    assert!(
        build.status.success(),
        "thinking qualification hatch failed"
    );
    assert!(
        !String::from_utf8_lossy(&build.stderr)
            .lines()
            .any(|line| line.trim_start().starts_with("warning:")
                || line.trim_start().starts_with("warning[")),
        "thinking qualification hatch emitted warnings"
    );
    let executable = fixture.0.join("dist").join(if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.into()
    });
    assert!(
        executable.is_file(),
        "missing emitted thinking qualification application"
    );
    executable
}

fn configure_profile(fixture: &Fixture, name: &str, provider: &str, model: &str, token: &str) {
    let mut add = fixture.command(env!("CARGO_BIN_EXE_cargo-ai"));
    add.args([
        "profile",
        "add",
        name,
        "--server",
        provider,
        "--model",
        model,
        "--auth",
        "api_key",
        "--max-output-tokens",
        "8192",
        "--output-format",
        "json",
    ]);
    terminal(&invoke(add, None));
    let mut set = fixture.command(env!("CARGO_BIN_EXE_cargo-ai"));
    set.args(["profile", "set", name, "--stdin", "--output-format", "json"]);
    terminal(&invoke(set, Some(token)));
}

fn image_inputs(provider: &str) -> Option<(String, String)> {
    let model = env::var("CARGO_AI_THINKING_IMAGE_MODEL")
        .ok()
        .filter(|value| !value.is_empty());
    let choice = env::var("CARGO_AI_THINKING_IMAGE_CHOICE")
        .ok()
        .filter(|value| !value.is_empty());
    match (model, choice) {
        (None, None) => None,
        (Some(model), Some(choice)) => {
            assert!(
                provider == "gemini"
                    && [&model, &choice]
                        .iter()
                        .all(|value| value.len() <= 1024 && !value.chars().any(char::is_control)),
                "invalid explicit image thinking inputs"
            );
            let support = thinking_image::gemini_support(
                "https://generativelanguage.googleapis.com/v1beta/interactions",
                &model,
            );
            let setting = thinking::ThinkingSetting::Choice {
                value: choice.clone(),
            };
            let outcome = thinking::resolve(Some(&setting), "qualification", support);
            assert!(
                outcome.applied_choice() == Some(choice.as_str()) && outcome.notice().is_none(),
                "image model and named choice are not exactly qualified"
            );
            Some((model, choice))
        }
        _ => panic!("image thinking requires both explicit model and choice"),
    }
}

fn one_successful_image_request(path: &Path, model: &str, token: &str) {
    let raw = fs::read_to_string(path).expect("missing image qualification usage evidence");
    assert!(
        raw.len() <= 64 * 1024 && !raw.contains(token),
        "image qualification usage redaction failed"
    );
    let events: Vec<Value> = raw
        .lines()
        .map(|line| serde_json::from_str(line).expect("invalid image qualification usage evidence"))
        .collect();
    let requests: Vec<_> = events
        .iter()
        .filter(|e| e["event_type"] == "provider_request_completed")
        .collect();
    assert!(
        requests.len() == 1
            && requests[0]["status"] == "success"
            && requests[0]["provider"]["server"] == "gemini"
            && requests[0]["provider"]["model"] == model
            && requests[0]["provider"]["profile"] == "thinking-image"
            && requests[0]["provider"]["auth_mode"] == "api_key"
            && requests[0]["step"]["kind"] == "generate_image",
        "image qualification request identity or count mismatch"
    );
    assert!(
        events
            .iter()
            .all(|e| e["root_run_id"] == events[0]["root_run_id"])
            && events.last().is_some_and(
                |e| e["event_type"] == "root_run_completed" && e["status"] == "success"
            ),
        "incomplete image qualification journey"
    );
}

fn one_successful_request(path: &Path, provider: &str, model: &str, token: &str) {
    let raw = fs::read_to_string(path).expect("missing qualification usage evidence");
    assert!(
        raw.len() <= 64 * 1024 && !raw.contains(token),
        "qualification usage redaction failed"
    );
    let events: Vec<Value> = raw
        .lines()
        .map(|line| serde_json::from_str(line).expect("invalid qualification usage evidence"))
        .collect();
    let expected = [
        "usage_log_started",
        "agent_run_started",
        "provider_request_completed",
        "agent_run_completed",
        "root_run_completed",
    ];
    assert!(
        events.len() == expected.len()
            && events
                .iter()
                .zip(expected)
                .all(|(e, kind)| e["event_type"] == kind),
        "unexpected qualification request count"
    );
    let request = &events[2];
    assert!(
        request["status"] == "success"
            && request["provider"]["server"] == provider
            && request["provider"]["model"] == model
            && request["provider"]["profile"] == "thinking-live"
            && request["provider"]["auth_mode"] == "api_key",
        "qualification request identity mismatch"
    );
    assert!(
        events[0]["root_run_id"]
            .as_str()
            .is_some_and(|id| !id.is_empty())
            && events[1]["agent_run_id"]
                .as_str()
                .is_some_and(|id| !id.is_empty())
            && events[1..4]
                .iter()
                .all(|e| e["agent_run_id"] == events[1]["agent_run_id"]
                    && e["parent_agent_run_id"].is_null()
                    && e["depth"] == 0)
            && events[3..].iter().all(|e| e["status"] == "success")
            && events
                .iter()
                .all(|e| e["root_run_id"] == events[0]["root_run_id"]),
        "inconsistent qualification completion"
    );
}

#[test]
#[ignore = "requires protected qualification identity and explicit provider/model/choice"]
fn live_thinking_journey_uses_isolated_stdin_credentials() {
    let provider = input("CARGO_AI_THINKING_PROVIDER");
    assert!(
        PROVIDERS.contains(&provider.as_str()),
        "unsupported thinking qualification provider"
    );
    let model = input("CARGO_AI_THINKING_MODEL");
    let choice = input("CARGO_AI_THINKING_CHOICE");
    let image_inputs = image_inputs(&provider);
    let candidate = input("CARGO_AI_SHA");
    let run_id = input("GITHUB_RUN_ID");
    let run_attempt = input("GITHUB_RUN_ATTEMPT");
    let probe_id = input("CARGO_AI_QUALIFICATION_PROBE");
    let report = PathBuf::from(input("CARGO_AI_QUALIFICATION_REPORT"));
    assert!(
        report.is_absolute(),
        "qualification report must be absolute"
    );
    let fixture = Fixture::new();
    let cli = env!("CARGO_BIN_EXE_cargo-ai");
    // Compile the actual emitted application before reading or injecting credentials.
    let generated = hatch(&fixture, "thinking_live", "answer.json");
    let generated_image = image_inputs.as_ref().map(|(model, choice)| {
        fs::write(fixture.0.join("image.json"), json!({
            "agent_definition_schema_version":"2026-10-01.r1",
            "agent_schema":{"type":"object","properties":{}},
            "actions":[{"name":"image","logic":{"==":[1,1]},"run":[{
                "kind":"generate_image","profile":"thinking-image","model":model,
                "thinking":{"mode":"choice","value":choice},
                "prompt":"One solid red circle on a plain white background.","path":"./image.jpg"
            }]}]
        }).to_string()).expect("image thinking fixture creation failed");
        hatch(&fixture, "thinking_image_live", "image.json")
    });
    let token = env::var(format!("{}_API_KEY", provider.to_uppercase()))
        .expect("missing qualification credential");
    assert!(
        !token.trim().is_empty()
            && token.len() <= 16 * 1024
            && !token.chars().any(char::is_control),
        "invalid qualification credential"
    );
    assert!(
        !model.contains(&token) && !choice.contains(&token),
        "qualification input redaction failed"
    );
    let mut discover = fixture.command(cli);
    discover.args([
        "models",
        "thinking",
        "--server",
        &provider,
        "--auth",
        "api_key",
        "--model",
        &model,
        "--stdin",
        "--output-format",
        "json",
        "--output-schema-version",
        "1",
    ]);
    let support = terminal(&invoke(discover, Some(&token)));
    assert!(
        support["command"] == "models thinking"
            && support["data"]["provider"] == provider
            && support["data"]["model"] == model
            && support["data"]["thinking"]["status"] == "configurable",
        "selected thinking control is not qualified"
    );
    let choices = support["data"]["thinking"]["choices"]
        .as_array()
        .expect("invalid qualified thinking choices");
    assert!(
        choices.iter().any(|c| c["value"] == choice)
            && !choices.iter().any(|c| c["value"] == UNAVAILABLE),
        "selected thinking choice is not exact"
    );
    configure_profile(&fixture, "thinking-live", &provider, &model, &token);
    let mut cases = Vec::new();
    for (index, name, emitted, selected) in [
        (0, "interpreted_choice", false, Some(choice.as_str())),
        (1, "generated_choice", true, Some(choice.as_str())),
        (2, "interpreted_unavailable", false, Some(UNAVAILABLE)),
        (3, "generated_provider_default", true, None),
    ] {
        let usage = fixture.0.join(format!("usage-{index}.ndjson"));
        let mut command = fixture.command(if emitted {
            generated.as_os_str()
        } else {
            std::ffi::OsStr::new(cli)
        });
        if !emitted {
            command.args([
                "run",
                "answer.json",
                "--output-format",
                "json",
                "--output-schema-version",
                "1",
            ]);
        }
        command
            .args([
                "--profile",
                "thinking-live",
                "--max-output-tokens",
                "8192",
                "--inference-timeout-in-sec",
                "120",
                "--max-runtime-in-sec",
                "150",
                "--max-agent-depth",
                "0",
                "--render-mode",
                "append-only",
                "--usage-log",
            ])
            .arg(&usage);
        if let Some(selected) = selected {
            command.args(["--thinking", selected]);
        } else {
            command.arg("--thinking-provider-default");
        }
        let output = invoke(command, None);
        safe_output(&output, &token);
        assert!(
            output.status.success(),
            "thinking qualification inference failed; no automatic retry"
        );
        if emitted {
            assert!(
                output.stderr.is_empty(),
                "emitted thinking qualification unexpectedly warned"
            );
        } else {
            let value = terminal(&output);
            let outcome = &value["data"]["thinking"]["records"][0]["outcome"];
            if index == 0 {
                assert!(
                    outcome["effective"]["mode"] == "choice"
                        && outcome["effective"]["value"] == choice
                        && outcome["fallback"].is_null()
                        && value["warnings"].as_array().is_some_and(Vec::is_empty),
                    "interpreted exact choice did not apply"
                );
            } else {
                assert!(
                    outcome["effective"]["mode"] == "provider_default"
                        && outcome["fallback"] == "unavailable_choice"
                        && value["warnings"][0]["code"] == "thinking.choice_unavailable",
                    "interpreted unavailable choice did not fall back"
                );
            }
        }
        one_successful_request(&usage, &provider, &model, &token);
        cases.push(json!({"case":name,"outcome":"pass","requests":1}));
    }
    let image = image_inputs.as_ref().map(|(image_model, image_choice)| {
        assert!(!image_model.contains(&token) && !image_choice.contains(&token), "image qualification input redaction failed");
        configure_profile(&fixture, "thinking-image", "gemini", image_model, &token);
        let mut image_cases = Vec::new();
        for (index, name, emitted) in [(0,"interpreted_image_choice",false),(1,"generated_image_choice",true)] {
            let path = fixture.0.join("image.jpg");
            let _ = fs::remove_file(&path);
            let usage = fixture.0.join(format!("image-usage-{index}.ndjson"));
            let mut command = fixture.command(if emitted { generated_image.as_ref().expect("missing emitted image application").as_os_str() } else { std::ffi::OsStr::new(cli) });
            if !emitted { command.args(["run", "image.json", "--output-format", "json", "--output-schema-version", "1"]); }
            command.args(["--profile", "thinking-image", "--max-output-tokens", "8192", "--inference-timeout-in-sec", "120", "--max-runtime-in-sec", "150", "--max-agent-depth", "0", "--render-mode", "append-only", "--usage-log"]).arg(&usage);
            let output = invoke(command, None);
            safe_output(&output, &token);
            assert!(output.status.success(), "image thinking qualification failed; no automatic retry");
            if emitted { assert!(output.stderr.is_empty(), "emitted image thinking unexpectedly warned"); }
            else {
                let value = terminal(&output);
                let outcome = &value["data"]["thinking"]["records"][0]["outcome"];
                assert!(outcome["effective"]["mode"] == "choice" && outcome["effective"]["value"] == *image_choice && outcome["fallback"].is_null() && value["warnings"].as_array().is_some_and(Vec::is_empty), "interpreted image choice did not apply");
            }
            one_successful_image_request(&usage, image_model, &token);
            let bytes = fs::read(path).expect("missing private image artifact");
            assert!(bytes.len() <= 20 * 1024 * 1024 && bytes.starts_with(&[0xff,0xd8]) && bytes.ends_with(&[0xff,0xd9]), "invalid private JPEG image artifact");
            image_cases.push(json!({"case":name,"outcome":"pass","requests":1}));
        }
        json!({"model":image_model,"choice":image_choice,"outcome":"pass","requests_started":2,"cases":image_cases})
    });
    let report_value = json!({"schema_version":1,"candidate":candidate,"run_id":run_id,"run_attempt":run_attempt,"probe_id":probe_id,"provider":provider,"model":model,"choice":choice,"outcome":"pass","requests_started":if image.is_some(){6}else{4},"max_output_tokens":8192,"cases":cases,"image":image});
    let encoded =
        serde_json::to_vec(&report_value).expect("sanitized thinking report encoding failed");
    assert!(
        !encoded
            .windows(token.len())
            .any(|part| part == token.as_bytes()),
        "thinking report redaction failed"
    );
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(report)
        .expect("thinking report destination must be new")
        .write_all(&encoded)
        .expect("thinking report write failed");
}
