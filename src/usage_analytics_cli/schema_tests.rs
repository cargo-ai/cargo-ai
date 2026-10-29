//! Exercise the published schema's finite vocabulary without another dependency.
use serde_json::Value;

fn matches(value: &Value, schema: &Value, root: &Value) -> bool {
    if let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
        return reference
            .strip_prefix('#')
            .and_then(|path| root.pointer(path))
            .is_some_and(|schema| matches(value, schema, root));
    }
    if schema
        .get("const")
        .is_some_and(|expected| value != expected)
        || schema
            .get("enum")
            .and_then(Value::as_array)
            .is_some_and(|values| !values.contains(value))
    {
        return false;
    }
    for (name, exact) in [("oneOf", true), ("anyOf", false)] {
        if let Some(options) = schema.get(name).and_then(Value::as_array) {
            let count = options
                .iter()
                .filter(|option| matches(value, option, root))
                .count();
            if count == 0 || exact && count != 1 {
                return false;
            }
        }
    }
    if let Some(kind) = schema.get("type").and_then(Value::as_str) {
        let valid = match kind {
            "object" => value.is_object(),
            "array" => value.is_array(),
            "string" => value.is_string(),
            "boolean" => value.is_boolean(),
            "integer" => value.as_u64().is_some() || value.as_i64().is_some(),
            "null" => value.is_null(),
            _ => panic!("Unsupported schema type {kind}"),
        };
        if !valid {
            return false;
        }
    }
    if let Some(values) = value.as_object() {
        if schema
            .get("required")
            .and_then(Value::as_array)
            .is_some_and(|required| {
                required
                    .iter()
                    .any(|key| !values.contains_key(key.as_str().unwrap()))
            })
        {
            return false;
        }
        if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
            for (key, value) in values {
                match properties.get(key) {
                    Some(property) if !matches(value, property, root) => return false,
                    None if schema["additionalProperties"] == false => return false,
                    _ => {}
                }
            }
        }
    }
    if let Some(values) = value.as_array() {
        if schema
            .get("maxItems")
            .and_then(Value::as_u64)
            .is_some_and(|max| values.len() as u64 > max)
        {
            return false;
        }
        if let Some(items) = schema.get("items") {
            if values.iter().any(|value| !matches(value, items, root)) {
                return false;
            }
        }
    }
    if value.is_number() {
        let integer = value
            .as_i64()
            .map(i128::from)
            .or_else(|| value.as_u64().map(i128::from))
            .unwrap();
        for (key, lower) in [("minimum", true), ("maximum", false)] {
            if let Some(bound) = schema.get(key) {
                let bound = bound
                    .as_i64()
                    .map(i128::from)
                    .or_else(|| bound.as_u64().map(i128::from))
                    .unwrap();
                if lower && integer < bound || !lower && integer > bound {
                    return false;
                }
            }
        }
    }
    if let Some(text) = value.as_str() {
        if schema
            .get("maxLength")
            .and_then(Value::as_u64)
            .is_some_and(|max| text.chars().count() as u64 > max)
        {
            return false;
        }
        if let Some(pattern) = schema.get("pattern").and_then(Value::as_str) {
            let valid = match pattern {
                "^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$" => {
                    uuid::Uuid::parse_str(text).is_ok_and(|id| id.to_string() == text)
                }
                "^(cai_event_|cai_run_|cai_agent_run_)?[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$" => {
                    let suffix = ["cai_event_", "cai_run_", "cai_agent_run_"].iter()
                        .find_map(|prefix| text.strip_prefix(prefix)).unwrap_or(text);
                    uuid::Uuid::parse_str(suffix).is_ok_and(|id| id.to_string() == suffix)
                }
                "^[0-9a-f]{64}$" => {
                    text.len() == 64
                        && text
                            .bytes()
                            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                }
                pattern
                    if pattern
                        == root["$defs"]["decimal_sequence"]["pattern"]
                            .as_str()
                            .unwrap() =>
                {
                    text.parse::<i64>().is_ok()
                        && !text.is_empty()
                        && text.len() <= 19
                        && text.bytes().all(|b| b.is_ascii_digit())
                        && (text == "0" || !text.starts_with('0'))
                }
                _ => panic!("Unsupported schema pattern {pattern}"),
            };
            if !valid {
                return false;
            }
        }
    }
    true
}
fn schema() -> Value {
    serde_json::from_str(include_str!("../../docs/schemas/usage-maintenance-v2.json")).unwrap()
}
pub(crate) fn assert_valid(value: &Value) {
    let schema = schema();
    assert!(
        matches(value, &schema, &schema),
        "Payload violates published schema: {value}"
    );
    for pointer in ["/result", "/local"] {
        let mut unexpected = value.clone();
        unexpected
            .pointer_mut(pointer)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert("unexpected_private_field".into(), Value::Bool(true));
        assert!(
            !matches(&unexpected, &schema, &schema),
            "Schema must close {pointer}"
        );
    }
    let mut wrong_operation = value.clone();
    wrong_operation["operation"] =
        Value::String(if value["operation"] == "usage retention configure" {
            "usage cloud enable".into()
        } else {
            "usage retention configure".into()
        });
    assert!(
        !matches(&wrong_operation, &schema, &schema),
        "Operation must select its exact result"
    );
}
#[test]
fn machine_schema_closes_every_object_and_uses_supported_keywords() {
    fn visit(schema: &Value) {
        let object = schema.as_object().unwrap();
        for key in object.keys() {
            assert!(
                [
                    "$schema",
                    "$id",
                    "title",
                    "description",
                    "type",
                    "additionalProperties",
                    "required",
                    "properties",
                    "$ref",
                    "$defs",
                    "const",
                    "enum",
                    "oneOf",
                    "anyOf",
                    "minimum",
                    "maximum",
                    "pattern",
                    "maxLength",
                    "maxItems",
                    "items"
                ]
                .contains(&key.as_str()),
                "Unvalidated keyword: {key}"
            );
        }
        if schema["type"] == "object" {
            assert_eq!(schema["additionalProperties"], false);
            assert!(schema["properties"].is_object());
            assert!(schema["required"].is_array());
        }
        for keyword in ["properties", "$defs"] {
            if let Some(values) = schema[keyword].as_object() {
                for value in values.values() {
                    visit(value);
                }
            }
        }
        for keyword in ["oneOf", "anyOf"] {
            if let Some(values) = schema[keyword].as_array() {
                for value in values {
                    visit(value);
                }
            }
        }
        if schema.get("items").is_some() {
            visit(&schema["items"]);
        }
    }
    visit(&schema());
}

#[test]
fn machine_schema_preserves_shared_timing_and_counter_boundaries() {
    use crate::usage_analytics::contract::TimingSupplement;
    let root = schema();
    let timing =
        serde_json::to_value(TimingSupplement::monotonic(i64::MAX as u64).unwrap()).unwrap();
    assert!(matches(&timing, &root["$defs"]["timing"], &root));
    let mut invalid = timing;
    invalid["elapsed_ms"] = serde_json::json!(i64::MAX as u64 + 1);
    assert!(!matches(&invalid, &root["$defs"]["timing"], &root));
    invalid["elapsed_ms"] = serde_json::json!(0);
    invalid["basis"] = serde_json::json!("wall_clock");
    assert!(!matches(&invalid, &root["$defs"]["timing"], &root));
    for (value, accepted) in [
        ("0", true),
        ("9223372036854775807", true),
        ("9223372036854775808", false),
        ("01", false),
        ("-1", false),
    ] {
        assert_eq!(
            matches(
                &serde_json::json!(value),
                &root["$defs"]["decimal_sequence"],
                &root
            ),
            accepted
        );
    }
    for (value, accepted) in [
        (0, false),
        (1, true),
        (i64::MAX as u64, true),
        (i64::MAX as u64 + 1, false),
    ] {
        assert_eq!(
            matches(&serde_json::json!(value), &root["$defs"]["counter"], &root),
            accepted
        );
    }
}

#[test]
fn machine_history_event_ids_match_shared_membership_identity() {
    use crate::usage_analytics::contract::EventSubmission;
    let root = schema();
    let uuid = "abcdef01-2345-4000-8000-000000000001";
    let mut cases = ["", "cai_event_", "cai_run_", "cai_agent_run_"]
        .into_iter()
        .map(|prefix| (format!("{prefix}{uuid}"), true))
        .collect::<Vec<_>>();
    cases.extend([
        (format!("cai_tool_{uuid}"), false),
        (format!("cai_event_{}", uuid.to_uppercase()), false),
        (format!("cai_event_cai_run_{uuid}"), false),
        (format!("{uuid}x"), false),
        ("cai_event_not-a-uuid".into(), false),
    ]);
    for (event_id, expected) in cases {
        let member_valid = EventSubmission::from_allowed_text(
            serde_json::json!({"schema_version":1,"event_id":event_id}).to_string(),
        )
        .is_ok();
        let schema_valid = matches(
            &serde_json::json!(event_id),
            &root["$defs"]["logical_event_id"],
            &root,
        );
        assert_eq!(member_valid, expected, "shared membership: {event_id}");
        assert_eq!(schema_valid, member_valid, "schema identity: {event_id}");
    }
}
