//! Closed, path-free attribution document shared by backup clients and service.
use serde_json::Value;

fn fields(value: &Value, required: &[&str], optional: &[&str]) -> bool {
    value.as_object().is_some_and(|object| {
        required.iter().all(|key| object.contains_key(*key))
            && object
                .keys()
                .all(|key| required.contains(&key.as_str()) || optional.contains(&key.as_str()))
    })
}

fn opaque(value: &Value) -> bool {
    value
        .as_str()
        .is_some_and(|text| uuid::Uuid::parse_str(text).is_ok_and(|id| id.to_string() == text))
}

fn reason(value: &Value) -> bool {
    value.as_str().is_some_and(|reason| {
        matches!(
            reason,
            "missing_install_id"
                | "config_unavailable"
                | "missing_package_identity"
                | "invalid_authored_project_id"
                | "package_root_unavailable"
                | "canonicalization_failed"
                | "non_unicode_path"
                | "agent_key_unavailable"
                | "environment_unavailable"
                | "package_and_location_unavailable"
                | "current_directory_unavailable"
                | "project_marker_invalid"
                | "project_marker_unreadable"
                | "captured_path_invalid"
                | "inherited_context_unavailable"
                | "inherited_context_invalid"
                | "current_executable_unavailable"
                | "executable_unreadable"
                | "not_reported"
                | "invalid_value"
                | "caller_environment_unavailable"
        )
    })
}

pub fn validate_dimension(value: &Value, kind: &str) -> bool {
    match value["state"].as_str() {
        Some("known") => {
            let extra = match kind {
                "package_location" | "workspace" => &["environment_id"][..],
                "agent" => &["identity_scope"][..],
                _ => &[][..],
            };
            fields(value, &["state", "id"], extra)
                && opaque(&value["id"])
                && match kind {
                    "package_location" | "workspace" => opaque(&value["environment_id"]),
                    "agent" => matches!(
                        value["identity_scope"].as_str(),
                        Some("package" | "package_location")
                    ),
                    _ => true,
                }
        }
        Some("unavailable") => fields(value, &["state", "reason"], &[]) && reason(&value["reason"]),
        Some("none") if kind == "workspace" => fields(value, &["state"], &[]),
        _ => false,
    }
}

fn safe_code(value: &Value, limit: usize) -> bool {
    value.as_str().is_some_and(|text| {
        !text.is_empty()
            && text.len() <= limit
            && text
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"_.:+-".contains(&byte))
    })
}

/// Validate the complete version-one cloud attribution shape.
pub fn validate(value: &Value) -> bool {
    fields(
        value,
        &[
            "schema_version",
            "environment",
            "package",
            "package_location",
            "agent",
            "workspace",
            "package_revision",
            "agent_revision",
            "runtime",
        ],
        &[],
    ) && value["schema_version"].as_u64() == Some(1)
        && [
            "environment",
            "package",
            "package_location",
            "agent",
            "workspace",
            "package_revision",
            "agent_revision",
        ]
        .iter()
        .all(|kind| validate_dimension(&value[*kind], kind))
        && fields(
            &value["runtime"],
            &["kind", "version", "build_target", "executable", "build"],
            &["generator_version"],
        )
        && matches!(value["runtime"]["kind"].as_str(), Some("cli" | "generated"))
        && safe_code(&value["runtime"]["version"], 128)
        && safe_code(&value["runtime"]["build_target"], 128)
        && value["runtime"]
            .get("generator_version")
            .is_none_or(|version| safe_code(version, 128))
        && validate_dimension(&value["runtime"]["executable"], "executable")
        && validate_dimension(&value["runtime"]["build"], "build")
        && (value["package_location"]["state"] != "known"
            || value["environment"]["state"] == "known"
                && value["package_location"]["environment_id"] == value["environment"]["id"])
        && (value["agent"]["state"] != "known"
            || match value["agent"]["identity_scope"].as_str() {
                Some("package") => value["package"]["state"] == "known",
                Some("package_location") => value["package_location"]["state"] == "known",
                _ => false,
            })
        && (value["package_revision"]["state"] != "known" || value["package"]["state"] == "known")
        && (value["agent_revision"]["state"] != "known" || value["agent"]["state"] == "known")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn closed_dimensions_reject_inconsistent_relationships_and_private_fields() {
        let id = "00000000-0000-4000-8000-000000000001";
        let mut record = json!({"schema_version":1,
            "environment":{"state":"known","id":id},
            "package":{"state":"known","id":id},
            "package_location":{"state":"known","id":id,"environment_id":id},
            "agent":{"state":"known","id":id,"identity_scope":"package"},
            "workspace":{"state":"none"},
            "package_revision":{"state":"known","id":id},
            "agent_revision":{"state":"known","id":id},
            "runtime":{"kind":"cli","version":"0.4.4","build_target":"aarch64-apple-darwin",
                "executable":{"state":"known","id":id},"build":{"state":"known","id":id}}
        });
        assert!(validate(&record));
        record["package_location"]["environment_id"] =
            json!("00000000-0000-4000-8000-000000000002");
        assert!(!validate(&record));
        record["package_location"]["environment_id"] = json!(id);
        record["package"] = json!({"state":"unavailable","reason":"missing_package_identity"});
        assert!(!validate(&record));
        record["package"] = json!({"state":"known","id":id,"path":"/private"});
        assert!(!validate(&record));
    }
}
