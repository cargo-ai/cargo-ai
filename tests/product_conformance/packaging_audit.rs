use super::{assert_success, data_cli, output_text, Fixture};
use serde_json::json;
use std::fs;

#[test]
fn explicit_packaging_skip_preserves_structure_compilation_and_runtime_audit() {
    let fixture = Fixture::new("packaging-audit");
    let project = fixture.root.join("project");
    fs::create_dir_all(project.join(".cargo-ai/tools/probe")).unwrap();
    fs::create_dir_all(project.join("tools/probe/src")).unwrap();
    fs::write(project.join(".cargo-ai/project.toml"), "format_version=1\n[tools]\nallow_global_fallback=false\n[build.default]\nagent_definitions=['agent.json']\ntools=['probe']\n").unwrap();
    fs::write(project.join(".cargo-ai/tools/probe/tool.json"), json!({"schema_version":1,"tool_id":"probe","source":{"manifest_path":"tools/probe/Cargo.toml"},"binary":{"default_name":"probe"},"artifacts":{}}).to_string()).unwrap();
    fs::write(
        project.join("tools/probe/Cargo.toml"),
        "[package]\nname='probe'\nversion='0.1.0'\nedition='2021'\n[workspace]\n",
    )
    .unwrap();
    let describe = json!({"protocol_version":1,"name":"probe","description":"Local audit fixture",
        "params":{"required":{"type":"string","required":true}},
        "result":{"type":"string","nullable":true},
        "resource_profile":{"network":"none","filesystem_read":"none","filesystem_write":"none","subprocess":"none","env_read":"none","credential_access":"none"},
        "self_test":{"supported":false,"safe":false},
        "examples":{"minimal_invoke":{"protocol_version":1,"params":{"required":"value"}},"full_invoke":{"protocol_version":1,"params":{"required":"value"}}}});
    let tool_source = format!("fn main() {{ assert_eq!(std::env::args().nth(1).as_deref(), Some(\"describe\")); std::fs::write(\"describe.called\", \"called\").unwrap(); println!(\"{{}}\", {text:?}); }}", text=describe.to_string());
    fs::write(project.join("tools/probe/src/main.rs"), &tool_source).unwrap();
    let definition = json!({"agent_definition_schema_version":"2026-03-03.r1",
        "inputs":[{"type":"text","name":"job","text":"local fixture"}],
        "agent_schema":{"type":"object","properties":{}},
        "actions":[{"name":"check","logic":{"==":[1,1]},"run":[{"kind":"tool","name":"probe","params":{}}]}]});
    fs::write(project.join("agent.json"), definition.to_string()).unwrap();
    let target = if cfg!(target_os = "macos") {
        if cfg!(target_arch = "aarch64") {
            "aarch64-apple-darwin"
        } else {
            "x86_64-apple-darwin"
        }
    } else if cfg!(all(target_os = "windows", target_env = "gnu")) {
        "x86_64-pc-windows-gnu"
    } else if cfg!(target_os = "windows") {
        "x86_64-pc-windows-msvc"
    } else if cfg!(all(target_arch = "aarch64", target_env = "musl")) {
        "aarch64-unknown-linux-musl"
    } else if cfg!(target_arch = "aarch64") {
        "aarch64-unknown-linux-gnu"
    } else if cfg!(target_env = "musl") {
        "x86_64-unknown-linux-musl"
    } else {
        "x86_64-unknown-linux-gnu"
    };
    let output = project.join("assembled");
    let build = [
        "build",
        "--target",
        target,
        "--output-dir",
        output.to_str().unwrap(),
        "--force",
    ];
    let rejected = data_cli(&fixture, &project, &build);
    assert!(!rejected.status.success(), "{}", output_text(&rejected));
    assert!(
        output_text(&rejected).contains("missing required tool param"),
        "{}",
        output_text(&rejected)
    );
    assert!(project.join("describe.called").is_file());
    fs::remove_file(project.join("describe.called")).unwrap();
    let mut skipped = build.to_vec();
    skipped.push("--ignore-tools");
    let result = data_cli(&fixture, &project, &skipped);
    assert_success(&result, "explicit packaging audit skip");
    assert!(output_text(&result).contains("Tool audit: skipped"));
    assert!(!project.join("describe.called").exists());
    let manifest: toml::Value =
        toml::from_str(&fs::read_to_string(output.join("cargo-ai-build.toml")).unwrap()).unwrap();
    assert_eq!(manifest["tool_audit"].as_str(), Some("skipped"));
    let managed: serde_json::Value =
        serde_json::from_slice(&fs::read(output.join(".cargo-ai/tools/probe/tool.json")).unwrap())
            .unwrap();
    let artifact = output
        .join(".cargo-ai/tools/probe")
        .join(managed["artifacts"][target]["path"].as_str().unwrap());
    assert!(artifact.is_file());
    assert_eq!(
        fs::read(output.join("agent.json")).unwrap(),
        fs::read(project.join("agent.json")).unwrap()
    );
    let runtime = data_cli(
        &fixture,
        &output,
        &[
            "run",
            "--config",
            "agent.json",
            "--server",
            "ollama",
            "--model",
            "fixture",
            "--url",
            "http://127.0.0.1:9",
        ],
    );
    assert!(!runtime.status.success(), "{}", output_text(&runtime));
    assert!(
        output_text(&runtime).contains("missing required tool param"),
        "{}",
        output_text(&runtime)
    );
    assert!(output.join("describe.called").is_file());

    let mut valid = definition.clone();
    valid["actions"][0]["run"][0]["params"]["required"] = json!("present");
    fs::write(project.join("agent.json"), valid.to_string()).unwrap();
    assert_success(
        &data_cli(&fixture, &project, &build),
        "default native audit",
    );
    let audited: toml::Value =
        toml::from_str(&fs::read_to_string(output.join("cargo-ai-build.toml")).unwrap()).unwrap();
    assert!(audited.get("tool_audit").is_none());
    fs::write(project.join("agent.json"), definition.to_string()).unwrap();

    let hatch = data_cli(
        &fixture,
        &project,
        &[
            "hatch",
            "checked",
            "--config",
            "agent.json",
            "--target",
            target,
            "--check",
        ],
    );
    assert!(!hatch.status.success(), "{}", output_text(&hatch));
    assert!(output_text(&hatch).contains("missing required tool param"));
    fs::write(project.join("agent.json"), "{}").unwrap();
    for args in [
        &skipped[..],
        &[
            "hatch",
            "checked",
            "--config",
            "agent.json",
            "--ignore-tools",
            "--check",
        ],
    ] {
        let invalid = data_cli(&fixture, &project, args);
        assert!(!invalid.status.success(), "{}", output_text(&invalid));
        assert!(
            output_text(&invalid).contains("agent_definition_schema_version"),
            "{}",
            output_text(&invalid)
        );
    }
    fs::remove_file(project.join("agent.json")).unwrap();
    let missing = data_cli(&fixture, &project, &skipped);
    assert!(!missing.status.success(), "{}", output_text(&missing));
    assert!(output_text(&missing).contains("agent.json"));
    fs::write(project.join("agent.json"), definition.to_string()).unwrap();
    fs::write(
        project.join("tools/probe/src/main.rs"),
        "compile_error!(\"compilation must still fail\");",
    )
    .unwrap();
    let compilation = data_cli(&fixture, &project, &skipped);
    assert!(
        !compilation.status.success(),
        "{}",
        output_text(&compilation)
    );
    assert!(output_text(&compilation).contains("compilation must still fail"));
}

#[test]
fn packaging_skip_does_not_advertise_an_unsupported_machine_leaf() {
    let fixture = Fixture::new("packaging-machine");
    for command in ["build", "hatch"] {
        let output = fixture
            .cargo_ai_command(&fixture.root)
            .args([
                "--no-update-check",
                "--output-format",
                "json",
                command,
                "fixture",
                "--ignore-tools",
            ])
            .output()
            .unwrap();
        assert!(!output.status.success());
        let response: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(response["outcome"], "failed");
        assert_eq!(response["error"]["code"], "cli.unsupported_contract");
        assert!(!output_text(&output).contains("Tool audit: skipped"));
    }
}
