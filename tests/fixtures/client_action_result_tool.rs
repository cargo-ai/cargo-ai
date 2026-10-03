use serde_json::{json, Value};

fn main() {
    if std::env::args().nth(1).as_deref() == Some("describe") {
        println!("{}", json!({
            "protocol_version":1,"name":"result_probe","description":"Exercise selected business results.",
            "params":{"value":{"type":"string","required":true},"artifacts":{"type":"string","required":true},"mode":{"type":"string","required":true}},
            "result":{"type":"string","nullable":true},
            "resource_profile":{"network":"none","filesystem_read":"required","filesystem_write":"required","subprocess":"none","env_read":"none","credential_access":"none"},
            "self_test":{"supported":false,"safe":false},
            "examples":{"minimal_invoke":{"protocol_version":1,"params":{"value":"null","artifacts":"[]","mode":"data"}},"full_invoke":{"protocol_version":1,"params":{"value":"null","artifacts":"[]","mode":"data"}}}
        }));
        return;
    }
    let request: Value = serde_json::from_reader(std::io::stdin().lock()).unwrap();
    let params = &request["params"];
    let result = match params["mode"].as_str().unwrap() {
        "missing" => Value::Null,
        "literal" => params["value"].clone(),
        "fail" => std::process::exit(1),
        mode => {
            let mut data: Value = serde_json::from_str(params["value"].as_str().unwrap()).unwrap();
            match mode {
                "save" => {
                    if let Ok(bytes) = std::fs::read("saved-state.json") {
                        let previous: Value = serde_json::from_slice(&bytes).unwrap();
                        if data["version"] != previous["version"] { std::process::exit(3); }
                    }
                    data["version"] = json!(data["version"].as_u64().unwrap() + 1);
                    std::fs::write("saved-state.json", serde_json::to_vec(&data).unwrap()).unwrap();
                    data = serde_json::from_slice(&std::fs::read("saved-state.json").unwrap()).unwrap();
                }
                "load" => data = serde_json::from_slice(&std::fs::read("saved-state.json").unwrap()).unwrap(),
                "search" => {
                    let saved: Value = serde_json::from_slice(&std::fs::read("saved-state.json").unwrap()).unwrap();
                    let query = data["query"].as_str().unwrap();
                    let panels = saved["panels"].as_array().unwrap().iter()
                        .filter(|panel| panel["id"].as_str().unwrap().contains(query))
                        .skip(data["offset"].as_u64().unwrap() as usize)
                        .take(data["limit"].as_u64().unwrap() as usize)
                        .cloned().collect::<Vec<_>>();
                    data = json!({"version":saved["version"],"panels":panels});
                }
                "persist" => {
                    std::fs::write("report-state.json", serde_json::to_vec(&data).unwrap()).unwrap();
                    data = serde_json::from_slice(&std::fs::read("report-state.json").unwrap()).unwrap();
                }
                _ => {}
            }
            let artifacts: Value = serde_json::from_str(params["artifacts"].as_str().unwrap()).unwrap();
            Value::String(json!({"data":data,"artifacts":artifacts}).to_string())
        }
    };
    println!("{}", json!({"protocol_version":1,"result":result}));
}
