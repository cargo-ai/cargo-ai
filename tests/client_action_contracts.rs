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

fn result_definition(schema: Value, scopes: Value) -> Value {
    json!({"agent_definition_schema_version":"2026-10-03.r1",
        "runtime_vars":{"business_json":{"type":"string"}},
        "agent_schema":{"type":"object","properties":{}},
        "result":{"source":"tool","schema":schema,"artifact_scopes":scopes},
        "actions":[{"name":"produce","logic":{"==":[1,1]},"run":[{"kind":"tool","name":"result_probe","produces_result":true,"params":{"value":{"var":"runtime.business_json"},"artifacts":"[]","mode":"data"}}]}]})
}

fn write_result_contract(f: &Fixture, definition: &Value, schema: &Value) -> Value {
    fs::write(f.root.join("result.json"), definition.to_string()).unwrap();
    fs::write(f.root.join("cargo-ai-actions.json"), json!({"schema_version":2,
        "artifact_scopes":[{"id":"reports","path":"reports","mime_types":["text/plain","application/json","image/png","audio/wav"]}],
        "interfaces":[{"id":"report","actions":["save"],"artifact_scopes":["reports"]}],
        "actions":[{"id":"save","target":"result.json","input_schema":{"type":"object","properties":{"document":schema},"required":["document"],"additionalProperties":false},"mappings":{"document":{"runtime_var":"business_json","encoding":"json"}},"required_capabilities":["business_inputs.v1","structured_results.v1"]}]
    }).to_string()).unwrap();
    catalog(f)["binding"].clone()
}

fn result_request(binding: Value, data: Value) -> Value {
    let mut value = request(&binding, "save", &[]);
    value["interface"] = json!("report");
    value["inputs"] = json!({"document":data});
    value
}

fn invoke_result(
    f: &Fixture,
    request: &Value,
    format: &str,
    include: bool,
    extra: &[&str],
) -> (Value, String) {
    let mut command = f.cargo_ai_command(&f.root);
    command.args([
        "run",
        "--project",
        f.root.to_str().unwrap(),
        "--interface",
        request["interface"].as_str().unwrap(),
        "--action",
        request["action"].as_str().unwrap(),
        "--action-request-stdin",
        "--profile",
        "fixture",
        "--output-format",
        format,
    ]);
    if include {
        command.arg("--include-result-content");
    }
    command.args(extra);
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
    let output = child.wait_with_output().unwrap();
    assert!(
        output.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let raw = String::from_utf8(output.stdout).unwrap();
    let terminal: Value = serde_json::from_str(raw.lines().last().unwrap()).unwrap();
    let response = if format == "ndjson" {
        terminal["data"].clone()
    } else {
        terminal
    };
    assert_eq!(
        output.status.success(),
        response["outcome"] == "succeeded",
        "{raw}"
    );
    (response, raw)
}

#[test]
fn selected_tools_round_trip_business_data_and_export_only_authorized_private_artifacts() {
    use base64::Engine;
    let f = fixture();
    configure(&f);
    fs::write(
        f.root.join(".cargo-ai/project.toml"),
        "[project]\nname='report-fixture'\n[runtime]\ndata_root='.cargo-ai/data'\n",
    )
    .unwrap();
    for args in [
        vec!["add", "tool", "result_probe"],
        vec!["tools", "build", "result_probe"],
    ] {
        if args[0] == "tools" {
            fs::write(
                f.root.join("tools/result_probe/src/main.rs"),
                include_str!("fixtures/client_action_result_tool.rs"),
            )
            .unwrap();
        }
        let output = f
            .cargo_ai_command(&f.root)
            .env("CARGO_NET_OFFLINE", "true")
            .args(args)
            .output()
            .unwrap();
        support::assert_success(&output, "prepare real result tool");
    }
    let schema = json!({"type":"object","properties":{"settings":{"type":"object","properties":{"model":{"type":"string"},"token":{"type":["string","null"]}},"required":["model","token"],"additionalProperties":false},"rows":{"type":"array","items":{"type":"integer"}}},"required":["settings","rows"],"additionalProperties":false});
    let data = json!({"settings":{"model":"private-result-sentinel","token":null},"rows":[1,2,3]});
    let mut definition = result_definition(schema.clone(), json!([]));
    definition["actions"][0]["run"][0]["params"]["mode"] = json!("persist");
    let binding = write_result_contract(&f, &definition, &schema);
    let request = result_request(binding, data.clone());
    for format in ["json", "ndjson"] {
        for include in [false, true] {
            let (value, raw) = invoke_result(&f, &request, format, include, &[]);
            assert_eq!(value["outcome"], "succeeded", "{value}");
            assert_eq!(value["data"]["result"]["availability"], "available");
            assert_eq!(raw.contains("private-result-sentinel"), include);
            if include {
                assert_eq!(value["data"]["result"]["content"], data);
            }
            if format == "ndjson" {
                for line in raw.lines().take(raw.lines().count() - 1) {
                    assert!(!line.contains("private-result-sentinel"));
                }
            }
        }
    }
    let mut old = request.clone();
    old["schema_version"] = json!(1);
    assert_eq!(
        invoke_result(&f, &old, "json", true, &[]).0["error"]["code"],
        "action.unsupported_contract"
    );

    fs::create_dir_all(f.root.join(".cargo-ai/data/reports")).unwrap();
    let report_bytes = b"private-report-bytes\n";
    fs::write(
        f.root.join(".cargo-ai/data/reports/output.txt"),
        report_bytes,
    )
    .unwrap();
    let report_json = br#"{"count":3,"status":"saved"}"#;
    fs::write(
        f.root.join(".cargo-ai/data/reports/output.json"),
        report_json,
    )
    .unwrap();
    definition["result"]["artifact_scopes"] = json!(["reports"]);
    definition["actions"][0]["run"][0]["params"]["artifacts"] = json!(
        json!([{"id":"report","scope":"reports","path":"output.txt","mime_type":"text/plain"},{"id":"summary","scope":"reports","path":"output.json","mime_type":"application/json"}])
            .to_string()
    );
    let binding = write_result_contract(&f, &definition, &schema);
    let mut request = result_request(binding.clone(), data.clone());
    assert_eq!(
        invoke_result(&f, &request, "json", true, &[]).0["error"]["code"],
        "artifact.access_denied"
    );
    request["artifact_access"] = json!({"version":1,"scopes":["reports"]});
    let (exported, _) = invoke_result(&f, &request, "json", true, &[]);
    assert_eq!(exported["outcome"], "succeeded", "{exported}");
    let result = &exported["data"]["result"];
    assert_eq!(result["content"], data);
    let descriptor = &result["artifact_references"][0];
    let grant = &result["artifact_read_grants"][0];
    assert!(descriptor.get("relative_path").is_none());
    let read_request = json!({"schema_version":1,"interface":"report","expected_binding":binding,"reference":descriptor["reference"],"read_grant":grant});
    let read = |request: &Value| {
        envelope(execute(
            &f,
            &[
                "actions",
                "artifact",
                "--project",
                f.root.to_str().unwrap(),
                "--interface",
                "report",
                "--request-stdin",
            ],
            Some(request),
        ))
    };
    let bytes = read(&read_request);
    assert_eq!(bytes["outcome"], "succeeded", "{bytes}");
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(bytes["data"]["data"].as_str().unwrap())
            .unwrap(),
        report_bytes
    );
    let json_read_request = json!({"schema_version":1,"interface":"report","expected_binding":binding,"reference":result["artifact_references"][1]["reference"],"read_grant":result["artifact_read_grants"][1]});
    let json_response = read(&json_read_request);
    assert_eq!(json_response["outcome"], "succeeded", "{json_response}");
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(json_response["data"]["data"].as_str().unwrap())
            .unwrap(),
        report_json
    );
    assert_eq!(catalog(&f)["binding"], binding);
    let (_, hidden) = invoke_result(&f, &request, "ndjson", false, &[]);
    assert!(!hidden.contains("artifact_read_grants"));
    assert!(!hidden.contains("output.txt"));
    assert!(!hidden.contains("private-result-sentinel"));
    let mut tampered = read_request.clone();
    tampered["read_grant"]["relative_path"] = json!("other.txt");
    assert_ne!(read(&tampered)["outcome"], "succeeded");
    fs::write(f.root.join(".cargo-ai/data/reports/output.txt"), "changed").unwrap();
    assert_ne!(read(&read_request)["outcome"], "succeeded");
    fs::remove_file(f.root.join(".cargo-ai/data/reports/output.txt")).unwrap();
    let (failed, _) = invoke_result(&f, &request, "json", true, &[]);
    assert_eq!(failed["outcome"], "partial", "{failed}");
    assert_eq!(failed["error"]["code"], "artifact.export_failed");
    assert_eq!(failed["data"]["result"]["content"], data);
    assert!(failed["data"]["result"]
        .get("artifact_read_grants")
        .is_none());

    // A separate saved-state/media workspace reuses the protocol without report-specific fields.
    let media = fixture();
    configure(&media);
    support::copy_tree(
        &f.root.join(".cargo-ai/tools"),
        &media.root.join(".cargo-ai/tools"),
    );
    support::copy_tree(&f.root.join("tools"), &media.root.join("tools"));
    fs::write(
        media.root.join(".cargo-ai/project.toml"),
        "[project]\nname='media-fixture'\n[runtime]\ndata_root='.cargo-ai/data'\n",
    )
    .unwrap();
    fs::create_dir_all(media.root.join(".cargo-ai/data/reports")).unwrap();
    let png=base64::engine::general_purpose::STANDARD.decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVR4nGNgYGAAAAAEAAH2FzhVAAAAAElFTkSuQmCC").unwrap();
    let mut wav = b"RIFF".to_vec();
    wav.extend(40u32.to_le_bytes());
    wav.extend(b"WAVEfmt ");
    wav.extend(16u32.to_le_bytes());
    wav.extend([1, 0, 1, 0]);
    wav.extend(8000u32.to_le_bytes());
    wav.extend(16000u32.to_le_bytes());
    wav.extend([2, 0, 16, 0]);
    wav.extend(b"data");
    wav.extend(4u32.to_le_bytes());
    wav.extend([0, 0, 0, 0]);
    fs::write(media.root.join(".cargo-ai/data/reports/frame.png"), &png).unwrap();
    fs::write(
        media.root.join(".cargo-ai/data/reports/narration.wav"),
        &wav,
    )
    .unwrap();
    let media_schema = json!({"type":"object","properties":{"version":{"type":"integer"},"panels":{"type":"array","items":{"type":"object","properties":{"id":{"type":"string"},"narration":{"type":"string"}},"required":["id","narration"],"additionalProperties":false}}},"required":["version","panels"],"additionalProperties":false});
    let state = json!({"version":2,"panels":[{"id":"panel-a","narration":"private-saved-state"},{"id":"panel-b","narration":"second saved panel"}]});
    // Save/load do not request previews; their success is independent of artifact permission.
    let mut media_definition = result_definition(media_schema.clone(), json!([]));
    media_definition["actions"][0]["run"][0]["params"]["mode"] = json!("save");
    let media_binding = write_result_contract(&media, &media_definition, &media_schema);
    let mut initial = state.clone();
    initial["version"] = json!(1);
    let saved = invoke_result(
        &media,
        &result_request(media_binding.clone(), initial.clone()),
        "json",
        true,
        &[],
    )
    .0;
    assert_eq!(saved["outcome"], "succeeded", "{saved}");
    assert_eq!(saved["data"]["result"]["content"], state);
    assert_eq!(
        serde_json::from_slice::<Value>(
            &fs::read(media.root.join(".cargo-ai/data/saved-state.json")).unwrap()
        )
        .unwrap(),
        state
    );
    let conflict = invoke_result(
        &media,
        &result_request(media_binding, initial),
        "json",
        true,
        &[],
    )
    .0;
    assert_ne!(conflict["outcome"], "succeeded");
    assert_eq!(
        serde_json::from_slice::<Value>(
            &fs::read(media.root.join(".cargo-ai/data/saved-state.json")).unwrap()
        )
        .unwrap(),
        state
    );
    media_definition["actions"][0]["run"][0]["params"]["mode"] = json!("search");
    let search_schema = json!({"type":"object","properties":{"query":{"type":"string"},"offset":{"type":"integer","minimum":0},"limit":{"type":"integer","minimum":1,"maximum":10}},"required":["query","offset","limit"],"additionalProperties":false});
    let search_binding = write_result_contract(&media, &media_definition, &search_schema);
    for query in [
        json!({"query":"panel","offset":1,"limit":1}),
        json!({"query":"panel-b","offset":0,"limit":1}),
    ] {
        let found = invoke_result(
            &media,
            &result_request(search_binding.clone(), query),
            "json",
            true,
            &[],
        )
        .0;
        assert_eq!(found["outcome"], "succeeded", "{found}");
        assert_eq!(
            found["data"]["result"]["content"],
            json!({"version":2,"panels":[state["panels"][1]]})
        );
    }
    media_definition["actions"][0]["run"][0]["params"]["mode"] = json!("load");
    media_definition["result"]["artifact_scopes"] = json!(["reports"]);
    media_definition["actions"][0]["run"][0]["params"]["artifacts"] = json!(json!([
        {"id":"image","scope":"reports","path":"frame.png","mime_type":"image/png"},
        {"id":"audio","scope":"reports","path":"narration.wav","mime_type":"audio/wav"}
    ])
    .to_string());
    let binding = write_result_contract(&media, &media_definition, &media_schema);
    let mut preview_request = result_request(binding.clone(), state.clone());
    preview_request["artifact_access"] = json!({"version":1,"scopes":["reports"]});
    let exported = invoke_result(&media, &preview_request, "json", true, &[]).0;
    assert_eq!(exported["outcome"], "succeeded", "{exported}");
    for (index, expected) in [png, wav].iter().enumerate() {
        let result = &exported["data"]["result"];
        let read_request = json!({"schema_version":1,"interface":"report","expected_binding":binding,"reference":result["artifact_references"][index]["reference"],"read_grant":result["artifact_read_grants"][index]});
        let response = envelope(execute(
            &media,
            &[
                "actions",
                "artifact",
                "--project",
                media.root.to_str().unwrap(),
                "--interface",
                "report",
                "--request-stdin",
            ],
            Some(&read_request),
        ));
        assert_eq!(response["outcome"], "succeeded", "{response}");
        assert_eq!(
            &base64::engine::general_purpose::STANDARD
                .decode(response["data"]["data"].as_str().unwrap())
                .unwrap(),
            expected
        );
    }

    // Inference drives actions, but only the selected tool supplies terminal business data.
    definition = result_definition(schema.clone(), json!([]));
    definition["agent_schema"] = json!({"type":"object","properties":{"answer":{"type":"string"}}});
    definition["inputs"] = json!([{"type":"text","text":"fixture"}]);
    let binding = write_result_contract(&f, &definition, &schema);
    let request = result_request(binding, data.clone());
    let server = support::OneShotHttpServer::json(
        "/v1/chat/completions",
        json!({"choices":[{"message":{"role":"assistant","content":json!({"answer":"private-inference-only"}).to_string()}}]}),
    );
    let config = f.cargo_ai_home.join("config.toml");
    let mut profiles: toml::Value = toml::from_str(&fs::read_to_string(&config).unwrap()).unwrap();
    profiles["profile"][0]
        .as_table_mut()
        .unwrap()
        .insert("url".into(), toml::Value::String(server.url.clone()));
    fs::write(&config, toml::to_string(&profiles).unwrap()).unwrap();
    let (model_result, raw) = invoke_result(&f, &request, "json", true, &[]);
    assert_eq!(model_result["outcome"], "succeeded", "{model_result}");
    assert_eq!(model_result["data"]["result"]["content"], data);
    assert!(!raw.contains("private-inference-only"));
    assert!(server.finish().starts_with("POST /v1/chat/completions"));
    configure(&f);

    // A later failure cannot erase known business state or turn the operation into success.
    definition = result_definition(schema.clone(), json!([]));
    let mut failing_step = definition["actions"][0]["run"][0].clone();
    failing_step
        .as_object_mut()
        .unwrap()
        .remove("produces_result");
    failing_step["params"]["mode"] = json!("fail");
    definition["actions"][0]["run"]
        .as_array_mut()
        .unwrap()
        .push(failing_step);
    let binding = write_result_contract(&f, &definition, &schema);
    let (failed, _) = invoke_result(
        &f,
        &result_request(binding, data.clone()),
        "json",
        true,
        &[],
    );
    assert_eq!(failed["outcome"], "failed", "{failed}");
    assert_eq!(failed["data"]["result"]["content"], data);

    #[cfg(unix)]
    {
        use std::time::{Duration, Instant};
        let mut definition = result_definition(schema.clone(), json!([]));
        definition["actions"][0]["run"].as_array_mut().unwrap().push(json!({"kind":"exec","program":"sh","args":["-c","echo ready > cancel.ready; sleep 20; echo late > cancel.late"]}));
        let binding = write_result_contract(&f, &definition, &schema);
        let request = result_request(binding, data.clone());
        let mut child = f
            .cargo_ai_command(&f.root)
            .args([
                "run",
                "--project",
                f.root.to_str().unwrap(),
                "--interface",
                "report",
                "--action",
                "save",
                "--action-request-stdin",
                "--profile",
                "fixture",
                "--output-format",
                "ndjson",
                "--include-result-content",
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
        let deadline = Instant::now() + Duration::from_secs(10);
        while !f.root.join("cancel.ready").exists() {
            assert!(
                child.try_wait().unwrap().is_none(),
                "producer exited before cancellation fixture started"
            );
            if Instant::now() > deadline {
                let _ = child.kill();
                panic!("cancel fixture did not start");
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
                panic!("cancel did not settle");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let output = child.wait_with_output().unwrap();
        assert_eq!(output.status.code(), Some(130));
        assert!(output.stderr.is_empty());
        let raw = String::from_utf8(output.stdout).unwrap();
        let terminal: Value = serde_json::from_str(raw.lines().last().unwrap()).unwrap();
        assert_eq!(terminal["data"]["outcome"], "canceled");
        assert_eq!(terminal["data"]["data"]["result"]["content"], data);
        assert!(!f.root.join("cancel.late").exists());
        let (timeout, _) =
            invoke_result(&f, &request, "json", true, &["--max-runtime-in-sec", "1"]);
        assert_eq!(timeout["error"]["code"], "runtime.timeout", "{timeout}");
        assert_eq!(timeout["data"]["result"]["content"], data);
    }

    for (schema, value, mode, expected) in [
        (json!({"type":"null"}), Value::Null, "data", None),
        (json!({"type":"integer"}), json!(42), "data", None),
        (json!({"type":"number"}), json!(2.5), "data", None),
        (json!({"type":"boolean"}), json!(true), "data", None),
        (json!({"type":"string"}), json!("saved"), "data", None),
        (
            json!({"type":"array","items":{"type":"integer"}}),
            json!([1, 2]),
            "data",
            None,
        ),
        (
            json!({"type":"null"}),
            Value::Null,
            "missing",
            Some("runtime.result_missing"),
        ),
        (
            json!({"type":"null"}),
            Value::Null,
            "literal",
            Some("runtime.result_invalid"),
        ),
    ] {
        let mut definition = result_definition(schema.clone(), json!([]));
        definition["actions"][0]["run"][0]["params"]["mode"] = json!(mode);
        let binding = write_result_contract(&f, &definition, &schema);
        let (result, _) = invoke_result(
            &f,
            &result_request(binding, value.clone()),
            "json",
            true,
            &[],
        );
        if let Some(code) = expected {
            assert_eq!(result["error"]["code"], code, "{result}");
        } else {
            assert_eq!(result["outcome"], "succeeded", "{result}");
            assert_eq!(result["data"]["result"]["availability"], "available");
            assert_eq!(result["data"]["result"]["content"], value);
        }
    }
    for (kind, workspace) in [("report", &f), ("media", &media)] {
        stable_business_journey(workspace, kind);
    }

    fn no_private_state(path: &std::path::Path) {
        for entry in fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            let metadata = entry.file_type().unwrap();
            assert!(!metadata.is_symlink());
            if metadata.is_dir() {
                no_private_state(&entry.path());
            } else {
                let bytes = fs::read(entry.path()).unwrap();
                for private in [
                    "private-result-sentinel",
                    "private-saved-state",
                    "private-report-bytes",
                    "private-inference-only",
                    "private-journey-business",
                ] {
                    assert!(!bytes.windows(private.len()).any(|value|value==private.as_bytes()),"Private business content persisted in configuration, usage or backup state: {}",entry.path().display());
                }
            }
        }
    }
    no_private_state(&f.cargo_ai_home);
    no_private_state(&media.cargo_ai_home);
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

fn closed(properties: Value, required: &[&str]) -> Value {
    json!({"type":"object","properties":properties,"required":required,"additionalProperties":false})
}
fn journey_request(
    binding: &Value,
    kind: &str,
    action: &str,
    inputs: Value,
    consent: bool,
) -> Value {
    let mut value = request(binding, action, &[]);
    value["interface"] = json!(kind);
    value["inputs"] = inputs;
    if consent {
        value["artifact_access"] = json!({"version":1,"scopes":["exports"]});
    }
    value
}
fn journey_read(f: &Fixture, kind: &str, binding: &Value, result: &Value, index: usize) -> Value {
    let request = json!({"schema_version":1,"interface":kind,"expected_binding":binding,"reference":result["artifact_references"][index]["reference"],"read_grant":result["artifact_read_grants"][index]});
    envelope(execute(
        f,
        &[
            "actions",
            "artifact",
            "--project",
            f.root.to_str().unwrap(),
            "--interface",
            kind,
            "--request-stdin",
        ],
        Some(&request),
    ))
}
fn stable_business_journey(f: &Fixture, kind: &str) {
    use base64::Engine;
    use sha2::{Digest, Sha256};
    let item_key = if kind == "report" {
        "sections"
    } else {
        "panels"
    };
    let text_key = if kind == "report" {
        "text"
    } else {
        "narration"
    };
    let settings = closed(
        json!({"model":{"type":"string"},"profile":{"type":["string","null"]},"config":closed(json!({"language":{"type":"string"},"note":{"type":["string","null"]}}),&["language","note"]),"token":{"type":["string","null"]}}),
        &["model", "profile", "config", "token"],
    );
    let item = closed(
        json!({"id":{"type":"string"},text_key:{"type":"string"},"state":{"type":"string","enum":["pending","complete","failed"]}}),
        &["id", text_key, "state"],
    );
    let editable = closed(
        json!({"id":{"type":"string"},"title":{"type":"string"},"settings":settings,item_key:{"type":"array","items":item,"maxItems":8}}),
        &["id", "title", "settings", item_key],
    );
    let representation = closed(
        json!({"id":{"type":"string"},"status":{"type":"string"},"mime_type":{"type":"string"}}),
        &["id", "status", "mime_type"],
    );
    let mut record = editable.clone();
    record["properties"]["version"] = json!({"type":"string"});
    record["properties"]["representations"] =
        json!({"type":"array","items":representation,"maxItems":8});
    record["required"]
        .as_array_mut()
        .unwrap()
        .extend([json!("version"), json!("representations")]);
    let mut selected = record.clone();
    selected["type"] = json!(["object", "null"]);
    let run_item = closed(
        json!({"id":{"type":"string"},"state":{"type":"string"}}),
        &["id", "state"],
    );
    let mut run = closed(
        json!({"id":{"type":"string"},"snapshot_version":{"type":"string"},"items":{"type":"array","items":run_item,"maxItems":8}}),
        &["id", "snapshot_version", "items"],
    );
    run["type"] = json!(["object", "null"]);
    let result_schema = closed(
        json!({"status":{"type":"string","enum":["loaded","searched","selected","saved","conflict","partial","preview","missing","unsupported"]},"records":{"type":"array","items":record,"maxItems":8},"selected":selected,"current_version":{"type":"string"},"total":{"type":"integer"},"run":run}),
        &[
            "status",
            "records",
            "selected",
            "current_version",
            "total",
            "run",
        ],
    );
    let declarations = [
        ("load", closed(json!({}), &[])),
        (
            "search",
            closed(
                json!({"document":closed(json!({"query":{"type":"string"},"offset":{"type":"integer","minimum":0},"limit":{"type":"integer","minimum":1,"maximum":8}}),&["query","offset","limit"])}),
                &["document"],
            ),
        ),
        (
            "select",
            closed(
                json!({"document":closed(json!({"id":{"type":"string"}}),&["id"])}),
                &["document"],
            ),
        ),
        (
            "save",
            closed(
                json!({"data":editable,"expected_version":{"type":"string"}}),
                &["data", "expected_version"],
            ),
        ),
        (
            "work",
            closed(
                json!({"document":closed(json!({"id":{"type":"string"},"ids":{"type":"array","items":{"type":"string"},"minItems":2,"maxItems":8}}),&["id","ids"]),"expected_version":{"type":"string"}}),
                &["document", "expected_version"],
            ),
        ),
        (
            "preview",
            closed(
                json!({"document":closed(json!({"id":{"type":"string"},"representation":{"type":"string","enum":["valid","secondary","missing","pdf","race","stale","generated","generated_secondary"]}}),&["id","representation"]),"expected_version":{"type":"string"}}),
                &["document", "expected_version"],
            ),
        ),
        (
            "required",
            closed(
                json!({"document":closed(json!({"id":{"type":"string"}}),&["id"]),"expected_version":{"type":"string"}}),
                &["document", "expected_version"],
            ),
        ),
    ];
    let mut actions = Vec::new();
    for (operation, input_schema) in declarations {
        let mut definition = result_definition(
            result_schema.clone(),
            if matches!(operation, "preview" | "required") {
                json!(["exports"])
            } else {
                json!([])
            },
        );
        definition["runtime_vars"]["business_json"]["default"] = json!("{}");
        definition["runtime_vars"]["expected_version"] = json!({"type":"string","default":""});
        definition["actions"][0]["run"][0]["params"]["mode"] =
            json!(format!("journey_{kind}_{operation}"));
        definition["actions"][0]["run"][0]["params"]["expected_version"] =
            json!({"var":"runtime.expected_version"});
        let target = format!("journey-{operation}.json");
        fs::write(f.root.join(&target), definition.to_string()).unwrap();
        let mut mappings = json!({});
        if operation != "load" {
            mappings[if operation == "save" {
                "data"
            } else {
                "document"
            }] = json!({"runtime_var":"business_json","encoding":"json"});
        }
        if matches!(operation, "save" | "work" | "preview" | "required") {
            mappings["expected_version"] = json!({"runtime_var":"expected_version"});
        }
        actions.push(json!({"id":operation,"target":target,"input_schema":input_schema,"mappings":mappings,"required_capabilities":["business_inputs.v1","structured_results.v1"]}));
    }
    let catalog_document = json!({"schema_version":2,"actions":actions,"artifact_scopes":[{"id":"exports","path":"exports","mime_types":["text/plain","application/json","image/png","audio/wav"]}],"interfaces":[{"id":kind,"actions":["load","search","select","save","work","preview","required"],"artifact_scopes":["exports"],"resources":["page","script"],"presentation":{"entrypoint":"page"}}],"resources":[{"id":"page","path":"journey.html","mime_type":"text/html"},{"id":"script","path":"journey.js","mime_type":"text/javascript"}]});
    fs::write(
        f.root.join("journey.html"),
        "<!doctype html><title>Fixture</title><script src='journey.js'></script>",
    )
    .unwrap();
    fs::write(f.root.join("journey.js"), "'use strict';").unwrap();
    fs::write(
        f.root.join("cargo-ai-actions.json"),
        catalog_document.to_string(),
    )
    .unwrap();
    let data_root = f.root.join(".cargo-ai/data");
    let exports = data_root.join("exports");
    fs::create_dir_all(&exports).unwrap();
    let png=base64::engine::general_purpose::STANDARD.decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVR4nGNgYGAAAAAEAAH2FzhVAAAAAElFTkSuQmCC").unwrap();
    let mut wav = b"RIFF".to_vec();
    wav.extend(40u32.to_le_bytes());
    wav.extend(b"WAVEfmt ");
    wav.extend(16u32.to_le_bytes());
    wav.extend([1, 0, 1, 0]);
    wav.extend(8000u32.to_le_bytes());
    wav.extend(16000u32.to_le_bytes());
    wav.extend([2, 0, 16, 0]);
    wav.extend(b"data");
    wav.extend(4u32.to_le_bytes());
    wav.extend([0, 0, 0, 0]);
    for (path, bytes) in [
        (
            "existing.txt",
            b"private-journey-business existing text".as_slice(),
        ),
        (
            "existing.json",
            br#"{"version":"7","note":null}"#.as_slice(),
        ),
        ("existing.png", png.as_slice()),
        ("existing.wav", wav.as_slice()),
        ("stale.txt", b"earlier report revision".as_slice()),
        ("stale.wav", wav.as_slice()),
    ] {
        fs::write(exports.join(path), bytes).unwrap();
    }
    let mime = if kind == "report" {
        "text/plain"
    } else {
        "image/png"
    };
    let records=(0..3).map(|index|json!({"id":format!("{kind}-record-{index}"),"version":"7","title":format!("{kind} draft {index}"),"settings":{"model":"private-journey-business","profile":null,"config":{"language":"en","note":null},"token":null},item_key:(0..3).map(|item|json!({"id":format!("{kind}-item-{item}"),text_key:format!("private-journey-business {item}"),"state":"pending"})).collect::<Vec<_>>(),"representations":[{"id":"valid","status":"available","mime_type":mime},{"id":"missing","status":"missing","mime_type":mime},{"id":"pdf","status":"unsupported","mime_type":"application/pdf"},{"id":"stale","status":"available","mime_type":if kind=="report" {"text/plain"} else {"audio/wav"}}]})).collect::<Vec<_>>();
    fs::write(
        data_root.join("journey-state.json"),
        json!({"records":records}).to_string(),
    )
    .unwrap();
    let binding = catalog(f)["binding"].clone();
    let policy_before = fs::read(f.cargo_ai_home.join("config.toml")).unwrap();
    let invoke = |action: &str, inputs: Value, consent: bool, format: &str, include: bool| {
        let req = journey_request(&binding, kind, action, inputs, consent);
        let result = invoke_result(f, &req, format, include, &[]);
        assert_eq!(
            catalog(f)["binding"],
            binding,
            "Business activity cannot invalidate the fixed presentation"
        );
        result
    };
    let (loaded, _) = invoke("load", json!({}), false, "json", true);
    assert_eq!(loaded["outcome"], "succeeded");
    assert_eq!(
        loaded["data"]["result"]["content"]["records"],
        json!(records)
    );
    assert_eq!(
        loaded["data"]["result"]["content"]["selected"]["id"],
        records[0]["id"]
    );
    assert!(loaded["data"]["result"]
        .get("artifact_references")
        .is_none());
    let (hidden, raw) = invoke("load", json!({}), false, "ndjson", false);
    assert_eq!(hidden["data"]["result"]["availability"], "available");
    assert!(!raw.contains("private-journey-business"));
    for (query, offset, expected) in [
        (kind.to_string(), 1, records[1].clone()),
        (format!("{kind}-record-2"), 0, records[2].clone()),
    ] {
        let (searched, _) = invoke(
            "search",
            json!({"document":{"query":query,"offset":offset,"limit":1}}),
            false,
            "ndjson",
            true,
        );
        assert_eq!(
            searched["data"]["result"]["content"]["records"],
            json!([expected])
        );
    }
    let id = records[1]["id"].clone();
    let (selected, _) = invoke("select", json!({"document":{"id":id}}), false, "json", true);
    assert_eq!(
        selected["data"]["result"]["content"]["selected"],
        records[1]
    );
    let preview_inputs = |representation: &str, version: &str| json!({"document":{"id":id,"representation":representation},"expected_version":version});
    let (good, _) = invoke("preview", preview_inputs("valid", "7"), true, "json", true);
    assert_eq!(good["outcome"], "succeeded", "{good}");
    let good_result = good["data"]["result"].clone();
    assert_eq!(
        good_result["artifact_references"].as_array().unwrap().len(),
        1
    );
    let good_bytes = fs::read(exports.join(if kind == "report" {
        "existing.txt"
    } else {
        "existing.png"
    }))
    .unwrap();
    let assert_good = || {
        let read = journey_read(f, kind, &binding, &good_result, 0);
        assert_eq!(read["outcome"], "succeeded", "{read}");
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(read["data"]["data"].as_str().unwrap())
                .unwrap(),
            good_bytes
        );
        assert_eq!(
            read["data"]["content_sha256"],
            format!("{:x}", Sha256::digest(&good_bytes))
        );
    };
    for (representation, status) in [("missing", "missing"), ("pdf", "unsupported")] {
        let (unavailable, _) = invoke(
            "preview",
            preview_inputs(representation, "7"),
            true,
            "json",
            true,
        );
        assert_eq!(unavailable["outcome"], "succeeded");
        assert_eq!(unavailable["data"]["result"]["content"]["status"], status);
        assert!(unavailable["data"]["result"]
            .get("artifact_read_grants")
            .is_none());
        assert_good();
    }
    let (race, _) = invoke("preview", preview_inputs("race", "7"), true, "json", true);
    assert_eq!(race["error"]["code"], "artifact.export_failed");
    assert_eq!(race["outcome"], "partial");
    assert!(race["data"]["result"].get("artifact_read_grants").is_none());
    assert_good();
    let calls_before = fs::read(data_root.join("journey-call-count.txt")).unwrap();
    let (denied, _) = invoke("preview", preview_inputs("valid", "7"), false, "json", true);
    assert_eq!(denied["error"]["code"], "artifact.access_denied");
    assert_eq!(
        fs::read(data_root.join("journey-call-count.txt")).unwrap(),
        calls_before
    );
    assert_good();
    let mut edit = records[1].clone();
    edit.as_object_mut().unwrap().remove("version");
    edit.as_object_mut().unwrap().remove("representations");
    edit["title"] = json!("edited draft");
    edit["settings"]["profile"] = json!("private-journey-business profile");
    edit["settings"]["config"]["language"] = json!("fr");
    let (saved, _) = invoke(
        "save",
        json!({"data":edit,"expected_version":"7"}),
        false,
        "ndjson",
        true,
    );
    assert_eq!(saved["outcome"], "succeeded", "{saved}");
    let saved_record = saved["data"]["result"]["content"]["selected"].clone();
    assert_eq!(saved_record["version"], "8");
    assert_eq!(saved_record["settings"], edit["settings"]);
    assert_eq!(saved_record[item_key], edit[item_key]);
    assert!(saved["data"]["result"]
        .get("artifact_read_grants")
        .is_none());
    assert_good();
    let saved_bytes = fs::read(data_root.join("journey-state.json")).unwrap();
    let (conflict, _) = invoke(
        "save",
        json!({"data":edit,"expected_version":"7"}),
        false,
        "json",
        true,
    );
    assert_eq!(
        conflict["outcome"], "succeeded",
        "Successful transport cannot turn a business conflict into a committed save"
    );
    assert_eq!(conflict["data"]["result"]["content"]["status"], "conflict");
    assert_eq!(
        conflict["data"]["result"]["content"]["current_version"],
        "8"
    );
    assert_eq!(
        conflict["data"]["result"]["content"]["selected"],
        saved_record
    );
    assert_eq!(
        fs::read(data_root.join("journey-state.json")).unwrap(),
        saved_bytes
    );
    let (reload, _) = invoke("select", json!({"document":{"id":id}}), false, "json", true);
    assert_eq!(
        reload["data"]["result"]["content"]["selected"],
        saved_record
    );
    edit["title"] = json!("deliberately reapplied draft");
    let (reapplied, _) = invoke(
        "save",
        json!({"data":edit,"expected_version":"8"}),
        false,
        "json",
        true,
    );
    assert_eq!(
        reapplied["data"]["result"]["content"]["selected"]["version"],
        "9"
    );
    let (partial, _) = invoke(
        "work",
        json!({"document":{"id":id,"ids":[format!("{kind}-item-0"),format!("{kind}-item-1")]},"expected_version":"9"}),
        false,
        "ndjson",
        true,
    );
    let partial_data = &partial["data"]["result"]["content"];
    assert_eq!(partial["outcome"], "succeeded");
    assert_eq!(partial_data["status"], "partial");
    assert_eq!(partial_data["run"]["snapshot_version"], "9");
    assert_eq!(
        partial_data["run"]["items"],
        json!([{"id":format!("{kind}-item-0"),"state":"complete"},{"id":format!("{kind}-item-1"),"state":"failed"}])
    );
    assert_eq!(partial_data["selected"]["version"], "10");
    assert_eq!(partial_data["selected"][item_key][2]["state"], "pending");
    let persisted: Value =
        serde_json::from_slice(&fs::read(data_root.join("journey-state.json")).unwrap()).unwrap();
    assert_eq!(persisted["records"][1], partial_data["selected"]);
    for (representation, path) in [
        (
            "secondary",
            if kind == "report" {
                "existing.json"
            } else {
                "existing.wav"
            },
        ),
        (
            "generated",
            if kind == "report" {
                "generated.txt"
            } else {
                "generated.png"
            },
        ),
        (
            "generated_secondary",
            if kind == "report" {
                "generated.json"
            } else {
                "generated.wav"
            },
        ),
    ] {
        let (exported, _) = invoke(
            "preview",
            preview_inputs(representation, "10"),
            true,
            "json",
            true,
        );
        assert_eq!(exported["outcome"], "succeeded", "{exported}");
        let read = journey_read(f, kind, &binding, &exported["data"]["result"], 0);
        assert_eq!(read["outcome"], "succeeded", "{read}");
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(read["data"]["data"].as_str().unwrap())
                .unwrap(),
            fs::read(exports.join(path)).unwrap()
        );
    }
    let (stale, _) = invoke("preview", preview_inputs("stale", "10"), true, "json", true);
    assert_eq!(stale["outcome"], "succeeded");
    let stale_result = stale["data"]["result"].clone();
    let stale_path = exports.join(if kind == "report" {
        "stale.txt"
    } else {
        "stale.wav"
    });
    let mut changed = if kind == "report" {
        b"changed report revision".to_vec()
    } else {
        wav.clone()
    };
    if kind == "media" {
        *changed.last_mut().unwrap() = 1;
    }
    fs::write(&stale_path, &changed).unwrap();
    let calls_before = fs::read(data_root.join("journey-call-count.txt")).unwrap();
    let state_before = fs::read(data_root.join("journey-state.json")).unwrap();
    assert_eq!(
        journey_read(f, kind, &binding, &stale_result, 0)["error"]["code"],
        "artifact.content_changed"
    );
    fs::remove_file(&stale_path).unwrap();
    assert_eq!(
        journey_read(f, kind, &binding, &stale_result, 0)["error"]["code"],
        "artifact.not_found"
    );
    assert_good();
    assert_eq!(
        fs::read(data_root.join("journey-call-count.txt")).unwrap(),
        calls_before
    );
    assert_eq!(
        fs::read(data_root.join("journey-state.json")).unwrap(),
        state_before
    );
    let (required, _) = invoke(
        "required",
        json!({"document":{"id":id},"expected_version":"10"}),
        true,
        "json",
        true,
    );
    assert_eq!(required["outcome"], "partial");
    assert_eq!(required["error"]["code"], "artifact.export_failed");
    assert_eq!(
        required["data"]["result"]["content"]["selected"]["version"],
        "10"
    );
    assert!(required["data"]["result"]
        .get("artifact_references")
        .is_none());
    assert!(required["data"]["result"]
        .get("artifact_read_grants")
        .is_none());
    assert_good();
    assert_eq!(
        fs::read(data_root.join("journey-state.json")).unwrap(),
        state_before
    );
    fs::write(&stale_path, &changed).unwrap();
    let (current, _) = invoke("preview", preview_inputs("stale", "10"), true, "json", true);
    assert_ne!(
        current["data"]["result"]["artifact_references"][0]["reference"],
        stale_result["artifact_references"][0]["reference"]
    );
    let read = journey_read(f, kind, &binding, &current["data"]["result"], 0);
    assert_eq!(read["outcome"], "succeeded");
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(read["data"]["data"].as_str().unwrap())
            .unwrap(),
        changed
    );
    assert_eq!(
        fs::read(f.cargo_ai_home.join("config.toml")).unwrap(),
        policy_before
    );
    for name in [
        "journey.html",
        "journey.js",
        "journey-load.json",
        "cargo-ai-actions.json",
    ] {
        let path = f.root.join(name);
        let calls_before = fs::read(data_root.join("journey-call-count.txt")).unwrap();
        let original = fs::read(&path).unwrap();
        let changed = match name {
            "journey-load.json" => {
                let mut value: Value = serde_json::from_slice(&original).unwrap();
                value["actions"][0]["name"] = json!("changed executable definition");
                serde_json::to_vec(&value).unwrap()
            }
            "cargo-ai-actions.json" => {
                let mut value: Value = serde_json::from_slice(&original).unwrap();
                value["artifact_scopes"][0]["path"] = json!("other-exports");
                serde_json::to_vec(&value).unwrap()
            }
            _ => {
                let mut changed = original.clone();
                changed.extend(b"\n/* changed fixture presentation */");
                changed
            }
        };
        fs::write(&path, changed).unwrap();
        assert_ne!(catalog(f)["binding"], binding);
        let old = journey_request(&binding, kind, "load", json!({}), false);
        let rejected = invoke_result(f, &old, "json", true, &[]).0;
        assert_eq!(
            rejected["error"]["code"], "action.stale_binding",
            "{name}: {rejected}"
        );
        let rejected = journey_read(f, kind, &binding, &good_result, 0);
        assert_eq!(
            rejected["error"]["code"], "artifact.stale_binding",
            "{name}: {rejected}"
        );
        assert_eq!(
            fs::read(data_root.join("journey-call-count.txt")).unwrap(),
            calls_before
        );
        assert_eq!(
            fs::read(data_root.join("journey-state.json")).unwrap(),
            state_before
        );
        fs::write(path, original).unwrap();
        assert_eq!(catalog(f)["binding"], binding);
        assert_good();
    }
}
