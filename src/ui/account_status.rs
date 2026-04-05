use serde_json::Value;

pub fn render_backend_ui(response: &Value) -> bool {
    let Some(lines) = render_backend_ui_lines(response) else {
        return false;
    };

    for line in lines {
        println!("{line}");
    }

    true
}

pub fn render_account_status_ui(response: &Value) -> bool {
    render_backend_ui(response)
}

fn render_backend_ui_lines(response: &Value) -> Option<Vec<String>> {
    let ui = response.get("ui")?;
    let schema = ui.get("schema").and_then(|v| v.as_str());
    if schema != Some("1.0") {
        return None;
    }

    let kind = ui.get("kind").and_then(|v| v.as_str()).unwrap_or("info");
    let title = ui.get("title").and_then(|v| v.as_str()).unwrap_or("Status");
    let summary = ui
        .get("summary")
        .and_then(|v| v.as_str())
        .unwrap_or("Status response received.");

    let kind_prefix = ui
        .get("icon")
        .and_then(|v| v.as_str())
        .filter(|icon| !icon.trim().is_empty())
        .unwrap_or_else(|| match kind {
            "success" => "✅",
            "error" => "⚠️",
            "failure" => "❌",
            _ => "ℹ️",
        });

    let mut lines = Vec::new();
    lines.push(format!("{kind_prefix} {title}"));
    push_multiline(&mut lines, summary);

    if let Some(variant) = ui.get("variant").and_then(|v| v.as_str()) {
        if !variant.trim().is_empty() {
            lines.push(format!("Variant: {variant}"));
        }
    }

    if let Some(sections) = ui.get("sections").and_then(|v| v.as_array()) {
        for section in sections {
            render_section(&mut lines, section);
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
                lines.push(String::new());
                lines.push("Actions:".to_string());
                printed_header = true;
            }

            if !label.is_empty() && !command.is_empty() {
                lines.push(format!("- {label}: {command}"));
            } else if !label.is_empty() {
                lines.push(format!("- {label}"));
            } else {
                lines.push(format!("- {command}"));
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
                lines.push(String::new());
                lines.push("Next steps:".to_string());
                printed_header = true;
            }

            lines.push(format!("- {text}"));
        }
    }

    Some(lines)
}

fn render_section(lines: &mut Vec<String>, section: &Value) {
    let section_type = section.get("type").and_then(|v| v.as_str()).unwrap_or("");
    let title = section.get("title").and_then(|v| v.as_str()).unwrap_or("");
    let title_style = section
        .get("title_style")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if !title.is_empty() {
        lines.push(String::new());
        if title_style == "plain" {
            lines.push(title.to_string());
        } else {
            lines.push(format!("{title}:"));
        }
    }

    match section_type {
        "kv" => {
            if let Some(items) = section.get("items").and_then(|v| v.as_array()) {
                let aligned_layout = section
                    .get("layout")
                    .and_then(|v| v.as_str())
                    .map(|layout| layout == "aligned")
                    .unwrap_or(false);

                let rendered_items: Vec<(String, String)> = items
                    .iter()
                    .map(|item| {
                        (
                            item.get("label")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string(),
                            item.get("value").map(value_to_string).unwrap_or_default(),
                        )
                    })
                    .filter(|(label, value)| !(label.is_empty() && value.is_empty()))
                    .collect();

                if aligned_layout {
                    let label_width = rendered_items
                        .iter()
                        .map(|(label, _)| label.len())
                        .max()
                        .unwrap_or(0);

                    for (label, value) in rendered_items {
                        if label.is_empty() {
                            lines.push(format!("  {value}"));
                        } else {
                            lines.push(format!("  {label:<width$}  {value}", width = label_width));
                        }
                    }
                } else {
                    for (label, value) in rendered_items {
                        if label.is_empty() {
                            lines.push(format!("- {value}"));
                        } else {
                            lines.push(format!("- {label}: {value}"));
                        }
                    }
                }
            }
        }
        "list" => {
            if let Some(items) = section.get("items").and_then(|v| v.as_array()) {
                for item in items {
                    let value = value_to_string(item);
                    if !value.is_empty() {
                        lines.push(format!("- {value}"));
                    }
                }
            }
        }
        "notice" => {
            if let Some(message) = section.get("message").and_then(|v| v.as_str()) {
                if !message.trim().is_empty() {
                    push_multiline(lines, message);
                }
            }
        }
        "json" => {
            if let Some(data) = section.get("data") {
                match serde_json::to_string_pretty(data) {
                    Ok(pretty) => {
                        for line in pretty.lines() {
                            lines.push(line.to_string());
                        }
                    }
                    Err(_) => lines.push(value_to_string(data)),
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

fn push_multiline(lines: &mut Vec<String>, text: &str) {
    for line in text.split('\n') {
        lines.push(line.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::render_backend_ui_lines;
    use serde_json::json;

    #[test]
    fn renders_aligned_plain_sections_for_status_style_ui() {
        let response = json!({
            "ui": {
                "schema": "1.0",
                "kind": "success",
                "icon": "✓",
                "title": "Account status",
                "summary": "Basic plan active.",
                "sections": [
                    {
                        "type": "kv",
                        "title": "Account",
                        "title_style": "plain",
                        "layout": "aligned",
                        "items": [
                            {"label": "Email", "value": "jane@example.com"},
                            {"label": "Handle", "value": "demo_handle"}
                        ]
                    },
                    {
                        "type": "kv",
                        "title": "Next steps",
                        "title_style": "plain",
                        "layout": "aligned",
                        "items": [
                            {"label": "Change handle", "value": "cargo ai account handle --set <handle>"},
                            {"label": "List agents", "value": "cargo ai account agents list"}
                        ]
                    }
                ]
            }
        });

        let lines = render_backend_ui_lines(&response).expect("expected rendered lines");
        assert_eq!(lines[0], "✓ Account status");
        assert_eq!(lines[1], "Basic plan active.");
        assert!(lines.iter().any(|line| line == "Account"));
        assert!(lines
            .iter()
            .any(|line| line == "  Email   jane@example.com"));
        assert!(lines.iter().any(|line| line == "  Handle  demo_handle"));
        assert!(lines.iter().any(|line| line == "Next steps"));
        assert!(lines
            .iter()
            .any(|line| { line == "  Change handle  cargo ai account handle --set <handle>" }));
    }

    #[test]
    fn keeps_existing_bullet_kv_behavior_without_aligned_layout() {
        let response = json!({
            "ui": {
                "schema": "1.0",
                "kind": "success",
                "title": "Mail preferences",
                "summary": "All account emails are currently enabled.",
                "sections": [
                    {
                        "type": "kv",
                        "title": "Summary",
                        "items": [
                            {"label": "State", "value": "Enabled"}
                        ]
                    }
                ]
            }
        });

        let lines = render_backend_ui_lines(&response).expect("expected rendered lines");
        assert!(lines.iter().any(|line| line == "Summary:"));
        assert!(lines.iter().any(|line| line == "- State: Enabled"));
    }
}
