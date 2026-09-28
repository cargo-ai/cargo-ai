use super::{data_cli, data_command, structural_definition};
use crate::support::{
    assert_success, copy_tree, openai_success_response, Fixture, OneShotHttpServer,
};
use serde_json::{json, Value};
use std::{collections::BTreeSet, fs, path::Path};

const PACKAGE_ID: &str = "c7bf7ebc-9138-4b83-bc79-634b5b2c6e92";

fn project(path: &Path, id: &str) {
    fs::create_dir_all(path.join(".cargo-ai")).unwrap();
    fs::write(
        path.join(".cargo-ai/project.toml"),
        format!("format_version = 1\n[project]\nid = '{id}'\nname = 'usage_fixture'\nversion = '1.0.0'\n[build.default]\nagent_definitions = ['agent.json']\nhatched_agents = []\ntools = []\nassets = []\n"),
    ).unwrap();
    fs::write(path.join("agent.json"), json!({
        "agent_definition_schema_version":"2026-03-03.r1",
        "inputs":[{"type":"text","text":"Return status ok."}],
        "agent_schema":{"type":"object","properties":{"status":{"type":"string"}},"required":["status"],"additionalProperties":false},
        "actions":[]
    }).to_string()).unwrap();
}

fn json_cli(fixture: &Fixture, cwd: &Path, args: &[&str]) -> Value {
    let out = data_cli(fixture, cwd, args);
    assert_success(&out, "usage query");
    serde_json::from_slice(&out.stdout).unwrap()
}

pub(super) fn events(fixture: &Fixture) -> Vec<Value> {
    let out = data_cli(
        fixture,
        &fixture.root,
        &["usage", "export", "--limit", "1000"],
    );
    assert_success(&out, "usage export");
    String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

pub(super) fn assert_caller_context_survives_children(fixture: &Fixture, expect_generated: bool) {
    let all = events(fixture);
    let agents: Vec<_> = all
        .iter()
        .filter(|event| event["event_type"] == "agent_run_started")
        .collect();
    assert!(!agents.is_empty());
    assert!(agents
        .iter()
        .any(|event| event["parent_agent_run_id"].is_string()));
    for child in agents
        .iter()
        .filter(|event| event["parent_agent_run_id"].is_string())
    {
        let root = agents
            .iter()
            .find(|event| {
                event["root_run_id"] == child["root_run_id"]
                    && event["parent_agent_run_id"].is_null()
            })
            .unwrap();
        assert_eq!(
            child["attribution"]["workspace"],
            root["attribution"]["workspace"]
        );
        assert!(child["attribution"]["environment"]["id"].is_string());
        assert!(child["attribution"]["runtime"]["executable_path"].is_string());
    }
    if expect_generated {
        assert!(agents
            .iter()
            .any(|event| event["attribution"]["runtime"]["kind"] == "generated"));
    }
}

pub(super) fn assert_generated_runtime_initializes_without_cli(fixture: &Fixture, project: &Path) {
    fs::write(
        project.join("identity_probe.json"),
        json!({
            "agent_definition_schema_version":"2026-03-03.r1",
            "inputs":[{"type":"text","name":"job","text":"local identity probe"}],
            "agent_schema":{"type":"object","properties":{}},
            "actions":[]
        })
        .to_string(),
    )
    .unwrap();
    assert_success(
        &data_cli(
            fixture,
            project,
            &["run", "--config", "identity_probe.json"],
        ),
        "interpreted identity probe",
    );
    let interpreted = events(fixture)
        .into_iter()
        .find(|event| {
            event["event_type"] == "agent_run_started"
                && event["attribution"]["agent"]["key"] == "identity_probe.json"
        })
        .expect("interpreted probe has a definition-relative key");
    assert_success(
        &data_cli(
            fixture,
            project,
            &["hatch", "identity_probe", "--config", "identity_probe.json"],
        ),
        "hatch independent identity probe",
    );
    let binary = project.join(if cfg!(windows) {
        "identity_probe.exe"
    } else {
        "identity_probe"
    });
    let isolated = Fixture::new("usage-generated-without-cli");
    let moved_binary = isolated.root.join(if cfg!(windows) {
        "renamed_probe.exe"
    } else {
        "renamed_probe"
    });
    fs::copy(&binary, &moved_binary).unwrap();
    fs::write(isolated.cargo_ai_home.join("config.toml"), "default_profile = 'local'\n[[profile]]\nname = 'local'\nserver = 'ollama'\nmodel = 'unused'\n").unwrap();
    let empty_path = isolated.root.join("empty-path");
    fs::create_dir(&empty_path).unwrap();
    let out = isolated
        .command(&moved_binary, &isolated.root)
        .env("PATH", &empty_path)
        .output()
        .unwrap();
    assert_success(&out, "generated runtime without installed CLI on PATH");
    let config: toml::Value =
        toml::from_str(&fs::read_to_string(isolated.cargo_ai_home.join("config.toml")).unwrap())
            .unwrap();
    let id = config["cargo_ai_metadata"]["cargo_ai_install_id"]
        .as_str()
        .unwrap();
    let recorded = events(&isolated);
    assert!(!recorded.is_empty());
    for event in recorded {
        assert_eq!(event["attribution"]["environment"]["id"], id);
        assert_eq!(event["attribution"]["runtime"]["kind"], "generated");
        assert!(event["attribution"]["package"]["id"].is_string());
        assert_eq!(
            event["attribution"]["package"]["id"],
            interpreted["attribution"]["package"]["id"]
        );
        assert_eq!(event["attribution"]["agent"]["key"], "identity_probe.json");
        assert_eq!(
            event["attribution"]["agent"]["id"],
            interpreted["attribution"]["agent"]["id"]
        );
        assert!(event["attribution"]["workspace"]["path"].is_null());
    }
}

fn provider_run(fixture: &Fixture, cwd: &Path, args: &[&str]) {
    let server = OneShotHttpServer::json("/v1/chat/completions", openai_success_response("ok"));
    let profile = format!("fixture-{}", uuid::Uuid::new_v4());
    assert_success(
        &data_cli(
            fixture,
            cwd,
            &[
                "profile",
                "add",
                &profile,
                "--server",
                "ollama",
                "--model",
                "fixture",
                "--url",
                &server.url,
                "--auth",
                "none",
                "--default",
            ],
        ),
        "local provider profile",
    );
    assert_success(&data_cli(fixture, cwd, args), "attributed provider run");
    assert!(server.finish().contains("Return status ok."));
}

#[test]
fn usage_attribution_groups_copies_and_preserves_caller_of_installed_package() {
    let fixture = Fixture::new("usage-locations");
    let a = fixture.root.join("a");
    let b = fixture.root.join("b");
    project(&a, PACKAGE_ID);
    copy_tree(&a, &b);
    let copied_metadata = b.join(".cargo-ai/project.toml");
    let next_version = fs::read_to_string(&copied_metadata)
        .unwrap()
        .replace("version = '1.0.0'", "version = '2.0.0'");
    fs::write(&copied_metadata, next_version).unwrap();
    let before_a = fs::read(a.join(".cargo-ai/project.toml")).unwrap();
    provider_run(&fixture, &a, &["run", "--config", "agent.json"]);
    provider_run(&fixture, &b, &["run", "--config", "agent.json"]);
    let summary = json_cli(
        &fixture,
        &a,
        &[
            "usage",
            "summary",
            "--schema-version",
            "2",
            "--group-by",
            "package,package_location,agent",
            "--json",
        ],
    );
    assert_eq!(summary["schema_version"], 2);
    assert_eq!(summary["summary"]["tokens"]["total_tokens"], 10);
    let groups = summary["groups"].as_array().unwrap();
    assert_eq!(groups.len(), 2, "{summary:#}");
    assert_eq!(groups[0]["key"]["package"], groups[1]["key"]["package"]);
    assert_eq!(groups[0]["key"]["agent"], groups[1]["key"]["agent"]);
    assert_ne!(
        groups[0]["key"]["package_location"],
        groups[1]["key"]["package_location"]
    );
    assert_eq!(
        fs::read(a.join(".cargo-ai/project.toml")).unwrap(),
        before_a
    );
    let local_events = events(&fixture);
    let revisions: BTreeSet<_> = local_events
        .iter()
        .filter(|event| event["event_type"] == "provider_request_completed")
        .map(|event| event["attribution"]["package"]["version"].as_str().unwrap())
        .collect();
    assert_eq!(revisions, BTreeSet::from(["1.0.0", "2.0.0"]));

    let payload = fixture.root.join("payload");
    assert_success(
        &data_cli(
            &fixture,
            &a,
            &[
                "package",
                "default",
                "--output-dir",
                payload.to_str().unwrap(),
            ],
        ),
        "package identified project",
    );
    assert_success(
        &data_cli(
            &fixture,
            &a,
            &[
                "packages",
                "install",
                payload.to_str().unwrap(),
                "--as",
                "shared",
            ],
        ),
        "install identified package",
    );
    provider_run(&fixture, &a, &["run", "shared::agent"]);
    provider_run(&fixture, &b, &["run", "shared::agent"]);
    let all = events(&fixture);
    let installed: Vec<_> = all
        .iter()
        .filter(|event| {
            event["event_type"] == "provider_request_completed"
                && event["agent"]["source"] == "installed_package"
        })
        .collect();
    assert_eq!(installed.len(), 2);
    assert_eq!(
        installed[0]["attribution"]["package"],
        installed[1]["attribution"]["package"]
    );
    assert_eq!(
        installed[0]["attribution"]["package_location"],
        installed[1]["attribution"]["package_location"]
    );
    assert_ne!(
        installed[0]["attribution"]["workspace"],
        installed[1]["attribution"]["workspace"]
    );
    assert_eq!(
        installed[0]["agent"]["project_root"],
        installed[1]["agent"]["project_root"]
    );
    let legacy = json_cli(&fixture, &a, &["usage", "summary", "--json"]);
    assert_eq!(legacy["schema_version"], 1);
    assert_eq!(legacy["summary"]["tokens"]["total_tokens"], 20);
    assert!(legacy["groups"]
        .as_array()
        .unwrap()
        .iter()
        .all(|group| group["key"].as_array().unwrap().len() == 5));
}

#[test]
fn usage_attribution_context_and_opt_out_do_not_initialize_identity() {
    let fixture = Fixture::new("usage-read-only");
    fs::remove_dir(&fixture.cargo_ai_home).unwrap();
    let context = json_cli(&fixture, &fixture.root, &["usage", "context", "--json"]);
    assert_eq!(context["schema_version"], 1);
    assert!(!fixture.cargo_ai_home.exists());
    fs::create_dir(&fixture.cargo_ai_home).unwrap();
    let config = "default_profile = 'local'\n[[profile]]\nname = 'local'\nserver = 'ollama'\nmodel = 'unused'\nauth_mode = 'none'\n";
    fs::write(fixture.cargo_ai_home.join("config.toml"), config).unwrap();
    let definition = fixture.root.join("local.json");
    fs::write(
        &definition,
        structural_definition("cargo", &["--version"]).to_string(),
    )
    .unwrap();
    let out = fixture
        .cargo_ai_command(&fixture.root)
        .env("CARGO_AI_USAGE_TRACKING", "off")
        .args(["--no-update-check", "run", "--config"])
        .arg(&definition)
        .output()
        .unwrap();
    assert_success(&out, "tracking-off local execution");
    let current = fs::read_to_string(fixture.cargo_ai_home.join("config.toml")).unwrap();
    assert!(!current.contains("cargo_ai_install_id"));
    assert!(!fixture.cargo_ai_home.join("usage").exists());
    let preserved = fs::read(fixture.cargo_ai_home.join("config.toml")).unwrap();
    json_cli(&fixture, &fixture.root, &["usage", "context", "--json"]);
    assert_eq!(
        fs::read(fixture.cargo_ai_home.join("config.toml")).unwrap(),
        preserved
    );
}

#[test]
fn usage_attribution_separates_homes_and_observes_actual_executable() {
    let first = Fixture::new("usage-home-a");
    let second = Fixture::new("usage-home-b");
    for fixture in [&first, &second] {
        fs::write(fixture.cargo_ai_home.join("config.toml"), "default_profile = 'local'\n[[profile]]\nname = 'local'\nserver = 'ollama'\nmodel = 'unused'\n").unwrap();
        fs::write(
            fixture.root.join("agent.json"),
            structural_definition("cargo", &["--version"]).to_string(),
        )
        .unwrap();
        assert_success(
            &data_cli(fixture, &fixture.root, &["run", "--config", "agent.json"]),
            "local tracked run",
        );
    }
    let first_events = events(&first);
    let second_events = events(&second);
    let first_id = first_events[0]["attribution"]["environment"]["id"].clone();
    let second_id = second_events[0]["attribution"]["environment"]["id"].clone();
    assert!(first_id.is_string() && second_id.is_string());
    assert_ne!(first_id, second_id);
    let before: BTreeSet<_> = first_events
        .iter()
        .map(|event| event["event_id"].as_str().unwrap().to_string())
        .collect();
    assert!(second_events
        .iter()
        .all(|event| !before.contains(event["event_id"].as_str().unwrap())));
    let binary = first.root.join(if cfg!(windows) {
        "second-cli.exe"
    } else {
        "second-cli"
    });
    fs::copy(env!("CARGO_BIN_EXE_cargo-ai"), &binary).unwrap();
    let out = data_command(&first, &binary, &first.root)
        .args(["--no-update-check", "run", "--config", "agent.json"])
        .output()
        .unwrap();
    assert_success(&out, "second executable same Home");
    let after = events(&first);
    let new = after
        .iter()
        .find(|event| !before.contains(event["event_id"].as_str().unwrap()))
        .unwrap();
    assert_eq!(new["attribution"]["environment"]["id"], first_id);
    assert_ne!(
        new["attribution"]["runtime"],
        first_events[0]["attribution"]["runtime"]
    );
    assert_eq!(events(&second), second_events);
}

#[test]
#[ignore = "requires an explicitly provisioned older Cargo AI binary in an isolated test installation"]
fn usage_attribution_legacy_binary_writes_initialized_store_without_rewriting_new_facts() {
    let binary = std::env::var_os("CARGO_AI_LEGACY_TEST_BINARY")
        .expect("set the isolated older binary path");
    let fixture = Fixture::new("usage-old-writer");
    fs::write(fixture.cargo_ai_home.join("config.toml"), "default_profile = 'local'\n[[profile]]\nname = 'local'\nserver = 'ollama'\nmodel = 'unused'\n").unwrap();
    fs::write(
        fixture.root.join("agent.json"),
        structural_definition("cargo", &["--version"]).to_string(),
    )
    .unwrap();
    assert_success(
        &data_cli(&fixture, &fixture.root, &["run", "--config", "agent.json"]),
        "initialize updated store",
    );
    let original = events(&fixture);
    let output = fixture
        .command(binary, &fixture.root)
        .args(["--no-update-check", "run", "--config", "agent.json"])
        .output()
        .unwrap();
    assert_success(&output, "older writer into updated store");
    let mixed = events(&fixture);
    assert!(mixed.len() > original.len());
    assert_eq!(&mixed[..original.len()], original.as_slice());
    assert!(mixed[original.len()..]
        .iter()
        .all(|event| event.get("attribution").is_none()));
    let version: i64 =
        rusqlite::Connection::open(fixture.cargo_ai_home.join("usage/usage.sqlite3"))
            .unwrap()
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
    assert_eq!(version, 1);
}
