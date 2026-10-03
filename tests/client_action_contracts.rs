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
        "report",
        "--action",
        "save",
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
