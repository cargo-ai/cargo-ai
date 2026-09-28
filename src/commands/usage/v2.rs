//! Opt-in attribution queries over a fixed snapshot of immutable local facts.
use super::{incomplete, selection, summarize_with_context, validate_time};
use crate::usage_store::{self, db_error};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use clap::ArgMatches;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
mod scratch;

const DEFAULT_GROUPS: [&str; 5] = [
    "day",
    "profile",
    "provider",
    "requested_model",
    "resolved_model",
];
const DIMENSIONS: [&str; 14] = [
    "day",
    "profile",
    "provider",
    "requested_model",
    "resolved_model",
    "environment",
    "package",
    "package_location",
    "agent",
    "workspace",
    "runtime_version",
    "package_revision",
    "agent_revision",
    "runtime_digest",
];
const ATTRIBUTION_FILTERS: [&str; 11] = [
    "environment",
    "package",
    "package-location",
    "agent",
    "workspace",
    "runtime-version",
    "package-version",
    "package-revision",
    "hosted-version",
    "agent-revision",
    "runtime-digest",
];

#[derive(Serialize, Deserialize)]
struct Cursor {
    command: String,
    snapshot: i64,
    snapshot_count: i64,
    after_sequence: i64,
    group_offset: usize,
    selection: String,
}

fn requested_groups(args: &ArgMatches) -> Result<Vec<String>, String> {
    let Some(raw) = args.get_one::<String>("group-by") else {
        return Ok(DEFAULT_GROUPS.iter().map(|value| (*value).into()).collect());
    };
    let mut result = Vec::new();
    for item in raw.split(',') {
        let name = item.trim().replace('-', "_");
        if !DIMENSIONS.contains(&name.as_str()) {
            return Err(format!("Unsupported usage group dimension: {item}"));
        }
        if result.contains(&name) {
            return Err(format!("Duplicate usage group dimension: {item}"));
        }
        result.push(name);
    }
    if result.is_empty() {
        return Err("At least one usage group dimension is required".into());
    }
    Ok(result)
}

pub(super) fn reject_v1_options(command: &str, args: &ArgMatches) -> Result<(), String> {
    for name in ATTRIBUTION_FILTERS {
        if args.get_one::<String>(name).is_some() {
            return Err(format!("--{name} requires --schema-version 2"));
        }
    }
    if command == "summary" {
        // Summary alone has group and pagination controls. Other v1 page flags
        // remain supported and retain their existing cursor contract.
        if args.get_one::<String>("group-by").is_some() {
            return Err("--group-by requires --schema-version 2".into());
        }
        if args.get_one::<u32>("limit").is_some() {
            return Err("summary --limit requires --schema-version 2".into());
        }
        if args.get_one::<String>("cursor").is_some() {
            return Err("summary --cursor requires --schema-version 2".into());
        }
    }
    Ok(())
}

fn selection_identity(command: &str, args: &ArgMatches, groups: &[String]) -> String {
    let mut fields = Map::new();
    fields.insert("command".into(), json!(command));
    fields.insert("group_by".into(), json!(groups));
    for name in [
        "after",
        "before",
        "profile",
        "provider",
        "model",
        "resolved-model",
        "environment",
        "package",
        "package-location",
        "agent",
        "workspace",
        "runtime-version",
        "package-version",
        "package-revision",
        "hosted-version",
        "agent-revision",
        "runtime-digest",
    ] {
        fields.insert(name.into(), json!(args.get_one::<String>(name)));
    }
    if command == "show" {
        fields.insert("run_id".into(), json!(args.get_one::<String>("run-id")));
    }
    format!(
        "{:x}",
        Sha256::digest(Value::Object(fields).to_string().as_bytes())
    )
}

fn snapshot_count(db: Option<&Connection>, snapshot: i64) -> Result<i64, String> {
    match db {
        Some(db) => db
            .query_row(
                "SELECT COUNT(*) FROM usage_events WHERE sequence<=?1",
                [snapshot],
                |row| row.get(0),
            )
            .map_err(db_error),
        None => Ok(0),
    }
}

fn confirm_snapshot(db: Option<&Connection>, cursor: &Cursor) -> Result<(), String> {
    if snapshot_count(db, cursor.snapshot)? != cursor.snapshot_count {
        return Err("Usage history within this snapshot was deleted; restart the v2 query".into());
    }
    Ok(())
}

fn read_cursor(
    command: &str,
    args: &ArgMatches,
    db: Option<&Connection>,
    groups: &[String],
) -> Result<Cursor, String> {
    let selection = selection_identity(command, args, groups);
    if let Some(encoded) = args.get_one::<String>("cursor") {
        let bytes = URL_SAFE_NO_PAD
            .decode(encoded)
            .map_err(|_| "Invalid v2 usage cursor")?;
        let cursor: Cursor =
            serde_json::from_slice(&bytes).map_err(|_| "Invalid v2 usage cursor")?;
        if cursor.command != command
            || cursor.selection != selection
            || cursor.snapshot < 0
            || cursor.after_sequence < 0
            || cursor.after_sequence > cursor.snapshot
        {
            return Err("V2 usage cursor does not match this command, filters or grouping; restart the query".into());
        }
        confirm_snapshot(db, &cursor)?;
        return Ok(cursor);
    }
    let snapshot = match db {
        Some(db) => db
            .query_row(
                "SELECT COALESCE(MAX(sequence),0) FROM usage_events",
                [],
                |row| row.get(0),
            )
            .map_err(db_error)?,
        None => 0,
    };
    Ok(Cursor {
        command: command.into(),
        snapshot,
        snapshot_count: snapshot_count(db, snapshot)?,
        after_sequence: 0,
        group_offset: 0,
        selection,
    })
}

fn encode_cursor(cursor: &Cursor) -> Result<String, String> {
    let bytes = serde_json::to_vec(cursor).map_err(|_| "Cannot encode usage cursor")?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

fn dimension(event: &Value, name: &str) -> Value {
    let attr = &event["attribution"];
    if !matches!(
        name,
        "day" | "profile" | "provider" | "requested_model" | "resolved_model"
    ) && attr["schema_version"] != 1
    {
        return Value::Null;
    }
    match name {
        "day" => event["timestamp"]
            .as_str()
            .and_then(|value| value.get(..10))
            .map_or(Value::Null, |value| json!(value)),
        "profile" => event["provider"]["profile"].clone(),
        "provider" => event["provider"]["server"].clone(),
        "requested_model" => event["provider"]["requested_model"].clone(),
        "resolved_model" => event["provider"]["resolved_model"].clone(),
        "environment" => identity(&attr["environment"], &["id", "source", "unknown_reason"]),
        "package" => identity(&attr["package"], &["id", "source"]),
        "package_location" => scoped_location(attr),
        "agent" => {
            let mut value = identity(&attr["agent"], &["id", "source", "key"]);
            if value["source"] == "location_key" {
                value["environment_id"] = attr["environment"]["id"].clone();
            }
            value
        }
        "workspace" => identity(
            &attr["workspace"],
            &["path", "source", "unknown_reason", "environment_id"],
        ),
        "runtime_version" => attr["runtime"]["version"].clone(),
        "package_revision" => identity(
            &attr["package"],
            &["version", "content_digest", "hosted_version_id"],
        ),
        "agent_revision" => attr["agent"]["definition_hash"].clone(),
        "runtime_digest" => attr["runtime"]["executable_sha256"].clone(),
        _ => Value::Null,
    }
}

fn identity(source: &Value, fields: &[&str]) -> Value {
    if !source.is_object() {
        return Value::Null;
    }
    let mut result = Map::new();
    for field in fields {
        result.insert((*field).into(), source[*field].clone());
    }
    Value::Object(result)
}

fn scoped_location(attr: &Value) -> Value {
    let mut value = identity(
        &attr["package_location"],
        &["path", "source", "unknown_reason"],
    );
    if value.is_object() {
        value["environment_id"] = attr["environment"]["id"].clone();
    }
    value
}

fn key(event: &Value, groups: &[String]) -> Value {
    let mut value = Map::new();
    for name in groups {
        value.insert(name.clone(), dimension(event, name));
    }
    Value::Object(value)
}

pub(super) fn execute(command: &str, args: &ArgMatches) -> Result<Option<Value>, String> {
    for name in ["after", "before"] {
        if let Some(value) = args.get_one::<String>(name) {
            validate_time(value)?;
        }
    }
    let groups = if command == "summary" {
        requested_groups(args)?
    } else {
        Vec::new()
    };
    let db = usage_store::open_database(false)?;
    if let Some(db) = db.as_ref() {
        // The first read below fixes one SQLite view for all source and temp
        // queries in this call. The read-only connection rolls back on drop.
        db.execute_batch("PRAGMA temp_store=FILE; BEGIN DEFERRED;")
            .map_err(db_error)?;
    }
    let mut cursor = read_cursor(command, args, db.as_ref(), &groups)?;
    let mut output = json!({
        "schema_version":2,
        "snapshot":cursor.snapshot.to_string(),
        "coverage":{"capture_incomplete":usage_store::incomplete(),"completion":"committed_events_only","unobservable_loss":"process termination or an unwritable home can leave unrecorded facts"},
    });
    if matches!(command, "show" | "export") {
        let limit = *args.get_one::<u32>("limit").unwrap() as usize;
        let run = if command == "show" {
            args.get_one::<String>("run-id").map(String::as_str)
        } else {
            None
        };
        let mut events = match db.as_ref() {
            Some(db) => selection::v2_event_page(
                db,
                cursor.snapshot,
                cursor.after_sequence,
                run,
                args,
                limit,
            )?,
            None => Vec::new(),
        };
        let more = events.len() > limit;
        events.truncate(limit);
        if more {
            cursor.after_sequence = events.last().unwrap().0;
            output["next_cursor"] = json!(encode_cursor(&cursor)?);
        } else {
            output["next_cursor"] = Value::Null;
        }
        output["coverage"]["capture_incomplete"] =
            json!(usage_store::incomplete() || events.iter().any(|(_, event)| incomplete(event)));
        confirm_snapshot(db.as_ref(), &cursor)?;
        if command == "export" {
            for (_, event) in events {
                println!("{}", event);
            }
            if more {
                eprintln!("Next cursor: {}", output["next_cursor"].as_str().unwrap());
            }
            return Ok(None);
        }
        output["events"] = json!(events
            .into_iter()
            .map(|(_, event)| event)
            .collect::<Vec<_>>());
        return Ok(Some(output));
    }
    if command == "runs" {
        let limit = *args.get_one::<u32>("limit").unwrap() as usize;
        let mut roots = match db.as_ref() {
            Some(db) => {
                selection::v2_root_page(db, cursor.snapshot, cursor.after_sequence, args, limit)?
            }
            None => Vec::new(),
        };
        let more = roots.len() > limit;
        roots.truncate(limit);
        let mut runs = Vec::with_capacity(roots.len());
        for (sequence, root) in roots {
            let records =
                selection::v2_root_records(db.as_ref().unwrap(), cursor.snapshot, &root, args)?;
            let context: Vec<_> = records.iter().map(|record| &record.event).collect();
            let facts: Vec<_> = records
                .iter()
                .filter(|record| record.selected)
                .map(|record| &record.event)
                .collect();
            let agent_run_ids: BTreeSet<_> = context
                .iter()
                .filter_map(|event| event["agent_run_id"].as_str())
                .collect();
            let status = context
                .iter()
                .find(|event| event["event_type"] == "root_run_completed")
                .map(|event| event["status"].clone())
                .unwrap_or(json!("interrupted_or_running"));
            output["coverage"]["capture_incomplete"] = json!(
                output["coverage"]["capture_incomplete"] == true
                    || context.iter().any(|event| incomplete(event))
            );
            runs.push(json!({"root_run_id":root,"agent_run_ids":agent_run_ids,"summary":summarize_with_context(&facts,&context)?,"status":status}));
            cursor.after_sequence = sequence;
        }
        output["coverage"]["lifecycle_scope"] = json!("selected_roots_at_snapshot");
        output["coverage"]["usage_scope"] =
            json!("matching_request_completions_and_unfinished_starts_in_interval");
        output["next_cursor"] = if more {
            json!(encode_cursor(&cursor)?)
        } else {
            Value::Null
        };
        output["runs"] = json!(runs);
        confirm_snapshot(db.as_ref(), &cursor)?;
        return Ok(Some(output));
    }
    if command != "summary" {
        return Err("Unsupported v2 usage operation".into());
    }
    let scratch = db
        .as_ref()
        .map(|db| scratch::Scratch::prepare(db, cursor.snapshot, args, &groups))
        .transpose()?;
    output["coverage"]["capture_incomplete"] = json!(
        usage_store::incomplete()
            || match scratch.as_ref() {
                Some(scratch) => scratch.capture_incomplete()?,
                None => false,
            }
    );
    output["coverage"]["lifecycle_scope"] =
        json!("selected_roots_at_snapshot; overlapping group lifecycle summaries are nonadditive");
    output["coverage"]["usage_scope"] = json!("matching_request_completions_and_unfinished_starts_in_interval; direct attempts count once");
    output["coverage"]["attribution"] = match scratch.as_ref() {
        Some(scratch) => scratch.attribution_coverage(&groups)?,
        None => {
            json!({"request_count":0,"legacy_request_count":0,"unsupported_schema_request_count":0,"dimensions":{}})
        }
    };
    output["coverage"]["processing"] = json!(
        "bounded row batches with query-local temporary SQLite scratch; complete selected snapshot"
    );
    output["summary"] = match scratch.as_ref() {
        Some(scratch) => scratch.summary(None)?,
        None => summarize_with_context(&[], &[])?,
    };
    output["group_by"] = json!(groups);
    let group_count = match scratch.as_ref() {
        Some(scratch) => scratch.group_count()? as usize,
        None => 0,
    };
    let limit = args.get_one::<u32>("limit").copied().unwrap_or(100) as usize;
    let end = cursor.group_offset.saturating_add(limit).min(group_count);
    if cursor.group_offset > group_count || cursor.group_offset > i64::MAX as usize {
        return Err("V2 usage cursor group position is invalid; restart the query".into());
    }
    let mut summaries = Vec::new();
    if let Some(scratch) = scratch.as_ref() {
        for (key_text, group_key) in scratch.group_page(cursor.group_offset, limit)? {
            summaries.push(json!({"key":group_key,"summary":scratch.summary(Some(&key_text))?}));
        }
    }
    cursor.group_offset = end;
    output["groups"] = json!(summaries);
    output["next_cursor"] = if end < group_count {
        json!(encode_cursor(&cursor)?)
    } else {
        Value::Null
    };
    confirm_snapshot(db.as_ref(), &cursor)?;
    Ok(Some(output))
}
