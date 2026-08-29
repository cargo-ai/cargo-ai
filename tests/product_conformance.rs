//! Process-level coverage for the public Cargo AI CLI spine.

mod support;

use std::fs;

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
    let fallback_sentinel = fixture.fallback_home.join("sentinel.txt");
    fs::write(&fallback_sentinel, "unchanged").expect("fallback sentinel should be written");
    let project = fixture.root.join("source");
    copy_tree(
        &repository_root().join("tests/fixtures/package_lifecycle"),
        &project,
    );
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

    let data_root = fixture.cargo_ai_home.join("packages/lifecycle/data");
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
