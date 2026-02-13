use serde_json::Value;

pub fn render_backend_ui(response: &Value) -> bool {
    let ui = match response.get("ui") {
        Some(v) => v,
        None => return false,
    };

    let schema = ui.get("schema").and_then(|v| v.as_str());
    if schema != Some("1.0") {
        return false;
    }

    let kind = ui.get("kind").and_then(|v| v.as_str()).unwrap_or("info");
    let title = ui.get("title").and_then(|v| v.as_str()).unwrap_or("Status");
    let summary = ui
        .get("summary")
        .and_then(|v| v.as_str())
        .unwrap_or("Status response received.");

    let kind_prefix = match kind {
        "success" => "✅",
        "error" => "⚠️",
        "failure" => "❌",
        _ => "ℹ️",
    };

    println!("{} {}", kind_prefix, title);
    println!("{}", summary);

    if let Some(variant) = ui.get("variant").and_then(|v| v.as_str()) {
        if !variant.trim().is_empty() {
            println!("Variant: {}", variant);
        }
    }

    if let Some(sections) = ui.get("sections").and_then(|v| v.as_array()) {
        for section in sections {
            render_section(section);
        }
    }

    if let Some(actions) = ui.get("actions").and_then(|v| v.as_array()) {
        let mut printed_header = false;

        for action in actions {
            let label = action.get("label").and_then(|v| v.as_str()).unwrap_or("");
            let command = action.get("command").and_then(|v| v.as_str()).unwrap_or("");

            if label.is_empty() && command.is_empty() {
                continue;
            }

            if !printed_header {
                println!("\nActions:");
                printed_header = true;
            }

            if !label.is_empty() && !command.is_empty() {
                println!("- {}: {}", label, command);
            } else if !label.is_empty() {
                println!("- {}", label);
            } else {
                println!("- {}", command);
            }
        }
    }

    if let Some(next_steps) = ui.get("next_steps").and_then(|v| v.as_array()) {
        let mut printed_header = false;

        for step in next_steps {
            let text = match step.as_str() {
                Some(s) if !s.trim().is_empty() => s,
                _ => continue,
            };

            if !printed_header {
                println!("\nNext steps:");
                printed_header = true;
            }

            println!("- {}", text);
        }
    }

    true
}

pub fn render_account_status_ui(response: &Value) -> bool {
    render_backend_ui(response)
}

fn render_section(section: &Value) {
    let section_type = section.get("type").and_then(|v| v.as_str()).unwrap_or("");
    let title = section.get("title").and_then(|v| v.as_str()).unwrap_or("");

    if !title.is_empty() {
        println!("\n{}:", title);
    }

    match section_type {
        "kv" => {
            if let Some(items) = section.get("items").and_then(|v| v.as_array()) {
                for item in items {
                    let label = item.get("label").and_then(|v| v.as_str()).unwrap_or("");
                    let value = item.get("value").map(value_to_string).unwrap_or_default();

                    if label.is_empty() && value.is_empty() {
                        continue;
                    }

                    if label.is_empty() {
                        println!("- {}", value);
                    } else {
                        println!("- {}: {}", label, value);
                    }
                }
            }
        }
        "list" => {
            if let Some(items) = section.get("items").and_then(|v| v.as_array()) {
                for item in items {
                    let value = value_to_string(item);
                    if !value.is_empty() {
                        println!("- {}", value);
                    }
                }
            }
        }
        "notice" => {
            if let Some(message) = section.get("message").and_then(|v| v.as_str()) {
                if !message.trim().is_empty() {
                    println!("{}", message);
                }
            }
        }
        "json" => {
            if let Some(data) = section.get("data") {
                match serde_json::to_string_pretty(data) {
                    Ok(pretty) => {
                        for line in pretty.lines() {
                            println!("{}", line);
                        }
                    }
                    Err(_) => println!("{}", value_to_string(data)),
                }
            }
        }
        _ => {
            // Unknown section types are intentionally ignored to keep rendering forward-compatible.
        }
    }
}

fn value_to_string(v: &Value) -> String {
    match v {
        Value::Null => "null".to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.to_string(),
        Value::Array(_) | Value::Object(_) => serde_json::to_string(v).unwrap_or_default(),
    }
}
