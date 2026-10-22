//! Original repeated-call graph exercised through the actual tool/child/provider path.
use super::*;

fn definitions(fixture: &Fixture, emitted: bool) -> Vec<String> {
    let mut names = Vec::new();
    let write = |name: &str, value: Value, names: &mut Vec<String>| {
        fs::write(fixture.root.join(name), value.to_string()).unwrap();
        names.push(name.to_owned());
    };
    fs::create_dir_all(fixture.root.join(".cargo-ai/tools/bridge/bin")).unwrap();
    fs::create_dir_all(fixture.root.join("tools/bridge/src")).unwrap();
    fs::write(
        fixture.root.join(".cargo-ai/project.toml"),
        "[project]\nname='repeated-evidence-fixture'\n",
    )
    .unwrap();
    fs::write(
        fixture.root.join("pixel.png"),
        BASE64_STANDARD.decode(PNG_B64).unwrap(),
    )
    .unwrap();
    names.push("pixel.png".into());
    fs::write(
        fixture.root.join("tools/bridge/Cargo.toml"),
        "[package]\nname='bridge'\nversion='0.1.0'\nedition='2021'\n[workspace]\n",
    )
    .unwrap();
    let describe = json!({"protocol_version":1,"supported_protocol_versions":[1,2],"name":"bridge","description":"Exercise a declared child","params":{},"result":{"type":"string","nullable":true},"resource_profile":{"network":"none","filesystem_read":"none","filesystem_write":"none","subprocess":"none","env_read":"none","credential_access":"none"},"self_test":{"supported":false,"safe":false},"examples":{"minimal_invoke":{"protocol_version":1,"params":{}},"full_invoke":{"protocol_version":1,"params":{}}}});
    let source = format!(
        r##"use std::io::{{self, BufRead, Write}};
fn main() {{
    if std::env::args().nth(1).as_deref() == Some("describe") {{ println!("{{}}", r#"{describe}"#); return; }}
    let mut input = io::stdin().lock();
    let mut line = String::new(); input.read_line(&mut line).unwrap();
    println!("{{}}", r#"{{"protocol_version":2,"type":"child_request","request_id":"one","call_site":"reader-child","inputs":{{}}}}"#);
    io::stdout().flush().unwrap();
    line.clear(); input.read_line(&mut line).unwrap();
    println!("{{}}", r#"{{"protocol_version":2,"type":"result","result":"done"}}"#);
}}
"##
    );
    fs::write(fixture.root.join("tools/bridge/src/main.rs"), source).unwrap();
    let binary = fixture.root.join(".cargo-ai/tools/bridge/bin/bridge");
    let compiled = Command::new("rustc")
        .args(["--edition=2021"])
        .arg(fixture.root.join("tools/bridge/src/main.rs"))
        .arg("-o")
        .arg(&binary)
        .output()
        .unwrap();
    assert!(
        compiled.status.success(),
        "{}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    assert!(compiled.stderr.is_empty());
    let version = Command::new("rustc").arg("-vV").output().unwrap();
    let version = String::from_utf8(version.stdout).unwrap();
    let target = version
        .lines()
        .find_map(|line| line.strip_prefix("host: "))
        .unwrap();
    write(
        ".cargo-ai/tools/bridge/tool.json",
        json!({"schema_version":1,"tool_id":"bridge","source":{"manifest_path":"tools/bridge/Cargo.toml"},"binary":{"default_name":"bridge"},"artifacts":{target:{"path":"bin/bridge"}}}),
        &mut names,
    );
    names.extend([
        "tools/bridge/Cargo.toml".into(),
        "tools/bridge/src/main.rs".into(),
        ".cargo-ai/tools/bridge/bin/bridge".into(),
    ]);
    let schema = json!({"type":"object","properties":{}});
    let text = json!({"operation":"text_generation","input_modalities":["image","text"],"structured_output":true});
    let image = json!({"operation":"image_generation","input_modalities":["image","text"],"structured_output":false});
    let empty_inputs =
        json!({"type":"object","properties":{},"required":[],"additionalProperties":false});
    let mut calls = Vec::new();
    for (label, count) in [("reader", 1), ("planner", 1), ("reviewer", 14)] {
        let leaf = format!("{label}-leaf.json");
        write(
            &leaf,
            json!({"agent_definition_schema_version":"2026-10-08.r1","inputs":[{"type":"text","text":"Inspect this fixture image"},{"type":"image","path":"./pixel.png"}],"agent_schema":{"type":"object","properties":{"status":{"type":"string"}}},"actions":[]}),
            &mut names,
        );
        let artifact = if emitted && label == "reader" {
            let built = fixture.hatch("evidence-leaf", &leaf);
            let name = built.file_name().unwrap().to_str().unwrap().to_owned();
            fs::copy(built, fixture.root.join(&name)).unwrap();
            names.push(name.clone());
            name
        } else {
            leaf.clone()
        };
        let wrapper = format!("{label}.json");
        let actions: Vec<_> = (0..count).map(|index| json!({"name":format!("forward-{index}"),"logic":{"==":[index,0]},"run":[{"kind":"agent","artifact":format!("./{artifact}")}]})).collect();
        write(
            &wrapper,
            json!({"agent_definition_schema_version":"2026-10-08.r1","agent_schema":schema,"actions":actions}),
            &mut names,
        );
        for index in 0..count {
            let mut call = json!({"id":format!("{label}-forward-{index}"),"locator":{"definition":wrapper,"site":format!("actions.{index}.run.0")},"kind":"child","role":"writer","target":leaf,"requirements":text});
            if artifact != leaf {
                call["artifact"] = json!(artifact);
            }
            calls.push(call);
        }
        calls.push(json!({"id":format!("{label}-root"),"locator":{"definition":leaf,"site":"root"},"kind":"root","role":"writer","requirements":text}));
        let mut launch = json!({"id":format!("{label}-child"),"locator":{"definition":"root.json","site":format!("tools.bridge.{label}")},"kind":"tool_child","target":wrapper,"requirements":{"operation":"text_generation"},"input_schema":empty_inputs});
        if emitted && label == "reader" {
            let built = fixture.hatch("evidence-reader", &wrapper);
            let name = built.file_name().unwrap().to_str().unwrap().to_owned();
            fs::copy(built, fixture.root.join(&name)).unwrap();
            names.push(name.clone());
            launch["artifact"] = json!(name);
        }
        calls.push(launch);
    }
    let actions: Vec<_> = (0..4).map(|index| json!({"name":format!("variant-{index}"),"logic":{"==":[0,1]},"run":[{"kind":"generate_image","prompt":"A fixture image","path":format!("./image-{index}.png")}]})).collect();
    write(
        "image.json",
        json!({"agent_definition_schema_version":"2026-10-08.r1","agent_schema":schema,"actions":actions}),
        &mut names,
    );
    for index in 0..4 {
        calls.push(json!({"id":format!("image-{index}"),"locator":{"definition":"image.json","site":format!("actions.{index}.run.0")},"kind":"image","role":"illustrator","requirements":image}));
    }
    for name in ["root.json", "batch.json"] {
        write(
            name,
            json!({"agent_definition_schema_version":"2026-10-08.r1","agent_schema":schema,"actions":[{"name":"work","logic":{"==":[1,1]},"run":[{"kind":"tool","name":"bridge","params":{}}]}]}),
            &mut names,
        );
    }
    for (id, from, to) in [
        ("image-child", "root.json", "image.json"),
        ("batch-child", "root.json", "batch.json"),
        ("batch-image", "batch.json", "image.json"),
    ] {
        calls.push(json!({"id":id,"locator":{"definition":from,"site":format!("tools.bridge.{id}")},"kind":"tool_child","target":to,"requirements":{"operation":"text_generation"},"input_schema":empty_inputs}));
    }
    assert_eq!(calls.len(), 29);
    let sites: Vec<_> = calls.iter().map(|call| call["id"].clone()).collect();
    let limits = json!({"max_runtime_secs":30,"max_output_tokens":256,"max_agent_depth":4});
    write(
        "cargo-ai-actions.json",
        json!({"schema_version":3,"actions":[{"id":"work","target":"root.json","input_schema":empty_inputs,"mappings":{}}],"interfaces":[{"id":"fixture","actions":["work"]}],"role_registry":{"version":1,"roles":[{"id":"writer","label":"Writer","purpose":"Read and review","requirements":text},{"id":"illustrator","label":"Illustrator","purpose":"Draw","requirements":image}],"call_sites":calls,"contexts":[{"key":{"action":"work","interface":"fixture","mode":"default"},"call_sites":sites,"resources":[],"data_scopes":[],"limits":limits}],"tool_content":{"bridge":["tools/bridge/Cargo.toml","tools/bridge/src/main.rs"]}}}),
        &mut names,
    );
    names
}

fn exercise(emitted: bool) {
    let provider = Provider::new();
    let fixture = Fixture::new(&provider);
    let names = definitions(&fixture, emitted);
    let discovery = machine(&fixture, &["actions", "list", "--project", "."], None);
    let limits = json!({"max_runtime_secs":30,"max_output_tokens":256,"max_agent_depth":4});
    let bindings: Vec<_> = [("writer","gpt-5.2"),("illustrator","gpt-image-2")].into_iter().map(|(role,model)|json!({"role":role,"profile":"a","profile_uuid":fixture.contexts[0]["profile_uuid"],"connection_generation":fixture.contexts[0]["connection_generation"],"provider":"openai","model":model,"settings":{}})).collect();
    let allowed: Vec<_> = [("text","gpt-5.2"),("image","gpt-image-2")].into_iter().map(|(kind,model)|json!({"profile":"a","request_kind":kind,"model":{"kind":"named","value":model},"thinking":{"mode":"provider_default"}})).collect();
    let mut request = json!({"schema_version":4,"interface":"fixture","action":"work","inputs":{},"expected_binding":discovery["binding"],"execution_policy":{"version":1,"allowed":allowed,"limits":limits},"attachment_grants":{},"role_execution":{"mode":"default","binding_revision":{"version":1,"revision":"evidence-fixture","bindings":bindings}}});
    let resolve = |request: &Value| {
        machine(
            &fixture,
            &[
                "actions",
                "resolve",
                "--project",
                ".",
                "--interface",
                "fixture",
                "--action",
                "work",
                "--request-stdin",
            ],
            Some(request),
        )
    };
    let resolution = resolve(&request);
    assert_eq!(resolution["ready"], true);
    let calls = resolution["calls"].as_array().unwrap();
    assert_eq!(calls.len(), 29);
    assert_eq!(
        calls
            .iter()
            .filter(|call| call["evidence"].as_array().unwrap().len() == 1)
            .count(),
        23
    );
    assert_eq!(
        calls
            .iter()
            .filter(|call| call["evidence"].as_array().unwrap().is_empty())
            .count(),
        6
    );
    // Model verbose native operation details with synthetic informational evidence.
    // This changes informational evidence only; native authority identity stays fixed.
    let mut transport_resolution = resolution.clone();
    for call in transport_resolution["calls"].as_array_mut().unwrap() {
        for record in call["evidence"].as_array_mut().unwrap() {
            record["operation_evidence"]["synthetic_details"] = json!("x".repeat(3500));
        }
    }
    let mut legacy = transport_resolution.clone();
    for call in legacy["calls"].as_array_mut().unwrap() {
        if let Some(record) = call["evidence"].as_array().unwrap().first().cloned() {
            let count = if call["call_site"]["role"] == "writer" {
                19
            } else {
                4
            };
            call["evidence"] = json!(vec![record; count]);
        }
    }
    let compact_bytes = serde_json::to_vec(&transport_resolution).unwrap().len();
    let legacy_bytes = serde_json::to_vec(&legacy).unwrap().len();
    assert!(
        compact_bytes * 5 < legacy_bytes,
        "dedup must substantially reduce the verbose native transport resolution"
    );
    request["role_execution"]["resolution_id"] = resolution["identity"].clone();
    let mut content = serde_json::Map::new();
    for name in names {
        let path = fixture.root.join(name);
        content.insert(
            path.to_str().unwrap().into(),
            json!(format!("{:x}", Sha256::digest(fs::read(&path).unwrap()))),
        );
    }
    let bootstrap = json!({"version":2,"resolution":transport_resolution,"bindings":request["role_execution"]["binding_revision"],"policy":request["execution_policy"],"definition":"root.json","package_root":fixture.root,"content_identities":content,"invocation_id":"evidence-fixture","binding_revision":"evidence-fixture","parent_permit_id":"fixture-parent"});
    let frame_bytes=serde_json::to_vec(&json!({"protocol":"cargo_ai_native_role","version":1,"type":"bootstrap","bootstrap":bootstrap})).unwrap().len()+1;
    assert!(frame_bytes < 1024 * 1024);
    let mut legacy_bootstrap = bootstrap.clone();
    legacy_bootstrap["resolution"] = legacy;
    let legacy_frame_bytes = serde_json::to_vec(&json!({"protocol":"cargo_ai_native_role","version":1,"type":"bootstrap","bootstrap":legacy_bootstrap})).unwrap().len()+1;
    assert!(
        legacy_frame_bytes > 1024 * 1024,
        "the original duplication must cross the unchanged frame bound"
    );
    println!("29-call resolution legacy={legacy_bytes} compact={compact_bytes} complete_frame={frame_bytes} legacy_frame={legacy_frame_bytes} emitted={emitted}");
    let public = public_structural_run(&fixture, &fixture.root, &fixture.root, &request);
    assert!(
        public.success,
        "public startup: {} {:?}",
        public.diagnostics, public.frames
    );
    assert_eq!(
        provider.take().len(),
        1,
        "public tool → wrapper → inference must dispatch exactly once"
    );
    let executable = if emitted {
        fixture.hatch("evidence-root", "root.json")
    } else {
        PathBuf::from(CLI)
    };
    let run = invoke(&fixture, &executable, bootstrap.clone(), Control::Allow);
    assert_success(&run);
    let requests = provider.take();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].1["model"], "gpt-5.2");
    assert!(run
        .frames
        .iter()
        .any(|frame| frame["type"] == "admit" && frame["boundary"]["call_site"] == "reader-root"));
    let denied = invoke(
        &fixture,
        &executable,
        bootstrap.clone(),
        Control::DenyAfter(0),
    );
    assert!(denied
        .frames
        .iter()
        .any(|frame| frame["type"] == "result" && !frame["error"].is_null()));
    assert!(provider.take().is_empty());
    let mut changed = request.clone();
    changed["execution_policy"]["allowed"]
        .as_array_mut()
        .unwrap()
        .pop();
    assert_ne!(resolve(&changed)["identity"], resolution["identity"]);
    let mut changed = request.clone();
    changed["role_execution"]["binding_revision"]["bindings"][0]["model"] =
        json!("different-model");
    assert_ne!(resolve(&changed)["identity"], resolution["identity"]);
    let old = bootstrap.clone();
    let credential_path = fixture.home.join("credentials.toml");
    let before = fs::read(&credential_path).unwrap();
    let text = String::from_utf8(before.clone())
        .unwrap()
        .replace("synthetic-private-token-a", "changed-private-token-a");
    fs::write(&credential_path, text).unwrap();
    let drift = invoke(&fixture, &executable, old, Control::Allow);
    assert!(
        !drift.success
            || drift
                .frames
                .iter()
                .any(|frame| frame["type"] == "result" && !frame["error"].is_null())
    );
    assert!(provider.take().is_empty());
    fs::write(credential_path, before).unwrap();
    assert!(fixture.build_warnings.lock().unwrap().is_empty());
}

#[test]
fn cli_original_repeated_evidence_graph_starts_with_exact_permissions() {
    exercise(false);
}

#[test]
#[ignore = "builds and executes original repeated graph with generated descendants"]
fn emitted_original_repeated_evidence_graph_starts_with_exact_permissions() {
    exercise(true);
}
