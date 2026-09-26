//! End-to-end local declaration inspection without account or provider state.
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    root: PathBuf,
    project: PathBuf,
    home: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "cargo-ai-requirements-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let project = root.join("project");
        let home = root.join("home");
        fs::create_dir_all(project.join(".cargo-ai")).unwrap();
        fs::create_dir_all(project.join("agents")).unwrap();
        fs::create_dir_all(&home).unwrap();
        fs::write(home.join("credentials.toml"), b"synthetic-secret-preserve").unwrap();
        fs::write(home.join("config.toml"), b"malformed startup config").unwrap();
        Self {
            root,
            project,
            home,
        }
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_cargo-ai"))
            .args(args)
            .current_dir(&self.project)
            .env("CARGO_AI_HOME", &self.home)
            .env("CARGO_AI_DISABLE_KEYCHAIN", "1")
            .output()
            .unwrap()
    }

    fn write_definition(&self, name: &str, fixture: &str) {
        fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR")).join(fixture),
            self.project.join("agents").join(name),
        )
        .unwrap();
    }

    fn assert_no_state_change(&self) {
        assert_eq!(
            fs::read(self.home.join("credentials.toml")).unwrap(),
            b"synthetic-secret-preserve"
        );
        assert_eq!(
            fs::read(self.home.join("config.toml")).unwrap(),
            b"malformed startup config"
        );
        assert_eq!(fs::read_dir(&self.home).unwrap().count(), 2);
        assert!(!self.project.join("target").exists());
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn explicit_config_is_offline_and_does_not_disclose_prompt_or_change_state() {
    let fixture = Fixture::new();
    fixture.write_definition(
        "single.json",
        "templates/shared/examples/agent-minimal.json",
    );
    let result = fixture.run(&["requirements", "--config", "agents/single.json"]);
    let stdout = String::from_utf8_lossy(&result.stdout);
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(result.status.success(), "{stderr}");
    assert!(stdout.contains("output answer: integer"), "{stdout}");
    assert!(stdout.contains("Direct steps:"), "{stdout}");
    assert!(!stdout.contains("What is 2 + 2?"));
    assert!(!stderr.contains("startup config"), "{stderr}");
    fixture.assert_no_state_change();
}

#[test]
fn build_selection_unites_declared_agents_and_keeps_assets_separate() {
    let fixture = Fixture::new();
    fixture.write_definition("first.json", "templates/shared/examples/agent-minimal.json");
    fixture.write_definition(
        "second.json",
        "templates/shared/examples/agent-enum-bounds-valid.json",
    );
    fs::write(fixture.project.join("agents/excluded.json"), b"not parsed").unwrap();
    fs::write(
        fixture.project.join(".cargo-ai/project.toml"),
        r#"
format_version = 1
[build.release]
agent_definitions = ["agents/first.json", "agents/second.json"]
hatched_agents = ["agents/first.json"]
tools = ["helper"]
assets = ["assets/unread.json"]
"#,
    )
    .unwrap();
    let result = fixture.run(&["requirements", "--build-profile", "release"]);
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(stdout.matches("Agent: agents/first.json").count(), 1);
    assert!(stdout.contains("Agent: agents/second.json"));
    assert!(stdout.contains("choices \"F\", \"C\"") || stdout.contains("choices \"C\", \"F\""));
    assert!(stdout.contains("asset: assets/unread.json"));
    assert!(!stdout.contains("Agent: agents/excluded.json"));
    fixture.assert_no_state_change();
}

#[test]
fn missing_and_unsafe_selections_fail_without_startup_effects() {
    let fixture = Fixture::new();
    fs::write(
        fixture.project.join(".cargo-ai/project.toml"),
        r#"
format_version = 1
[build.release]
agent_definitions = ["../outside.json"]
"#,
    )
    .unwrap();
    for args in [
        vec!["requirements"],
        vec![
            "requirements",
            "--config",
            "agents/a.json",
            "--build-profile",
            "release",
        ],
        vec!["requirements", "--build-profile", "missing"],
        vec!["requirements", "--build-profile", "release"],
    ] {
        let result = fixture.run(&args);
        assert!(!result.status.success(), "{args:?}");
    }
    fixture.assert_no_state_change();
}

#[test]
fn asset_only_profile_is_explicitly_unassessed_and_invalid_definition_is_actionable() {
    let fixture = Fixture::new();
    fs::write(
        fixture.project.join(".cargo-ai/project.toml"),
        "format_version = 1\n[build.assets_only]\nassets = [\"data/archive.json\"]\n[build.invalid]\nagent_definitions = [\"agents/bad.json\"]\n",
    )
    .unwrap();
    let asset_report = fixture.run(&["requirements", "--build-profile", "assets_only"]);
    assert!(asset_report.status.success());
    let stdout = String::from_utf8_lossy(&asset_report.stdout);
    assert!(stdout.contains("agent workload requirements are unassessed"));
    assert!(stdout.contains("asset: data/archive.json"));
    fs::write(fixture.project.join("agents/bad.json"), b"{broken json").unwrap();
    let invalid = fixture.run(&["requirements", "--build-profile", "invalid"]);
    assert!(!invalid.status.success());
    let stderr = String::from_utf8_lossy(&invalid.stderr);
    assert!(stderr.contains("Invalid definition"), "{stderr}");
    let bad_path = Path::new("agents").join("bad.json");
    assert!(
        stderr.contains(bad_path.to_string_lossy().as_ref()),
        "{stderr}"
    );
    assert!(!stderr.contains("broken json"));
    fixture.assert_no_state_change();
}

#[cfg(feature = "developer-tools")]
#[test]
fn installed_alias_reuses_declared_report_and_preserves_existing_fields() {
    let fixture = Fixture::new();
    fixture.write_definition("first.json", "templates/shared/examples/agent-minimal.json");
    fs::write(
        fixture.project.join(".cargo-ai/project.toml"),
        "format_version = 1\n[project]\nname = \"requirements_fixture\"\nversion = \"1.0.0\"\n[build.default]\nagent_definitions = [\"agents/first.json\"]\n",
    )
    .unwrap();
    // Packaging and installation are fixture setup; the inspect assertion is
    // against the installed verified payload, not the source project.
    let packaged = fixture.root.join("package-output");
    let package = Command::new(env!("CARGO_BIN_EXE_cargo-ai"))
        .args(["--no-update-check", "package", "--output-dir"])
        .arg(&packaged)
        .current_dir(&fixture.project)
        .env("CARGO_AI_HOME", &fixture.home)
        .env("CARGO_AI_DISABLE_KEYCHAIN", "1")
        .output()
        .unwrap();
    assert!(
        package.status.success(),
        "{}",
        String::from_utf8_lossy(&package.stderr)
    );
    let install = Command::new(env!("CARGO_BIN_EXE_cargo-ai"))
        .args(["--no-update-check", "packages", "install"])
        .arg(&packaged)
        .args(["--as", "fixture"])
        .current_dir(&fixture.project)
        .env("CARGO_AI_HOME", &fixture.home)
        .env("CARGO_AI_DISABLE_KEYCHAIN", "1")
        .output()
        .unwrap();
    assert!(
        install.status.success(),
        "{}",
        String::from_utf8_lossy(&install.stderr)
    );
    let inspect = fixture.run(&["--no-update-check", "packages", "inspect", "fixture"]);
    assert!(
        inspect.status.success(),
        "{}",
        String::from_utf8_lossy(&inspect.stderr)
    );
    let stdout = String::from_utf8_lossy(&inspect.stdout);
    for expected in [
        "Identity:  requirements_fixture",
        "Permissions:",
        "Entrypoints:",
        "Agent: agents/first.json",
        "output answer: integer",
    ] {
        assert!(stdout.contains(expected), "missing {expected}: {stdout}");
    }
}

#[test]
fn media_steps_are_reported_without_reading_assets_or_disclosing_prompts() {
    let fixture = Fixture::new();
    fs::write(
        fixture.project.join("agents/media.json"),
        r#"{
          "agent_definition_schema_version": "2026-09-09.r1",
          "agent_schema": {"type": "object", "properties": {}},
          "actions": [{
            "name": "media",
            "logic": {"==": [1, 1]},
            "run": [
              {"kind": "generate_image", "prompt": "synthetic-secret-prompt", "path": "./out.png", "reference_images": [{"path": "./missing-ref.png"}]},
              {"kind": "generate_audio", "text": "synthetic-secret-speech", "voice": "alloy", "path": "./out.mp3"},
              {"kind": "transcribe_audio", "audio": {"path": "./missing-input.wav"}, "output_variable": "transcript"}
            ]
          }]
        }"#,
    ).unwrap();
    let result = fixture.run(&["requirements", "--config", "agents/media.json"]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stdout = String::from_utf8_lossy(&result.stdout);
    for expected in [
        "image generation; output format png; references path ./missing-ref.png",
        "speech generation; output format mp3; voice alloy",
        "audio transcription; source ./missing-input.wav",
        "reference image: ./missing-ref.png",
        "transcription source: ./missing-input.wav",
    ] {
        assert!(stdout.contains(expected), "missing {expected}: {stdout}");
    }
    assert!(!stdout.contains("synthetic-secret-prompt"));
    assert!(!stdout.contains("synthetic-secret-speech"));
    fixture.assert_no_state_change();
}

#[test]
fn malformed_project_metadata_reports_location_without_source_content() {
    let fixture = Fixture::new();
    fs::write(
        fixture.project.join(".cargo-ai/project.toml"),
        "format_version = 1\n[build.release]\nagent_definitions = [\"agents/example.json\"]\napi_key = \"synthetic-secret-project-metadata\" trailing\n",
    )
    .unwrap();
    let result = fixture.run(&["requirements", "--build-profile", "release"]);
    assert!(!result.status.success());
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(stderr.contains("Invalid project metadata"), "{stderr}");
    assert!(stderr.contains("line 4"), "{stderr}");
    assert!(stderr.contains("Fix TOML syntax"), "{stderr}");
    assert!(!stderr.contains("synthetic-secret-project-metadata"));
    assert!(!stderr.contains("api_key ="));
    fixture.assert_no_state_change();
}

#[test]
fn accepted_input_spellings_report_canonical_kinds_and_file_needs() {
    let fixture = Fixture::new();
    fs::write(
        fixture.project.join("agents/inputs.json"),
        r#"{
          "agent_definition_schema_version": "2026-09-09.r1",
          "inputs": [
            {"name": "photo", "type": " IMAGE ", "path": "./photo.png"},
            {"name": "backup", "type": "image", "path": "./backup.png"},
            {"name": "document", "type": "FILE", "path": "./document.pdf"}
          ],
          "agent_schema": {"type": "object", "properties": {"answer": {"type": "string"}}},
          "actions": []
        }"#,
    )
    .unwrap();
    let result = fixture.run(&["requirements", "--config", "agents/inputs.json"]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stdout = String::from_utf8_lossy(&result.stdout);
    for expected in [
        "input image (inputs[0])",
        "input image (inputs[1])",
        "input file (inputs[2])",
        "image input: ./photo.png",
        "image input: ./backup.png",
        "file input: ./document.pdf",
    ] {
        assert!(stdout.contains(expected), "missing {expected}: {stdout}");
    }
    assert_eq!(stdout.matches("  - input image (agents:").count(), 1);
    assert!(!stdout.contains("input  IMAGE "));
    fixture.assert_no_state_change();
}
