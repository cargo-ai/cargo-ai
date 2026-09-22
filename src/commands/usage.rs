//! Stable application-facing queries over immutable usage facts.
use crate::usage_store::{self, db_error};
use clap::ArgMatches;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
mod selection;

pub(crate) fn run(matches: &ArgMatches) -> bool {
    match execute(matches) {
        Ok(value) => {
            if let Some(value) = value {
                println!("{}", serde_json::to_string_pretty(&value).unwrap());
            }
            true
        }
        Err(error) => {
            println!(
                "{}",
                json!({"schema_version":1,"error":{"kind":"usage_operation_failed","message":error}})
            );
            false
        }
    }
}
fn envelope() -> Value {
    json!({"schema_version":1,"coverage":{"capture_incomplete":usage_store::incomplete(),"completion":"committed_events_only","unobservable_loss":"process termination or an unwritable home can leave unrecorded facts"}})
}
fn validate_time(value: &str) -> Result<(), String> {
    time::OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339)
        .map_err(|_| "Usage time filters require RFC3339 UTC timestamps".to_string())
        .and_then(|timestamp| {
            if timestamp.offset() == time::UtcOffset::UTC {
                Ok(())
            } else {
                Err("Usage time filters require UTC timestamps".into())
            }
        })
}
fn execute(matches: &ArgMatches) -> Result<Option<Value>, String> {
    let (command, args) = matches
        .subcommand()
        .ok_or("A usage subcommand is required")?;
    if command == "settings" {
        let mut settings = usage_store::settings()?;
        if let Some(value) = args.get_one::<String>("tracking") {
            settings.tracking = value == "on";
            crate::config::settings::mutate_usage_tracking(settings.tracking)?;
        }
        let mut output = envelope();
        output["settings"] = serde_json::to_value(settings).unwrap();
        output["effective_tracking"] = json!(usage_store::tracking_enabled()?);
        return Ok(Some(output));
    }
    let db = usage_store::open_database(command == "delete" && args.get_flag("confirm"))?;
    if command == "delete" {
        let before = args.get_one::<String>("before");
        if let Some(before) = before {
            validate_time(before)?;
        }
        let mut count = 0;
        if let Some(mut db) = db {
            let transaction = db
                .transaction_with_behavior(if args.get_flag("confirm") {
                    rusqlite::TransactionBehavior::Immediate
                } else {
                    rusqlite::TransactionBehavior::Deferred
                })
                .map_err(db_error)?;
            count=transaction.query_row("SELECT COUNT(*) FROM usage_events WHERE (?1 IS NULL OR julianday(created_at)<julianday(?1))",[before],|row|row.get::<_,i64>(0)).map_err(db_error)?;
            if args.get_flag("confirm") {
                transaction.execute("DELETE FROM usage_events WHERE (?1 IS NULL OR julianday(created_at)<julianday(?1))",[before]).map_err(db_error)?;
                transaction.commit().map_err(db_error)?;
            }
        }
        let mut output = envelope();
        output["selected_records"] = json!(count);
        output["deleted"] = json!(args.get_flag("confirm"));
        output["scope"]=json!("local history and queued selections only; remote copies remain; already transmitted uploads may complete");
        return Ok(Some(output));
    }
    for flag in ["after", "before"] {
        if let Some(value) = args.get_one::<String>(flag) {
            validate_time(value)?;
        }
    }
    let mut snapshot = 0i64;
    let mut after_sequence = 0i64;
    if let Some(db) = db.as_ref() {
        snapshot = db
            .query_row(
                "SELECT COALESCE(MAX(sequence),0) FROM usage_events",
                [],
                |row| row.get(0),
            )
            .map_err(db_error)?;
    }
    if command != "summary" {
        if let Some(cursor) = args.get_one::<String>("cursor") {
            let (high, after) = cursor.split_once(':').ok_or("Invalid usage cursor")?;
            snapshot = high.parse().map_err(|_| "Invalid usage cursor")?;
            after_sequence = after.parse().map_err(|_| "Invalid usage cursor")?;
            if snapshot < 0 || after_sequence < 0 || after_sequence > snapshot {
                return Err("Invalid usage cursor".into());
            }
        }
    }
    let mut output = envelope();
    output["snapshot"] = json!(snapshot.to_string());
    if matches!(command, "show" | "export") {
        let limit = *args.get_one::<u32>("limit").unwrap() as usize;
        let run = if command == "show" {
            args.get_one::<String>("run-id").map(String::as_str)
        } else {
            None
        };
        let mut events = if let Some(db) = db.as_ref() {
            selection::event_page(db, snapshot, after_sequence, run, args, limit)?
        } else {
            Vec::new()
        };
        let more = events.len() > limit;
        events.truncate(limit);
        output["next_cursor"] = if more {
            json!(format!("{}:{}", snapshot, events.last().unwrap().0))
        } else {
            Value::Null
        };
        output["coverage"]["capture_incomplete"] =
            json!(usage_store::incomplete() || events.iter().any(|(_, event)| incomplete(event)));
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
    let limit = if command == "runs" {
        Some(*args.get_one::<u32>("limit").unwrap() as usize)
    } else {
        None
    };
    let mut roots = if let Some(db) = db.as_ref() {
        selection::root_page(db, snapshot, after_sequence, args, limit)?
    } else {
        Vec::new()
    };
    let more = limit.is_some_and(|limit| roots.len() > limit);
    if let Some(limit) = limit {
        roots.truncate(limit);
    }
    let mut histories = Vec::with_capacity(roots.len());
    for (sequence, root) in roots {
        let records = selection::root_records(db.as_ref().unwrap(), snapshot, &root, args)?;
        histories.push((sequence, root, records));
    }
    let context: Vec<_> = histories
        .iter()
        .flat_map(|(_, _, records)| records.iter().map(|record| &record.event))
        .collect();
    let selected: Vec<_> = histories
        .iter()
        .flat_map(|(_, _, records)| {
            records
                .iter()
                .filter(|record| record.selected)
                .map(|record| &record.event)
        })
        .collect();
    output["coverage"]["capture_incomplete"] =
        json!(usage_store::incomplete() || context.iter().any(|event| incomplete(event)));
    output["coverage"]["lifecycle_scope"] = json!("selected_roots_at_snapshot");
    output["coverage"]["usage_scope"] =
        json!("matching_request_completions_and_unfinished_starts_in_interval");
    if command == "summary" {
        output["summary"] = summarize_with_context(&selected, &context)?;
        let mut groups: BTreeMap<String, Vec<&Value>> = BTreeMap::new();
        for event in selected_requests(&selected, &context) {
            let day = event["timestamp"]
                .as_str()
                .unwrap_or("")
                .get(..10)
                .unwrap_or("");
            let key = json!([
                day,
                event["provider"]["profile"],
                event["provider"]["server"],
                event["provider"]["requested_model"],
                event["provider"]["resolved_model"]
            ])
            .to_string();
            groups.entry(key).or_default().push(event);
        }
        let mut summaries = Vec::with_capacity(groups.len());
        for (key, events) in groups {
            let roots: BTreeSet<_> = events
                .iter()
                .filter_map(|event| event["root_run_id"].as_str())
                .collect();
            let lifecycle: Vec<_> = context
                .iter()
                .copied()
                .filter(|event| roots.contains(event["root_run_id"].as_str().unwrap_or("")))
                .collect();
            summaries.push(json!({"key":serde_json::from_str::<Value>(&key).unwrap(),"summary":summarize_with_context(&events,&lifecycle)?}));
        }
        output["groups"] = json!(summaries);
        return Ok(Some(output));
    }
    let mut runs = Vec::with_capacity(histories.len());
    for (_, root, records) in &histories {
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
        runs.push(json!({"root_run_id":root,"agent_run_ids":agent_run_ids,"summary":summarize_with_context(&facts,&context)?,"status":status}));
    }
    output["next_cursor"] = if more {
        json!(format!("{}:{}", snapshot, histories.last().unwrap().0))
    } else {
        Value::Null
    };
    output["runs"] = json!(runs);
    Ok(Some(output))
}

fn incomplete(event: &Value) -> bool {
    event["capture_incomplete"] == true || event["capture_coverage"] == "incomplete"
}

fn attempt_key(event: &Value) -> Option<(&str, &str)> {
    Some((
        event["root_run_id"].as_str().unwrap_or(""),
        event["attempt_id"]
            .as_str()
            .or_else(|| event["operation_id"].as_str())?,
    ))
}

fn selected_requests<'a>(events: &[&'a Value], context: &[&Value]) -> Vec<&'a Value> {
    let completed: BTreeSet<_> = context
        .iter()
        .filter(|event| event["event_type"] == "provider_request_completed")
        .filter_map(|event| attempt_key(event))
        .collect();
    let mut started = BTreeSet::new();
    events
        .iter()
        .copied()
        .filter(|event| {
            if event["event_type"] == "provider_request_completed" {
                return true;
            }
            if event["event_type"] != "provider_request_started" {
                return false;
            }
            attempt_key(event).is_none_or(|key| !completed.contains(&key) && started.insert(key))
        })
        .collect()
}

#[cfg(test)]
fn summarize(events: &[&Value]) -> Result<Value, String> {
    summarize_with_context(events, events)
}

fn summarize_with_context(events: &[&Value], context: &[&Value]) -> Result<Value, String> {
    let requests = selected_requests(events, context);
    let mut tokens = serde_json::Map::new();
    let mut unknown = serde_json::Map::new();
    for field in ["input_tokens", "output_tokens", "total_tokens"] {
        let mut sum = 0u64;
        let mut missing = 0;
        for event in &requests {
            if let Some(value) = event["usage"][field]
                .as_u64()
                .filter(|_| event["event_type"] == "provider_request_completed")
            {
                sum = sum
                    .checked_add(value)
                    .ok_or("Usage aggregate overflow; inspect individual exact integer facts")?;
            } else {
                missing += 1;
            }
        }
        tokens.insert(field.into(), json!(sum));
        unknown.insert(field.into(), json!(missing));
    }
    let count = |kind: &str| {
        context
            .iter()
            .filter(|event| event["event_type"] == kind)
            .count()
    };
    let roots: BTreeSet<_> = context
        .iter()
        .filter_map(|event| event["root_run_id"].as_str())
        .collect();
    let agents: BTreeSet<_> = context
        .iter()
        .filter_map(|event| event["agent_run_id"].as_str())
        .collect();
    let completed: BTreeSet<_> = context
        .iter()
        .filter(|event| event["event_type"] == "root_run_completed")
        .filter_map(|event| event["root_run_id"].as_str())
        .collect();
    let failures = requests
        .iter()
        .filter(|event| event["status"] == "failed")
        .count();
    let percentiles = |kind: &str| {
        let source = if kind == "provider_request_completed" {
            events
        } else {
            context
        };
        let mut durations: Vec<_> = source
            .iter()
            .filter(|event| event["event_type"] == kind)
            .filter_map(|event| event["duration_ms"].as_u64())
            .collect();
        durations.sort_unstable();
        let percentile = |p: usize| {
            if durations.is_empty() {
                Value::Null
            } else {
                json!(durations[(durations.len() * p).div_ceil(100).saturating_sub(1)])
            }
        };
        json!({"count":durations.len(),"p50_ms":percentile(50),"p95_ms":percentile(95)})
    };
    let completed_agents: BTreeSet<_> = context
        .iter()
        .filter(|event| event["event_type"] == "agent_run_completed")
        .filter_map(|event| event["agent_run_id"].as_str())
        .collect();
    let started_tools: BTreeSet<_> = context
        .iter()
        .filter(|event| event["event_type"] == "tool_run_started")
        .filter_map(|event| event["operation_id"].as_str())
        .collect();
    let completed_tools: BTreeSet<_> = context
        .iter()
        .filter(|event| event["event_type"] == "tool_run_completed")
        .filter_map(|event| event["operation_id"].as_str())
        .collect();
    Ok(json!({
        "tokens": tokens, "unknown_request_counts": unknown,
        "capture_incomplete": context.iter().any(|event| incomplete(event)),
        "capture_incomplete_event_count": context.iter().filter(|event| incomplete(event)).count(),
        "request_count": requests.len(), "unfinished_request_count":requests.iter().filter(|event|event["event_type"]=="provider_request_started").count(), "root_run_count": roots.len(), "agent_run_count": agents.len(),
        "tool_count": count("tool_run_started"), "tool_completed_count": count("tool_run_completed"),
        "incomplete_run_count": roots.difference(&completed).count(),
        "incomplete_agent_run_count": agents.difference(&completed_agents).count(),
        "incomplete_tool_count": started_tools.difference(&completed_tools).count(),
        "error_count": failures, "error_rate": {"numerator": failures, "denominator": requests.len()},
        "retry_rate": {
            "numerator": requests.iter().filter(|event| event["attempt_index"].as_u64().unwrap_or(1) > 1).count(),
            "denominator": requests.len(),
        },
        "latency": {"provider": percentiles("provider_request_completed"),
            "run_wall_clock": percentiles("root_run_completed"), "tool": percentiles("tool_run_completed")},
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn aggregates_known_tokens_without_parent_double_count_or_unknown_zero() {
        let a = json!({"event_type":"provider_request_completed","root_run_id":"root","agent_run_id":"child","status":"success","usage":{"input_tokens":4,"output_tokens":0},"duration_ms":10});
        let b = json!({"event_type":"provider_request_completed","root_run_id":"root","agent_run_id":"parent","status":"failed","usage":null,"duration_ms":20});
        let c = json!({"event_type":"root_run_completed","root_run_id":"root","duration_ms":25,"usage":{"input_tokens":999}});
        let result = summarize(&[&a, &b, &c]).unwrap();
        assert_eq!(result["tokens"]["input_tokens"], 4);
        assert_eq!(result["unknown_request_counts"]["input_tokens"], 1);
        assert_eq!(result["tokens"]["output_tokens"], 0);
        assert_eq!(result["unknown_request_counts"]["output_tokens"], 1);
        assert_eq!(result["root_run_count"], 1);
        assert_eq!(result["latency"]["run_wall_clock"]["p95_ms"], 25);
    }
    #[test]
    fn restored_incomplete_coverage_survives_aggregation() {
        let event = json!({"event_type":"provider_request_completed","root_run_id":"restored-root","usage":{"input_tokens":8},"capture_incomplete":true,"restored":true});
        let summary = summarize(&[&event]).unwrap();
        assert_eq!(summary["tokens"]["input_tokens"], 8);
        assert_eq!(summary["capture_incomplete"], true);
        assert_eq!(summary["capture_incomplete_event_count"], 1);
    }

    #[test]
    fn aggregate_overflow_is_explicit() {
        let a =
            json!({"event_type":"provider_request_completed","usage":{"input_tokens":u64::MAX}});
        let b = json!({"event_type":"provider_request_completed","usage":{"input_tokens":1}});
        assert!(summarize(&[&a, &b]).is_err());
    }
    #[test]
    fn unfinished_attempts_are_unknown_and_start_completion_pairs_count_once() {
        let start = json!({"event_type":"provider_request_started","root_run_id":"root","attempt_id":"done","status":"started"});
        let completed = json!({"event_type":"provider_request_completed","root_run_id":"root","attempt_id":"done","usage":{"input_tokens":7,"output_tokens":3,"total_tokens":10},"duration_ms":12});
        let unfinished = json!({"event_type":"provider_request_started","root_run_id":"root","attempt_id":"unfinished","status":"started"});
        let summary = summarize(&[&start, &completed, &unfinished]).unwrap();
        assert_eq!(summary["request_count"], 2);
        assert_eq!(summary["unfinished_request_count"], 1);
        assert_eq!(summary["unknown_request_counts"]["input_tokens"], 1);
        assert_eq!(summary["tokens"]["input_tokens"], 7);
        assert_eq!(summary["latency"]["provider"]["count"], 1);
        let before_completion = summarize_with_context(&[&start], &[&start, &completed]).unwrap();
        assert_eq!(before_completion["request_count"], 0);
        assert_eq!(before_completion["unfinished_request_count"], 0);
        assert_eq!(before_completion["tokens"]["input_tokens"], 0);
    }
}

#[cfg(test)]
mod contract_tests {
    use super::*;
    use std::path::PathBuf;
    struct Home {
        path: PathBuf,
        old: Option<std::ffi::OsString>,
        tracking: Option<std::ffi::OsString>,
    }
    impl Home {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("usage-contract-{}", uuid::Uuid::now_v7()));
            let old = std::env::var_os("CARGO_AI_HOME");
            let tracking = std::env::var_os("CARGO_AI_USAGE_TRACKING");
            unsafe {
                std::env::set_var("CARGO_AI_HOME", &path);
                std::env::remove_var("CARGO_AI_USAGE_TRACKING");
            }
            Self {
                path,
                old,
                tracking,
            }
        }
    }
    impl Drop for Home {
        fn drop(&mut self) {
            unsafe {
                if let Some(old) = &self.old {
                    std::env::set_var("CARGO_AI_HOME", old);
                } else {
                    std::env::remove_var("CARGO_AI_HOME");
                }
                if let Some(old) = &self.tracking {
                    std::env::set_var("CARGO_AI_USAGE_TRACKING", old);
                } else {
                    std::env::remove_var("CARGO_AI_USAGE_TRACKING");
                }
            }
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
    fn query(args: &[&str]) -> Value {
        let matches = crate::args::parse_cli(
            "cargo-ai",
            ["cargo-ai", "usage"]
                .iter()
                .copied()
                .chain(args.iter().copied())
                .map(std::ffi::OsString::from)
                .collect(),
        )
        .unwrap();
        execute(matches.subcommand_matches("usage").unwrap())
            .unwrap()
            .unwrap()
    }
    fn record(id: &str, root: &str, value: u64) {
        let db = usage_store::open_database(true).unwrap().unwrap();
        usage_store::insert_event(&db,&json!({"schema_version":1,"event_id":id,"root_run_id":root,"agent_run_id":root,"event_type":"provider_request_completed","timestamp":"2026-09-22T00:00:00Z","provider":{"server":"typesafe","requested_model":"jev","resolved_model":"jev-1"},"status":"success","usage":{"input_tokens":value,"output_tokens":0,"total_tokens":value}})).unwrap();
    }
    #[test]
    fn empty_reads_and_optout_do_not_create_history() {
        let home = Home::new();
        assert_eq!(query(&["runs", "--json"])["runs"], json!([]));
        assert!(!home.path.exists());
        assert_eq!(query(&["summary", "--json"])["summary"]["request_count"], 0);
        assert!(!home.path.exists());
        query(&["settings", "--tracking", "off", "--json"]);
        assert!(
            crate::usage_log::UsageLogContext::from_runtime(None, 0, None)
                .unwrap()
                .is_none()
        );
        assert!(!home.path.join("usage").exists());
    }
    #[test]
    fn automatic_capture_query_and_settings_preserve_existing_fields() {
        let home = Home::new();
        std::fs::create_dir_all(&home.path).unwrap();
        std::fs::write(
            home.path.join("config.toml"),
            "profile = []\n[future]\nvalue='keep'\n",
        )
        .unwrap();
        let (_, mut guard) = crate::usage_log::UsageLogContext::from_runtime(None, 0, None)
            .unwrap()
            .unwrap();
        guard.finish_success();
        let runs = query(&["runs", "--json"]);
        assert_eq!(runs["runs"].as_array().unwrap().len(), 1);
        assert_eq!(runs["runs"][0]["status"], "success");
        query(&["settings", "--tracking", "off", "--json"]);
        let config = std::fs::read_to_string(home.path.join("config.toml")).unwrap();
        assert!(config.contains("keep"));
        assert_eq!(
            query(&["runs", "--json"])["runs"].as_array().unwrap().len(),
            1
        );
    }
    #[test]
    fn pages_keep_snapshot_and_delete_is_explicit_local_only() {
        let _home = Home::new();
        record("one", "a", 4);
        record("two", "b", 6);
        let first = query(&["runs", "--json", "--limit", "1"]);
        let cursor = first["next_cursor"].as_str().unwrap();
        record("three", "c", 100);
        let second = query(&["runs", "--json", "--limit", "1", "--cursor", cursor]);
        assert_eq!(second["runs"].as_array().unwrap().len(), 1);
        assert_eq!(second["runs"][0]["root_run_id"], "b");
        assert!(second["next_cursor"].is_null());
        assert_eq!(query(&["delete", "--all", "--json"])["selected_records"], 3);
        assert_eq!(
            query(&["summary", "--json"])["summary"]["tokens"]["input_tokens"],
            110
        );
        assert_eq!(
            query(&[
                "delete",
                "--before",
                "2026-09-23T00:00:00Z",
                "--confirm",
                "--json"
            ])["selected_records"],
            3
        );
        assert_eq!(query(&["summary", "--json"])["summary"]["request_count"], 0);
    }
    #[test]
    fn metadata_filters_keep_exact_requested_and_resolved_models() {
        let _home = Home::new();
        record("one", "a", 4);
        assert_eq!(
            query(&["summary", "--json", "--model", "jev"])["summary"]["request_count"],
            1
        );
        assert_eq!(
            query(&["summary", "--json", "--resolved-model", "jev"])["summary"]["request_count"],
            0
        );
        assert_eq!(
            query(&["summary", "--json", "--resolved-model", "jev-1"])["summary"]["request_count"],
            1
        );
    }

    #[test]
    fn late_root_page_does_not_decode_preceding_or_following_roots() {
        let _home = Home::new();
        for index in 0..40 {
            record(&format!("event-{index}"), &format!("root-{index}"), index);
        }
        let first = query(&["runs", "--limit", "1", "--json"]);
        assert_eq!(first["runs"][0]["root_run_id"], "root-0");
        let db = usage_store::open_database(true).unwrap().unwrap();
        db.execute(
            "UPDATE usage_events SET record_json='not JSON' WHERE sequence<39 OR sequence=40",
            [],
        )
        .unwrap();
        let page = query(&["runs", "--limit", "1", "--cursor", "40:38", "--json"]);
        assert_eq!(page["runs"].as_array().unwrap().len(), 1);
        assert_eq!(page["runs"][0]["root_run_id"], "root-38");
        assert_eq!(page["next_cursor"], "40:39");
    }

    #[test]
    fn show_and_export_select_cursor_limit_and_filters_before_decoding() {
        let _home = Home::new();
        for index in 0..40 {
            record(&format!("event-{index}"), "root", index);
        }
        let db = usage_store::open_database(true).unwrap().unwrap();
        db.execute(
            "UPDATE usage_events SET record_json='not JSON' WHERE sequence<39",
            [],
        )
        .unwrap();
        let shown = query(&[
            "show",
            "root",
            "--cursor",
            "40:38",
            "--limit",
            "1",
            "--provider",
            "typesafe",
            "--json",
        ]);
        assert_eq!(shown["events"].as_array().unwrap().len(), 1);
        assert_eq!(shown["events"][0]["event_id"], "event-38");
        assert_eq!(shown["next_cursor"], "40:39");
        let matches = crate::args::parse_cli(
            "cargo-ai",
            [
                "cargo-ai",
                "usage",
                "export",
                "--cursor",
                "40:38",
                "--limit",
                "1",
                "--provider",
                "typesafe",
            ]
            .into_iter()
            .map(std::ffi::OsString::from)
            .collect(),
        )
        .unwrap();
        let usage = matches.subcommand_matches("usage").unwrap();
        let args = usage.subcommand_matches("export").unwrap();
        let events = selection::event_page(&db, 40, 38, None, args, 1).unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].1["event_id"], "event-38");
        assert!(execute(usage).unwrap().is_none());
    }

    #[test]
    fn interval_usage_keeps_snapshot_lifecycle_without_out_of_window_tokens() {
        let _home = Home::new();
        let db = usage_store::open_database(true).unwrap().unwrap();
        let insert = |id: &str, kind: &str, timestamp: &str, extra: Value| {
            let mut event = json!({"schema_version":1,"event_id":id,"root_run_id":"root","agent_run_id":"agent","event_type":kind,"timestamp":timestamp,"provider":{"server":"typesafe","requested_model":"jev","resolved_model":"jev-1"}});
            event
                .as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            usage_store::insert_event(&db, &event).unwrap();
        };
        insert(
            "start",
            "agent_run_started",
            "2026-09-21T23:59:50Z",
            json!({}),
        );
        insert(
            "tool-start",
            "tool_run_started",
            "2026-09-21T23:59:51Z",
            json!({"operation_id":"tool-1"}),
        );
        insert(
            "request",
            "provider_request_completed",
            "2026-09-21T23:59:59Z",
            json!({"attempt_id":"request-1","usage":{"input_tokens":7},"duration_ms":5}),
        );
        insert(
            "tool-end",
            "tool_run_completed",
            "2026-09-22T00:00:01Z",
            json!({"operation_id":"tool-1","duration_ms":10000}),
        );
        insert(
            "request-later",
            "provider_request_completed",
            "2026-09-22T00:00:02Z",
            json!({"attempt_id":"request-2","usage":{"input_tokens":1000},"duration_ms":99}),
        );
        insert(
            "agent-end",
            "agent_run_completed",
            "2026-09-22T00:00:03Z",
            json!({"status":"success","duration_ms":13000}),
        );
        insert(
            "root-end",
            "root_run_completed",
            "2026-09-22T00:00:03Z",
            json!({"status":"success","duration_ms":13000}),
        );
        let args = [
            "--after",
            "2026-09-21T23:59:58Z",
            "--before",
            "2026-09-22T00:00:00Z",
            "--provider",
            "typesafe",
            "--json",
        ];
        let runs = query(&[&["runs"][..], &args].concat());
        assert_eq!(runs["runs"][0]["status"], "success");
        let summary = query(&[&["summary"][..], &args].concat());
        for result in [
            &runs["runs"][0]["summary"],
            &summary["summary"],
            &summary["groups"][0]["summary"],
        ] {
            assert_eq!(result["tokens"]["input_tokens"], 7);
            assert_eq!(result["request_count"], 1);
            assert_eq!(result["incomplete_run_count"], 0);
            assert_eq!(result["incomplete_agent_run_count"], 0);
            assert_eq!(result["incomplete_tool_count"], 0);
            assert_eq!(result["latency"]["provider"]["p95_ms"], 5);
            assert_eq!(result["latency"]["run_wall_clock"]["p95_ms"], 13000);
            assert_eq!(result["latency"]["tool"]["p95_ms"], 10000);
        }
        assert_eq!(
            summary["coverage"]["lifecycle_scope"],
            "selected_roots_at_snapshot"
        );
        let historical = query(&[
            "runs",
            "--cursor",
            "3:0",
            "--before",
            "2026-09-22T00:00:00Z",
            "--json",
        ]);
        assert_eq!(historical["runs"][0]["status"], "interrupted_or_running");
    }
}
