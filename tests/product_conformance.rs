//! Process-level coverage for the public Cargo AI CLI spine.

mod support;
#[path = "product_conformance/usage_attribution.rs"]
mod usage_attribution;

use base64::Engine as _;
use serde_json::{json, Value};
use std::fs;
use std::io::Write;
use std::process::Stdio;

use support::{
    assert_success, copy_tree, openai_success_response, output_text, repository_root, Fixture,
    OneShotHttpServer,
};

#[test]
fn version_scaffold_guidance_and_invalid_definition_are_process_safe() {
    let fixture = Fixture::new("product");
    let version = fixture
        .cargo_ai_command(&fixture.root)
        .args(["--no-update-check", "version"])
        .output()
        .expect("version command should start");
    assert_success(&version, "version command");
    assert!(!String::from_utf8_lossy(&version.stdout).trim().is_empty());

    let project = fixture.root.join("project");
    fs::create_dir_all(&project).expect("project root should be created");
    let user_agents = project.join("AGENTS.md");
    fs::write(&user_agents, "user-owned guidance\n").expect("user guidance should be written");
    let init = fixture
        .cargo_ai_command(&fixture.root)
        .args(["--no-update-check", "init"])
        .arg(&project)
        .args(["--vcs", "none"])
        .output()
        .expect("init command should start");
    assert_success(&init, "init command");
    let guidance = fixture
        .cargo_ai_command(&project)
        .args(["--no-update-check", "add", "guidance", "--style", "codex"])
        .output()
        .expect("guidance command should start");
    assert_success(&guidance, "guidance command");
    assert_eq!(
        fs::read_to_string(&user_agents).expect("user guidance should remain readable"),
        "user-owned guidance\n"
    );
    assert!(project.join(".cargo-ai/guidance/start-here.md").is_file());

    let invalid_definition =
        repository_root().join("tests/fixtures/product_conformance/invalid_definition.json");
    let invalid = fixture
        .cargo_ai_command(&fixture.root)
        .args(["--no-update-check", "run", "--config"])
        .arg(invalid_definition)
        .output()
        .expect("invalid run should start");
    assert!(!invalid.status.success());
    assert!(output_text(&invalid).contains("agent_definition_schema_version"));
}

#[test]
fn real_cli_package_lifecycle_is_isolated_and_fail_closed() {
    let fixture = Fixture::new("lifecycle");
    // Resolve Unix temporary-path aliases before configuring no-follow ownership
    // roots. Keep ordinary Windows paths compatible with downstream Cargo/MSVC.
    #[cfg(unix)]
    let fixture = {
        let mut fixture = fixture;
        fixture.root = fs::canonicalize(&fixture.root).unwrap();
        fixture.cargo_ai_home = fs::canonicalize(&fixture.cargo_ai_home).unwrap();
        fixture.fallback_home = fs::canonicalize(&fixture.fallback_home).unwrap();
        fixture
    };
    let fallback_sentinel = fixture.fallback_home.join("sentinel.txt");
    fs::write(&fallback_sentinel, "unchanged").expect("fallback sentinel should be written");
    let project = fixture.root.join("source");
    copy_tree(
        &repository_root().join("tests/fixtures/package_lifecycle"),
        &project,
    );
    prepare_lifecycle_artifact_contract(&fixture, &project);
    let package_root = fixture.root.join("package");

    let package = fixture
        .cargo_ai_command(&project)
        .args(["--no-update-check", "package", "default", "--output-dir"])
        .arg(&package_root)
        .arg("--force")
        .output()
        .expect("package command should start");
    assert_success(&package, "package command");
    assert!(package_root.join("cargo-ai-package.toml").is_file());

    let install = fixture
        .cargo_ai_command(&project)
        .args(["--no-update-check", "packages", "install"])
        .arg(&package_root)
        .args(["--as", "lifecycle"])
        .output()
        .expect("package install should start");
    assert_success(&install, "package install");

    let inspect = fixture
        .cargo_ai_command(&project)
        .args(["--no-update-check", "packages", "inspect", "lifecycle"])
        .output()
        .expect("package inspect should start");
    assert_success(&inspect, "package inspect");
    let inspect_text = output_text(&inspect);
    assert!(inspect_text.contains("Identity:  lifecycle_fixture"));
    assert!(inspect_text.contains("qualification_smoke"));

    let hatch = fixture
        .cargo_ai_command(&project)
        .args([
            "--no-update-check",
            "hatch",
            "lifecycle::qualification_smoke",
            "--check",
        ])
        .output()
        .expect("installed hatch check should start");
    assert_success(&hatch, "installed hatch check");

    let server = OneShotHttpServer::json("/v1/chat/completions", openai_success_response("ready"));
    let run = fixture
        .cargo_ai_command(&project)
        .args([
            "--no-update-check",
            "run",
            "lifecycle::qualification_smoke",
            "--server",
            "openai",
            "--model",
            "qualification-model",
            "--url",
            &server.url,
            "--token",
            "fixture-token",
            "--render-mode",
            "append-only",
        ])
        .output()
        .expect("installed run should start");
    let request = server.finish();
    assert_success(&run, "installed run");
    assert!(request.starts_with("POST /v1/chat/completions HTTP/1.1"));

    let reinstall = fixture
        .cargo_ai_command(&project)
        .args(["--no-update-check", "packages", "install"])
        .arg(&package_root)
        .args(["--as", "lifecycle"])
        .output()
        .expect("package reinstall should start");
    assert_success(&reinstall, "package reinstall");

    fs::write(fixture.cargo_ai_home.join("config.toml"), "secret_store='file'\ndefault_profile='fixture'\n[[profile]]\nname='fixture'\nserver='ollama'\nmodel='fixture-model'\nauth_mode='none'\n").unwrap();
    let catalog = lifecycle_machine(
        &fixture,
        &project,
        &["actions", "list", "--package", "lifecycle"],
        None,
    );
    assert_eq!(catalog["outcome"], "succeeded", "{catalog}");
    let binding = catalog["data"]["binding"].clone();
    let action_request = json!({"schema_version":2,"interface":"reports","action":"save","inputs":{},"expected_binding":binding,
        "execution_policy":{"version":1,"allowed":[{"profile":"fixture","request_kind":"text","model":{"kind":"named","value":"fixture-model"},"thinking":{"mode":"provider_default"}}],"limits":{"max_runtime_secs":60,"max_output_tokens":512,"max_agent_depth":4}},
        "artifact_access":{"version":1,"scopes":["exports"]}});
    let issued = lifecycle_machine(
        &fixture,
        &project,
        &[
            "run",
            "--package",
            "lifecycle",
            "--interface",
            "reports",
            "--action",
            "save",
            "--action-request-stdin",
            "--include-result-content",
        ],
        Some(&action_request),
    );
    assert_eq!(issued["outcome"], "succeeded", "{issued}");
    assert_eq!(
        issued["data"]["result"]["content"],
        json!({"owner":"alias","version":"1"})
    );
    let result = &issued["data"]["result"];
    let read_request = json!({"schema_version":1,"interface":"reports","expected_binding":binding,"reference":result["artifact_references"][0]["reference"],"read_grant":result["artifact_read_grants"][0]});
    assert!(result["artifact_references"][0]
        .get("relative_path")
        .is_none());
    let artifact_args = [
        "actions",
        "artifact",
        "--package",
        "lifecycle",
        "--interface",
        "reports",
        "--request-stdin",
    ];
    let bytes = lifecycle_machine(&fixture, &project, &artifact_args, Some(&read_request));
    assert_eq!(bytes["outcome"], "succeeded", "{bytes}");
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(bytes["data"]["data"].as_str().unwrap())
            .unwrap(),
        b"alias-owned report"
    );
    let data_root = fixture.cargo_ai_home.join("packages/lifecycle/data");
    assert_eq!(
        fs::read(data_root.join("exports/report.txt")).unwrap(),
        b"alias-owned report"
    );
    assert!(!project.join("exports").exists());
    assert!(!project.join(".cargo-ai/data").exists());
    assert!(!fixture
        .cargo_ai_home
        .join("packages/lifecycle/package/exports")
        .exists());
    // The retained grant is independent of the caller's working directory, and
    // neither a consuming project's data nor immutable package bytes can replace it.
    let unrelated = fixture.root.join("unrelated");
    fs::create_dir_all(unrelated.join(".cargo-ai/data/exports")).unwrap();
    fs::write(
        unrelated.join(".cargo-ai/data/exports/report.txt"),
        "unrelated project data",
    )
    .unwrap();
    let other_cwd = lifecycle_machine(&fixture, &unrelated, &artifact_args, Some(&read_request));
    assert_eq!(other_cwd["outcome"], "succeeded", "{other_cwd}");
    assert_eq!(
        other_cwd["data"]["content_sha256"],
        bytes["data"]["content_sha256"]
    );
    let mut tampered = read_request.clone();
    tampered["read_grant"]["relative_path"] = json!("../../package/qualification_smoke.json");
    assert_ne!(
        lifecycle_machine(&fixture, &project, &artifact_args, Some(&tampered))["outcome"],
        "succeeded"
    );
    fs::write(data_root.join("business-version.json"), "2").unwrap();
    assert_eq!(
        lifecycle_machine(
            &fixture,
            &project,
            &["actions", "list", "--package", "lifecycle"],
            None
        )["data"]["binding"],
        binding
    );
    assert_eq!(
        lifecycle_machine(&fixture, &project, &artifact_args, Some(&read_request))["outcome"],
        "succeeded"
    );
    // Reuse this package and compiled source tool for upgrade and exact rollback.
    for (from, to, downgrade) in [("0.1.0", "0.2.0", false), ("0.2.0", "0.1.0", true)] {
        let path = project.join(".cargo-ai/project.toml");
        let contents = fs::read_to_string(&path).unwrap();
        fs::write(
            &path,
            contents.replace(
                &format!("version = \"{from}\""),
                &format!("version = \"{to}\""),
            ),
        )
        .unwrap();
        let packaged = fixture
            .cargo_ai_command(&project)
            .args(["--no-update-check", "package", "default", "--output-dir"])
            .arg(&package_root)
            .arg("--force")
            .output()
            .unwrap();
        assert_success(&packaged, "repackage lifecycle revision");
        let mut command = fixture.cargo_ai_command(&project);
        command
            .args(["--no-update-check", "packages", "install"])
            .arg(&package_root)
            .args(["--as", "lifecycle"]);
        if downgrade {
            command.arg("--downgrade");
        }
        assert_success(&command.output().unwrap(), "install lifecycle revision");
        let current = lifecycle_machine(
            &fixture,
            &project,
            &["actions", "list", "--package", "lifecycle"],
            None,
        );
        assert_eq!(current["data"]["binding"]["package"]["version"], to);
        assert_eq!(
            fs::read(data_root.join("exports/report.txt")).unwrap(),
            b"alias-owned report"
        );
        assert_eq!(
            fs::read(data_root.join("business-version.json")).unwrap(),
            b"2"
        );
        let stale = lifecycle_machine(&fixture, &project, &artifact_args, Some(&read_request));
        assert_ne!(stale["outcome"], "succeeded", "{stale}");
        if downgrade {
            assert_eq!(current["data"]["binding"], binding);
            assert_eq!(stale["error"]["code"], "artifact.stale_context", "{stale}");
        }
    }
    fs::create_dir_all(&data_root).expect("package data root should be created");
    fs::write(data_root.join("keep.txt"), "protected").expect("package data should be written");
    let protected_uninstall = fixture
        .cargo_ai_command(&project)
        .args(["--no-update-check", "packages", "uninstall", "lifecycle"])
        .output()
        .expect("protected uninstall should start");
    assert!(!protected_uninstall.status.success());
    assert!(output_text(&protected_uninstall).contains("--delete-data"));

    let uninstall = fixture
        .cargo_ai_command(&project)
        .args([
            "--no-update-check",
            "packages",
            "uninstall",
            "lifecycle",
            "--delete-data",
        ])
        .output()
        .expect("explicit uninstall should start");
    assert_success(&uninstall, "explicit package uninstall");
    assert!(!fixture.cargo_ai_home.join("packages/lifecycle").exists());
    let reinstalled = fixture
        .cargo_ai_command(&project)
        .args(["--no-update-check", "packages", "install"])
        .arg(&package_root)
        .args(["--as", "lifecycle"])
        .output()
        .unwrap();
    assert_success(&reinstalled, "reinstall identical lifecycle package");
    let fresh = lifecycle_machine(
        &fixture,
        &project,
        &["actions", "list", "--package", "lifecycle"],
        None,
    );
    assert_eq!(fresh["data"]["binding"], binding);
    let old = lifecycle_machine(&fixture, &project, &artifact_args, Some(&read_request));
    assert_eq!(old["error"]["code"], "artifact.stale_context", "{old}");
    let regenerated = lifecycle_machine(
        &fixture,
        &project,
        &[
            "run",
            "--package",
            "lifecycle",
            "--interface",
            "reports",
            "--action",
            "save",
            "--action-request-stdin",
            "--include-result-content",
        ],
        Some(&action_request),
    );
    assert_eq!(regenerated["outcome"], "succeeded", "{regenerated}");
    assert_ne!(
        regenerated["data"]["result"]["artifact_references"][0]["reference"],
        read_request["reference"]
    );
    assert_eq!(
        fs::read(data_root.join("exports/report.txt")).unwrap(),
        b"alias-owned report"
    );
    let cleanup = fixture
        .cargo_ai_command(&project)
        .args([
            "--no-update-check",
            "packages",
            "uninstall",
            "lifecycle",
            "--delete-data",
        ])
        .output()
        .unwrap();
    assert_success(&cleanup, "cleanup reinstalled fixture");
    assert!(!fixture.cargo_ai_home.join("packages/lifecycle").exists());
    let staging = fixture.cargo_ai_home.join("packages/.staging");
    assert!(
        !staging.exists()
            || fs::read_dir(staging)
                .expect("staging root should be readable")
                .next()
                .is_none()
    );
    assert_eq!(
        fs::read_to_string(fallback_sentinel).expect("fallback sentinel should be readable"),
        "unchanged"
    );
}

fn lifecycle_machine(
    fixture: &Fixture,
    cwd: &std::path::Path,
    args: &[&str],
    request: Option<&Value>,
) -> Value {
    let mut command = fixture.cargo_ai_command(cwd);
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
    let response: Value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|_| panic!("Invalid lifecycle frame: {}", output_text(&output)));
    assert_eq!(
        output.status.success(),
        response["outcome"] == "succeeded",
        "{response}"
    );
    assert_eq!(response["completion"]["terminal"], true, "{response}");
    response
}

fn prepare_lifecycle_artifact_contract(fixture: &Fixture, project: &std::path::Path) {
    assert_success(
        &fixture
            .cargo_ai_command(project)
            .args(["--no-update-check", "add", "tool", "artifact_writer"])
            .output()
            .unwrap(),
        "scaffold lifecycle result tool",
    );
    // The fixture implements the tool protocol in its dependency-free binary.
    fs::remove_file(project.join("tools/artifact_writer/src/lib.rs")).unwrap();
    fs::write(
        project.join("tools/artifact_writer/Cargo.toml"),
        "[package]\nname = \"artifact_writer\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(
        project.join("tools/artifact_writer/Cargo.lock"),
        "version = 4\n\n[[package]]\nname = \"artifact_writer\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    let description = json!({"protocol_version":1,"name":"artifact_writer","description":"Write an alias-owned report.","params":{},"result":{"type":"string","nullable":true},"resource_profile":{"network":"none","filesystem_read":"required","filesystem_write":"required","subprocess":"none","env_read":"none","credential_access":"none"},"self_test":{"supported":false,"safe":false},"examples":{"minimal_invoke":{"protocol_version":1,"params":{}},"full_invoke":{"protocol_version":1,"params":{}}}}).to_string();
    let response = json!({"protocol_version":1,"result":json!({"data":{"owner":"alias","version":"1"},"artifacts":[{"id":"report","scope":"exports","path":"report.txt","mime_type":"text/plain"}]}).to_string()}).to_string();
    fs::write(project.join("tools/artifact_writer/src/main.rs"),format!(r##"
use std::io::Read;
fn main() {{
    if std::env::args().nth(1).as_deref()==Some("describe") {{ println!("{{}}",r#"{description}"#);return; }}
    let mut request=String::new();std::io::stdin().read_to_string(&mut request).unwrap();
    std::fs::create_dir_all("exports").unwrap();
    if !std::path::Path::new("exports/report.txt").exists() {{ std::fs::write("exports/report.txt",b"alias-owned report").unwrap(); }}
    println!("{{}}",r#"{response}"#);
}}
"##)).unwrap();
    fs::write(project.join("report.json"),json!({"agent_definition_schema_version":"2026-10-03.r1","agent_schema":{"type":"object","properties":{}},"result":{"source":"tool","schema":{"type":"object","properties":{"owner":{"type":"string"},"version":{"type":"string"}},"required":["owner","version"],"additionalProperties":false},"artifact_scopes":["exports"]},"actions":[{"name":"save","logic":{"==":[1,1]},"run":[{"kind":"tool","name":"artifact_writer","params":{},"produces_result":true}]}]}).to_string()).unwrap();
    fs::write(project.join("cargo-ai-actions.json"),json!({"schema_version":2,"actions":[{"id":"save","target":"report.json","input_schema":{"type":"object","properties":{},"required":[],"additionalProperties":false},"mappings":{}}],"interfaces":[{"id":"reports","actions":["save"],"artifact_scopes":["exports"]}],"artifact_scopes":[{"id":"exports","path":"exports","mime_types":["text/plain"]}]}).to_string()).unwrap();
    let path = project.join(".cargo-ai/project.toml");
    let metadata = fs::read_to_string(&path)
        .unwrap()
        .replace(
            "agent_definitions = [\"qualification_smoke.json\"]",
            "agent_definitions = [\"qualification_smoke.json\", \"report.json\"]",
        )
        .replace("tools = []", "tools = [\"artifact_writer\"]")
        .replace("assets = []", "assets = [\"cargo-ai-actions.json\"]");
    fs::write(path, metadata).unwrap();
}

fn data_cli(fixture: &Fixture, root: &std::path::Path, args: &[&str]) -> std::process::Output {
    data_command(fixture, env!("CARGO_BIN_EXE_cargo-ai"), root)
        .arg("--no-update-check")
        .args(args)
        .output()
        .expect("CLI should start")
}

fn data_command(
    fixture: &Fixture,
    program: impl AsRef<std::ffi::OsStr>,
    root: &std::path::Path,
) -> std::process::Command {
    let mut paths = vec![std::path::Path::new(env!("CARGO_BIN_EXE_cargo-ai"))
        .parent()
        .unwrap()
        .to_path_buf()];
    paths.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    let mut command = fixture.command(program, root);
    command.env("PATH", std::env::join_paths(paths).unwrap());
    command
}

fn structural_definition(program: &str, args: &[&str]) -> serde_json::Value {
    serde_json::json!({
        "agent_definition_schema_version": "2026-03-03.r1",
        "inputs": [{"type":"text", "name":"job", "text":"local fixture"}],
        "agent_schema": {"type":"object", "properties":{}},
        "actions": [{"name":"probe", "logic":{"==":[1,1]}, "run":[{"kind":"exec", "program":program, "args":args}]}]
    })
}

#[test]
fn project_data_assembly_excludes_mutable_files_and_preserves_rejected_outputs() {
    let fixture = Fixture::new("data-assembly");
    let project = fixture.root.join("project");
    assert_success(
        &data_cli(
            &fixture,
            &fixture.root,
            &["new", project.to_str().unwrap(), "--vcs", "none"],
        ),
        "new",
    );
    assert!(!project.join(".cargo-ai/data").exists());
    let metadata = project.join(".cargo-ai/project.toml");
    let base = fs::read_to_string(&metadata).unwrap();
    assert!(base.contains("data_root = \".cargo-ai/data\""));
    fs::write(
        project.join("agent.json"),
        structural_definition("cargo", &["--version"]).to_string(),
    )
    .unwrap();
    fs::create_dir_all(project.join(".cargo-ai/data")).unwrap();
    fs::write(
        project.join(".cargo-ai/data/private-state.txt"),
        "private runtime sentinel",
    )
    .unwrap();
    fs::create_dir_all(project.join("assets/nested/.cargo-ai/data")).unwrap();
    fs::write(
        project.join("assets/nested/.cargo-ai/data/auth.json"),
        "fake credential sentinel",
    )
    .unwrap();
    fs::write(project.join("assets/seed.txt"), "immutable seed").unwrap();
    for file in ["AGENTS.md", "CLAUDE.md", "user.txt"] {
        fs::write(project.join(file), "user-owned sentinel").unwrap();
    }
    assert_success(
        &fixture
            .command("git", &project)
            .arg("init")
            .output()
            .unwrap(),
        "fixture Git init",
    );
    assert_success(
        &fixture
            .command("git", &project)
            .args([
                "add",
                "-f",
                ".cargo-ai/data",
                "AGENTS.md",
                "CLAUDE.md",
                "user.txt",
            ])
            .output()
            .unwrap(),
        "fixture tracked data",
    );
    let index = fs::read(project.join(".git/index")).unwrap();
    for operation in ["build", "package"] {
        fs::write(&metadata, format!("{base}\n[build.default]\nagent_definitions = ['agent.json']\nassets = ['assets']\n")).unwrap();
        let output = fixture.root.join(operation);
        let command = data_cli(
            &fixture,
            &project,
            &[
                operation,
                "default",
                "--output-dir",
                output.to_str().unwrap(),
            ],
        );
        assert_success(&command, operation);
        assert!(!output.join(".cargo-ai/data").exists());
        assert!(!output.join("assets/nested/.cargo-ai/data").exists());
        assert_eq!(
            fs::read_to_string(output.join("assets/seed.txt")).unwrap(),
            "immutable seed"
        );
        assert!(fs::read_to_string(output.join(".cargo-ai/project.toml"))
            .unwrap()
            .contains("data_root = \".cargo-ai/data\""));
        fs::write(output.join("keep.txt"), "preserve output").unwrap();
        for input in [".cargo-ai", ".cargo-ai/data/private-state.txt"] {
            fs::write(
                &metadata,
                format!("{base}\n[build.default]\nassets = ['{input}']\n"),
            )
            .unwrap();
            let rejected = data_cli(
                &fixture,
                &project,
                &[
                    operation,
                    "default",
                    "--output-dir",
                    output.to_str().unwrap(),
                    "--force",
                ],
            );
            assert!(!rejected.status.success(), "{}", output_text(&rejected));
            assert_eq!(
                fs::read_to_string(output.join("keep.txt")).unwrap(),
                "preserve output"
            );
        }
        fs::write(
            &metadata,
            format!("{base}\n[build.default]\nassets = ['assets']\n"),
        )
        .unwrap();
        for destination in [project.join(".cargo-ai/data"), project.join("assets")] {
            let rejected = data_cli(
                &fixture,
                &project,
                &[
                    operation,
                    "default",
                    "--output-dir",
                    destination.to_str().unwrap(),
                    "--force",
                ],
            );
            assert!(!rejected.status.success(), "{}", output_text(&rejected));
            assert_eq!(
                fs::read_to_string(project.join("assets/seed.txt")).unwrap(),
                "immutable seed"
            );
        }
        #[cfg(unix)]
        {
            let external = fixture.root.join("external.txt");
            fs::write(&external, "external sentinel").unwrap();
            let link = project.join("assets/linked.txt");
            std::os::unix::fs::symlink(&external, &link).unwrap();
            let rejected = data_cli(
                &fixture,
                &project,
                &[
                    operation,
                    "default",
                    "--output-dir",
                    output.to_str().unwrap(),
                    "--force",
                ],
            );
            assert!(!rejected.status.success());
            assert_eq!(
                fs::read_to_string(output.join("keep.txt")).unwrap(),
                "preserve output"
            );
            assert_eq!(fs::read_to_string(external).unwrap(), "external sentinel");
            fs::remove_file(link).unwrap();
        }
    }
    assert_eq!(fs::read(project.join(".git/index")).unwrap(), index);
    for file in ["AGENTS.md", "CLAUDE.md", "user.txt"] {
        assert_eq!(
            fs::read_to_string(project.join(file)).unwrap(),
            "user-owned sentinel"
        );
    }
    assert_eq!(
        fs::read_to_string(project.join(".cargo-ai/data/private-state.txt")).unwrap(),
        "private runtime sentinel"
    );
}

#[test]
fn writing_tool_keeps_sibling_children_across_interpreted_generated_and_installed_runs() {
    let fixture = Fixture::new("data-tool");
    let project = fixture.root.join("project");
    assert_success(
        &data_cli(
            &fixture,
            &fixture.root,
            &["new", project.to_str().unwrap(), "--vcs", "none"],
        ),
        "new",
    );
    assert_success(
        &data_cli(
            &fixture,
            &project,
            &[
                "profile",
                "add",
                "fixture",
                "--server",
                "ollama",
                "--model",
                "fixture",
                "--default",
            ],
        ),
        "isolated profile",
    );
    assert_success(
        &data_cli(&fixture, &project, &["add", "tool", "writer"]),
        "scaffold tool",
    );
    let tool_path = project.join("tools/writer/src/tool.rs");
    let mut tool = fs::read_to_string(&tool_path).unwrap();
    for resource in ["filesystem_read", "filesystem_write", "subprocess"] {
        tool = tool.replace(
            &format!("{resource}: AccessLevel::None"),
            &format!("{resource}: AccessLevel::Required"),
        );
    }
    tool.truncate(tool.find("pub(crate) fn invoke(").unwrap());
    tool.push_str(r#"
pub(crate) fn invoke(_params: BTreeMap<String, Value>, context: InvocationContext) -> Result<Option<String>, ToolError> {
    std::fs::write("result.txt", "owned output").map_err(|e| ToolError::new(e.to_string()))?;
    std::fs::write("compile-mode.txt", if cfg!(debug_assertions) { "dev" } else { "release" })
        .map_err(|e| ToolError::new(e.to_string()))?;
    let binary = if cfg!(windows) { "./child_probe.exe" } else { "./child_probe" };
    let child = context.invoke_agent(ChildAgentRequest::new(binary))?;
    if !child.stdout.contains("immutable seed") { return Err(ToolError::new("immutable input was lost")); }
    let json_child = context.invoke_agent(ChildAgentRequest::new("./child.json"))?;
    if !json_child.stdout.contains("cargo ") { return Err(ToolError::new("JSON child did not execute")); }
    for target in ["../child.json", "./nested/child.json", "/child.json", "./missing-child", "./linked-child"] {
        if context.invoke_agent(ChildAgentRequest::new(target)).is_ok() { return Err(ToolError::new("invalid child path accepted")); }
    }
    Ok(Some("children verified".to_string()))
}
"#);
    fs::write(&tool_path, &tool).unwrap();
    fs::write(project.join("seed.txt"), "immutable seed").unwrap();
    fs::write(
        project.join("probe.rs"),
        "fn main() { println!(\"{}\", std::fs::read_to_string(\"seed.txt\").unwrap()); }",
    )
    .unwrap();
    let binary_name = if cfg!(windows) {
        "child_probe.exe"
    } else {
        "child_probe"
    };
    assert_success(
        &fixture
            .command("rustc", &project)
            .args(["probe.rs", "-o", binary_name])
            .output()
            .unwrap(),
        "fixture executable",
    );
    fs::write(
        project.join("child.json"),
        structural_definition("cargo", &["--version"]).to_string(),
    )
    .unwrap();
    let parent = serde_json::json!({
        "agent_definition_schema_version":"2026-03-03.r1",
        "inputs":[{"type":"text", "name":"job", "text":"local fixture"}],
        "agent_schema":{"type":"object", "properties":{}},
        "actions":[{"name":"write", "logic":{"==":[1,1]}, "run":[{"kind":"tool", "name":"writer", "params":{}}]}]
    });
    fs::write(project.join("parent.json"), parent.to_string()).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(project.join("child.json"), project.join("linked-child")).unwrap();
    assert_success(
        &data_cli(&fixture, &project, &["tools", "build", "writer"]),
        "build tool",
    );
    let author_manifest_path = project.join("tools/writer/Cargo.toml");
    let author_lock_path = project.join("tools/writer/Cargo.lock");
    let author_manifest = fs::read(&author_manifest_path).unwrap();
    let author_lock = fs::read(&author_lock_path).unwrap();
    let managed_tool: serde_json::Value = serde_json::from_slice(
        &fs::read(project.join(".cargo-ai/tools/writer/tool.json")).unwrap(),
    )
    .unwrap();
    let target = managed_tool["artifacts"]
        .as_object()
        .unwrap()
        .keys()
        .next()
        .unwrap()
        .clone();
    assert!(managed_tool["artifacts"][&target]["path"]
        .as_str()
        .unwrap()
        .replace('\\', "/")
        .contains("/dev/"));
    for name in ["AGENTS.md", "CLAUDE.md"] {
        fs::write(
            project.join(name),
            "user instructions; preserve these bytes",
        )
        .unwrap();
    }
    assert!(!project.join(".cargo-ai/data").exists());
    assert_success(
        &data_cli(&fixture, &project, &["run", "--config", "parent.json"]),
        "interpreted writing tool",
    );
    assert_eq!(
        fs::read_to_string(project.join(".cargo-ai/data/result.txt")).unwrap(),
        "owned output"
    );
    assert!(!project.join("result.txt").exists());
    assert_eq!(
        fs::read_to_string(project.join(".cargo-ai/data/compile-mode.txt")).unwrap(),
        "dev"
    );
    assert!(!project.join(".cargo-ai/data/child.json").exists());
    fs::remove_file(project.join(".cargo-ai/data/result.txt")).unwrap();
    let check = data_cli(
        &fixture,
        &project,
        &["hatch", "checked", "--config", "parent.json", "--check"],
    );
    assert_success(&check, "dev check before hatch");
    assert!(output_text(&check).contains("Cargo profile  dev"));
    assert!(!project
        .join(if cfg!(windows) {
            "checked.exe"
        } else {
            "checked"
        })
        .exists());
    let hatch = data_cli(
        &fixture,
        &project,
        &["hatch", "owned", "--config", "parent.json"],
    );
    assert_success(&hatch, "hatch writing parent");
    assert!(output_text(&hatch).contains("Cargo profile  release"));
    let recheck = data_cli(
        &fixture,
        &project,
        &["hatch", "checked", "--config", "parent.json", "--check"],
    );
    assert_success(&recheck, "dev check after release hatch");
    assert!(output_text(&recheck).contains("Reused warmed template"));
    let generated = project.join(if cfg!(windows) { "owned.exe" } else { "owned" });
    let mut cache_roots = vec![fixture.cargo_ai_home.join("templates")];
    for _ in 0..3 {
        cache_roots = cache_roots
            .into_iter()
            .flat_map(|root| {
                fs::read_dir(root)
                    .unwrap()
                    .map(|entry| entry.unwrap().path())
                    .filter(|path| path.is_dir())
                    .collect::<Vec<_>>()
            })
            .collect();
    }
    assert_eq!(cache_roots.len(), 1);
    let cache = &cache_roots[0];
    assert!(cache.join("dev").is_dir());
    assert!(cache.join("release").is_dir());
    let release_target = cache.join("release/target");
    let binaries = [
        release_target.join("release"),
        release_target.join(&target).join("release"),
    ]
    .into_iter()
    .map(|dir| dir.join(generated.file_name().unwrap()))
    .filter(|path| path.is_file())
    .collect::<Vec<_>>();
    assert_eq!(binaries.len(), 1);
    assert_eq!(
        fs::read(&generated).unwrap(),
        fs::read(&binaries[0]).unwrap()
    );
    assert_success(
        &data_command(&fixture, &generated, &project)
            .output()
            .unwrap(),
        "generated writing tool",
    );
    usage_attribution::assert_caller_context_survives_children(&fixture, true);
    usage_attribution::assert_generated_runtime_initializes_without_cli(&fixture, &project);
    assert!(project.join(".cargo-ai/data/result.txt").is_file());
    verify_project_image_paths(&fixture, &project);
    #[cfg(unix)]
    fs::remove_file(project.join("linked-child")).unwrap();
    let metadata = project.join(".cargo-ai/project.toml");
    let base = fs::read_to_string(&metadata).unwrap();
    fs::write(
        &metadata,
        base.replace("data_root = \".cargo-ai/data\"\n", ""),
    )
    .unwrap();
    assert_success(
        &data_cli(&fixture, &project, &["run", "--config", "parent.json"]),
        "legacy bridge cwd",
    );
    assert_eq!(
        fs::read_to_string(project.join("result.txt")).unwrap(),
        "owned output"
    );
    fs::create_dir_all(project.join("tools/writer/.cargo-ai/data")).unwrap();
    fs::write(
        project.join("tools/writer/.cargo-ai/data/private.txt"),
        "source data sentinel",
    )
    .unwrap();
    let selection = format!("agent_definitions = ['parent.json', 'child.json']\ntools = ['writer']\nassets = ['seed.txt', '{binary_name}']\n");
    fs::write(&metadata, format!("{base}\n[build.default]\n{selection}hatched_agents = ['parent.json']\n[build.release]\n{selection}\n")).unwrap();
    let build = data_cli(&fixture, &project, &["build"]);
    assert_success(&build, "default release assembly");
    assert!(output_text(&build).contains("Cargo profile: release"));
    let built_root = project
        .join("target/cargo-ai/build/default")
        .join(&target)
        .join("release");
    let built_manifest: toml::Value =
        toml::from_str(&fs::read_to_string(built_root.join("cargo-ai-build.toml")).unwrap())
            .unwrap();
    assert_eq!(built_manifest["profile"].as_str(), Some("default"));
    assert_eq!(
        built_manifest["cargo_compile_profile"].as_str(),
        Some("release")
    );
    assert_success(
        &data_command(
            &fixture,
            built_root.join(if cfg!(windows) {
                "parent.exe"
            } else {
                "parent"
            }),
            &built_root,
        )
        .output()
        .unwrap(),
        "assembled release agent and tool",
    );
    assert_eq!(
        fs::read_to_string(built_root.join(".cargo-ai/data/compile-mode.txt")).unwrap(),
        "release"
    );
    let explicit_build = fixture.root.join("explicit-build");
    assert_success(
        &data_cli(
            &fixture,
            &project,
            &[
                "build",
                "release",
                "--output-dir",
                explicit_build.to_str().unwrap(),
            ],
        ),
        "named selection and exact output directory",
    );
    assert!(explicit_build.join("cargo-ai-build.toml").is_file());
    assert!(!explicit_build.join("release").exists());
    assert_eq!(fs::read(&author_manifest_path).unwrap(), author_manifest);
    assert_eq!(fs::read(&author_lock_path).unwrap(), author_lock);
    let package = fixture.root.join("package");
    assert_success(
        &data_cli(
            &fixture,
            &project,
            &[
                "package",
                "default",
                "--output-dir",
                package.to_str().unwrap(),
            ],
        ),
        "package writer",
    );
    assert!(!package.join(".cargo-ai/data").exists());
    assert!(!package.join("tools/writer/.cargo-ai/data").exists());
    assert!(!package.join("tools/writer/target").exists());
    assert!(!package.join(".cargo-ai/tools/writer/bin").exists());
    assert_eq!(
        fs::read(package.join("tools/writer/Cargo.toml")).unwrap(),
        author_manifest
    );
    assert_eq!(
        fs::read(package.join("tools/writer/Cargo.lock")).unwrap(),
        author_lock
    );
    let recipient = Fixture::new("recipient");
    assert_success(
        &data_cli(
            &recipient,
            &recipient.root,
            &[
                "profile",
                "add",
                "fixture",
                "--server",
                "ollama",
                "--model",
                "fixture",
                "--default",
            ],
        ),
        "recipient isolated profile",
    );
    assert_success(
        &data_cli(
            &recipient,
            &recipient.root,
            &[
                "packages",
                "install",
                package.to_str().unwrap(),
                "--as",
                "owned",
            ],
        ),
        "recipient install",
    );
    assert_success(
        &data_cli(&recipient, &recipient.root, &["run", "owned::parent"]),
        "recipient independent run",
    );
    usage_attribution::assert_caller_context_survives_children(&recipient, false);
    assert!(!recipient.root.join("result.txt").exists());
    assert_eq!(
        fs::read_to_string(
            recipient
                .cargo_ai_home
                .join("packages/owned/data/compile-mode.txt")
        )
        .unwrap(),
        "release"
    );
    assert_eq!(
        fs::read(
            recipient
                .cargo_ai_home
                .join("packages/owned/package/tools/writer/Cargo.toml")
        )
        .unwrap(),
        author_manifest
    );
    assert_eq!(
        fs::read_to_string(
            recipient
                .cargo_ai_home
                .join("packages/owned/data/result.txt")
        )
        .unwrap(),
        "owned output"
    );
    fs::write(&tool_path, tool.replace("owned output", "edited output")).unwrap();
    // Rebuild edited author code while the installed recipient retains its own version.
    assert_success(
        &data_cli(&fixture, &project, &["tools", "build", "writer"]),
        "retained source rebuild",
    );
    assert_success(
        &data_cli(&fixture, &project, &["run", "--config", "parent.json"]),
        "edited source run",
    );
    assert_eq!(
        fs::read_to_string(project.join(".cargo-ai/data/result.txt")).unwrap(),
        "edited output"
    );
    assert_eq!(
        fs::read_to_string(
            recipient
                .cargo_ai_home
                .join("packages/owned/data/result.txt")
        )
        .unwrap(),
        "owned output"
    );
    // Cargo-native profile overrides remain authoritative and source-owned.
    let customized_manifest = format!(
        "{}\n[profile.release]\ndebug-assertions = true\n",
        String::from_utf8(author_manifest).unwrap()
    );
    fs::write(&author_manifest_path, &customized_manifest).unwrap();
    let custom_build = fixture.root.join("custom-profile");
    assert_success(
        &data_cli(
            &fixture,
            &project,
            &[
                "build",
                "release",
                "--output-dir",
                custom_build.to_str().unwrap(),
            ],
        ),
        "Cargo-native release settings",
    );
    assert_success(
        &data_cli(&fixture, &custom_build, &["run", "--config", "parent.json"]),
        "customized release tool",
    );
    assert_eq!(
        fs::read_to_string(custom_build.join(".cargo-ai/data/compile-mode.txt")).unwrap(),
        "dev"
    );
    assert_eq!(
        fs::read_to_string(&author_manifest_path).unwrap(),
        customized_manifest
    );
    assert_eq!(fs::read(&author_lock_path).unwrap(), author_lock);
    for name in ["AGENTS.md", "CLAUDE.md"] {
        assert_eq!(
            fs::read_to_string(project.join(name)).unwrap(),
            "user instructions; preserve these bytes"
        );
    }
}

fn verify_project_image_paths(fixture: &Fixture, project: &std::path::Path) {
    fs::write(
        project.join(".cargo-ai/data/reference.png"),
        "fixture reference",
    )
    .unwrap();
    let definition = serde_json::json!({
        "agent_definition_schema_version":"2026-03-03.r1",
        "runtime_vars":{"reference":{"type":"string","default":"reference.png"}},
        "inputs":[{"type":"text","name":"job","text":"local fixture"}],
        "agent_schema":{"type":"object","properties":{}},
        "actions":[{"name":"image","logic":{"==":[1,1]},"run":[{
            "kind":"generate_image","model":"gpt-image-2","prompt":"fixture",
            "reference_images":[{"path":[{"var":"runtime.reference"}]}],"path":"image.png"
        }]}]
    });
    fs::write(project.join("image.json"), definition.to_string()).unwrap();
    assert_success(
        &data_cli(
            fixture,
            project,
            &["hatch", "image_writer", "--config", "image.json"],
        ),
        "hatch image writer",
    );
    for generated in [false, true] {
        let server = OneShotHttpServer::json(
            "/v1/images/edits",
            serde_json::json!({"data":[{"b64_json":"aW1hZ2U="}]}),
        );
        let url = server.url.replace("/images/edits", "/chat/completions");
        let mut command = if generated {
            data_command(
                fixture,
                project.join(if cfg!(windows) {
                    "image_writer.exe"
                } else {
                    "image_writer"
                }),
                project,
            )
        } else {
            let mut command = data_command(fixture, env!("CARGO_BIN_EXE_cargo-ai"), project);
            command.args(["--no-update-check", "run", "--config", "image.json"]);
            command
        };
        let run = command
            .args([
                "--server",
                "openai",
                "--model",
                "gpt-image-2",
                "--url",
                &url,
                "--token",
                "fixture-token",
            ])
            .output()
            .unwrap();
        assert_success(&run, "isolated image output with dynamic reference");
        let request = server.finish();
        assert!(request.contains("reference.png"));
        assert_eq!(
            fs::read(project.join(".cargo-ai/data/image.png")).unwrap(),
            b"image"
        );
        assert!(!project.join("image.png").exists());
        fs::remove_file(project.join(".cargo-ai/data/image.png")).unwrap();
    }
}
