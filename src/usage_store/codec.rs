//! Static, versioned event codec. JSON values are never routed through SQL numbers.
use serde_json::{Map, Value};

pub(super) const VERSION: i64 = 1;
pub(super) const CONTEXTS: [&str; 4] = ["agent", "provider", "attribution", "restored_attribution"];

// Unexpected types stay in the residual object. The descriptor is a storage
// contract, independent of the fields observed in any particular Home.
const TEXT: &[&str] = &[
    "event_id",
    "event_type",
    "timestamp",
    "root_run_id",
    "agent_run_id",
    "parent_agent_run_id",
    "operation_id",
    "status",
    "request_id",
    "attempt_id",
    "tool_name",
    "action",
];
const NUMBER: &[&str] = &[
    "schema_version",
    "backup_min_schema_version",
    "duration_ms",
    "depth",
    "attempt",
    "request_attempt",
];
const OBJECT: &[&str] = &["usage", "timing", "error", "tool"];
const BOOLEAN: &[&str] = &["restored", "incomplete"];

pub(super) struct Encoded {
    pub(super) envelope: String,
    pub(super) residual: String,
    pub(super) contexts: [Option<String>; 4],
    pub(super) opaque: Option<String>,
}

pub(super) fn encode(raw: &str) -> Result<Encoded, String> {
    let parsed = serde_json::from_str::<Value>(raw);
    let Ok(Value::Object(mut residual)) = parsed else {
        // Previously malformed/non-object facts remain visible to coverage
        // reporting and export; migration must not silently discard them.
        return Ok(Encoded {
            envelope: "{}".into(),
            residual: "{}".into(),
            contexts: std::array::from_fn(|_| None),
            opaque: Some(raw.into()),
        });
    };
    let mut contexts = std::array::from_fn(|_| None);
    for (index, name) in CONTEXTS.iter().enumerate() {
        if let Some(value) = residual.remove(*name) {
            contexts[index] = Some(value.to_string());
        }
    }
    let mut envelope = Map::new();
    for (name, value) in &residual {
        if (TEXT.contains(&name.as_str()) && value.is_string())
            || (NUMBER.contains(&name.as_str()) && value.is_number())
            || (OBJECT.contains(&name.as_str()) && value.is_object())
            || (BOOLEAN.contains(&name.as_str()) && value.is_boolean())
        {
            envelope.insert(name.clone(), value.clone());
        }
    }
    for name in envelope.keys() {
        residual.remove(name);
    }
    Ok(Encoded {
        envelope: Value::Object(envelope).to_string(),
        residual: Value::Object(residual).to_string(),
        contexts,
        opaque: None,
    })
}

/// Compose complete JSON text, preserving explicit nulls and wide integers.
/// Each fragment is already canonical JSON; no json_patch/json_extract is used.
pub(super) fn view_sql() -> String {
    let mut parts = vec![
        "NULLIF(substr(e.envelope_json,2,length(e.envelope_json)-2),'')".to_string(),
        "NULLIF(substr(e.residual_json,2,length(e.residual_json)-2),'')".to_string(),
    ];
    for (index, name) in CONTEXTS.iter().enumerate() {
        parts.push(format!("CASE WHEN c{index}.context_id IS NOT NULL THEN '\"{name}\":'||c{index}.canonical_json END"));
    }
    // concat_ws ignores SQL NULL (missing), while the JSON text 'null' remains.
    let composition = format!("'{{'||concat_ws(',',{})||'}}'", parts.join(","));
    let joins = CONTEXTS
        .iter()
        .enumerate()
        .map(|(i, name)| {
            format!(" LEFT JOIN usage_contexts c{i} ON c{i}.context_id=e.{name}_context_id")
        })
        .collect::<String>();
    format!("CREATE VIEW usage_event_records AS SELECT e.sequence,e.event_id,e.root_run_id,e.agent_run_id,e.event_type,e.created_at,COALESCE(e.opaque_json,{composition}) AS record_json FROM usage_events e{joins};")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn codec_keeps_unknown_null_mismatched_types_and_unsigned_values() {
        let raw = serde_json::json!({"event_id":"id","event_type":"x","timestamp":"now","usage":{"input_tokens":u64::MAX},"duration_ms":u64::MAX,"restored":"wrong type","status":null,"agent":null,"provider":[false,u64::MAX],"custom":{"a":null,"z":[1,true]}}).to_string();
        let encoded = encode(&raw).unwrap();
        let envelope: Value = serde_json::from_str(&encoded.envelope).unwrap();
        let residual: Value = serde_json::from_str(&encoded.residual).unwrap();
        assert_eq!(envelope["duration_ms"], u64::MAX);
        assert_eq!(residual["restored"], "wrong type");
        assert!(residual.get("status").unwrap().is_null());
        assert_eq!(encoded.contexts[0].as_deref(), Some("null"));
        assert!(encoded.contexts[2].is_none());
        let db = rusqlite::Connection::open_in_memory().unwrap();
        super::super::initialize_test_database(&db);
        let original: Value = serde_json::from_str(&raw).unwrap();
        super::super::insert_event(&db, &original).unwrap();
        let reconstructed: String = db
            .query_row("SELECT record_json FROM usage_event_records", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&reconstructed).unwrap(),
            original
        );
    }
}
