use serde_json::{json, Value};

fn main() {
    if std::env::args().nth(1).as_deref() == Some("describe") {
        println!(
            "{}",
            json!({
                "protocol_version":1,"name":"result_probe","description":"Exercise selected business results.",
                "params":{"value":{"type":"string","required":true},"artifacts":{"type":"string","required":true},"mode":{"type":"string","required":true},"expected_version":{"type":"string","required":false,"default":""}},
                "result":{"type":"string","nullable":true},
                "resource_profile":{"network":"none","filesystem_read":"required","filesystem_write":"required","subprocess":"none","env_read":"none","credential_access":"none"},
                "self_test":{"supported":false,"safe":false},
                "examples":{"minimal_invoke":{"protocol_version":1,"params":{"value":"null","artifacts":"[]","mode":"data"}},"full_invoke":{"protocol_version":1,"params":{"value":"null","artifacts":"[]","mode":"data"}}}
            })
        );
        return;
    }
    let request: Value = serde_json::from_reader(std::io::stdin().lock()).unwrap();
    let params = &request["params"];
    let result = if let Some(mode) = params["mode"].as_str().unwrap().strip_prefix("journey_") {
        journey(mode, params)
    } else {
        match params["mode"].as_str().unwrap() {
            "missing" => Value::Null,
            "literal" => params["value"].clone(),
            "fail" => std::process::exit(1),
            mode => {
                let mut data: Value =
                    serde_json::from_str(params["value"].as_str().unwrap()).unwrap();
                match mode {
                    "save" => {
                        if let Ok(bytes) = std::fs::read("saved-state.json") {
                            let previous: Value = serde_json::from_slice(&bytes).unwrap();
                            if data["version"] != previous["version"] {
                                std::process::exit(3);
                            }
                        }
                        data["version"] = json!(data["version"].as_u64().unwrap() + 1);
                        std::fs::write("saved-state.json", serde_json::to_vec(&data).unwrap())
                            .unwrap();
                        data = serde_json::from_slice(&std::fs::read("saved-state.json").unwrap())
                            .unwrap();
                    }
                    "load" => {
                        data = serde_json::from_slice(&std::fs::read("saved-state.json").unwrap())
                            .unwrap()
                    }
                    "search" => {
                        let saved: Value =
                            serde_json::from_slice(&std::fs::read("saved-state.json").unwrap())
                                .unwrap();
                        let query = data["query"].as_str().unwrap();
                        let panels = saved["panels"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .filter(|panel| panel["id"].as_str().unwrap().contains(query))
                            .skip(data["offset"].as_u64().unwrap() as usize)
                            .take(data["limit"].as_u64().unwrap() as usize)
                            .cloned()
                            .collect::<Vec<_>>();
                        data = json!({"version":saved["version"],"panels":panels});
                    }
                    "persist" => {
                        std::fs::write("report-state.json", serde_json::to_vec(&data).unwrap())
                            .unwrap();
                        data = serde_json::from_slice(&std::fs::read("report-state.json").unwrap())
                            .unwrap();
                    }
                    _ => {}
                }
                let artifacts: Value =
                    serde_json::from_str(params["artifacts"].as_str().unwrap()).unwrap();
                Value::String(json!({"data":data,"artifacts":artifacts}).to_string())
            }
        }
    };
    println!("{}", json!({"protocol_version":1,"result":result}));
}

fn journey(mode: &str, params: &Value) -> Value {
    let (kind, operation) = mode.split_once('_').unwrap();
    let input: Value = serde_json::from_str(params["value"].as_str().unwrap()).unwrap();
    let mut calls: u64 = std::fs::read_to_string("journey-call-count.txt")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    calls += 1;
    std::fs::write("journey-call-count.txt", calls.to_string()).unwrap();
    let mut state: Value =
        serde_json::from_slice(&std::fs::read("journey-state.json").unwrap()).unwrap();
    let mut data = json!({"status":"loaded","records":[],"selected":null,"current_version":"","total":state["records"].as_array().unwrap().len(),"run":null});
    let mut artifacts = Vec::new();
    let input_id = input["id"].as_str();
    let index = input_id
        .and_then(|id| {
            state["records"]
                .as_array()
                .unwrap()
                .iter()
                .position(|record| record["id"] == id)
        })
        .unwrap_or(0);
    match operation {
        "load" => {
            data["records"] = state["records"].clone();
            data["selected"] = state["records"][0].clone();
        }
        "search" => {
            let query = input["query"].as_str().unwrap();
            let records = state["records"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|record| {
                    record["title"].as_str().unwrap().contains(query)
                        || record["id"].as_str().unwrap().contains(query)
                })
                .cloned()
                .collect::<Vec<_>>();
            data["status"] = json!("searched");
            data["total"] = json!(records.len());
            data["records"] = json!(records
                .into_iter()
                .skip(input["offset"].as_u64().unwrap() as usize)
                .take(input["limit"].as_u64().unwrap() as usize)
                .collect::<Vec<_>>());
        }
        "select" => {
            data["status"] = json!("selected");
            data["selected"] = state["records"][index].clone();
        }
        "save" => {
            let record = &mut state["records"][index];
            if params["expected_version"] != record["version"] {
                data["status"] = json!("conflict");
            } else {
                for key in [
                    "title",
                    "settings",
                    if kind == "report" {
                        "sections"
                    } else {
                        "panels"
                    },
                ] {
                    record[key] = input[key].clone();
                }
                record["version"] =
                    json!(
                        (record["version"].as_str().unwrap().parse::<u64>().unwrap() + 1)
                            .to_string()
                    );
                std::fs::write("journey-state.json", serde_json::to_vec(&state).unwrap()).unwrap();
                data["status"] = json!("saved");
            }
            data["selected"] = state["records"][index].clone();
        }
        "work" => {
            let record = &mut state["records"][index];
            if params["expected_version"] != record["version"] {
                data["status"] = json!("conflict");
            } else {
                let snapshot = record["version"].clone();
                let item_key = if kind == "report" {
                    "sections"
                } else {
                    "panels"
                };
                let ids = input["ids"].as_array().unwrap();
                let mut outcomes = Vec::new();
                for (position, id) in ids.iter().enumerate() {
                    let item = record[item_key]
                        .as_array_mut()
                        .unwrap()
                        .iter_mut()
                        .find(|item| item["id"] == *id)
                        .unwrap();
                    let status = if position == 0 { "complete" } else { "failed" };
                    item["state"] = json!(status);
                    outcomes.push(json!({"id":id,"state":status}));
                }
                record["version"] =
                    json!((snapshot.as_str().unwrap().parse::<u64>().unwrap() + 1).to_string());
                std::fs::write("journey-state.json", serde_json::to_vec(&state).unwrap()).unwrap();
                let (first, second) = if kind == "report" {
                    ("existing.txt", "existing.json")
                } else {
                    ("existing.png", "existing.wav")
                };
                let (new_first, new_second) = if kind == "report" {
                    ("generated.txt", "generated.json")
                } else {
                    ("generated.png", "generated.wav")
                };
                std::fs::copy(format!("exports/{first}"), format!("exports/{new_first}")).unwrap();
                std::fs::copy(format!("exports/{second}"), format!("exports/{new_second}"))
                    .unwrap();
                data["status"] = json!("partial");
                data["run"] = json!({"id":format!("{kind}-run-{calls}"),"snapshot_version":snapshot,"items":outcomes});
            }
            data["selected"] = state["records"][index].clone();
        }
        "preview" | "required" => {
            data["selected"] = state["records"][index].clone();
            if params["expected_version"] != data["selected"]["version"] {
                data["status"] = json!("conflict");
            } else {
                data["status"] = json!("preview");
                let representations = if operation == "required" {
                    vec!["valid", "race"]
                } else {
                    vec![input["representation"].as_str().unwrap()]
                };
                for representation in representations {
                    if representation == "missing" {
                        data["status"] = json!("missing");
                        continue;
                    }
                    if representation == "pdf" {
                        data["status"] = json!("unsupported");
                        continue;
                    }
                    let (path, mime) = match (kind, representation) {
                        ("report", "valid") => ("existing.txt", "text/plain"),
                        ("report", "secondary") => ("existing.json", "application/json"),
                        ("report", "stale") => ("stale.txt", "text/plain"),
                        ("report", "race") => ("missing.txt", "text/plain"),
                        ("report", "generated") => ("generated.txt", "text/plain"),
                        ("report", "generated_secondary") => ("generated.json", "application/json"),
                        ("media", "valid") => ("existing.png", "image/png"),
                        ("media", "secondary") => ("existing.wav", "audio/wav"),
                        ("media", "stale") => ("stale.wav", "audio/wav"),
                        ("media", "race") => ("missing.png", "image/png"),
                        ("media", "generated") => ("generated.png", "image/png"),
                        ("media", "generated_secondary") => ("generated.wav", "audio/wav"),
                        _ => panic!("unexpected fixture representation"),
                    };
                    artifacts.push(
                        json!({"id":representation,"scope":"exports","path":path,"mime_type":mime}),
                    );
                }
            }
        }
        _ => panic!("unexpected journey operation"),
    }
    if !data["selected"].is_null() {
        data["current_version"] = data["selected"]["version"].clone();
    }
    Value::String(json!({"data":data,"artifacts":artifacts}).to_string())
}
