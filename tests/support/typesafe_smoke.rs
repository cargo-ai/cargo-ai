//! Mixed Choice/Score process coverage, sharing the provider smoke lifecycle.

use super::*;
use serde_json::json;

const TOKEN: &str = "typesafe-provider-smoke-token";
const MODEL: &str = "jev-1.13.0";
const EXAMPLE: &str = include_str!("../../templates/guidance/examples/jev-choice-score.json");
const DEFAULT_CONTEXT: &str = "For context A question will be asked and you will need to return the answer in the specified JSON format. \n";

fn assert_state(request: &Value, inputs: &[&str]) {
    let expected: Vec<&str> = std::iter::once(DEFAULT_CONTEXT)
        .chain(inputs.iter().copied())
        .collect();
    assert_eq!(request["state"], json!(expected));
}

fn example(fixture: &Fixture) {
    fs::write(&fixture.definition, EXAMPLE).unwrap();
}

fn response(department: &str, score: f64) -> Value {
    json!({"model":MODEL,"answers":{
        "department":{"type":"choice","choice":department},
        "urgency":{"type":"score","score":score}
    },"usage":{"input_tokens":17,"output_tokens":4}})
}

fn mock(body: Value) -> MockServer {
    MockServer::respond_after_at("/v1/systemone", Duration::ZERO, 200, body.to_string())
}

fn text(output: &Output) -> String {
    format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn command(fixture: &Fixture, executable: Option<&Path>) -> Command {
    let mut command =
        fixture.isolated_command(executable.unwrap_or(Path::new(env!("CARGO_BIN_EXE_cargo-ai"))));
    // Child JSON execution must find this candidate, never the installed CLI.
    let mut paths = vec![Path::new(env!("CARGO_BIN_EXE_cargo-ai"))
        .parent()
        .unwrap()
        .to_path_buf()];
    paths.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    command
        .env("PATH", std::env::join_paths(paths).unwrap())
        .env_remove("TYPESAFE_API_KEY")
        .env_remove("CARGO_AI_USAGE_ROOT_RUN_ID")
        .env_remove("CARGO_AI_USAGE_PARENT_AGENT_RUN_ID")
        .env("NO_PROXY", "127.0.0.1,localhost")
        .env("no_proxy", "127.0.0.1,localhost");
    if executable.is_none() {
        command
            .args(["--no-update-check", "run", "--config"])
            .arg(&fixture.definition);
    }
    command
        .args(["--render-mode", "append-only", "--usage-log"])
        .arg(&fixture.usage);
    command
}

fn fake_command(fixture: &Fixture, executable: Option<&Path>, url: &str) -> Command {
    let mut command = command(fixture, executable);
    command.args([
        "--server",
        "typesafe",
        "--model",
        MODEL,
        "--token",
        TOKEN,
        "--url",
        url,
        "--inference-timeout-in-sec",
        "2",
    ]);
    command
}

fn body(request: &str) -> Value {
    serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap()
}

fn assert_request(request: &str) -> Value {
    assert!(request.starts_with("POST /v1/systemone HTTP/1.1"));
    assert!(request
        .to_ascii_lowercase()
        .contains(&format!("authorization: bearer {TOKEN}")));
    let body = body(request);
    let definition: Value = serde_json::from_str(EXAMPLE).unwrap();
    let properties = &definition["agent_schema"]["properties"];
    assert_eq!(body["model"], MODEL);
    assert_eq!(body["questions"]["department"]["type"], "choice");
    assert_eq!(
        body["questions"]["department"]["instructions"],
        properties["department"]["description"]
    );
    assert_eq!(
        body["questions"]["department"]["criteria"],
        json!({"billing":"billing","technical":"technical","sales":"sales"})
    );
    assert_eq!(body["questions"]["urgency"]["type"], "score");
    assert_eq!(
        body["questions"]["urgency"]["criteria"],
        properties["urgency"]["rubric"]
    );
    assert_eq!(
        body["questions"]["urgency"]["instructions"],
        properties["urgency"]["description"]
    );
    assert!(body.get("temperature").is_none());
    assert!(body.get("max_output_tokens").is_none());
    body
}

fn events(fixture: &Fixture) -> Vec<Value> {
    let raw = fs::read_to_string(&fixture.usage).unwrap();
    assert!(!raw.contains(TOKEN));
    raw.lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn assert_usage(fixture: &Fixture) {
    let events = events(fixture);
    let event = events
        .iter()
        .rev()
        .find(|event| event["event_type"] == "provider_request_completed")
        .unwrap();
    assert_eq!(event["provider"]["server"], "typesafe");
    assert_eq!(event["provider"]["model"], MODEL);
    assert_eq!(event["usage"]["input_tokens"], 17);
    assert_eq!(event["usage"]["output_tokens"], 4);
    assert_eq!(event["usage"]["total_tokens"], 21);
}

fn assert_example(fixture: &Fixture, executable: Option<&Path>) {
    for (native, high) in [
        (0.0, false),
        (1.49, false),
        (1.5, true),
        (1.6, true),
        (2.0, true),
    ] {
        let mock = mock(response("technical", native));
        let output = fake_command(fixture, executable, &mock.url)
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", text(&output));
        assert_eq!(
            text(&output).contains("JEV_HIGH_URGENCY"),
            high,
            "native score {native}: {}",
            text(&output)
        );
        assert!(text(&output).contains(&format!("Provider response model: {MODEL}")));
        assert!(!text(&output).contains(TOKEN));
        let request = assert_request(&mock.finish());
        let definition: Value = serde_json::from_str(EXAMPLE).unwrap();
        assert_state(
            &request,
            &[definition["inputs"][0]["text"].as_str().unwrap()],
        );
        assert_usage(fixture);
    }
}

#[test]
fn interpreted_typesafe_public_example_maps_scores_and_drives_condition() {
    let fixture = Fixture::new();
    example(&fixture);
    assert_example(&fixture, None);
    assert_input_overrides(&fixture, None);
    assert_unsupported_inputs(&fixture, None);
}

fn hatch(fixture: &Fixture, name: &str) -> PathBuf {
    let output_dir = fixture.root.join("dist");
    let output = fixture
        .isolated_command(env!("CARGO_BIN_EXE_cargo-ai"))
        .env_remove("TYPESAFE_API_KEY")
        .args(["--no-update-check", "hatch", name, "--config"])
        .arg(&fixture.definition)
        .arg("--output-dir")
        .arg(&output_dir)
        .arg("--force")
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", text(&output));
    if fixture.home.join("batch-seed-marker").exists() {
        assert!(
            text(&output).contains("Reused warmed template"),
            "{}",
            text(&output)
        );
    }
    output_dir.join(if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    })
}

#[test]
#[ignore = "run explicitly in the provider smoke CI lane"]
fn generated_typesafe_smoke_isolated_and_deterministic() {
    generated_typesafe_case(&Fixture::new());
}

pub(super) fn generated_typesafe_case(fixture: &Fixture) {
    example(fixture);
    let executable = hatch(fixture, "typesafe_provider_smoke");
    assert_example(fixture, Some(&executable));
    assert_input_overrides(fixture, Some(&executable));
    assert_unsupported_inputs(fixture, Some(&executable));
    assert_invalid_answers(fixture, Some(&executable));
    assert_general_provider_adaptation(fixture, Some(&executable));
    child_definition(fixture);
    fs::write(
        &fixture.definition,
        serde_json::to_vec(&live_definition(fixture)).unwrap(),
    )
    .unwrap();
    let executable = hatch(fixture, "typesafe_journey_smoke");
    assert_labeled_fixture(fixture, Some(&executable));
}

fn assert_input_overrides(fixture: &Fixture, executable: Option<&Path>) {
    for mode in ["append", "prepend"] {
        let mock = mock(response("billing", 0.0));
        let output = fake_command(fixture, executable, &mock.url)
            .args([
                "--input-override",
                "message=named replacement",
                "--input-mode",
                mode,
                "--input-text",
                "first extra",
                "--input-text",
                "second extra",
            ])
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", text(&output));
        let expected = if mode == "append" {
            ["named replacement", "first extra", "second extra"]
        } else {
            ["first extra", "second extra", "named replacement"]
        };
        assert_state(&assert_request(&mock.finish()), &expected);
    }
    let url = MockServer::respond_after_at(
        "/message",
        Duration::ZERO,
        200,
        "URL billing message".into(),
    );
    let mock = mock(response("billing", 0.0));
    let output = fake_command(fixture, executable, &mock.url)
        .args(["--input-mode", "replace", "--input-url", &url.url])
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", text(&output));
    let expected_url_text = format!("Web resource from {}:\nURL billing message", url.url);
    let fetched = url.finish();
    assert!(fetched.starts_with("GET /message HTTP/1.1"));
    assert!(!fetched.to_ascii_lowercase().contains("authorization:"));
    assert!(!fetched.contains(TOKEN));
    assert_state(&assert_request(&mock.finish()), &[&expected_url_text]);
}

fn no_request(listener: &TcpListener) {
    listener.set_nonblocking(true).unwrap();
    assert!(
        matches!(listener.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock),
        "compatibility rejection must precede provider transmission"
    );
}

fn assert_unsupported_inputs(fixture: &Fixture, executable: Option<&Path>) {
    for kind in ["image", "file"] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/v1/systemone", listener.local_addr().unwrap());
        let missing = fixture.root.join(format!("must-not-read.{kind}"));
        let output = fake_command(fixture, executable, &url)
            .arg(format!("--input-{kind}"))
            .arg(&missing)
            .output()
            .unwrap();
        assert!(!output.status.success());
        let diagnostic = text(&output);
        assert!(
            diagnostic.contains("$.inputs[") && diagnostic.contains("Jev"),
            "{diagnostic}"
        );
        assert!(
            !diagnostic.contains("No such file") && !diagnostic.contains("os error 2"),
            "{diagnostic}"
        );
        assert!(!diagnostic.contains("JEV_HIGH_URGENCY"));
        no_request(&listener);
    }
}

fn assert_invalid_answers(fixture: &Fixture, executable: Option<&Path>) {
    let mut cases = Vec::new();
    let mut missing = response("technical", 2.0);
    missing["answers"]
        .as_object_mut()
        .unwrap()
        .remove("department");
    cases.push(missing);
    let mut extra = response("technical", 2.0);
    extra["answers"]["unrequested"] = json!({"type":"choice","choice":"private-payload"});
    cases.push(extra);
    cases.push(response("private-payload", 2.0));
    cases.push(response("technical", 2.1));
    let mut wrong_type = response("technical", 2.0);
    wrong_type["answers"]["urgency"]["type"] = json!("choice");
    cases.push(wrong_type);
    let mut wrong_value = response("technical", 2.0);
    wrong_value["answers"]["urgency"]["score"] = json!("private-payload");
    cases.push(wrong_value);
    for body in cases {
        let mock = mock(body);
        let output = fake_command(fixture, executable, &mock.url)
            .output()
            .unwrap();
        let _ = mock.finish();
        assert!(!output.status.success());
        let diagnostic = text(&output);
        assert!(diagnostic.contains("$.answers"), "{diagnostic}");
        assert!(!diagnostic.contains("private-payload") && !diagnostic.contains(TOKEN));
        assert!(!diagnostic.contains("JEV_HIGH_URGENCY"));
        assert!(!fixture.root.join("action-ran.txt").exists());
    }
}

fn marker_example(fixture: &Fixture) -> Value {
    let marker: Value =
        serde_json::from_slice(&fs::read(marker_definition(fixture)).unwrap()).unwrap();
    let mut definition: Value = serde_json::from_str(EXAMPLE).unwrap();
    definition["actions"][0]["run"] = marker["actions"][0]["run"].clone();
    definition
}

#[test]
fn interpreted_typesafe_invalid_answers_fail_before_marker_action() {
    let fixture = Fixture::new();
    let definition = marker_example(&fixture);
    fs::write(
        &fixture.definition,
        serde_json::to_vec(&definition).unwrap(),
    )
    .unwrap();
    assert_invalid_answers(&fixture, None);
    let mock = mock(response("technical", 2.0));
    let output = fake_command(&fixture, None, &mock.url).output().unwrap();
    let _ = mock.finish();
    assert!(output.status.success(), "{}", text(&output));
    assert_eq!(
        fs::read_to_string(fixture.root.join("action-ran.txt"))
            .unwrap()
            .trim(),
        "ran"
    );
}

fn profiles(fixture: &Fixture, url: &str, setting: &str) {
    fs::write(fixture.home.join("config.toml"), format!(
        "default_profile = 'existing'\nsecret_store = 'file'\n\n[[profile]]\nname = 'existing'\nserver = 'ollama'\nmodel = 'existing-model'\nauth_mode = 'none'\n\n[[profile]]\nname = 'jev'\nserver = 'typesafe'\nmodel = '{MODEL}'\nauth_mode = 'api_key'\nurl = '{url}'\n{setting}\n"
    )).unwrap();
}

fn check(fixture: &Fixture, profile: Option<&str>) -> Output {
    let mut command = fixture.isolated_command(env!("CARGO_BIN_EXE_cargo-ai"));
    command
        .args([
            "--no-update-check",
            "hatch",
            "jev_check",
            "--check",
            "--config",
        ])
        .arg(&fixture.definition)
        .arg("--output-dir")
        .arg(fixture.root.join("dist"));
    if let Some(profile) = profile {
        command.args(["--profile", profile]);
    }
    command.output().unwrap()
}

#[test]
fn typesafe_hatch_targets_exact_profile_without_secrets_and_preserves_generic_check() {
    let fixture = Fixture::new();
    example(&fixture);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/v1/systemone", listener.local_addr().unwrap());
    profiles(&fixture, &url, "");
    // An unreadable credential shape detects accidental secret-store access.
    fs::create_dir(fixture.home.join("credentials.toml")).unwrap();
    let original: toml::Value =
        toml::from_str(&fs::read_to_string(fixture.home.join("config.toml")).unwrap()).unwrap();
    let output = check(&fixture, Some("jev"));
    assert!(output.status.success(), "{}", text(&output));
    assert!(text(&output).contains("Jev compatibility: passed"));
    assert!(text(&output).contains("runtime-dependent"));
    let missing = check(&fixture, Some("missing"));
    assert!(!missing.status.success());
    assert!(
        text(&missing).contains("no default profile was substituted"),
        "{}",
        text(&missing)
    );
    let after: toml::Value =
        toml::from_str(&fs::read_to_string(fixture.home.join("config.toml")).unwrap()).unwrap();
    assert_eq!(after["profile"], original["profile"]);
    assert_eq!(after["default_profile"], original["default_profile"]);
    assert!(fixture.home.join("credentials.toml").is_dir());
    assert!(
        !text(&output).contains("credentials.toml"),
        "{}",
        text(&output)
    );
    for setting in ["temperature = 0.5", "max_output_tokens = 128"] {
        profiles(&fixture, &url, setting);
        let rejected = check(&fixture, Some("jev"));
        assert!(!rejected.status.success());
        assert!(text(&rejected).contains("clear-"), "{}", text(&rejected));
        let generic = check(&fixture, None);
        assert!(generic.status.success(), "{}", text(&generic));
        assert!(!text(&generic).contains("Jev compatibility: passed"));
        let run = command(&fixture, None)
            .args(["--profile", "jev", "--token", TOKEN])
            .output()
            .unwrap();
        assert!(!run.status.success());
        assert!(text(&run).contains("clear-"), "{}", text(&run));
    }
    no_request(&listener);
}

fn assert_general_provider_adaptation(fixture: &Fixture, executable: Option<&Path>) {
    let mock = MockServer::respond_after_at(
        "/v1/chat/completions",
        Duration::ZERO,
        200,
        openai_success_response(r#"{"department":"technical","urgency":80}"#),
    );
    let output = command(fixture, executable)
        .args([
            "--server",
            "openai",
            "--model",
            "openai-smoke",
            "--token",
            OPENAI_TEST_TOKEN,
            "--url",
            &mock.url,
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", text(&output));
    assert!(text(&output).contains("JEV_HIGH_URGENCY"));
    let request = body(&mock.finish());
    let score = &request["response_format"]["json_schema"]["schema"]["properties"]["urgency"];
    assert!(score.get("rubric").is_none());
    let description = score["description"].as_str().unwrap();
    let definition: Value = serde_json::from_str(EXAMPLE).unwrap();
    let mut previous = 0;
    for level in definition["agent_schema"]["properties"]["urgency"]["rubric"]
        .as_array()
        .unwrap()
    {
        let at = description.find(level.as_str().unwrap()).unwrap();
        assert!(at > previous);
        previous = at;
    }
    let authored = &definition["agent_schema"]["properties"]["urgency"];
    assert!(description.contains(&format!(
        "inclusive numeric range [{}, {}]",
        authored["minimum"], authored["maximum"]
    )));
    assert_eq!(score["minimum"], authored["minimum"]);
    assert_eq!(score["maximum"], authored["maximum"]);
}

#[test]
fn interpreted_typesafe_example_preserves_rubric_on_general_provider() {
    let fixture = Fixture::new();
    example(&fixture);
    assert_general_provider_adaptation(&fixture, None);
}

fn child_definition(fixture: &Fixture) {
    fs::write(fixture.root.join("child.json"), definition_json()).unwrap();
}

fn add_child(definition: &mut Value) {
    definition["actions"].as_array_mut().unwrap().push(json!({
        "name":"explicit_child", "logic":{"==":[1,1]},
        "run":[{"kind":"agent","artifact":"./child.json","profile":"local-child"}]
    }));
}

fn child_profile(fixture: &Fixture, url: &str) {
    let mut file = fs::OpenOptions::new()
        .append(true)
        .open(fixture.home.join("config.toml"))
        .unwrap();
    write!(file, "\n[[profile]]\nname = 'local-child'\nserver = 'ollama'\nmodel = 'child-smoke'\nauth_mode = 'none'\nurl = '{url}'\n").unwrap();
}

fn assert_child(request: &str) {
    let request_body = body(request);
    assert_eq!(request_body["model"], "child-smoke");
    assert!(!request.to_ascii_lowercase().contains("authorization:"));
    assert!(!request.contains(TOKEN));
}

#[test]
fn typesafe_action_only_skips_incompatible_root_and_selects_child_profile() {
    let fixture = Fixture::new();
    child_definition(&fixture);
    let mut definition = marker_example(&fixture);
    definition["agent_schema"]["properties"] = json!({});
    definition["inputs"] = json!([{"type":"file","name":"unused","path":"not-read.pdf"}]);
    definition["actions"] = json!([]);
    add_child(&mut definition);
    fs::write(
        &fixture.definition,
        serde_json::to_vec(&definition).unwrap(),
    )
    .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    profiles(
        &fixture,
        &format!("http://{}/v1/systemone", listener.local_addr().unwrap()),
        "temperature = 0.5",
    );
    let mock = MockServer::ollama_success();
    child_profile(&fixture, &mock.url);
    let output = command(&fixture, None)
        .args(["--profile", "jev", "--token", TOKEN])
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", text(&output));
    assert_child(&mock.finish());
    no_request(&listener);
    assert_eq!(
        events(&fixture)
            .iter()
            .filter(|event| event["event_type"] == "provider_request_completed")
            .count(),
        1
    );
}

// Frozen representative judgments; each case is invoked once per execution path.
const LIVE_CASES: [(&str, &str, f64, f64); 3] = [
    ("Please send a copy of last month's invoice by next week. This is a routine request; our service is working normally.", "billing", 0.0, 20.0),
    ("All users are locked out of production and there is no workaround. Please restore access immediately.", "technical", 80.0, 100.0),
    ("Please send a licensing quote today so we can finish our purchasing review. Our existing service is working normally.", "sales", 30.0, 70.0),
];

fn live_definition(fixture: &Fixture) -> Value {
    let mut definition = marker_example(fixture);
    let (program, mut args) = if cfg!(windows) {
        ("cmd.exe", vec![json!("/C"), json!("echo")])
    } else {
        ("/bin/echo", Vec::new())
    };
    args.extend([
        json!("JEV_JUDGMENT"),
        json!({"var":"department"}),
        json!({"var":"urgency"}),
    ]);
    definition["actions"].as_array_mut().unwrap().push(json!({
        "name":"record_judgment", "logic":{"==":[1,1]},
        "run":[{"kind":"exec","program":program,"args":args}]
    }));
    definition["runtime_vars"] = json!({"invoke_child":{"type":"boolean","default":false}});
    add_child(&mut definition);
    definition["actions"]
        .as_array_mut()
        .unwrap()
        .last_mut()
        .unwrap()["logic"] = json!({"==":[{"var":"runtime.invoke_child"},true]});
    definition
}

fn install_live_profile(fixture: &Fixture, model: &str, key: &str) {
    let cli = env!("CARGO_BIN_EXE_cargo-ai");
    let add = fixture
        .isolated_command(cli)
        .env_remove("TYPESAFE_API_KEY")
        .args([
            "--no-update-check",
            "profile",
            "add",
            "jev-live",
            "--server",
            "typesafe",
            "--model",
            model,
            "--auth",
            "api_key",
        ])
        .output()
        .unwrap();
    assert!(
        add.status.success(),
        "TypeSafe live profile creation failed"
    );
    let mut child = fixture
        .isolated_command(cli)
        .env_remove("TYPESAFE_API_KEY")
        .args(["--no-update-check", "profile", "set", "jev-live", "--stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(key.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "TypeSafe isolated stdin credential setup failed"
    );
    assert!(
        !text(&output).contains(key),
        "profile setup must not expose the key"
    );
}

fn judgment(output: &Output) -> Option<(String, f64)> {
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|line| {
            let (_, values) = line.split_once("JEV_JUDGMENT ")?;
            let mut fields = values.split_whitespace();
            let department = fields.next()?;
            if !["billing", "technical", "sales"].contains(&department) {
                return None;
            }
            let score = fields.next()?.parse::<f64>().ok()?;
            (score.is_finite() && (0.0..=100.0).contains(&score))
                .then(|| (department.to_string(), score))
        })
}

fn point_live_profile_at_mock(fixture: &Fixture, url: &str) {
    let path = fixture.home.join("config.toml");
    let mut config: toml::Value = toml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    let profile = config["profile"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|profile| profile["name"].as_str() == Some("jev-live"))
        .unwrap();
    profile
        .as_table_mut()
        .unwrap()
        .insert("url".into(), toml::Value::String(url.to_string()));
    fs::write(path, toml::to_string(&config).unwrap()).unwrap();
}

fn assert_labeled_fixture(fixture: &Fixture, executable: Option<&Path>) {
    install_live_profile(fixture, MODEL, TOKEN);
    for (case, native) in [(0, 0.0), (1, 1.6), (2, 1.0)] {
        let mock = mock(response(LIVE_CASES[case].1, native));
        point_live_profile_at_mock(fixture, &mock.url);
        let score = live_run(fixture, executable, case, None, false, MODEL, TOKEN);
        assert_eq!(score, native * 50.0);
        assert_state(&assert_request(&mock.finish()), &[LIVE_CASES[case].0]);
    }
    let parent = mock(response("technical", 2.0));
    let child = MockServer::ollama_success();
    point_live_profile_at_mock(fixture, &parent.url);
    child_profile(fixture, &child.url);
    live_run(fixture, executable, 1, None, true, MODEL, TOKEN);
    assert_request(&parent.finish());
    assert_child(&child.finish());
    let events = events(fixture);
    assert_eq!(
        events
            .iter()
            .filter(|event| event["event_type"] == "provider_request_completed")
            .count(),
        2
    );
}

#[test]
fn interpreted_typesafe_labeled_fixture_and_mixed_child_are_deterministic() {
    let fixture = Fixture::new();
    child_definition(&fixture);
    fs::write(
        &fixture.definition,
        serde_json::to_vec(&live_definition(&fixture)).unwrap(),
    )
    .unwrap();
    assert_labeled_fixture(&fixture, None);
}

fn live_run(
    fixture: &Fixture,
    executable: Option<&Path>,
    case: usize,
    input_url: Option<&str>,
    invoke_child: bool,
    model: &str,
    key: &str,
) -> f64 {
    let marker = fixture.root.join("action-ran.txt");
    if marker.exists() {
        fs::remove_file(&marker).unwrap();
    }
    if fixture.usage.exists() {
        fs::remove_file(&fixture.usage).unwrap();
    }
    let mut command = command(fixture, executable);
    command.args([
        "--profile",
        "jev-live",
        "--inference-timeout-in-sec",
        "60",
        "--max-runtime-in-sec",
        "120",
    ]);
    if let Some(url) = input_url {
        command.args(["--input-mode", "replace", "--input-url", url]);
    } else {
        command.args([
            "--input-override",
            &format!("message={}", LIVE_CASES[case].0),
        ]);
    }
    if invoke_child {
        command.args(["--run-var", "invoke_child=true"]);
    }
    let started = Instant::now();
    let output = command
        .output()
        .expect("live TypeSafe process should start");
    let wall_ms = started.elapsed().as_millis();
    let raw_events = fs::read_to_string(&fixture.usage).unwrap_or_default();
    assert!(
        !text(&output).contains(key) && !raw_events.contains(key),
        "live diagnostics must not expose credentials"
    );
    let provider_events: Vec<Value> = raw_events
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|event| {
            event["event_type"] == "provider_request_completed"
                && event["provider"]["server"] == "typesafe"
        })
        .collect();
    let returned = String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|line| {
            line.strip_prefix("Provider response model: ")
                .map(str::to_owned)
        });
    let observed = judgment(&output);
    let message_bytes = LIVE_CASES[case].0.len();
    let state_bytes = DEFAULT_CONTEXT.len()
        + input_url.map_or(message_bytes, |url| {
            format!("Web resource from {url}:\n{}", LIVE_CASES[case].0).len()
        });
    eprintln!("TypeSafe live case={} path={} url={} child={} message_bytes={} state_bytes={} exit={:?} whole_ms={} requested_model={} returned_model={:?} judgment={:?}",
        case, if executable.is_some() { "standalone" } else { "interpreted" }, input_url.is_some(), invoke_child,
        message_bytes, state_bytes, output.status.code(), wall_ms, model, returned, observed);
    for event in &provider_events {
        eprintln!(
            "TypeSafe live inference: status={} duration_ms={} usage={}",
            event["status"], event["duration_ms"], event["usage"]
        );
    }
    assert!(
        output.status.success(),
        "bounded TypeSafe live invocation failed; no automatic retry is permitted"
    );
    assert_eq!(
        provider_events.len(),
        1,
        "each live case must issue exactly one TypeSafe inference"
    );
    assert_eq!(provider_events[0]["provider"]["model"], model);
    assert_eq!(provider_events[0]["status"], "success");
    assert!(provider_events[0]["usage"]["input_tokens"]
        .as_u64()
        .is_some());
    assert!(provider_events[0]["usage"]["output_tokens"]
        .as_u64()
        .is_some());
    assert!(
        returned.is_some_and(|name| !name.is_empty()),
        "returned TypeSafe model identity must be recorded"
    );
    let (department, score) = observed.expect("authored action must expose the validated judgment");
    assert_eq!(department, LIVE_CASES[case].1);
    assert!(
        (LIVE_CASES[case].2..=LIVE_CASES[case].3).contains(&score),
        "judgment is outside the frozen labeled interval"
    );
    assert_eq!(
        marker.exists(),
        case == 1,
        "only the high-urgency case may write the marker"
    );
    score
}

#[test]
#[ignore = "requires TYPESAFE_API_KEY and TYPESAFE_MODEL; exactly eight approved live inferences, no retries"]
fn live_typesafe_journey_uses_isolated_stdin_credentials() {
    let model = std::env::var("TYPESAFE_MODEL").expect("TYPESAFE_MODEL is required");
    assert!(!model.trim().is_empty());
    let fixture = Fixture::new();
    child_definition(&fixture);
    let definition = live_definition(&fixture);
    fs::write(
        &fixture.definition,
        serde_json::to_vec_pretty(&definition).unwrap(),
    )
    .unwrap();
    // Finish compilation before opening the live key or spending the inference budget.
    let executable = hatch(&fixture, "typesafe_live_journey");
    let key = std::env::var("TYPESAFE_API_KEY").expect("TYPESAFE_API_KEY is required");
    assert!(!key.is_empty());
    install_live_profile(&fixture, &model, &key);
    for executable in [None, Some(executable.as_path())] {
        for case in 0..LIVE_CASES.len() {
            live_run(&fixture, executable, case, None, false, &model, &key);
        }
    }
    let url = MockServer::respond_after_at(
        "/billing-message",
        Duration::ZERO,
        200,
        LIVE_CASES[0].0.into(),
    );
    live_run(&fixture, None, 0, Some(&url.url), false, &model, &key);
    let fetch = url.finish();
    assert!(fetch.starts_with("GET /billing-message HTTP/1.1"));
    assert!(!fetch.to_ascii_lowercase().contains("authorization:") && !fetch.contains(&key));
    let child = MockServer::ollama_success();
    child_profile(&fixture, &child.url);
    live_run(&fixture, None, 1, None, true, &model, &key);
    let child_request = child.finish();
    assert_child(&child_request);
    assert!(!child_request.contains(&key));
}
