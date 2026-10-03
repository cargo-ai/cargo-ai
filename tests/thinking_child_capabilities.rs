//! Passive artifact declarations and production child-invocation boundaries.

#[path = "../src/generated_capabilities.rs"]
mod generated_capabilities;
#[cfg(unix)]
#[path = "../src/generated_capabilities/record.rs"]
mod generated_capability_record;

#[cfg(unix)]
use serde_json::json;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let base = std::env::var_os("RUNNER_TEMP")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let root = base.join(format!("thinking-child-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(root.join("home")).unwrap();
        fs::create_dir_all(root.join("user-home")).unwrap();
        fs::write(
            root.join("home/config.toml"),
            "secret_store='file'\nprofile=[]\n",
        )
        .unwrap();
        Self { root }
    }

    fn command(&self, program: &Path) -> Command {
        let mut command = Command::new(program);
        command
            .current_dir(&self.root)
            .env("CARGO_AI_HOME", self.root.join("home"))
            .env("CARGO_AI_DISABLE_KEYCHAIN", "1")
            .env("CODEX_HOME", self.root.join("codex"))
            .env("HOME", self.root.join("user-home"));
        command
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn successful_json(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "command failed: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    serde_json::from_slice(&output.stdout).expect("one complete JSON document")
}

#[test]
fn actual_cli_and_copied_cli_declare_exactly_one_matching_run_capability() {
    let fixture = Fixture::new();
    let original = Path::new(env!("CARGO_BIN_EXE_cargo-ai"));
    let copied = fixture
        .root
        .join(format!("renamed-cli{}", std::env::consts::EXE_SUFFIX));
    fs::copy(original, &copied).unwrap();
    for artifact in [original, copied.as_path()] {
        // The reader rejects duplicates, so success proves one complete record.
        let passive = generated_capabilities::capabilities_for_artifact(artifact)
            .expect("actual CLI must retain exactly one complete runtime declaration");
        assert!(passive.is_cli_run());
        assert!(passive.supports_thinking());
        let declaration = serde_json::to_value(passive).unwrap();
        assert_eq!(declaration["revision"], 4);
        assert_eq!(
            declaration["structured_results"]["execution_checking"],
            true
        );
        assert_eq!(
            declaration["structured_results"]["terminal_delivery"],
            passive.is_cli_run()
        );
        assert_eq!(
            declaration["structured_results"]["artifact_read"],
            passive.is_cli_run()
        );
        let output = fixture
            .command(artifact)
            .args(["capabilities", "--output-format", "json"])
            .output()
            .unwrap();
        let response = successful_json(&output);
        assert_eq!(response["outcome"], "succeeded");
        assert_eq!(
            response["data"]["runtime_capabilities"],
            serde_json::to_value(passive).unwrap()
        );
    }
}

#[test]
#[ignore = "reuse the freshly emitted artifact from the selected generated-build proof"]
fn actual_generated_and_copied_binary_inspect_agree_with_passive_capabilities() {
    let artifact = PathBuf::from(
        std::env::var_os("CARGO_AI_TEST_GENERATED_CHILD_BINARY")
            .expect("set CARGO_AI_TEST_GENERATED_CHILD_BINARY to the freshly built artifact"),
    );
    let artifact = fs::canonicalize(artifact).unwrap();
    let fixture = Fixture::new();
    let copied = fixture
        .root
        .join(format!("renamed-child{}", std::env::consts::EXE_SUFFIX));
    fs::copy(&artifact, &copied).unwrap();
    for artifact in [artifact.as_path(), copied.as_path()] {
        let passive = generated_capabilities::capabilities_for_artifact(artifact)
            .expect("emitted binary must retain exactly one complete runtime declaration");
        assert!(!passive.is_cli_run());
        assert!(passive.supports_thinking());
        let declaration = serde_json::to_value(passive).unwrap();
        assert_eq!(declaration["revision"], 4);
        assert_eq!(
            declaration["structured_results"]["execution_checking"],
            true
        );
        assert_eq!(
            declaration["structured_results"]["terminal_delivery"],
            passive.is_cli_run()
        );
        assert_eq!(
            declaration["structured_results"]["artifact_read"],
            passive.is_cli_run()
        );
        let output = fixture
            .command(artifact)
            .args(["inspect", "--json"])
            .output()
            .unwrap();
        let inspect = successful_json(&output);
        assert_eq!(
            inspect["runtime_capabilities"],
            serde_json::to_value(passive).unwrap()
        );
    }
}

#[cfg(unix)]
#[derive(Clone, Copy, Debug)]
enum Declaration {
    Opaque,
    Generated,
    CliRun,
    LegacyGenerated,
    Conflicting,
}

#[cfg(unix)]
fn argument_capture_child(fixture: &Fixture, artifact: &Path, declaration: Declaration) {
    use std::os::unix::fs::PermissionsExt;
    let mut script = format!(
        "#!/bin/sh\nprintf 'executed\\n' >> '{}'\nprintf '%s\\n' \"$@\" > '{}'\nprintf '{{\"status\":\"child\"}}\\n'\nexit 0\n",
        fixture.root.join("executions").display(),
        fixture.root.join("arguments").display(),
    )
    .into_bytes();
    match declaration {
        Declaration::Opaque => {}
        Declaration::Generated => script.extend(generated_capability_record::encoded_record(false)),
        Declaration::CliRun => script.extend(generated_capability_record::encoded_record(true)),
        Declaration::LegacyGenerated => {
            let mut record = generated_capability_record::encoded_record(false);
            let offset = generated_capability_record::OPEN.len()
                + generated_capability_record::IDENTITY.len();
            record[offset..offset + 4].copy_from_slice(&1u32.to_le_bytes());
            record[offset + 4..offset + 8].copy_from_slice(&0b1111u32.to_le_bytes());
            script.extend(record);
        }
        Declaration::Conflicting => {
            script.extend(generated_capability_record::encoded_record(false));
            script.extend(generated_capability_record::encoded_record(true));
        }
    }
    fs::write(artifact, script).unwrap();
    fs::set_permissions(artifact, fs::Permissions::from_mode(0o755)).unwrap();
}

#[cfg(unix)]
fn assert_child_case(
    json_child: bool,
    declaration: Declaration,
    selection: Option<Value>,
    expected_flags: &[&str],
    forwarded: Option<bool>,
) {
    let fixture = Fixture::new();
    let bin = fixture.root.join("bin");
    fs::create_dir(&bin).unwrap();
    let artifact = if json_child {
        fs::write(fixture.root.join("child.json"), "{}").unwrap();
        bin.join("cargo-ai")
    } else {
        fixture.root.join("child")
    };
    argument_capture_child(&fixture, &artifact, declaration);
    let output_status = if selection
        .as_ref()
        .is_some_and(|setting| setting["value"]["var"] == "status")
    {
        "exact-choice-".repeat(40)
    } else {
        "ok".into()
    };
    let mut step =
        json!({"kind":"agent","artifact":if json_child {"./child.json"} else {"./child"}});
    if let Some(selection) = selection {
        step["thinking"] = selection;
    }
    let definition = json!({
        "agent_definition_schema_version":"2026-10-01.r1",
        "inputs":[{"type":"text","text":"fixture"}],
        "agent_schema":{"type":"object","properties":{"status":{"type":"string"}}},
        "actions":[
            {"name":"skip","logic":{"==":[1,2]},"run":[{"kind":"agent","artifact":"./child.json"}]},
            {"name":"invoke","logic":{"==":[1,1]},"run":[step]}
        ]
    });
    fs::write(fixture.root.join("parent.json"), definition.to_string()).unwrap();
    let mut server = mockito::Server::new();
    let metadata = server
        .mock("POST", "/api/show")
        .expect(1)
        .with_body(r#"{"model":"fixture","thinking":{"values":["low","high"]}}"#)
        .create();
    let inference = server
        .mock("POST", "/v1/chat/completions")
        .expect(1)
        .match_request(|request| {
            let body: Value = serde_json::from_slice(request.body().unwrap()).unwrap();
            body["reasoning_effort"] == "high"
        })
        .with_body(json!({"choices":[{"message":{"content":json!({"status":output_status}).to_string()}}]}).to_string())
        .create();
    let output = fixture
        .command(Path::new(env!("CARGO_BIN_EXE_cargo-ai")))
        // A controlled PATH selects exactly this CLI for JSON children, without
        // introducing a cargo wrapper or consulting the user's installed CLI.
        .env("PATH", &bin)
        .args([
            "run",
            "parent.json",
            "--server",
            "ollama",
            "--model",
            "fixture",
            "--url",
            &format!("{}/v1/chat/completions", server.url()),
            "--thinking",
            "high",
            "--output-format",
            "json",
        ])
        .output()
        .unwrap();
    let terminal = successful_json(&output);
    assert_eq!(terminal["outcome"], "succeeded");
    metadata.assert();
    inference.assert();
    assert_eq!(
        fs::read_to_string(fixture.root.join("executions")).unwrap(),
        "executed\n",
        "child must execute once, with no executable capability probe"
    );
    let arguments = fs::read_to_string(fixture.root.join("arguments")).unwrap();
    let arguments: Vec<&str> = arguments.lines().collect();
    assert!(!arguments.contains(&"inspect"));
    assert!(!arguments.contains(&"capabilities"));
    if json_child {
        assert_eq!(&arguments[..2], ["run", "./child.json"]);
    }
    let thinking_flags: Vec<&str> = arguments
        .iter()
        .enumerate()
        .filter_map(|(index, value)| {
            (value.starts_with("--thinking")
                || index > 0 && matches!(arguments[index - 1], "--thinking" | "--thinking-choice"))
            .then_some(*value)
        })
        .collect();
    assert_eq!(thinking_flags, expected_flags, "{declaration:?}");
    let records = terminal["data"]["thinking"]["records"].as_array().unwrap();
    let forwarding: Vec<&Value> = records
        .iter()
        .filter(|record| record["kind"] == "child_forwarding")
        .collect();
    match forwarded {
        None => assert!(
            forwarding.is_empty(),
            "omission must preserve child defaults"
        ),
        Some(forwarded) => {
            assert_eq!(forwarding.len(), 1);
            assert_eq!(forwarding[0]["scope"]["action_index"], 1);
            assert_eq!(forwarding[0]["scope"]["step_index"], 0);
            assert_eq!(
                forwarding[0]["disposition"],
                if forwarded {
                    "forwarded"
                } else {
                    "not_forwarded"
                }
            );
            if matches!(declaration, Declaration::LegacyGenerated) {
                assert_eq!(forwarding[0]["declaration_source"], "embedded_artifact");
            }
            assert_eq!(forwarding[0]["effective"], "child_unverified");
            assert_eq!(
                terminal["warnings"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|warning| warning["code"] == "thinking.child_capability_unavailable"),
                !forwarded
            );
        }
    }
}

#[cfg(unix)]
#[test]
fn compiled_children_forward_only_explicit_settings_to_generated_runtime_declarations() {
    for declaration in [
        Declaration::Opaque,
        Declaration::CliRun,
        Declaration::Conflicting,
    ] {
        assert_child_case(
            false,
            declaration,
            Some(json!({"mode":"choice","value":"Ultra"})),
            &[],
            Some(false),
        );
        assert_child_case(
            false,
            declaration,
            Some(json!({"mode":"provider_default"})),
            &[],
            Some(false),
        );
    }
    assert_child_case(
        false,
        Declaration::Generated,
        Some(json!({"mode":"choice","value":"Ultra"})),
        &["--thinking-choice", "Ultra"],
        Some(true),
    );
    assert_child_case(
        false,
        Declaration::Generated,
        Some(json!({"mode":"provider_default"})),
        &["--thinking-provider-default"],
        Some(true),
    );
    assert_child_case(false, Declaration::Generated, None, &[], None);
    let choice = "exact-choice-".repeat(40);
    assert_child_case(
        false,
        Declaration::Generated,
        Some(json!({"mode":"choice","value":choice})),
        &["--thinking-choice", &choice],
        Some(true),
    );
}

#[cfg(unix)]
#[test]
fn json_children_forward_only_explicit_settings_to_selected_cli_run_declarations() {
    for declaration in [
        Declaration::Opaque,
        Declaration::Generated,
        Declaration::Conflicting,
    ] {
        assert_child_case(
            true,
            declaration,
            Some(json!({"mode":"choice","value":"Ultra"})),
            &[],
            Some(false),
        );
        assert_child_case(
            true,
            declaration,
            Some(json!({"mode":"provider_default"})),
            &[],
            Some(false),
        );
    }
    assert_child_case(
        true,
        Declaration::CliRun,
        Some(json!({"mode":"choice","value":"Ultra"})),
        &["--thinking-choice", "Ultra"],
        Some(true),
    );
    assert_child_case(
        true,
        Declaration::CliRun,
        Some(json!({"mode":"provider_default"})),
        &["--thinking-provider-default"],
        Some(true),
    );
    assert_child_case(true, Declaration::CliRun, None, &[], None);
    let choice = "exact-choice-".repeat(40);
    assert_child_case(
        true,
        Declaration::CliRun,
        Some(json!({"mode":"choice","value":{"var":"status"}})),
        &["--thinking-choice", &choice],
        Some(true),
    );
    assert_child_case(
        true,
        Declaration::CliRun,
        Some(json!({"mode":"choice","value":choice})),
        &["--thinking-choice", &choice],
        Some(true),
    );
}

#[cfg(unix)]
#[test]
fn boolean_and_literal_choice_forwarding_respect_old_runtime_boundaries() {
    for json_child in [false, true] {
        let declaration = if json_child {
            Declaration::CliRun
        } else {
            Declaration::Generated
        };
        for mode in ["on", "off"] {
            assert_child_case(
                json_child,
                declaration,
                Some(json!({"mode":mode})),
                &["--thinking", mode],
                Some(true),
            );
        }
        assert_child_case(
            json_child,
            declaration,
            Some(json!({"mode":"choice","value":"on"})),
            &["--thinking-choice", "on"],
            Some(true),
        );
    }
    for mode in ["on", "off"] {
        assert_child_case(
            false,
            Declaration::LegacyGenerated,
            Some(json!({"mode":mode})),
            &[],
            Some(false),
        );
    }
    assert_child_case(
        false,
        Declaration::LegacyGenerated,
        Some(json!({"mode":"choice","value":"on"})),
        &["--thinking", "on"],
        Some(true),
    );
    assert_child_case(
        false,
        Declaration::LegacyGenerated,
        Some(json!({"mode":"provider_default"})),
        &["--thinking-provider-default"],
        Some(true),
    );
}
