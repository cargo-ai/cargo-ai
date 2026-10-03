//! Portable action requests exercise the real CLI with isolated state.
#[allow(dead_code)]
mod support;
use serde_json::{json, Value};
use std::{
    fs,
    io::Write,
    process::{Output, Stdio},
};
use support::Fixture;

fn fixture() -> Fixture {
    let f = Fixture::new("client-actions");
    let examples = support::repository_root().join("templates/guidance/examples");
    for name in [
        "client-action-coordinator.json",
        "client-action-review.json",
        "client-action-controls.js",
    ] {
        fs::copy(examples.join(name), f.root.join(name)).unwrap();
    }
    fs::copy(
        examples.join("client-actions.json"),
        f.root.join("cargo-ai-actions.json"),
    )
    .unwrap();
    fs::create_dir_all(f.root.join(".cargo-ai")).unwrap();
    fs::write(
        f.root.join(".cargo-ai/project.toml"),
        "[project]\nname='action-fixture'\n",
    )
    .unwrap();
    f
}

fn execute(f: &Fixture, args: &[&str], request: Option<&Value>) -> Output {
    let mut command = f.cargo_ai_command(&f.root);
    command.args(args).args(["--output-format", "json"]);
    if let Some(request) = request {
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
    }
}
fn envelope(output: Output) -> Value {
    assert!(
        output.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(response["schema_version"], 1);
    assert_eq!(response["completion"]["terminal"], true);
    assert_eq!(
        output.status.success(),
        response["outcome"] == "succeeded",
        "{response}"
    );
    response
}
fn catalog(f: &Fixture) -> Value {
    let response = envelope(execute(
        f,
        &["actions", "list", "--project", f.root.to_str().unwrap()],
        None,
    ));
    assert_eq!(response["outcome"], "succeeded", "{response}");
    response["data"].clone()
}
fn request(binding: &Value, action: &str, ids: &[&str]) -> Value {
    let mut value: Value = serde_json::from_str(include_str!(
        "../templates/guidance/examples/client-action-request.json"
    ))
    .unwrap();
    value["expected_binding"] = binding.clone();
    value["action"] = json!(action);
    value["inputs"]["panel_ids"] = json!(ids);
    value
}
fn validate(f: &Fixture, request: &Value) -> Value {
    envelope(execute(
        f,
        &[
            "actions",
            "validate",
            "--project",
            f.root.to_str().unwrap(),
            "--interface",
            "panels",
            "--action",
            request["action"].as_str().unwrap(),
            "--request-stdin",
        ],
        Some(request),
    ))
}
fn configure(f: &Fixture) {
    fs::write(f.cargo_ai_home.join("config.toml"), "secret_store='file'\ndefault_profile='fixture'\n[[profile]]\nname='fixture'\nserver='ollama'\nmodel='fixture-model'\nauth_mode='none'\n").unwrap();
}

#[test]
fn inherited_policy_rejects_before_plain_cli_startup_writes() {
    let f = fixture();
    let config = "default_profile='fixture'\n[[profile]]\nname='fixture'\nserver='ollama'\nmodel='fixture-model'\ntoken='synthetic-migration-fixture'\n";
    fs::write(f.cargo_ai_home.join("config.toml"), config).unwrap();
    for encoded in [None, Some(""), Some("{invalid")] {
        for machine in [false, true] {
            let mut command = f.cargo_ai_command(&f.root);
            command
                .args(["run", "--config", "missing-definition.json"])
                .env("CARGO_AI_EXECUTION_POLICY_REQUIRED_V1", "1")
                .env_remove("CARGO_AI_EXECUTION_POLICY_V1");
            if let Some(encoded) = encoded {
                command.env("CARGO_AI_EXECUTION_POLICY_V1", encoded);
            }
            if machine {
                command.args(["--output-format", "json"]);
            }
            let output = command.output().unwrap();
            assert!(!output.status.success());
            if machine {
                let value: Value = serde_json::from_slice(&output.stdout).unwrap();
                assert_eq!(value["error"]["code"], "action.execution_policy_invalid");
            } else {
                assert!(String::from_utf8_lossy(&output.stderr)
                    .contains("action.execution_policy_invalid"));
            }
            assert_eq!(
                fs::read_to_string(f.cargo_ai_home.join("config.toml")).unwrap(),
                config
            );
            assert_eq!(fs::read_dir(&f.cargo_ai_home).unwrap().count(), 1);
            assert_eq!(fs::read_dir(&f.fallback_home).unwrap().count(), 0);
        }
    }
}

fn routing_markers(f: &Fixture) {
    for (file, marker) in [
        ("client-action-coordinator.json", "generate.marker"),
        ("client-action-review.json", "review.marker"),
    ] {
        let path = f.root.join(file);
        let mut definition: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        definition["actions"] = json!([{"name":"observe_input","logic":{"==":[1,1]},"run":[
            {"kind":"exec","program":"sh","args":["-c",format!("printf '%s' \"$1\" > {marker}"),"fixture",{"var":"runtime.panel_ids_json"}],"platform":"macos"},
            {"kind":"exec","program":"sh","args":["-c",format!("printf '%s' \"$1\" > {marker}"),"fixture",{"var":"runtime.panel_ids_json"}],"platform":"linux"},
            {"kind":"exec","program":"cmd","args":["/C","echo",{"var":"runtime.panel_ids_json"},">",marker],"platform":"windows"}
        ]}]);
        fs::write(path, definition.to_string()).unwrap();
    }
}

#[test]
#[ignore = "run explicitly with the existing Node.js development runtime"]
fn browser_adapter_requests_execute_through_the_real_cli() {
    let f = fixture();
    configure(&f);
    routing_markers(&f);
    let output = std::process::Command::new("node")
        .arg(support::repository_root().join("tests/fixtures/client_action_controls.mjs"))
        .arg(f.root.join("client-action-controls.js"))
        .output()
        .unwrap();
    support::assert_success(&output, "browser adapter fixture");
    let business: Vec<Value> = serde_json::from_slice(&output.stdout).unwrap();
    let discovered = catalog(&f);
    for item in business {
        let action = item["action"].as_str().unwrap();
        let mut value = request(&discovered["binding"], action, &["placeholder"]);
        value["inputs"] = item["inputs"].clone();
        let result = envelope(execute(
            &f,
            &[
                "run",
                "--project",
                f.root.to_str().unwrap(),
                "--interface",
                "panels",
                "--action",
                action,
                "--action-request-stdin",
                "--profile",
                "fixture",
            ],
            Some(&value),
        ));
        assert_eq!(result["outcome"], "succeeded", "{result}");
        let marker = if action == "review" {
            "review.marker"
        } else {
            "generate.marker"
        };
        assert!(fs::read_to_string(f.root.join(marker))
            .unwrap()
            .contains("p1"));
    }
}

#[test]
fn passive_discovery_resources_and_validation_need_no_profile_or_browser() {
    let f = fixture();
    let initial_home = fs::read_dir(&f.cargo_ai_home).unwrap().count();
    let discovered = catalog(&f);
    assert_eq!(discovered["actions"].as_array().unwrap().len(), 3);
    for (action, ids) in [
        ("generate-one", vec!["p1"]),
        ("generate-selected", vec!["p1", "p2"]),
        ("review", vec!["p2"]),
    ] {
        let result = validate(&f, &request(&discovered["binding"], action, &ids));
        assert_eq!(result["data"]["valid"], true, "{result}");
        assert_eq!(result["data"]["execution_authorized"], false);
        assert_eq!(
            result["data"]["mappings"]["runtime_vars"],
            json!(["panel_ids_json"])
        );
    }
    let resource = json!({"schema_version":1,"interface":"panels","resource":"controls","expected_binding":discovered["binding"]});
    let result = envelope(execute(
        &f,
        &[
            "actions",
            "resource",
            "--project",
            f.root.to_str().unwrap(),
            "--interface",
            "panels",
            "--resource",
            "controls",
            "--request-stdin",
        ],
        Some(&resource),
    ));
    assert_eq!(result["outcome"], "succeeded", "{result}");
    assert_eq!(
        fs::read_dir(&f.cargo_ai_home).unwrap().count(),
        initial_home
    );
    assert!(!f.cargo_ai_home.join("config.toml").exists());
}

#[test]
fn stale_script_and_invalid_business_inputs_fail_with_typed_private_safe_errors() {
    let f = fixture();
    let discovered = catalog(&f);
    let mut invalid = request(&discovered["binding"], "generate-one", &["p1"]);
    invalid["inputs"]["token"] = json!("synthetic-private-input");
    let denied = validate(&f, &invalid);
    assert_eq!(denied["error"]["code"], "action.invalid_inputs");
    assert!(!denied.to_string().contains("synthetic-private-input"));
    fs::write(
        f.root.join("client-action-controls.js"),
        "// changed presentation\n",
    )
    .unwrap();
    let stale = validate(
        &f,
        &request(&discovered["binding"], "generate-one", &["p1"]),
    );
    assert_eq!(stale["error"]["code"], "action.stale_binding");
    let fresh = catalog(&f);
    assert_ne!(fresh["binding"], discovered["binding"]);
    assert_eq!(
        validate(&f, &request(&fresh["binding"], "generate-one", &["p1"]))["outcome"],
        "succeeded"
    );
}

#[test]
fn web_native_and_serialized_headless_requests_share_routing_and_terminal_association() {
    let f = fixture();
    configure(&f);
    routing_markers(&f);
    let discovered = catalog(&f);
    // These business vectors match the thin web adapter and a native client's selections.
    // Persisting and reading the complete third request models page-free invocation only.
    for (index, action) in ["generate-one", "review", "generate-selected"]
        .into_iter()
        .enumerate()
    {
        let panel = format!("panel-private-{index}");
        let value = request(&discovered["binding"], action, &[&panel]);
        let saved = f.root.join("scheduled-request.json");
        fs::write(&saved, value.to_string()).unwrap();
        let serialized: Value = serde_json::from_slice(&fs::read(&saved).unwrap()).unwrap();
        let result = envelope(execute(
            &f,
            &[
                "run",
                "--project",
                f.root.to_str().unwrap(),
                "--interface",
                "panels",
                "--action",
                action,
                "--action-request-stdin",
                "--profile",
                "fixture",
            ],
            Some(&serialized),
        ));
        assert_eq!(result["outcome"], "succeeded", "{result}");
        assert!(!result["request_id"].as_str().unwrap().is_empty());
        assert_eq!(
            result["data"]["client_action"],
            json!({"interface":"panels","action":action,"binding":discovered["binding"]})
        );
        assert!(!result.to_string().contains(&panel));
        let marker = if action == "review" {
            "review.marker"
        } else {
            "generate.marker"
        };
        assert!(
            fs::read_to_string(f.root.join(marker))
                .unwrap()
                .contains(&panel),
            "target and mapped input must match the selected action"
        );
        let other = if action == "review" {
            "generate.marker"
        } else {
            "review.marker"
        };
        if let Ok(value) = fs::read_to_string(f.root.join(other)) {
            assert!(!value.contains(&panel));
        }
    }
}

#[test]
fn denied_effective_profile_model_is_correlated_and_creates_no_session() {
    let f = fixture();
    configure(&f);
    let discovered = catalog(&f);
    let value = request(&discovered["binding"], "generate-one", &["p1"]);
    let before = fs::read_dir(&f.cargo_ai_home)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect::<Vec<_>>();
    let result = envelope(execute(
        &f,
        &[
            "run",
            "--project",
            f.root.to_str().unwrap(),
            "--interface",
            "panels",
            "--action",
            "generate-one",
            "--action-request-stdin",
            "--profile",
            "fixture",
            "--model",
            "not-granted",
        ],
        Some(&value),
    ));
    assert_eq!(
        result["error"]["code"], "action.execution_selection_denied",
        "{result}"
    );
    assert_eq!(result["data"]["client_action"]["action"], "generate-one");
    assert_eq!(
        fs::read_dir(&f.cargo_ai_home)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect::<Vec<_>>(),
        before
    );
}

#[cfg(unix)]
#[test]
fn action_cancellation_retains_binding_and_existing_owned_child_cleanup() {
    use std::time::{Duration, Instant};
    let f = fixture();
    configure(&f);
    let definition = f.root.join("client-action-coordinator.json");
    let mut content: Value = serde_json::from_slice(&fs::read(&definition).unwrap()).unwrap();
    content["actions"] = json!([{"name":"wait","logic":{"==":[1,1]},"run":[{"kind":"exec","program":"sh","args":["-c","echo $$ > action.pid; sleep 20; echo late > action.late"]}]}]);
    fs::write(&definition, content.to_string()).unwrap();
    let discovered = catalog(&f);
    let value = request(&discovered["binding"], "generate-one", &["p1"]);
    let mut child = f
        .cargo_ai_command(&f.root)
        .args([
            "run",
            "--project",
            f.root.to_str().unwrap(),
            "--interface",
            "panels",
            "--action",
            "generate-one",
            "--action-request-stdin",
            "--profile",
            "fixture",
            "--output-format",
            "ndjson",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(value.to_string().as_bytes())
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(8);
    while !f.root.join("action.pid").exists() {
        if child.try_wait().unwrap().is_some() {
            panic!(
                "action exited before child launch: {}",
                support::output_text(&child.wait_with_output().unwrap())
            );
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("action child did not start");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(std::process::Command::new("/bin/kill")
        .args(["-INT", &child.id().to_string()])
        .status()
        .unwrap()
        .success());
    let deadline = Instant::now() + Duration::from_secs(6);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("action cancellation did not settle");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(130));
    assert!(output.stderr.is_empty());
    let events: Vec<Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let resolved = events
        .iter()
        .position(|event| event["event_type"] == "client_action_resolved")
        .unwrap();
    for event in &events[resolved..] {
        assert_eq!(event["client_action"]["binding"], discovered["binding"]);
    }
    let terminal = events.last().unwrap();
    assert_eq!(terminal["data"]["outcome"], "canceled");
    assert_eq!(terminal["data"]["data"]["owned_child_cleanup"], "completed");
    assert!(!f.root.join("action.late").exists());
}
