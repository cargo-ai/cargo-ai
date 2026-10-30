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
    let mut qualified_completions = 0;
    for event in recorded {
        if matches!(
            event["event_type"].as_str(),
            Some(
                "agent_run_completed"
                    | "root_run_completed"
                    | "tool_run_completed"
                    | "provider_request_completed"
            )
        ) {
            assert_eq!(event["elapsed_timing"]["version"], 1);
            assert_eq!(event["elapsed_timing"]["basis"], "monotonic_elapsed");
            assert_eq!(event["elapsed_timing"]["elapsed_ms"], event["duration_ms"]);
            qualified_completions += 1;
        }
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
    assert!(
        qualified_completions >= 2,
        "actual emitted agent/root completions carry producer timing provenance"
    );
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
    let completed = events(fixture)
        .into_iter()
        .rev()
        .find(|event| event["event_type"] == "provider_request_completed")
        .expect("actual native provider completion is captured");
    assert_eq!(completed["elapsed_timing"]["version"], 1);
    assert_eq!(completed["elapsed_timing"]["basis"], "monotonic_elapsed");
    assert_eq!(
        completed["elapsed_timing"]["elapsed_ms"],
        completed["duration_ms"]
    );
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
fn usage_attribution_legacy_binary_refuses_normalized_store_without_rewriting_facts() {
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
    // Agent execution remains best effort even when an older capture runtime
    // refuses a newer ledger. Historical v1 coexistence does not apply to v2.
    assert_success(&output, "older execution with incompatible usage storage");
    assert_eq!(events(&fixture), original);
    assert!(fixture
        .cargo_ai_home
        .join("usage/capture-incomplete")
        .is_file());
    let version: i64 =
        rusqlite::Connection::open(fixture.cargo_ai_home.join("usage/usage.sqlite3"))
            .unwrap()
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
    assert_eq!(version, 2);
}

#[test]
#[ignore = "run explicitly with a compiled historical store probe in CARGO_AI_LEGACY_STORE_TEST_BINARY"]
fn usage_normalization_generated_runtime_migrates_and_fences_historical_writer() {
    use std::io::{BufRead, BufReader, Write};
    use std::process::Stdio;
    let historical = std::env::var_os("CARGO_AI_LEGACY_STORE_TEST_BINARY")
        .expect("set the isolated compiled historical store probe path");
    let author = Fixture::new("usage-normalization-author");
    let workspace = author.root.join("project");
    project(&workspace, PACKAGE_ID);
    fs::write(
        workspace.join("agent.json"),
        json!({
            "agent_definition_schema_version":"2026-03-03.r1",
            "inputs":[{"type":"text","name":"job","text":"local normalization probe"}],
            "agent_schema":{"type":"object","properties":{}},
            "actions":[]
        })
        .to_string(),
    )
    .unwrap();
    let hatch = data_cli(
        &author,
        &workspace,
        &["hatch", "normalization_probe", "--config", "agent.json"],
    );
    assert_success(&hatch, "hatch actual normalization probe");
    assert!(
        !String::from_utf8_lossy(&hatch.stderr).contains("warning:"),
        "{}",
        String::from_utf8_lossy(&hatch.stderr)
    );
    let binary = workspace.join(if cfg!(windows) {
        "normalization_probe.exe"
    } else {
        "normalization_probe"
    });
    assert!(binary.is_file());
    use sha2::{Digest, Sha256};
    eprintln!(
        "Emitted normalization runtime SHA256: {:x}",
        Sha256::digest(fs::read(&binary).unwrap())
    );

    let selected = Fixture::new("usage-normalization-selected");
    fs::write(selected.cargo_ai_home.join("config.toml"), "default_profile = 'local'\n[[profile]]\nname = 'local'\nserver = 'ollama'\nmodel = 'unused'\nauth_mode = 'none'\n").unwrap();
    let dir = selected.cargo_ai_home.join("usage");
    fs::create_dir(&dir).unwrap();
    let db = rusqlite::Connection::open(dir.join("usage.sqlite3")).unwrap();
    db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;
        CREATE TABLE usage_events(sequence INTEGER PRIMARY KEY AUTOINCREMENT,event_id TEXT UNIQUE NOT NULL,root_run_id TEXT,agent_run_id TEXT,event_type TEXT NOT NULL,created_at TEXT NOT NULL,record_json TEXT NOT NULL);
        CREATE INDEX usage_events_root ON usage_events(root_run_id,sequence);
        CREATE INDEX usage_events_agent ON usage_events(agent_run_id,sequence);
        CREATE TABLE usage_meta(key TEXT PRIMARY KEY,value TEXT NOT NULL);
        CREATE TABLE usage_backup_projection(event_id TEXT PRIMARY KEY REFERENCES usage_events(event_id) ON DELETE CASCADE,record_json TEXT NOT NULL);
        CREATE TABLE usage_backup_queue(event_id TEXT NOT NULL REFERENCES usage_events(event_id) ON DELETE CASCADE,binding TEXT NOT NULL,generation INTEGER NOT NULL,acknowledged INTEGER NOT NULL DEFAULT 0,PRIMARY KEY(event_id,binding,generation));
        PRAGMA user_version=1;").unwrap();
    let original = json!({"schema_version":1,"event_id":"synthetic-original","root_run_id":"synthetic-root","event_type":"provider_request_completed","timestamp":"2026-10-27T00:00:00Z","agent":{"revision":"old","path":"/synthetic/project"},"provider":null,"attribution":{"schema_version":99,"opaque":null},"restored_attribution":false,"usage":{"input_tokens":u64::MAX},"duration_ms":"wrong-type","status":null,"extension":{"null":null,"wide":u64::MAX}});
    db.execute("INSERT INTO usage_events(sequence,event_id,root_run_id,event_type,created_at,record_json) VALUES(77,'synthetic-original','synthetic-root','provider_request_completed','2026-10-27T00:00:00Z',?1)", [original.to_string()]).unwrap();
    let projection = " { \"schema_version\" : 1, \"kept\" : null } ";
    db.execute(
        "INSERT INTO usage_backup_projection VALUES('synthetic-original',?1)",
        [projection],
    )
    .unwrap();
    db.execute(
        "INSERT INTO usage_backup_queue VALUES('synthetic-original','synthetic-binding',1,0)",
        [],
    )
    .unwrap();
    // The exact historical code opens the v1 connection before the real emitted
    // runtime changes the schema. It stays alive across the migration boundary.
    let mut old = selected
        .command(&historical, &selected.root)
        .arg("wait")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut readiness = String::new();
    BufReader::new(old.stdout.take().unwrap())
        .read_line(&mut readiness)
        .unwrap();
    assert_eq!(readiness.trim(), "ready");
    let empty_path = selected.root.join("empty-path");
    fs::create_dir(&empty_path).unwrap();
    let output = selected
        .command(&binary, &selected.root)
        .env("PATH", &empty_path)
        .output()
        .unwrap();
    assert_success(
        &output,
        "actual emitted runtime migration without CLI on PATH",
    );
    assert_eq!(
        db.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
            .unwrap(),
        2
    );
    old.stdin.take().unwrap().write_all(b"append\n").unwrap();
    let old_output = old.wait_with_output().unwrap();
    assert!(
        !old_output.status.success(),
        "already-open historical writer unexpectedly appended"
    );
    assert!(String::from_utf8_lossy(&old_output.stderr).contains("record_json"));
    let later = selected
        .command(&historical, &selected.root)
        .arg("fresh")
        .output()
        .unwrap();
    assert!(
        !later.status.success(),
        "later historical runtime unexpectedly opened v2"
    );
    assert!(String::from_utf8_lossy(&later.stderr).contains("newer incompatible schema"));

    let stored: String = db
        .query_row(
            "SELECT record_json FROM usage_event_records WHERE event_id='synthetic-original'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(serde_json::from_str::<Value>(&stored).unwrap(), original);
    assert_eq!(
        db.query_row(
            "SELECT sequence FROM usage_events WHERE event_id='synthetic-original'",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        77
    );
    assert_eq!(
        db.query_row(
            "SELECT record_json FROM usage_backup_projection",
            [],
            |row| row.get::<_, String>(0)
        )
        .unwrap(),
        projection
    );
    assert_eq!(
        db.query_row(
            "SELECT binding||':'||generation||':'||acknowledged FROM usage_backup_queue",
            [],
            |row| row.get::<_, String>(0)
        )
        .unwrap(),
        "synthetic-binding:1:0"
    );
    let all = events(&selected);
    assert_eq!(all.first(), Some(&original));
    let appended: Vec<_> = all.iter().skip(1).collect();
    assert!(!appended.is_empty());
    assert!(appended
        .iter()
        .all(|event| event["attribution"]["runtime"]["kind"] == "generated"));
    assert_eq!(
        appended
            .iter()
            .filter(|event| event["event_type"] == "root_run_completed")
            .count(),
        1
    );
    assert!(!dir.join("capture-incomplete").exists());
    let image: String = db
        .query_row(
            "SELECT value FROM usage_meta WHERE key='schema_v2_recovery_image'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let recovery =
        rusqlite::Connection::open_with_flags(image, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    assert_eq!(
        recovery
            .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        recovery
            .query_row("SELECT COUNT(*) FROM usage_events", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        db.query_row(
            "SELECT COUNT(*) FROM usage_events WHERE event_id='historical-writer'",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    eprintln!("Actual emitted runtime: v1 event77 preserved, {} generated facts appended, recovery retained; already-open and later historical writers refused", appended.len());
}
