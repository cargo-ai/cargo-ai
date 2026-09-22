//! Closed cloud metadata projection. Local labels and paths never leave the device.
use crate::usage_store::db_error;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Map, Value};

fn opaque(db: &Connection, kind: &str, value: &str) -> Result<String, String> {
    let existing: Option<String> = db
        .query_row(
            "SELECT opaque_id FROM usage_backup_ids WHERE kind=?1 AND local_value=?2",
            params![kind, value],
            |r| r.get(0),
        )
        .optional()
        .map_err(db_error)?;
    if let Some(id) = existing {
        return Ok(id);
    }
    let id = uuid::Uuid::new_v4().to_string();
    db.execute(
        "INSERT OR IGNORE INTO usage_backup_ids(kind,local_value,opaque_id) VALUES(?1,?2,?3)",
        params![kind, value, id],
    )
    .map_err(db_error)?;
    db.query_row(
        "SELECT opaque_id FROM usage_backup_ids WHERE kind=?1 AND local_value=?2",
        params![kind, value],
        |r| r.get(0),
    )
    .map_err(db_error)
}
fn code(value: &Value, max: usize) -> Option<Value> {
    value
        .as_str()
        .filter(|s| {
            !s.is_empty()
                && s.len() <= max
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_.:-".contains(&b))
        })
        .map(|s| json!(s))
}
fn model(value: &Value) -> Option<Value> {
    value
        .as_str()
        .filter(|s| {
            !s.is_empty()
                && s.len() <= 256
                && !s.starts_with('/')
                && !s.contains("..")
                && !s.contains("://")
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_.:/-".contains(&b))
        })
        .map(|s| json!(s))
}
fn millis(value: &Value) -> Option<Value> {
    time::OffsetDateTime::parse(
        value.as_str()?,
        &time::format_description::well_known::Rfc3339,
    )
    .ok()
    .and_then(|v| i64::try_from(v.unix_timestamp_nanos() / 1_000_000).ok())
    .filter(|v| *v >= 0)
    .map(|v| json!(v))
}
fn id(value: &Value) -> Option<Value> {
    let s = value.as_str()?;
    let raw = [
        "cai_event_",
        "cai_run_",
        "cai_agent_run_",
        "cai_operation_",
        "cai_attempt_",
        "cai_tool_run_",
    ]
    .iter()
    .find_map(|prefix| s.strip_prefix(prefix))
    .unwrap_or(s);
    let uuid = uuid::Uuid::parse_str(raw).ok()?;
    if uuid.to_string() != raw {
        return None;
    }
    if s.starts_with("cai_operation_")
        || s.starts_with("cai_attempt_")
        || s.starts_with("cai_tool_run_")
    {
        Some(json!(raw))
    } else {
        Some(json!(s))
    }
}

fn cloud_id(value: &Value) -> bool {
    value.as_str().is_some_and(|value| {
        let raw = ["cai_event_", "cai_run_", "cai_agent_run_"]
            .iter()
            .find_map(|prefix| value.strip_prefix(prefix))
            .unwrap_or(value);
        uuid::Uuid::parse_str(raw).is_ok_and(|id| id.to_string() == raw)
    })
}

fn family(kind: &str) -> Option<&'static str> {
    match kind {
        "usage_log_started" | "agent_run_started" => Some("run_started"),
        "agent_run_completed" | "root_run_completed" => Some("run_finished"),
        "provider_request_started" | "provider_request_completed" => Some("provider_request"),
        "tool_run_started" | "tool_run_completed" => Some("tool_call"),
        _ => None,
    }
}

fn valid_details(value: &Value) -> bool {
    value.is_null()
        || value.as_object().is_some_and(|fields| {
            fields.iter().all(|(key, value)| match key.as_str() {
                "cached_tokens"
                | "audio_tokens"
                | "image_tokens"
                | "text_tokens"
                | "reasoning_tokens"
                | "accepted_prediction_tokens"
                | "rejected_prediction_tokens"
                | "cache_creation_input_tokens"
                | "cache_read_input_tokens"
                | "total_cached_tokens"
                | "total_thought_tokens"
                | "total_tool_use_tokens" => value.is_null() || value.as_u64().is_some(),
                "input_tokens_by_modality" | "output_tokens_by_modality" => {
                    value.is_null()
                        || value.as_array().is_some_and(|items| {
                            items.len() <= 16
                                && items.iter().all(|item| {
                                    item.as_object().is_some_and(|row| {
                                        row.len() == 2
                                            && row
                                                .get("modality")
                                                .and_then(Value::as_str)
                                                .is_some_and(|m| {
                                                    matches!(
                                                        m,
                                                        "TEXT"
                                                            | "IMAGE"
                                                            | "AUDIO"
                                                            | "VIDEO"
                                                            | "DOCUMENT"
                                                            | "text"
                                                            | "image"
                                                            | "audio"
                                                            | "video"
                                                            | "document"
                                                    )
                                                })
                                            && (row
                                                .get("token_count")
                                                .or_else(|| row.get("tokenCount")))
                                            .and_then(Value::as_u64)
                                            .is_some()
                                    })
                                })
                        })
                }
                _ => false,
            })
        })
}

/// Treat remote metadata as an untrusted versioned document, including cached
/// projections attached to restored events. Reject extra fields instead of copying them.
pub(super) fn validate_cloud_record(record: &Value) -> Result<(), String> {
    let invalid = || "Backup contains an invalid or unsupported metadata record".to_owned();
    if serde_json::to_vec(record).map_err(|_| invalid())?.len() > 16 * 1024 {
        return Err(invalid());
    }
    let fields = record.as_object().ok_or_else(invalid)?;
    if record["schema_version"].as_u64() != Some(1)
        || record["normalization_version"].as_u64() != Some(1)
        || ["event_id", "root_run_id", "agent_run_id"]
            .iter()
            .any(|key| !cloud_id(&record[*key]))
        || !record["occurred_at_ms"].as_i64().is_some_and(|v| v >= 0)
        || record["event_kind"].as_str().and_then(family).is_none()
        || record["event_kind"].as_str().and_then(family) != record["event_type"].as_str()
    {
        return Err(invalid());
    }
    for (key, value) in fields {
        let valid = match key.as_str() {
            "schema_version" | "normalization_version" | "event_type" | "event_kind" => true,
            "event_id" | "root_run_id" | "agent_run_id" | "parent_run_id" | "device_id"
            | "operation_id" | "request_attempt_id" | "project_id" | "profile_id" | "agent_id"
            | "tool_id" | "action_id" => value.is_null() || cloud_id(value),
            "occurred_at_ms" | "started_at_ms" | "ended_at_ms" => {
                value.is_null()
                    || value.as_i64().is_some_and(|ms| {
                        ms >= 0
                            && time::OffsetDateTime::from_unix_timestamp_nanos(
                                i128::from(ms) * 1_000_000,
                            )
                            .is_ok()
                    })
            }
            "elapsed_ms"
            | "attempt_index"
            | "retry_count"
            | "tool_invocation_count"
            | "first_response_chunk_ms"
            | "step_index"
            | "depth" => value.is_null() || value.as_i64().is_some_and(|v| v >= 0),
            "http_status" => {
                value.is_null() || value.as_u64().is_some_and(|v| (100..=599).contains(&v))
            }
            "provider" => {
                value.is_null()
                    || value.as_str().is_some_and(|s| {
                        matches!(
                            s,
                            "openai"
                                | "anthropic"
                                | "gemini"
                                | "mistral"
                                | "ollama"
                                | "typesafe"
                                | "xai"
                        )
                    })
            }
            "requested_model" | "resolved_model" => value.is_null() || model(value).is_some(),
            "provider_request_id" => value.is_null() || code(value, 256).is_some(),
            "runtime_version" | "package_revision" => value.is_null() || code(value, 128).is_some(),
            "operation_kind" | "error_category" | "finish_reason" | "token_semantics" => {
                value.is_null() || code(value, 64).is_some()
            }
            "provider_completion" => {
                value.is_null()
                    || value.as_str().is_some_and(|s| {
                        matches!(
                            s,
                            "unknown"
                                | "response_received"
                                | "unknown_or_failed_before_response"
                                | "not_dispatched"
                        )
                    })
            }
            "status" => {
                value.is_null()
                    || value.as_str().is_some_and(|s| {
                        matches!(
                            s,
                            "started"
                                | "succeeded"
                                | "failed"
                                | "cancelled"
                                | "interrupted"
                                | "unknown"
                        )
                    })
            }
            "coverage" => {
                value.is_null()
                    || value.as_str().is_some_and(|s| {
                        matches!(s, "reported" | "derived" | "unavailable" | "incomplete")
                    })
            }
            "capture_incomplete" => value.is_null() || value.is_boolean(),
            "tokens" => {
                value.is_null()
                    || value.as_object().is_some_and(|tokens| {
                        tokens.iter().all(|(key, value)| {
                            matches!(
                                key.as_str(),
                                "input_tokens"
                                    | "output_tokens"
                                    | "total_tokens"
                                    | "cache_read_tokens"
                                    | "cache_write_tokens"
                                    | "reasoning_tokens"
                                    | "input_audio_tokens"
                                    | "output_audio_tokens"
                                    | "input_image_tokens"
                                    | "output_image_tokens"
                            ) && (value.is_null() || value.as_u64().is_some())
                        })
                    })
            }
            "input_token_details" | "output_token_details" => valid_details(value),
            "token_coverage" => {
                value.is_null()
                    || value.as_object().is_some_and(|coverage| {
                        coverage.iter().all(|(key, value)| match key.as_str() {
                            "input_tokens" | "output_tokens" | "total_tokens" => {
                                value.as_str().is_some_and(|v| {
                                    matches!(
                                        v,
                                        "reported"
                                            | "derived_input_plus_output"
                                            | "reported_or_provider_native_sum"
                                            | "unavailable"
                                    )
                                })
                            }
                            "normalization_version" => value.as_u64() == Some(1),
                            "detail_semantics" => {
                                value.as_str()
                                    == Some("provider_native_details_may_overlap_headline_counters")
                            }
                            _ => false,
                        })
                    })
            }
            _ => false,
        };
        if !valid {
            return Err(invalid());
        }
    }
    Ok(())
}

pub(super) fn project(db: &Connection, event: &Value) -> Result<Value, String> {
    // Restored facts carry their exact accepted projection, so another device can
    // compare immutable identity without replacing its original private labels.
    if let Some(projection) = event.get("backup_projection") {
        validate_cloud_record(projection)?;
        if ["event_id", "root_run_id", "agent_run_id"]
            .iter()
            .any(|key| event[*key] != projection[*key])
            || event["event_type"] != projection["event_kind"]
        {
            return Err("Restored usage identity differs from its backup projection".into());
        }
        return Ok(projection.clone());
    }
    let kind = event["event_type"]
        .as_str()
        .ok_or("Usage event lacks its kind")?;
    let family = match kind {
        "usage_log_started" | "agent_run_started" => "run_started",
        "agent_run_completed" | "root_run_completed" => "run_finished",
        "provider_request_started" | "provider_request_completed" => "provider_request",
        "tool_run_started" | "tool_run_completed" => "tool_call",
        _ => return Err("Usage event needs a newer backup projection".into()),
    };
    let mut out = Map::new();
    out.insert("schema_version".into(), json!(1));
    out.insert("normalization_version".into(), json!(1));
    out.insert("event_type".into(), json!(family));
    out.insert("event_kind".into(), json!(kind));
    for key in ["event_id", "root_run_id", "agent_run_id"] {
        out.insert(
            key.into(),
            id(&event[key]).ok_or("Usage event has an invalid identity")?,
        );
    }
    out.insert(
        "occurred_at_ms".into(),
        millis(&event["timestamp"]).ok_or("Usage event has an invalid timestamp")?,
    );
    for (local, remote) in [
        ("parent_agent_run_id", "parent_run_id"),
        ("operation_id", "operation_id"),
        ("attempt_id", "request_attempt_id"),
    ] {
        if let Some(value) = id(&event[local]) {
            out.insert(remote.into(), value);
        }
    }
    for (local, remote) in [("started_at", "started_at_ms"), ("ended_at", "ended_at_ms")] {
        if let Some(value) = millis(&event[local]) {
            out.insert(remote.into(), value);
        }
    }
    for (local, remote) in [
        ("duration_ms", "elapsed_ms"),
        ("attempt_index", "attempt_index"),
        ("retry_count", "retry_count"),
        ("depth", "depth"),
    ] {
        if let Some(value) = event[local].as_i64().filter(|v| *v >= 0) {
            out.insert(remote.into(), json!(value));
        }
    }
    for key in ["runtime_version", "package_revision"] {
        if let Some(value) = code(&event[key], 128) {
            out.insert(key.into(), value);
        }
    }
    for (value, key, limit) in [
        (&event["provider_request_id"], "provider_request_id", 256),
        (&event["finish_reason"], "finish_reason", 64),
        (&event["error"]["kind"], "error_category", 64),
        (&event["step"]["kind"], "operation_kind", 64),
    ] {
        if let Some(value) = code(value, limit) {
            out.insert(key.into(), value);
        }
    }
    if let Some(value) = event["provider_completion"].as_str().filter(|s| {
        matches!(
            *s,
            "unknown"
                | "response_received"
                | "unknown_or_failed_before_response"
                | "not_dispatched"
        )
    }) {
        out.insert("provider_completion".into(), json!(value));
    }
    if let Some(status) = event["error"]["http_status"]
        .as_u64()
        .filter(|n| (100..=599).contains(n))
    {
        out.insert("http_status".into(), json!(status));
    }
    if let Some(provider) = event["provider"]["server"].as_str().filter(|s| {
        matches!(
            *s,
            "openai" | "anthropic" | "gemini" | "mistral" | "ollama" | "typesafe" | "xai"
        )
    }) {
        out.insert("provider".into(), json!(provider));
    }
    for key in ["requested_model", "resolved_model"] {
        if let Some(value) = model(&event["provider"][key]) {
            out.insert(key.into(), value);
        }
    }
    for (kind, value) in [
        ("project", &event["agent"]["project_root"]),
        ("agent", &event["agent"]),
        ("profile", &event["provider"]["profile"]),
        ("action", &event["step"]["action"]),
        ("tool", &event["tool"]["name"]),
    ] {
        if !value.is_null() {
            out.insert(
                format!("{kind}_id"),
                json!(opaque(db, kind, &value.to_string())?),
            );
        }
    }
    if !event["tool"]["action"].is_null() {
        out.insert(
            "action_id".into(),
            json!(opaque(db, "action", &event["tool"]["action"].to_string())?),
        );
    }
    if let Some(value) = event["step"]["step_index"]
        .as_i64()
        .or_else(|| event["tool"]["step_index"].as_i64())
        .filter(|v| *v >= 0)
    {
        out.insert("step_index".into(), json!(value));
    }
    out.insert("device_id".into(), json!(opaque(db, "device", "current")?));
    out.insert(
        "status".into(),
        json!(match event["status"].as_str() {
            Some("success") => "succeeded",
            Some("failed") => "failed",
            Some("cancelled") => "cancelled",
            _ if kind.ends_with("started") => "started",
            _ => "unknown",
        }),
    );
    if kind == "tool_run_completed" {
        out.insert("tool_invocation_count".into(), json!(1));
    }
    let usage = &event["usage"];
    let mut tokens = Map::new();
    for key in [
        "input_tokens",
        "output_tokens",
        "total_tokens",
        "cache_read_tokens",
        "cache_write_tokens",
        "reasoning_tokens",
        "input_audio_tokens",
        "output_audio_tokens",
        "input_image_tokens",
        "output_image_tokens",
    ] {
        if let Some(value) = usage[key].as_u64() {
            tokens.insert(key.into(), json!(value));
        }
    }
    out.insert(
        "coverage".into(),
        json!(if tokens.is_empty() {
            "unavailable"
        } else {
            "reported"
        }),
    );
    if !tokens.is_empty() {
        out.insert("tokens".into(), Value::Object(tokens));
    }
    out.insert(
        "token_semantics".into(),
        json!("provider_native_details_may_overlap_headline_counters"),
    );
    let mut coverage = Map::new();
    for key in ["input_tokens", "output_tokens", "total_tokens"] {
        if let Some(value) = event["coverage"][key].as_str().filter(|v| {
            matches!(
                *v,
                "reported"
                    | "derived_input_plus_output"
                    | "reported_or_provider_native_sum"
                    | "unavailable"
            )
        }) {
            coverage.insert(key.into(), json!(value));
        }
    }
    if !coverage.is_empty() {
        out.insert("token_coverage".into(), Value::Object(coverage));
    }
    for key in ["input_token_details", "output_token_details"] {
        if let Some(details) = crate::providers::runtime::sanitize_token_details(&usage[key]) {
            out.insert(key.into(), details);
        }
    }
    if let Some(incomplete) = event["capture_incomplete"].as_bool() {
        out.insert("capture_incomplete".into(), json!(incomplete));
        if incomplete {
            out.insert("coverage".into(), json!("incomplete"));
        }
    }
    let result = Value::Object(out);
    if serde_json::to_vec(&result)
        .map_err(|_| "Cannot encode backup metadata")?
        .len()
        > 16 * 1024
    {
        return Err("Usage metadata exceeds the backup record limit".into());
    }
    validate_cloud_record(&result)?;
    Ok(result)
}

pub(super) fn restore(record: &Value) -> Result<Value, String> {
    validate_cloud_record(record)?;
    let ms = record["occurred_at_ms"]
        .as_i64()
        .ok_or("Backup record lacks its timestamp")?;
    let timestamp = time::OffsetDateTime::from_unix_timestamp_nanos(i128::from(ms) * 1_000_000)
        .map_err(|_| "Invalid backup timestamp")?
        .format(&time::format_description::well_known::Rfc3339)
        .map_err(|_| "Invalid backup timestamp")?;
    let mut event = json!({"schema_version":1,"event_id":record["event_id"],"root_run_id":record["root_run_id"],"agent_run_id":record["agent_run_id"],"parent_agent_run_id":record["parent_run_id"],"event_type":record["event_kind"],"timestamp":timestamp,"runtime_version":record["runtime_version"],"duration_ms":record["elapsed_ms"],"status":if record["status"]=="succeeded"{"success"}else{record["status"].as_str().unwrap_or("unknown")},"provider":{"server":record["provider"],"model":record["requested_model"],"requested_model":record["requested_model"],"resolved_model":record["resolved_model"],"profile":record["profile_id"]},"provider_request_id":record["provider_request_id"],"finish_reason":record["finish_reason"],"usage":record.get("tokens").cloned().unwrap_or(Value::Null),"coverage":record["token_coverage"],"backup_projection":record,"restored":true});
    for key in [
        "attempt_index",
        "retry_count",
        "depth",
        "package_revision",
        "capture_incomplete",
        "provider_completion",
    ] {
        if !record[key].is_null() {
            event[key] = record[key].clone();
        }
    }
    for key in ["input_token_details", "output_token_details"] {
        if !record[key].is_null() {
            if event["usage"].is_null() {
                event["usage"] = json!({});
            }
            event["usage"][key] = record[key].clone();
        }
    }
    if !event["usage"].is_null() {
        if let Some(source) = record["token_coverage"]["total_tokens"]
            .as_str()
            .filter(|source| matches!(*source, "reported" | "derived_input_plus_output"))
        {
            event["usage"]["total_tokens_source"] = json!(source);
        }
    }
    for (source, target) in [("started_at_ms", "started_at"), ("ended_at_ms", "ended_at")] {
        if let Some(ms) = record[source].as_i64() {
            event[target] = json!(time::OffsetDateTime::from_unix_timestamp_nanos(
                i128::from(ms) * 1_000_000
            )
            .map_err(|_| "Invalid backup timestamp")?
            .format(&time::format_description::well_known::Rfc3339)
            .map_err(|_| "Invalid backup timestamp")?);
        }
    }
    event["agent"] = json!({"project_id":record["project_id"],"agent_id":record["agent_id"]});
    if !record["coverage"].is_null() {
        event["capture_coverage"] = record["coverage"].clone();
    }
    event["operation_id"] = record["operation_id"].clone();
    event["attempt_id"] = record["request_attempt_id"].clone();
    event["step"] = json!({"kind":record["operation_kind"],"action":record["action_id"],"step_index":record["step_index"]});
    event["tool"] = json!({"name":record["tool_id"],"action":record["action_id"],"step_index":record["step_index"]});
    if !record["error_category"].is_null() {
        event["error"] =
            json!({"kind":record["error_category"],"http_status":record["http_status"]});
    }
    Ok(event)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record() -> Value {
        json!({"schema_version":1,"normalization_version":1,
            "event_id":"cai_event_00000000-0000-4000-8000-000000000003",
            "root_run_id":"cai_run_00000000-0000-4000-8000-000000000001",
            "agent_run_id":"cai_agent_run_00000000-0000-4000-8000-000000000002",
            "event_type":"provider_request","event_kind":"provider_request_completed",
            "project_id":"00000000-0000-4000-8000-000000000010",
            "agent_id":"00000000-0000-4000-8000-000000000011",
            "package_revision":"1.2.3","capture_incomplete":true,"coverage":"incomplete",
            "provider_completion":"response_received",
            "occurred_at_ms":1,"tokens":{"input_tokens":u64::MAX,"output_tokens":null},
            "input_token_details":{"input_tokens_by_modality":[{"modality":"image","token_count":u64::MAX}]},
            "token_coverage":{"input_tokens":"reported","output_tokens":"unavailable","total_tokens":"derived_input_plus_output"}})
    }

    #[test]
    fn restored_projection_preserves_unsigned_facts_and_subtypes() {
        let db = Connection::open_in_memory().unwrap();
        let original = record();
        let restored = restore(&original).unwrap();
        assert_eq!(restored["usage"]["input_tokens"], u64::MAX);
        assert!(restored["usage"]["output_tokens"].is_null());
        assert_eq!(restored["event_type"], "provider_request_completed");
        assert_eq!(restored["provider_completion"], "response_received");
        assert_eq!(restored["agent"]["project_id"], original["project_id"]);
        assert_eq!(restored["agent"]["agent_id"], original["agent_id"]);
        assert_eq!(restored["package_revision"], "1.2.3");
        assert_eq!(restored["capture_incomplete"], true);
        assert_eq!(restored["capture_coverage"], "incomplete");
        assert_eq!(restored["coverage"], original["token_coverage"]);
        assert_eq!(project(&db, &restored).unwrap(), original);
        for (kind, family) in [
            ("usage_log_started", "run_started"),
            ("agent_run_started", "run_started"),
            ("agent_run_completed", "run_finished"),
            ("root_run_completed", "run_finished"),
            ("provider_request_started", "provider_request"),
            ("tool_run_started", "tool_call"),
            ("tool_run_completed", "tool_call"),
        ] {
            let mut value = original.clone();
            value["event_type"] = json!(family);
            value["event_kind"] = json!(kind);
            assert_eq!(restore(&value).unwrap()["event_type"], kind);
        }
    }

    #[test]
    fn request_start_and_undispatched_completion_keep_attempt_identity_without_usage() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE usage_backup_ids(kind TEXT,local_value TEXT,opaque_id TEXT, UNIQUE(kind,local_value));").unwrap();
        let operation = uuid::Uuid::new_v4().to_string();
        let attempt = uuid::Uuid::new_v4().to_string();
        for (kind, outcome, status) in [
            ("provider_request_started", "unknown", "started"),
            ("provider_request_completed", "not_dispatched", "failed"),
        ] {
            let event = json!({"event_id":format!("cai_event_{}",uuid::Uuid::new_v4()),
                "root_run_id":"cai_run_00000000-0000-4000-8000-000000000001",
                "agent_run_id":"cai_agent_run_00000000-0000-4000-8000-000000000002",
                "event_type":kind,"provider_completion":outcome,"status":status,
                "operation_id":format!("cai_operation_{operation}"),
                "attempt_id":format!("cai_attempt_{attempt}"),"attempt_index":1,"retry_count":0,
                "timestamp":"2026-09-22T00:00:00Z","started_at":"2026-09-22T00:00:00Z"});
            let projected = project(&db, &event).unwrap();
            let restored = restore(&projected).unwrap();
            assert_eq!(restored["event_type"], kind);
            assert_eq!(restored["provider_completion"], outcome);
            assert_eq!(restored["operation_id"], operation);
            assert_eq!(restored["attempt_id"], attempt);
            assert!(restored["usage"].is_null());
            assert!(restored["duration_ms"].is_null());
            assert_eq!(project(&db, &restored).unwrap(), projected);
        }
    }

    #[test]
    fn tool_roundtrip_retains_distinct_operations_and_unfinished_invocations() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE usage_backup_ids(kind TEXT,local_value TEXT,opaque_id TEXT, UNIQUE(kind,local_value));").unwrap();
        let root = format!("cai_run_{}", uuid::Uuid::new_v4());
        let agent = format!("cai_agent_run_{}", uuid::Uuid::new_v4());
        let mut starts = std::collections::BTreeSet::new();
        let mut completions = std::collections::BTreeSet::new();
        for index in 0..3 {
            let operation = uuid::Uuid::new_v4().to_string();
            for kind in ["tool_run_started", "tool_run_completed"] {
                if index == 2 && kind == "tool_run_completed" {
                    continue;
                }
                let local = json!({"event_id":format!("cai_event_{}",uuid::Uuid::new_v4()),
                    "root_run_id":root,"agent_run_id":agent,"event_type":kind,
                    "operation_id":format!("cai_tool_run_{operation}"),
                    "timestamp":"2026-09-22T00:00:00Z","tool":{"name":"same-tool"}});
                let projected = project(&db, &local).unwrap();
                let restored = restore(&projected).unwrap();
                assert_eq!(restored["operation_id"], operation);
                assert_eq!(project(&db, &restored).unwrap(), projected);
                if kind == "tool_run_started" {
                    starts.insert(operation.clone());
                } else {
                    completions.insert(operation.clone());
                }
            }
        }
        assert_eq!(starts.len(), 3);
        assert_eq!(completions.len(), 2);
        assert_eq!(starts.difference(&completions).count(), 1);
    }

    #[test]
    fn remote_restore_rejects_unknown_shapes_and_private_payloads() {
        for (key, value) in [
            ("schema_version", json!(2)),
            ("normalization_version", json!(0)),
            ("event_id", json!("../../private")),
            ("root_run_id", Value::Null),
            ("event_kind", json!("tool_run_completed")),
            ("event_kind", Value::Null),
            ("occurred_at_ms", json!(-1)),
            ("ended_at_ms", json!(i64::MAX)),
            ("requested_model", json!("https://private.example/model")),
            ("elapsed_ms", json!(1.5)),
            ("provider_completion", json!("private-payload")),
            ("tokens", json!({"input_tokens":-1})),
            ("tokens", json!({"output_tokens":true})),
            ("input_token_details", json!({"prompt":"secret"})),
            ("token_coverage", json!({"input_tokens":"estimated"})),
            ("metadata", json!({"secret":"never persist"})),
            ("profile", json!("private label")),
        ] {
            let mut changed = record();
            changed[key] = value;
            assert!(restore(&changed).is_err(), "accepted invalid field {key}");
        }
        let mut changed = record();
        changed["input_token_details"]["input_tokens_by_modality"][0]["payload"] = json!("secret");
        assert!(restore(&changed).is_err());
        changed = record();
        changed["input_token_details"]["input_tokens_by_modality"] =
            json!(vec![json!({"modality":"IMAGE","tokenCount":1}); 17]);
        assert!(restore(&changed).is_err());
        changed = record();
        changed["private_payload"] = json!("x".repeat(16 * 1024));
        assert!(restore(&changed).is_err());
    }

    #[test]
    fn cached_projection_is_validated_and_cannot_change_local_identity() {
        let db = Connection::open_in_memory().unwrap();
        let mut restored = restore(&record()).unwrap();
        restored["backup_projection"]["prompt"] = json!("never upload");
        assert!(project(&db, &restored).is_err());
        restored = restore(&record()).unwrap();
        restored["event_id"] = json!("cai_event_00000000-0000-4000-8000-000000000099");
        assert!(project(&db, &restored).is_err());
    }
}
