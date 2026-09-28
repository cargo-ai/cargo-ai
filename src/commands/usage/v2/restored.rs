//! Opaque restored identities retain their authenticated backup namespace.
use rusqlite::types::Value as Parameter;
use serde_json::{json, Value};

fn uuid(value: &Value) -> bool {
    value
        .as_str()
        .is_some_and(|s| uuid::Uuid::parse_str(s).is_ok_and(|id| id.to_string() == s))
}

pub(crate) fn attribution(event: &Value) -> Option<&Value> {
    let restored = &event["restored_attribution"];
    let namespace = &restored["namespace"];
    if event["restored"] != true
        || restored["schema_version"] != 1
        || restored.as_object()?.len() != 3
        || namespace.as_object()?.len() != 2
        || namespace["kind"] != "usage_backup"
        || !uuid(&namespace["account_binding"])
        || !crate::usage_backup::attribution::validate(&restored["value"])
    {
        return None;
    }
    Some(&restored["value"])
}

fn dimension_path(name: &str) -> Option<&'static str> {
    Some(match name {
        "environment" => "environment",
        "package" => "package",
        "package_location" => "package_location",
        "agent" => "agent",
        "workspace" => "workspace",
        "package_revision" => "package_revision",
        "agent_revision" => "agent_revision",
        "runtime_version" => "runtime.version",
        "runtime_digest" => "runtime.build",
        _ => return None,
    })
}

pub(crate) fn dimension(event: &Value, name: &str) -> Option<Value> {
    let attr = attribution(event)?;
    dimension_path(name)?;
    let component = match name {
        "runtime_digest" => attr["runtime"]["build"].clone(),
        "runtime_version" => match attr["runtime"]["version"].as_str() {
            Some(version) => json!({"state":"known", "value":version}),
            None => json!({"state":"unavailable", "reason":"not_recorded"}),
        },
        _ => attr[name].clone(),
    };
    let mut key = component.as_object()?.clone();
    key.insert(
        "namespace".into(),
        event["restored_attribution"]["namespace"].clone(),
    );
    key.insert("dimension".into(), json!(name));
    let mut value = Value::Object(key);
    value["filter_value"] = json!(value.to_string());
    Some(value)
}

pub(crate) fn has_namespace(raw: &str) -> bool {
    serde_json::from_str::<Value>(raw)
        .ok()
        .is_some_and(|v| v.get("namespace").is_some())
}

fn equals(filter: &mut String, values: &mut Vec<Parameter>, path: &str, value: &str) {
    values.push(Parameter::Text(value.into()));
    filter.push_str(&format!(
        " AND json_extract(CASE WHEN json_valid(record_json) THEN record_json END,'{path}')=?{}",
        values.len()
    ));
}

/// All SQL paths come from this fixed contract; supplied values are parameters.
pub(crate) fn append_filter(
    flag: &str,
    raw: &str,
    filter: &mut String,
    values: &mut Vec<Parameter>,
) -> Result<bool, String> {
    if !has_namespace(raw) {
        return Ok(false);
    }
    let invalid = || format!("--{flag} requires an exact restored group filter_value");
    if raw.len() > 4096 {
        return Err(invalid());
    }
    let value: Value = serde_json::from_str(raw).map_err(|_| invalid())?;
    let fields = value.as_object().ok_or_else(invalid)?;
    let name = flag.replace('-', "_");
    let path = dimension_path(&name).ok_or_else(invalid)?;
    let namespace = &value["namespace"];
    if value["dimension"] != name
        || namespace.as_object().is_none_or(|n| n.len() != 2)
        || namespace["kind"] != "usage_backup"
        || !uuid(&namespace["account_binding"])
    {
        return Err(invalid());
    }
    let mut expected = vec!["namespace", "dimension", "state"];
    let mut predicates = Vec::new();
    match value["state"].as_str() {
        Some("known") => {
            if name == "runtime_version" {
                expected.push("value");
                if value["value"].as_str().is_none_or(|s| {
                    s.is_empty()
                        || s.len() > 128
                        || !s
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b"_.:+-".contains(&b))
                }) {
                    return Err(invalid());
                }
                predicates.push((format!("$.restored_attribution.value.{path}"), "value"));
            } else {
                expected.push("id");
                if !uuid(&value["id"]) {
                    return Err(invalid());
                }
                predicates.push((format!("$.restored_attribution.value.{path}.id"), "id"));
                if matches!(name.as_str(), "package_location" | "workspace") {
                    expected.push("environment_id");
                    if !uuid(&value["environment_id"]) {
                        return Err(invalid());
                    }
                    predicates.push((
                        format!("$.restored_attribution.value.{path}.environment_id"),
                        "environment_id",
                    ));
                }
                if name == "agent" {
                    expected.push("identity_scope");
                    if !matches!(
                        value["identity_scope"].as_str(),
                        Some("package" | "package_location")
                    ) {
                        return Err(invalid());
                    }
                    predicates.push((
                        format!("$.restored_attribution.value.{path}.identity_scope"),
                        "identity_scope",
                    ));
                }
            }
        }
        Some("none") if name == "workspace" => (),
        Some("unavailable") if name != "runtime_version" => {
            expected.push("reason");
            if value["reason"].as_str().is_none_or(|s| {
                s.is_empty()
                    || s.len() > 64
                    || !s.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')
            }) {
                return Err(invalid());
            }
            predicates.push((
                format!("$.restored_attribution.value.{path}.reason"),
                "reason",
            ));
        }
        Some("unavailable") if name == "runtime_version" && value["reason"] == "not_recorded" => {
            expected.push("reason");
        }
        _ => return Err(invalid()),
    }
    if fields.len() != expected.len() || fields.keys().any(|k| !expected.contains(&k.as_str())) {
        return Err(invalid());
    }
    if name != "runtime_version" {
        let mut component = value.clone();
        component.as_object_mut().unwrap().remove("namespace");
        component.as_object_mut().unwrap().remove("dimension");
        let kind = if name == "runtime_digest" {
            "build"
        } else {
            &name
        };
        if !crate::usage_backup::attribution::validate_dimension(&component, kind) {
            return Err(invalid());
        }
    }
    filter.push_str(" AND json_valid(record_json)");
    let safe = "CASE WHEN json_valid(record_json) THEN record_json END";
    filter.push_str(&format!(" AND COALESCE(json_extract({safe},'$.attribution.schema_version'),0)<>1 AND json_extract({safe},'$.restored')=1 AND json_extract({safe},'$.restored_attribution.schema_version')=1 AND json_extract({safe},'$.restored_attribution.value.schema_version')=1"));
    equals(
        filter,
        values,
        "$.restored_attribution.namespace.kind",
        "usage_backup",
    );
    equals(
        filter,
        values,
        "$.restored_attribution.namespace.account_binding",
        namespace["account_binding"].as_str().unwrap(),
    );
    if name == "runtime_version" {
        if value["state"] == "unavailable" {
            filter.push_str(&format!(
                " AND json_extract({safe},'$.restored_attribution.value.runtime.version') IS NULL"
            ));
        }
    } else {
        equals(
            filter,
            values,
            &format!("$.restored_attribution.value.{path}.state"),
            value["state"].as_str().unwrap(),
        );
    }
    for (path, key) in predicates {
        equals(filter, values, &path, value[key].as_str().unwrap());
    }
    Ok(true)
}
