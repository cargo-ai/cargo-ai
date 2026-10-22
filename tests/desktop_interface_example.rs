//! The shipped Desktop example uses real managed tools and no model/provider fixture.
#[allow(dead_code)]
mod support;
use base64::Engine;
use serde_json::{json, Value};
use std::{
    fs,
    io::Write,
    path::Path,
    process::{Output, Stdio},
    thread,
    time::{Duration, Instant},
};
use support::Fixture;

fn example() -> std::path::PathBuf {
    support::repository_root().join("templates/guidance/examples/desktop-interface")
}
fn invoke(f: &Fixture, project: &Path, args: &[&str], request: Option<&Value>) -> Output {
    let mut command = f.cargo_ai_command(project);
    command
        .args(["--no-update-check"])
        .args(args)
        .args(["--output-format", "json"]);
    if let Some(value) = request {
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
            .write_all(value.to_string().as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    } else {
        command.output().unwrap()
    }
}
fn success(output: Output) -> Value {
    support::assert_success(&output, "Desktop example CLI contract");
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["outcome"], "succeeded", "{value}");
    value["data"].clone()
}
fn selectors<'a>(operation: &'a str, project: &'a str) -> Vec<&'a str> {
    vec![
        "actions",
        operation,
        "--project",
        project,
        "--interface",
        "notes",
        "--action",
        "preview-note",
        "--request-stdin",
    ]
}

#[test]
fn desktop_example_scaffold_catalog_resources_and_native_tool_execute_without_provider() {
    let f = Fixture::new("desktop-example");
    let project = f.root.join("project");
    success(invoke(
        &f,
        &f.root,
        &["new", project.to_str().unwrap(), "--vcs", "none"],
        None,
    ));
    support::copy_tree(&example().join("ui"), &project.join("ui"));
    for name in ["cargo-ai-actions.json", "preview-note.json"] {
        fs::copy(example().join(name), project.join(name)).unwrap();
    }
    let source: Value =
        serde_json::from_slice(&fs::read(project.join("cargo-ai-actions.json")).unwrap()).unwrap();
    assert_eq!(source["schema_version"], 3);
    assert_eq!(source["role_registry"]["roles"], json!([]));
    assert_eq!(source["role_registry"]["call_sites"], json!([]));
    assert_eq!(
        source["role_registry"]["contexts"][0]["call_sites"],
        json!([])
    );
    let definition: Value =
        serde_json::from_slice(&fs::read(project.join("preview-note.json")).unwrap()).unwrap();
    assert_eq!(definition["agent_schema"]["properties"], json!({}));
    assert_eq!(definition["actions"][0]["run"][0]["produces_result"], true);
    let output = f
        .cargo_ai_command(&project)
        .args(["--no-update-check", "add", "tool", "desktop_preview"])
        .output()
        .unwrap();
    support::assert_success(&output, "scaffold Desktop preview tool");
    fs::copy(
        example().join("tool.rs"),
        project.join("tools/desktop_preview/src/tool.rs"),
    )
    .unwrap();
    let output = f
        .cargo_ai_command(&project)
        .env("CARGO_NET_OFFLINE", "true")
        .args(["--no-update-check", "tools", "build", "desktop_preview"])
        .output()
        .unwrap();
    support::assert_success(&output, "build real scaffolded preview tool");
    let managed = project.join(".cargo-ai/tools/desktop_preview");
    let manifest: Value =
        serde_json::from_slice(&fs::read(managed.join("tool.json")).unwrap()).unwrap();
    let artifact = manifest["artifacts"]
        .as_object()
        .unwrap()
        .values()
        .next()
        .unwrap();
    let description = f
        .command(managed.join(artifact["path"].as_str().unwrap()), &project)
        .arg("describe")
        .output()
        .unwrap();
    support::assert_success(&description, "describe the real managed tool");
    let description: Value = serde_json::from_slice(&description.stdout).unwrap();
    assert!(description["supported_protocol_versions"]
        .as_array()
        .unwrap()
        .contains(&json!(2)));
    assert!(description["resource_profile"]
        .as_object()
        .unwrap()
        .values()
        .all(|access| access == "none"));
    // Fixed author inventory must cover every scaffold source, including its generated lock.
    let listed: std::collections::BTreeSet<_> = source["role_registry"]["tool_content"]
        ["desktop_preview"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p.as_str().unwrap().to_string())
        .collect();
    let mut actual = std::collections::BTreeSet::new();
    for name in ["Cargo.toml", "Cargo.lock"] {
        actual.insert(format!("tools/desktop_preview/{name}"));
    }
    for item in fs::read_dir(project.join("tools/desktop_preview/src")).unwrap() {
        let item = item.unwrap();
        assert!(item.path().is_file());
        actual.insert(format!(
            "tools/desktop_preview/src/{}",
            item.file_name().to_str().unwrap()
        ));
    }
    assert_eq!(listed, actual);
    let path = project.to_str().unwrap();
    let catalog = success(invoke(
        &f,
        &project,
        &["actions", "list", "--project", path],
        None,
    ));
    assert_eq!(catalog["schema_version"], 3);
    for resource in source["resources"].as_array().unwrap() {
        let id = resource["id"].as_str().unwrap();
        let request = json!({"schema_version":1,"interface":"notes","resource":id,"expected_binding":catalog["binding"]});
        let response = success(invoke(
            &f,
            &project,
            &[
                "actions",
                "resource",
                "--project",
                path,
                "--interface",
                "notes",
                "--resource",
                id,
                "--request-stdin",
            ],
            Some(&request),
        ));
        assert_eq!(response["mime_type"], resource["mime_type"]);
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(response["data"].as_str().unwrap())
                .unwrap(),
            fs::read(project.join(resource["path"].as_str().unwrap())).unwrap()
        );
    }
    let limits = &source["role_registry"]["contexts"][0]["limits"];
    let mut request = json!({"schema_version":3,"interface":"notes","action":"preview-note",
        "inputs":{"text":"Hello Desktop"},"expected_binding":catalog["binding"],
        "execution_policy":{"version":1,"allowed":[],"limits":limits},"attachment_grants":{},
        "role_execution":{"mode":"default","binding_revision":{"version":1,"revision":"local-preview","bindings":[]}}});
    request["expected_binding"] = catalog["binding"].clone();
    request["inputs"]["text"] = json!("A real <note> 🧋");
    let resolved = success(invoke(
        &f,
        &project,
        &selectors("resolve", path),
        Some(&request),
    ));
    assert_eq!(resolved["ready"], true, "{resolved}");
    assert_eq!(resolved["required_selections"], json!([]));
    assert_eq!(resolved["execution_authorized"], false);
    request["role_execution"]["resolution_id"] = resolved["identity"].clone();
    success(invoke(
        &f,
        &project,
        &selectors("validate", path),
        Some(&request),
    ));
    // The test supplies only the native structural session; it opens no provider listener.
    let mut child = f
        .cargo_ai_command(&project)
        .args([
            "--no-update-check",
            "run",
            "--project",
            path,
            "--interface",
            "notes",
            "--action",
            "preview-note",
            "--action-request-stdin",
            "--role-session",
            "--include-result-content",
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
    let started = Instant::now();
    while child.try_wait().unwrap().is_none() {
        if started.elapsed() > Duration::from_secs(45) {
            child.kill().unwrap();
            panic!("tool-only Desktop action did not settle");
        }
        thread::sleep(Duration::from_millis(20));
    }
    let output = child.wait_with_output().unwrap();
    drop(input);
    support::assert_success(&output, "execute native tool-only action");
    let frames: Vec<Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let terminal = frames.last().unwrap();
    assert_eq!(terminal["data"]["outcome"], "succeeded", "{frames:?}");
    let result = &terminal["data"]["data"]["result"];
    assert_eq!(result["availability"], "available", "{terminal}");
    assert_eq!(
        result["content"],
        json!({"status":"previewed","text":"A real <note> 🧋"})
    );
    assert!(frames
        .iter()
        .all(|frame| !frame.to_string().contains("provider_request")));
    let mut invalid = request.clone();
    invalid["inputs"]["text"] = json!("x".repeat(4097));
    let failure = invoke(&f, &project, &selectors("validate", path), Some(&invalid));
    assert!(!failure.status.success());
    let config: toml::Value =
        toml::from_str(&fs::read_to_string(f.cargo_ai_home.join("config.toml")).unwrap()).unwrap();
    assert!(
        config["profile"].as_array().unwrap().is_empty(),
        "the local operation must not require a provider profile"
    );
}

#[test]
#[ignore = "run explicitly with the existing Node.js development runtime"]
fn desktop_example_consumer_preserves_drafts_and_observes_settled_operations() {
    let output = std::process::Command::new("node")
        .arg(support::repository_root().join("tests/fixtures/desktop_interface_example.mjs"))
        .arg(example().join("ui/preview.js"))
        .output()
        .unwrap();
    support::assert_success(&output, "Desktop example consumer lifecycle");
}
