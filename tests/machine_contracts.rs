//! Application contracts exercise isolated production entrypoints.
#[allow(dead_code)]
mod support;
use serde_json::{json, Value};
use std::{fs, process::Output};
use support::Fixture;

fn run(f: &Fixture, args: &[&str]) -> Output {
    f.cargo_ai_command(&f.root)
        .env("CODEX_HOME", f.root.join("synthetic-codex"))
        .args(args)
        .args(["--output-format", "json", "--output-schema-version", "1"])
        .output()
        .unwrap()
}
fn response(output: &Output) -> Value {
    assert!(
        output.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let v: Value = serde_json::from_slice(&output.stdout).expect("exactly one JSON response");
    assert_eq!(v["schema_version"], 1);
    assert_eq!(v["completion"]["terminal"], true);
    assert!(!v["request_id"].as_str().unwrap().is_empty());
    assert_eq!(output.status.success(), v["outcome"] == "succeeded");
    v
}
fn configure(f: &Fixture) {
    fs::write(
        f.cargo_ai_home.join("config.toml"),
        "secret_store='file'\nprofile=[]\n",
    )
    .unwrap();
}

#[test]
fn profile_continuity_command_preserves_reference_and_reports_readiness_separately() {
    let f = Fixture::new("machine-profile-continuity");
    fs::write(f.cargo_ai_home.join("config.toml"), "secret_store='file'\n[[profile]]\nname='fixture'\nserver='ollama'\nmodel='fixture-model'\nauth_mode='none'\n").unwrap();
    let enrolled = response(&run(&f, &["profile", "refresh-context", "fixture"]));
    assert_eq!(enrolled["outcome"], "succeeded", "{enrolled}");
    let context = &enrolled["data"]["context"];
    let config_before = fs::read(f.cargo_ai_home.join("config.toml")).unwrap();
    let arguments = [
        "profile",
        "validate-context",
        "fixture",
        "--profile-uuid",
        context["profile_uuid"].as_str().unwrap(),
        "--connection-generation",
        context["connection_generation"].as_str().unwrap(),
    ];
    let validated = response(&run(&f, &arguments));
    assert_eq!(validated["outcome"], "succeeded", "{validated}");
    assert_eq!(
        validated["payload_schema"],
        "cargo-ai.profile.validate-context.v1"
    );
    assert_eq!(validated["data"]["schema_version"], 1);
    assert_eq!(validated["data"]["status"], "unchanged");
    assert_eq!(validated["data"]["context"], *context);
    assert_eq!(validated["data"]["execution_ready"], true);
    assert_eq!(validated["data"]["review_required"], false);
    for effect in ["local", "remote", "context_metadata", "public_key_cache"] {
        assert_eq!(validated["data"]["effects"][effect], "unapplied");
    }
    let mut renewal = arguments.to_vec();
    renewal.push("--renew");
    let renewed = response(&run(&f, &renewal));
    assert_eq!(renewed["data"]["context"], *context);
    assert_eq!(renewed["data"]["execution_ready"], true);
    let reenrolled = response(&run(&f, &["profile", "refresh-context", "fixture"]));
    assert_eq!(reenrolled["data"]["context"], *context);
    assert_eq!(
        fs::read(f.cargo_ai_home.join("config.toml")).unwrap(),
        config_before
    );

    let mut wrong = arguments;
    wrong[6] = "another-generation";
    let rejected = response(&run(&f, &wrong));
    assert_eq!(rejected["outcome"], "succeeded", "{rejected}");
    assert_eq!(rejected["data"]["execution_ready"], false);
    assert_ne!(rejected["data"]["status"], "unchanged");
    assert_ne!(rejected["data"]["status"], "renewed");

    let capabilities = response(&run(&f, &["capabilities"]));
    let contract = capabilities["data"]["contracts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["command"] == "profile validate-context")
        .unwrap();
    assert_eq!(contract["payload_schema"], validated["payload_schema"]);
    assert_eq!(contract["continuity_contract_version"], 1);
    assert_eq!(contract["schema_versions"], json!([1]));
    let schema: Value = serde_json::from_str(include_str!(
        "../docs/schemas/profile-context-validation-v1.json"
    ))
    .unwrap();
    assert_eq!(schema["properties"]["schema_version"]["const"], 1);
    for payload in [&validated, &renewed, &rejected] {
        assert!(schema["properties"]["status"]["enum"]
            .as_array()
            .unwrap()
            .contains(&payload["data"]["status"]));
        for required in schema["required"].as_array().unwrap() {
            assert!(payload["data"].get(required.as_str().unwrap()).is_some());
        }
    }
}

#[test]
fn action_payload_advertisements_match_legacy_and_role_catalog_responses() {
    use std::io::Write;
    use std::process::Stdio;
    let f = Fixture::new("machine-action-payload-versions");
    configure(&f);
    fs::create_dir_all(f.root.join(".cargo-ai")).unwrap();
    fs::write(
        f.root.join(".cargo-ai/project.toml"),
        "[project]\nname='payload-fixture'\n",
    )
    .unwrap();
    fs::write(f.root.join("agent.json"), json!({"agent_definition_schema_version":"2026-10-06.r1","agent_schema":{"type":"object","properties":{"answer":{"type":"string"}}},"actions":[]}).to_string()).unwrap();
    let capabilities = response(&run(&f, &["capabilities"]));
    let contracts = capabilities["data"]["contracts"].as_array().unwrap();
    for version in [2, 3] {
        let mut catalog = json!({"schema_version":version,"actions":[{"id":"work","target":"agent.json","input_schema":{"type":"object","properties":{},"required":[],"additionalProperties":false},"mappings":{}}],"interfaces":[{"id":"fixture","actions":["work"]}]});
        let limits = json!({"max_runtime_secs":60,"max_output_tokens":512,"max_agent_depth":4});
        if version == 3 {
            let requirements = json!({"operation":"text_generation","input_modalities":["text"],"structured_output":false,"settings":{}});
            catalog["role_registry"] = json!({"version":1,"roles":[{"id":"writer","label":"Writer","purpose":"Write fixture content","requirements":requirements}],"call_sites":[{"id":"root","locator":{"definition":"agent.json","site":"root"},"kind":"root","role":"writer","requirements":requirements}],"contexts":[{"key":{"action":"work","interface":"fixture","mode":"default"},"call_sites":["root"],"resources":[],"limits":limits}]});
        }
        fs::write(f.root.join("cargo-ai-actions.json"), catalog.to_string()).unwrap();
        let listed = response(&run(&f, &["actions", "list", "--project", "."]));
        assert_eq!(listed["outcome"], "succeeded", "{listed}");
        let mut request = json!({"schema_version":version,"interface":"fixture","action":"work","inputs":{},"expected_binding":listed["data"]["binding"],"execution_policy":{"version":1,"allowed":[],"limits":limits}});
        if version == 3 {
            request["role_execution"] = json!({"mode":"default","binding_revision":{"version":1,"revision":"fixture","bindings":[]},"resolution_id":"0".repeat(64)});
        }
        let mut child = f
            .cargo_ai_command(&f.root)
            .env("CODEX_HOME", f.root.join("synthetic-codex"))
            .args([
                "actions",
                "validate",
                "--project",
                ".",
                "--interface",
                "fixture",
                "--action",
                "work",
                "--request-stdin",
                "--output-format",
                "json",
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
            .write_all(request.to_string().as_bytes())
            .unwrap();
        let validated = response(&child.wait_with_output().unwrap());
        assert_eq!(validated["outcome"], "succeeded", "{validated}");
        assert_eq!(validated["data"]["execution_authorized"], false);
        for (name, value, selector) in [
            ("actions list", &listed, "catalog_version"),
            ("actions validate", &validated, "request_version"),
        ] {
            let contract = contracts
                .iter()
                .find(|entry| entry["command"] == name)
                .unwrap();
            assert_eq!(contract["schema_versions"], json!([1]));
            assert_eq!(
                contract["payload_schema"],
                format!("cargo-ai.{}.v2", name.replace(' ', "."))
            );
            let advertised = contract["payload_schemas"]
                .as_array()
                .unwrap()
                .iter()
                .find(|entry| entry["condition"][selector] == version)
                .unwrap();
            assert_eq!(advertised["payload_schema"], value["payload_schema"]);
            assert_eq!(value["data"]["schema_version"], version);
            assert_eq!(value["schema_version"], 1);
        }
    }
}

#[test]
fn thinking_choices_and_default_fallback_survive_json_and_ndjson_terminals() {
    for format in ["json", "ndjson"] {
        for (choice, applied, boolean_model) in [
            ("high", true, false),
            ("max", false, false),
            ("ON", true, true),
            ("oFf", true, true),
            ("on", false, false),
        ] {
            let f = Fixture::new("machine-thinking");
            configure(&f);
            fs::write(f.root.join("answer.json"), json!({
                "agent_definition_schema_version":"2026-03-03.r1",
                "inputs":[{"type":"text","text":"fixture"}],
                "agent_schema":{"type":"object","properties":{"answer":{"type":"string"}},"required":["answer"],"additionalProperties":false},"actions":[]
            }).to_string()).unwrap();
            let mut server = mockito::Server::new();
            let metadata = server
                .mock("POST", "/api/show")
                .with_body(if boolean_model {
                    r#"{"model":"fixture","thinking":{"values":[false,true],"default":true}}"#
                } else {
                    r#"{"model":"fixture","thinking":{"values":["low","high"],"default":"low"}}"#
                })
                .create();
            let inference = server
                .mock("POST", "/v1/chat/completions")
                .match_request(move |request| {
                    let body: Value = serde_json::from_slice(request.body().unwrap()).unwrap();
                    if applied {
                        body["reasoning_effort"]
                            == if boolean_model {
                                if choice.eq_ignore_ascii_case("on") {
                                    "medium"
                                } else {
                                    "none"
                                }
                            } else {
                                "high"
                            }
                    } else {
                        body.get("reasoning_effort").is_none()
                    }
                })
                .with_body(r#"{"choices":[{"message":{"content":"{\"answer\":\"fixture\"}"}}]}"#)
                .create();
            let output = f
                .cargo_ai_command(&f.root)
                .args([
                    "run",
                    "answer.json",
                    "--server",
                    "ollama",
                    "--model",
                    "fixture",
                    "--url",
                    &format!("{}/v1/chat/completions", server.url()),
                    "--thinking",
                    choice,
                    "--output-format",
                    format,
                ])
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stdout)
            );
            assert!(output.stderr.is_empty());
            let frames: Vec<Value> = String::from_utf8(output.stdout)
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
            let terminal = if format == "ndjson" {
                &frames.last().unwrap()["data"]
            } else {
                &frames[0]
            };
            let result = &terminal["data"]["thinking"]["records"][0]["outcome"];
            let mode = if choice.eq_ignore_ascii_case("on") {
                "on"
            } else if choice.eq_ignore_ascii_case("off") {
                "off"
            } else {
                "choice"
            };
            assert_eq!(result["requested"]["mode"], mode);
            if mode == "choice" {
                assert_eq!(result["requested"]["value"], choice);
            }
            assert_eq!(
                result["effective"]["mode"],
                if applied { mode } else { "provider_default" }
            );
            assert_eq!(terminal["warnings"].as_array().unwrap().is_empty(), applied);
            if !applied {
                assert_eq!(
                    terminal["warnings"][0]["code"],
                    "thinking.choice_unavailable"
                );
            }
            metadata.assert();
            inference.assert();
        }
    }
}

#[test]
fn thinking_fallback_survives_a_later_provider_failure() {
    for format in ["json", "ndjson"] {
        let f = Fixture::new("machine-thinking-failure");
        configure(&f);
        fs::write(f.root.join("answer.json"), json!({
            "agent_definition_schema_version":"2026-03-03.r1", "inputs":[{"type":"text","text":"fixture"}],
            "agent_schema":{"type":"object","properties":{"answer":{"type":"string"}},"required":["answer"],"additionalProperties":false},"actions":[]
        }).to_string()).unwrap();
        let mut server = mockito::Server::new();
        let metadata = server.mock("POST", "/api/show").with_body("{}").create();
        let inference = server
            .mock("POST", "/v1/chat/completions")
            .match_request(|request| {
                let body: Value = serde_json::from_slice(request.body().unwrap()).unwrap();
                body.get("reasoning_effort").is_none()
            })
            .with_body(r#"{"choices":[]}"#)
            .create();
        let output = f
            .cargo_ai_command(&f.root)
            .args([
                "run",
                "answer.json",
                "--server",
                "ollama",
                "--model",
                "custom-model",
                "--url",
                &format!("{}/v1/chat/completions", server.url()),
                "--thinking",
                "max",
                "--output-format",
                format,
            ])
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(output.stderr.is_empty());
        let frames: Vec<Value> = String::from_utf8(output.stdout)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let terminal = if format == "ndjson" {
            &frames.last().unwrap()["data"]
        } else {
            &frames[0]
        };
        assert_eq!(terminal["outcome"], "failed");
        assert_eq!(
            terminal["data"]["thinking"]["records"][0]["outcome"]["fallback"], "unknown_support",
            "{terminal}"
        );
        assert_eq!(
            terminal["warnings"][0]["effective"]["mode"],
            "provider_default"
        );
        metadata.assert();
        inference.assert();
    }
}

#[test]
fn capabilities_and_rejected_negotiation_are_passive_and_secret_safe() {
    let f = Fixture::new("machine-passive");
    let absent = f.root.join("absent");
    let output = f
        .cargo_ai_command(&f.root)
        .env("CODEX_HOME", f.root.join("synthetic-codex"))
        .env("CARGO_AI_HOME", &absent)
        .args(["capabilities", "--output-format", "json"])
        .output()
        .unwrap();
    let data = response(&output);
    assert_eq!(data["outcome"], "succeeded");
    assert!(!absent.exists());
    assert!(data["data"]["contracts"]
        .as_array()
        .unwrap()
        .iter()
        .any(|row| row["command"] == "models list"));
    let version = response(&run(&f, &["--version"]));
    for field in ["version", "developer_tools", "target_os", "target_arch"] {
        assert!(!version["data"][field].is_null());
    }
    for args in [
        vec![
            "--help",
            "--output-format",
            "json",
            "--output-schema-version",
            "99",
        ],
        vec![
            "--version",
            "--output-format=json",
            "--output-schema-version=99",
        ],
        vec!["run", "--help", "--output-format", "ndjson"],
        vec!["--version", "--output-format", "yaml"],
        vec!["--help", "--output-schema-version", "1"],
    ] {
        let output = f
            .cargo_ai_command(&f.root)
            .env("CODEX_HOME", f.root.join("synthetic-codex"))
            .env("CARGO_AI_HOME", &absent)
            .args(args)
            .output()
            .unwrap();
        assert_eq!(
            response(&output)["error"]["code"],
            "cli.unsupported_contract"
        );
        assert_eq!(output.status.code(), Some(2));
        assert!(!absent.exists());
    }
    let help = f
        .cargo_ai_command(&f.root)
        .env("CODEX_HOME", f.root.join("synthetic-codex"))
        .env("CARGO_AI_HOME", &absent)
        .args([
            "--help",
            "--output-format=json",
            "--output-schema-version=1",
        ])
        .output()
        .unwrap();
    assert_eq!(response(&help)["command"], "capabilities");
    assert!(!absent.exists());
    let output = f
        .cargo_ai_command(&f.root)
        .env("CODEX_HOME", f.root.join("synthetic-codex"))
        .env("CARGO_AI_HOME", &absent)
        .args([
            "profile",
            "add",
            "unapplied",
            "--server",
            "ollama",
            "--model",
            "x",
            "--auth",
            "none",
            "--output-format",
            "json",
            "--output-schema-version",
            "99",
        ])
        .output()
        .unwrap();
    assert_eq!(
        response(&output)["error"]["code"],
        "cli.unsupported_contract"
    );
    assert!(!absent.exists());
    let output = run(
        &f,
        &[
            "profile",
            "set",
            "x",
            "--temperature",
            "synthetic-misplaced-secret",
        ],
    );
    assert_eq!(response(&output)["error"]["code"], "cli.invalid_input");
    assert!(!String::from_utf8_lossy(&output.stdout).contains("synthetic-misplaced-secret"));
}

#[test]
fn profile_machine_changes_and_legacy_readers_share_persisted_state() {
    let f = Fixture::new("machine-profile");
    configure(&f);
    assert_eq!(
        response(&run(
            &f,
            &[
                "profile",
                "add",
                "fixture",
                "--server",
                "ollama",
                "--model",
                "exact:id",
                "--auth",
                "none",
                "--default"
            ]
        ))["outcome"],
        "succeeded"
    );
    let before = fs::read(f.cargo_ai_home.join("config.toml")).unwrap();
    let show = response(&run(&f, &["profile", "show", "fixture"]));
    assert_eq!(show["data"]["profile"]["model"], "exact:id");
    assert_eq!(
        fs::read(f.cargo_ai_home.join("config.toml")).unwrap(),
        before
    );
    let legacy = f
        .cargo_ai_command(&f.root)
        .env("CODEX_HOME", f.root.join("synthetic-codex"))
        .args(["profile", "show", "fixture", "--no-update-check"])
        .output()
        .unwrap();
    assert!(legacy.status.success());
    assert!(String::from_utf8_lossy(&legacy.stdout).contains("Model:   exact:id"));
    let updated = response(&run(
        &f,
        &["profile", "set", "fixture", "--model", "other:exact"],
    ));
    assert_eq!(updated["outcome"], "succeeded");
    let refusal = response(&run(&f, &["profile", "remove", "fixture"]));
    assert_eq!(refusal["outcome"], "requires_interaction");
    assert_eq!(
        response(&run(&f, &["profile", "remove", "fixture", "--yes"]))["outcome"],
        "succeeded"
    );
}

#[test]
fn runtime_provider_results_require_explicit_private_content_opt_in() {
    for include in [false, true] {
        let f = Fixture::new("machine-private-result");
        configure(&f);
        let private = "private answer fixture";
        let server = support::OneShotHttpServer::json(
            "/v1/chat/completions",
            json!({"choices":[{"message":{"role":"assistant","content":json!({"answer":private}).to_string()}}]}),
        );
        fs::write(f.root.join("answer.json"),json!({"agent_definition_schema_version":"2026-03-03.r1","inputs":[{"type":"text","text":"fixture"}],"agent_schema":{"type":"object","properties":{"answer":{"type":"string"}},"required":["answer"],"additionalProperties":false},"actions":[]}).to_string()).unwrap();
        let mut command = f.cargo_ai_command(&f.root);
        command.env("CODEX_HOME", f.root.join("synthetic-codex"));
        command.args([
            "run",
            "answer.json",
            "--server",
            "ollama",
            "--model",
            "fixture",
            "--url",
            &server.url,
            "--output-format",
            "json",
        ]);
        if include {
            command.arg("--include-result-content");
        }
        let output = command.output().unwrap();
        let value = response(&output);
        assert_eq!(value["outcome"], "succeeded", "{value}");
        assert_eq!(value["data"]["result"]["availability"], "available");
        assert_eq!(
            String::from_utf8_lossy(&output.stdout).contains(private),
            include
        );
        if include {
            assert_eq!(value["data"]["result"]["content"]["answer"], private);
        } else {
            assert_eq!(value["data"]["result"]["content_included"], false);
        }
        assert!(server.finish().starts_with("POST /v1/chat/completions"));
    }
}

#[test]
fn capabilities_distinguish_replacement_actions_and_selected_result_delivery() {
    let f = Fixture::new("result-capabilities");
    let output = run(&f, &["capabilities"]);
    let value = response(&output);
    let contracts = value["data"]["contracts"].as_array().unwrap();
    for (name, payload) in [
        ("actions list", "cargo-ai.actions.list.v2"),
        ("actions validate", "cargo-ai.actions.validate.v2"),
        ("actions artifact", "cargo-ai.actions.artifact.v1"),
        ("actions resource", "cargo-ai.actions.resource.v1"),
    ] {
        let contract = contracts
            .iter()
            .find(|item| item["command"] == name)
            .unwrap();
        assert_eq!(contract["payload_schema"], payload);
        assert_eq!(contract["schema_versions"], json!([1]));
    }
    let run = contracts
        .iter()
        .find(|item| item["command"] == "run")
        .unwrap();
    assert_eq!(run["client_actions"]["catalog_versions"], json!([2, 3]));
    assert_eq!(run["client_actions"]["request_versions"], json!([2, 3]));
    let roles = &run["native_roles"];
    assert_eq!(roles["role_contract_version"], 1);
    assert_eq!(roles["binding_authorizes_execution"], false);
    assert_eq!(roles["operation_access"], "unverified_until_invocation");
    assert_eq!(roles["session"]["version"], 1);
    assert_eq!(roles["session"]["control_ack_deadline_ms"], 2_000);
    assert_eq!(roles["session"]["max_control_frames"], 256);
    let results = &run["structured_results"];
    assert_eq!(results["definition_revision"], "2026-10-03.r1");
    assert_eq!(
        results["business_types"],
        json!(["object", "array", "string", "integer", "number", "boolean", "null"])
    );
    assert_eq!(
        results["artifact_access"]["limits"]["file_bytes"],
        4 * 1024 * 1024
    );
    assert_eq!(
        results["artifact_access"]["limits"]["read_types"],
        json!(["text/plain", "application/json", "image/png", "audio/wav"])
    );
    assert_eq!(
        value["data"]["runtime_capabilities"]["structured_results"]["terminal_delivery"],
        true
    );
    for (source, revision) in [
        (include_str!("../docs/schemas/action-catalog-v2.json"), 2),
        (include_str!("../docs/schemas/action-request-v2.json"), 2),
        (
            include_str!("../docs/schemas/action-artifact-request-v1.json"),
            1,
        ),
    ] {
        let schema: Value = serde_json::from_str(source).unwrap();
        assert_eq!(schema["properties"]["schema_version"]["const"], revision);
    }
}

#[test]
fn inventory_leaf_prerequisites_never_fall_back_to_prose_or_network() {
    let f = Fixture::new("machine-inventory");
    configure(&f);
    let cases: &[&[&str]] = &[
        &["profile", "list"],
        &["profile", "show", "missing"],
        &["account", "status"],
        &["account", "register", "fixture@example.test"],
        &["account", "confirm", "--stdin"],
        &["account", "deactivate"],
        &["auth", "login", "openai"],
        &["packages", "list"],
        &["packages", "inspect", "missing"],
        &["packages", "list", "--account", "--all"],
        &["packages", "inspect", "missing", "--account", "--json"],
        &[
            "packages",
            "install",
            "--account",
            "--source-id",
            "source",
            "--version-id",
            "version",
            "--as",
            "alias",
            "--accept-permissions",
        ],
        &[
            "packages",
            "pull",
            "--source-id",
            "source",
            "--version-id",
            "version",
            "--output-dir",
            "out",
        ],
        &["packages", "visibility", "--name", "missing", "--public"],
        &["packages", "publish"],
        &["agents", "list", "--all"],
        &[
            "agents",
            "pull",
            "--name",
            "missing",
            "--definition-path",
            "/",
            "--stdout",
        ],
        &[
            "agents",
            "push",
            "--name",
            "missing",
            "--definition-path",
            "/",
            "--json-file",
            "absent.json",
        ],
        &[
            "agents",
            "visibility",
            "--name",
            "missing",
            "--definition-path",
            "/",
            "--private",
        ],
        &["usage", "context", "--json"],
        &["usage", "summary", "--json"],
        &["usage", "summary", "--json", "--schema-version", "2"],
        &["usage", "runs", "--json", "--limit", "50"],
        &["usage", "runs", "--json", "--schema-version", "2"],
        &["usage", "show", "missing", "--json", "--limit", "50"],
        &[
            "usage",
            "show",
            "missing",
            "--json",
            "--schema-version",
            "2",
        ],
        &["usage", "settings", "--json"],
        &["usage", "backup", "status", "--json"],
        &["usage", "backup", "enable", "--json"],
        &["usage", "backup", "disable", "--json"],
        &["usage", "backup", "sync", "--json"],
        &["usage", "backup", "include-history", "--json"],
        &[
            "run",
            "absent.json",
            "--render-mode",
            "append-only",
            "--max-runtime-in-sec",
            "1",
        ],
    ];
    for args in cases {
        let v = response(&run(&f, args));
        assert_ne!(v["error"]["code"], "cli.unsupported_contract", "{args:?}");
        assert_ne!(
            v["error"]["code"], "cli.invalid_input",
            "{args:?}: malformed fixture arguments"
        );
    }
    assert!(!f.root.join("out").exists());
}

#[cfg(unix)]
#[test]
fn runtime_stream_frames_real_lifecycle_and_private_child_output_isolated() {
    let f = Fixture::new("machine-runtime");
    configure(&f);
    let definition = json!({"agent_definition_schema_version":"2026-03-03.r1","inputs":[{"type":"text","name":"job","text":"local fixture"}],"agent_schema":{"type":"object","properties":{}},"actions":[{"name":"local","logic":{"==":[1,1]},"run":[{"kind":"exec","program":"sh","args":["-c","printf 'private child sentinel\\n'; printf 'private stderr sentinel\\n' >&2"]}]}]});
    fs::write(f.root.join("local.json"), definition.to_string()).unwrap();
    let output = f
        .cargo_ai_command(&f.root)
        .env("CODEX_HOME", f.root.join("synthetic-codex"))
        .args([
            "run",
            "local.json",
            "--server",
            "ollama",
            "--model",
            "unused",
            "--output-format",
            "ndjson",
            "--max-runtime-in-sec",
            "5",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(output.stderr.is_empty());
    assert!(!String::from_utf8_lossy(&output.stdout).contains("private child sentinel"));
    let events: Vec<Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(events.first().unwrap()["event_type"], "operation_started");
    assert_eq!(events.last().unwrap()["event_type"], "operation_completed");
    assert_eq!(events.last().unwrap()["data"]["outcome"], "succeeded");
    for (index, event) in events.iter().enumerate() {
        assert_eq!(event["sequence"], index as u64);
        assert_eq!(event["operation_id"], events[0]["operation_id"]);
    }
    assert!(events
        .iter()
        .any(|event| event["event_type"] == "action_started"));
}

#[cfg(unix)]
#[test]
fn blocked_stream_pipe_cannot_prevent_cancellation_and_owned_child_cleanup() {
    use std::process::Stdio;
    use std::time::{Duration, Instant};
    let f = Fixture::new("machine-blocked-pipe");
    configure(&f);
    let child_pid = f.root.join("child.pid");
    let script = format!("echo $$ > '{}'; sleep 30 & wait", child_pid.display());
    let definition = json!({"agent_definition_schema_version":"2026-03-03.r1","inputs":[{"type":"text","name":"job","text":"local fixture"}],"agent_schema":{"type":"object","properties":{}},"actions":[{"name":"x".repeat(60000),"logic":{"==":[1,1]},"run":[{"kind":"exec","program":"sh","args":["-c",script]}]}]});
    fs::write(f.root.join("blocked.json"), definition.to_string()).unwrap();
    let mut child = f
        .cargo_ai_command(&f.root)
        .env("CODEX_HOME", f.root.join("synthetic-codex"))
        .args([
            "run",
            "blocked.json",
            "--server",
            "ollama",
            "--model",
            "unused",
            "--output-format",
            "ndjson",
            "--max-runtime-in-sec",
            "30",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let start = Instant::now();
    while !child_pid.exists() && start.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(child_pid.exists(), "owned child did not start");
    let owned_pid = fs::read_to_string(&child_pid).unwrap();
    assert!(std::process::Command::new("/bin/kill")
        .args(["-INT", &child.id().to_string()])
        .status()
        .unwrap()
        .success());
    let deadline = Instant::now() + Duration::from_secs(6);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("Blocked stdout prevented bounded cancellation");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    assert!(!status.success());
    let alive = std::process::Command::new("/bin/kill")
        .args(["-0", owned_pid.trim()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();
    assert!(!alive.success(), "owned child survived cancellation");
    // The caller abandoned its output channel; absence of a delivered terminal
    // must remain an incomplete invocation, not a success claim.
}

#[cfg(unix)]
#[test]
fn cooperative_cancel_settles_parallel_lanes_before_terminal_cleanup_claim() {
    use std::{
        process::Stdio,
        time::{Duration, Instant},
    };
    let f = Fixture::new("machine-parallel-cancel");
    configure(&f);
    let mut actions = vec![];
    for index in 0..2 {
        let script = format!("echo $$ > lane-{index}.pid; sleep 2; echo late > lane-{index}.late");
        actions.push(json!({"name":format!("lane-{index}"),"logic":{"==":[1,1]},"run":[{"kind":"exec","program":"sh","args":["-c",script]}]}));
    }
    fs::write(f.root.join("parallel.json"),json!({"agent_definition_schema_version":"2026-03-03.r1","action_execution":"parallel","inputs":[{"type":"text","name":"job","text":"fixture"}],"agent_schema":{"type":"object","properties":{}},"actions":actions}).to_string()).unwrap();
    let mut child = f
        .cargo_ai_command(&f.root)
        .env("CODEX_HOME", f.root.join("synthetic-codex"))
        .args([
            "run",
            "parallel.json",
            "--server",
            "ollama",
            "--model",
            "unused",
            "--output-format",
            "ndjson",
            "--max-runtime-in-sec",
            "10",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !(0..2).all(|i| f.root.join(format!("lane-{i}.pid")).exists()) {
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("parallel lanes did not start");
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
            panic!("cooperative cancel did not settle");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(130));
    assert!(output.stderr.is_empty());
    let events: Vec<Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|s| serde_json::from_str(s).unwrap())
        .collect();
    let terminal = events.last().unwrap();
    assert_eq!(terminal["event_type"], "operation_completed");
    assert_eq!(terminal["data"]["outcome"], "canceled");
    assert_eq!(terminal["data"]["data"]["owned_child_cleanup"], "completed");
    std::thread::sleep(Duration::from_millis(2200));
    for index in 0..2 {
        assert!(
            !f.root.join(format!("lane-{index}.late")).exists(),
            "canceled lane applied a late effect"
        );
        let pid = fs::read_to_string(f.root.join(format!("lane-{index}.pid"))).unwrap();
        assert!(!std::process::Command::new("/bin/kill")
            .args(["-0", pid.trim()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap()
            .success());
    }
}

#[cfg(feature = "developer-tools")]
#[test]
fn source_backed_package_install_keeps_build_diagnostics_out_of_contract_output() {
    let f = Fixture::new("machine-source-install");
    configure(&f);
    let project = f.root.join("source");
    support::copy_tree(
        &support::repository_root().join("tests/fixtures/package_lifecycle"),
        &project,
    );
    let metadata = project.join(".cargo-ai/project.toml");
    fs::write(
        &metadata,
        fs::read_to_string(&metadata)
            .unwrap()
            .replace("tools = []", "tools = [\"fixture_tool\"]"),
    )
    .unwrap();
    let tool = project.join("tools/fixture_tool");
    fs::create_dir_all(tool.join("src")).unwrap();
    fs::write(
        tool.join("Cargo.toml"),
        "[package]\nname='fixture_tool'\nversion='0.1.0'\nedition='2021'\n",
    )
    .unwrap();
    fs::write(
        tool.join("Cargo.lock"),
        "version = 4\n[[package]]\nname='fixture_tool'\nversion='0.1.0'\n",
    )
    .unwrap();
    fs::write(tool.join("src/main.rs"), "fn main() {}\n").unwrap();
    fs::write(tool.join("build.rs"),"fn main() { println!(\"cargo:warning=private build stdout sentinel\"); eprintln!(\"private build stderr sentinel\"); }\n").unwrap();
    let manifest = project.join(".cargo-ai/tools/fixture_tool");
    fs::create_dir_all(&manifest).unwrap();
    fs::write(manifest.join("tool.json"),json!({"schema_version":1,"tool_id":"fixture_tool","source":{"manifest_path":"tools/fixture_tool/Cargo.toml"},"binary":{"default_name":"fixture_tool"},"artifacts":{}}).to_string()).unwrap();
    let package = f.root.join("package");
    let assembled = f
        .cargo_ai_command(&project)
        .env("CODEX_HOME", f.root.join("synthetic-codex"))
        .args([
            "package",
            "default",
            "--output-dir",
            package.to_str().unwrap(),
            "--output-format",
            "json",
        ])
        .output()
        .unwrap();
    assert_eq!(response(&assembled)["outcome"], "succeeded");
    let installed = response(&run(
        &f,
        &[
            "packages",
            "install",
            package.to_str().unwrap(),
            "--as",
            "fixture",
        ],
    ));
    assert_eq!(installed["outcome"], "succeeded", "{installed}");
    assert_eq!(installed["data"]["readback"], "verified");
    assert_eq!(installed["data"]["package"]["alias"], "fixture");
    assert_eq!(
        response(&run(&f, &["packages", "inspect", "fixture"]))["outcome"],
        "succeeded"
    );
}

#[cfg(unix)]
#[test]
fn runtime_deadline_and_child_output_limit_have_distinct_safe_errors() {
    for (script, code, seconds) in [
        ("sleep 5", "runtime.timeout", "1"),
        ("head -c 1200000 /dev/zero", "runtime.output_limit", "5"),
    ] {
        let f = Fixture::new("machine-runtime-limits");
        configure(&f);
        fs::write(f.root.join("limits.json"),json!({"agent_definition_schema_version":"2026-03-03.r1","inputs":[{"type":"text","name":"job","text":"fixture"}],"agent_schema":{"type":"object","properties":{}},"actions":[{"name":"bounded","logic":{"==":[1,1]},"run":[{"kind":"exec","program":"sh","args":["-c",script]}]}]}).to_string()).unwrap();
        let v = response(&run(
            &f,
            &[
                "run",
                "limits.json",
                "--server",
                "ollama",
                "--model",
                "unused",
                "--max-runtime-in-sec",
                seconds,
            ],
        ));
        assert_eq!(v["outcome"], "failed", "{v}");
        assert_eq!(v["error"]["code"], code);
    }
}

#[test]
fn account_discovery_capabilities_and_error_contract_are_additive() {
    let f = Fixture::new("machine-account-catalog");
    let output = run(&f, &["capabilities"]);
    let capabilities = response(&output);
    let models = capabilities["data"]["contracts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|contract| contract["command"] == "models list")
        .unwrap();
    for variant in models["variants"].as_array().unwrap() {
        assert_eq!(
            variant["auth_modes"],
            json!(["none", "api_key", "openai_account"])
        );
    }
    let output = run(
        &f,
        &[
            "models",
            "list",
            "--server",
            "openai",
            "--auth",
            "openai_account",
        ],
    );
    let result = response(&output);
    assert_eq!(result["error"]["code"], "discovery.credentials_required");
    assert_eq!(result["error"]["retryable"], false);
    assert!(result["data"].is_null());
    let schema: Value =
        serde_json::from_str(include_str!("../docs/schemas/model-catalog-v1.json")).unwrap();
    assert_eq!(
        schema["properties"]["auth"]["enum"],
        json!(["none", "api_key", "openai_account"])
    );
    assert!(!schema["required"]
        .as_array()
        .unwrap()
        .contains(&json!("compatibility")));
    assert_eq!(
        schema["properties"]["compatibility"]["properties"]["kind"]["const"],
        "codex_backend"
    );
    assert_eq!(
        schema["properties"]["compatibility"]["required"],
        json!(["kind", "client_version"])
    );
}
