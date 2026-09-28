//! Database-side selection keeps a page independent of unrelated history size.
use crate::usage_store::db_error;
use clap::ArgMatches;
use rusqlite::{params_from_iter, types::Value as Parameter, Connection};
use serde_json::Value;

const FILTER: &str = "sequence<=?1
    AND (?2 IS NULL OR root_run_id=?2 OR agent_run_id=?2)
    AND (?3 IS NULL OR julianday(created_at)>=julianday(?3))
    AND (?4 IS NULL OR julianday(created_at)<julianday(?4))
    AND (?5 IS NULL OR json_extract(record_json,'$.provider.profile')=?5)
    AND (?6 IS NULL OR json_extract(record_json,'$.provider.server')=?6)
    AND (?7 IS NULL OR json_extract(record_json,'$.provider.requested_model')=?7)
    AND (?8 IS NULL OR json_extract(record_json,'$.provider.resolved_model')=?8)";

fn parameters(snapshot: i64, run: Option<&str>, args: &ArgMatches) -> Vec<Parameter> {
    let text =
        |value: Option<&str>| value.map_or(Parameter::Null, |value| Parameter::Text(value.into()));
    let mut values = vec![Parameter::Integer(snapshot), text(run)];
    for flag in [
        "after",
        "before",
        "profile",
        "provider",
        "model",
        "resolved-model",
    ] {
        values.push(text(args.get_one::<String>(flag).map(String::as_str)));
    }
    values
}

// The expressions are fixed here, so user supplied dimension names never become SQL.
const ATTRIBUTION_FILTERS: [(&str, &str); 7] = [
    ("environment", "$.attribution.environment.id"),
    ("runtime-version", "$.attribution.runtime.version"),
    ("package-version", "$.attribution.package.version"),
    ("package-revision", "$.attribution.package.content_digest"),
    ("hosted-version", "$.attribution.package.hosted_version_id"),
    ("agent-revision", "$.attribution.agent.definition_hash"),
    ("runtime-digest", "$.attribution.runtime.executable_sha256"),
];

fn identity_filter(
    args: &ArgMatches,
    name: &str,
    fields: &[(&str, &str)],
    filter: &mut String,
    values: &mut Vec<Parameter>,
) -> Result<(), String> {
    let Some(raw) = args.get_one::<String>(name) else {
        return Ok(());
    };
    let parsed: Value = serde_json::from_str(raw)
        .map_err(|_| format!("--{name} requires a structured JSON identity value"))?;
    let object = parsed
        .as_object()
        .ok_or_else(|| format!("--{name} requires a structured JSON identity value"))?;
    if object.len() != fields.len()
        || fields.iter().any(|(key, _)| {
            object
                .get(*key)
                .and_then(Value::as_str)
                .is_none_or(str::is_empty)
        })
    {
        return Err(format!(
            "--{name} requires exactly {} nonempty string fields",
            fields
                .iter()
                .map(|(field, _)| *field)
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    for (key, path) in fields {
        let value = object[*key].as_str().unwrap();
        values.push(Parameter::Text(value.into()));
        filter.push_str(&format!(
            " AND json_extract(record_json,'{path}')=?{}",
            values.len()
        ));
    }
    Ok(())
}

fn v2_filter(
    snapshot: i64,
    run: Option<&str>,
    args: &ArgMatches,
) -> Result<(String, Vec<Parameter>), String> {
    let mut values = parameters(snapshot, run, args);
    let mut filter = FILTER.to_string();
    if [
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
    ]
    .iter()
    .any(|name| args.get_one::<String>(name).is_some())
    {
        filter.push_str(" AND json_extract(record_json,'$.attribution.schema_version')=1");
    }
    for (name, path) in ATTRIBUTION_FILTERS {
        if let Some(value) = args.get_one::<String>(name) {
            values.push(Parameter::Text(value.clone()));
            filter.push_str(&format!(
                " AND json_extract(record_json,'{path}')=?{}",
                values.len()
            ));
        }
    }
    if let Some(raw) = args.get_one::<String>("package") {
        let parsed: Value = serde_json::from_str(raw)
            .map_err(|_| "--package requires a structured JSON identity value")?;
        if !matches!(
            parsed["source"].as_str(),
            Some("authored_project_id" | "hosted_source_id")
        ) {
            return Err("--package source must be authored_project_id or hosted_source_id".into());
        }
    }
    identity_filter(
        args,
        "package",
        &[
            ("source", "$.attribution.package.source"),
            ("id", "$.attribution.package.id"),
        ],
        &mut filter,
        &mut values,
    )?;
    identity_filter(
        args,
        "package-location",
        &[
            ("environment_id", "$.attribution.environment.id"),
            ("path", "$.attribution.package_location.path"),
        ],
        &mut filter,
        &mut values,
    )?;
    identity_filter(
        args,
        "workspace",
        &[
            ("environment_id", "$.attribution.workspace.environment_id"),
            ("path", "$.attribution.workspace.path"),
        ],
        &mut filter,
        &mut values,
    )?;
    if let Some(raw) = args.get_one::<String>("agent") {
        let parsed: Value = serde_json::from_str(raw)
            .map_err(|_| "--agent requires a structured JSON identity value")?;
        let source = parsed["source"]
            .as_str()
            .ok_or("--agent requires source and id")?;
        if !matches!(source, "package_key" | "location_key") {
            return Err("--agent source must be package_key or location_key".into());
        }
        let fields: &[(&str, &str)] = if source == "location_key" {
            &[
                ("source", "$.attribution.agent.source"),
                ("id", "$.attribution.agent.id"),
                ("environment_id", "$.attribution.environment.id"),
            ]
        } else {
            &[
                ("source", "$.attribution.agent.source"),
                ("id", "$.attribution.agent.id"),
            ]
        };
        identity_filter(args, "agent", fields, &mut filter, &mut values)?;
    }
    Ok((filter, values))
}

pub(super) fn v2_event_page(
    db: &Connection,
    snapshot: i64,
    after_sequence: i64,
    run: Option<&str>,
    args: &ArgMatches,
    limit: usize,
) -> Result<Vec<(i64, Value)>, String> {
    let (filter, mut values) = v2_filter(snapshot, run, args)?;
    values.push(Parameter::Integer(after_sequence));
    let after = values.len();
    values.push(Parameter::Integer(limit as i64 + 1));
    let count = values.len();
    let sql = format!("SELECT sequence,record_json FROM usage_events WHERE sequence>?{after} AND {filter} ORDER BY sequence LIMIT ?{count}");
    let mut statement = db.prepare(&sql).map_err(db_error)?;
    let rows = statement
        .query_map(params_from_iter(values), |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(db_error)?;
    rows.map(|row| {
        let (sequence, encoded) = row.map_err(db_error)?;
        Ok((sequence, decode(&encoded)?))
    })
    .collect()
}

pub(super) fn v2_root_page(
    db: &Connection,
    snapshot: i64,
    after_sequence: i64,
    args: &ArgMatches,
    limit: usize,
) -> Result<Vec<(i64, String)>, String> {
    let (filter, mut values) = v2_filter(snapshot, None, args)?;
    values.push(Parameter::Integer(after_sequence));
    let after = values.len();
    values.push(Parameter::Integer(limit as i64 + 1));
    let count = values.len();
    let sql = format!("SELECT MIN(sequence) AS first_sequence,root_run_id FROM usage_events WHERE root_run_id IS NOT NULL AND {filter} GROUP BY root_run_id HAVING first_sequence>?{after} ORDER BY first_sequence LIMIT ?{count}");
    let mut statement = db.prepare(&sql).map_err(db_error)?;
    let rows = statement
        .query_map(params_from_iter(values), |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(db_error)?;
    rows.collect::<Result<_, _>>().map_err(db_error)
}

pub(super) fn v2_root_records(
    db: &Connection,
    snapshot: i64,
    root: &str,
    args: &ArgMatches,
) -> Result<Vec<Record>, String> {
    let (filter, mut values) = v2_filter(snapshot, None, args)?;
    values.push(Parameter::Text(root.into()));
    let root_parameter = values.len();
    let sql = format!("SELECT record_json,COALESCE(({filter}),0) FROM usage_events WHERE root_run_id=?{root_parameter} AND sequence<=?1 ORDER BY sequence");
    let mut statement = db.prepare(&sql).map_err(db_error)?;
    let rows = statement
        .query_map(params_from_iter(values), |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, bool>(1)?))
        })
        .map_err(db_error)?;
    rows.map(|row| {
        let (encoded, selected) = row.map_err(db_error)?;
        Ok(Record {
            event: decode(&encoded)?,
            selected,
        })
    })
    .collect()
}

pub(super) fn v2_context_page(
    db: &Connection,
    snapshot: i64,
    after_sequence: i64,
    args: &ArgMatches,
    limit: usize,
) -> Result<Vec<(i64, Value, bool)>, String> {
    let (filter, mut values) = v2_filter(snapshot, None, args)?;
    values.push(Parameter::Integer(after_sequence));
    let after = values.len();
    values.push(Parameter::Integer(limit as i64));
    let count = values.len();
    let sql = format!("SELECT sequence,record_json,COALESCE(({filter}),0) FROM usage_events WHERE sequence>?{after} AND sequence<=?1 AND root_run_id IN (SELECT root_run_id FROM temp.v2_roots) ORDER BY sequence LIMIT ?{count}");
    let mut statement = db.prepare(&sql).map_err(db_error)?;
    let rows = statement
        .query_map(params_from_iter(values), |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, bool>(2)?,
            ))
        })
        .map_err(db_error)?;
    rows.map(|row| {
        let (sequence, encoded, selected) = row.map_err(db_error)?;
        Ok((sequence, decode(&encoded)?, selected))
    })
    .collect()
}

pub(super) fn v2_populate_roots(
    db: &Connection,
    snapshot: i64,
    args: &ArgMatches,
) -> Result<(), String> {
    let (filter, values) = v2_filter(snapshot, None, args)?;
    let sql = format!("INSERT INTO temp.v2_roots(root_run_id) SELECT DISTINCT root_run_id FROM usage_events WHERE root_run_id IS NOT NULL AND {filter}");
    db.execute(&sql, params_from_iter(values))
        .map_err(db_error)?;
    Ok(())
}

fn decode(encoded: &str) -> Result<Value, String> {
    serde_json::from_str(encoded)
        .map_err(|_| "Stored usage record is invalid; history was left unchanged".into())
}

pub(super) fn event_page(
    db: &Connection,
    snapshot: i64,
    after_sequence: i64,
    run: Option<&str>,
    args: &ArgMatches,
    limit: usize,
) -> Result<Vec<(i64, Value)>, String> {
    let mut values = parameters(snapshot, run, args);
    values.extend([
        Parameter::Integer(after_sequence),
        Parameter::Integer(limit as i64 + 1),
    ]);
    let mut statement = db.prepare(&format!("SELECT sequence,record_json FROM usage_events WHERE sequence>?9 AND {FILTER} ORDER BY sequence LIMIT ?10")).map_err(db_error)?;
    let rows = statement
        .query_map(params_from_iter(values), |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(db_error)?;
    rows.map(|row| {
        let (sequence, encoded) = row.map_err(db_error)?;
        Ok((sequence, decode(&encoded)?))
    })
    .collect()
}

pub(super) fn root_page(
    db: &Connection,
    snapshot: i64,
    after_sequence: i64,
    args: &ArgMatches,
    limit: Option<usize>,
) -> Result<Vec<(i64, String)>, String> {
    let mut values = parameters(snapshot, None, args);
    values.extend([
        Parameter::Integer(after_sequence),
        Parameter::Integer(limit.map_or(-1, |limit| limit as i64 + 1)),
    ]);
    let mut statement = db.prepare(&format!("SELECT MIN(sequence) AS first_sequence,root_run_id FROM usage_events WHERE root_run_id IS NOT NULL AND {FILTER} GROUP BY root_run_id HAVING first_sequence>?9 ORDER BY first_sequence LIMIT ?10")).map_err(db_error)?;
    let rows = statement
        .query_map(params_from_iter(values), |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .map_err(db_error)?;
    rows.collect::<Result<_, _>>().map_err(db_error)
}

pub(super) struct Record {
    pub(super) event: Value,
    pub(super) selected: bool,
}

pub(super) fn root_records(
    db: &Connection,
    snapshot: i64,
    root: &str,
    args: &ArgMatches,
) -> Result<Vec<Record>, String> {
    let mut values = parameters(snapshot, None, args);
    values.push(Parameter::Text(root.into()));
    // Context is restricted to the already selected root and the same snapshot.
    // The flag controls usage totals; out-of-window completion remains context.
    let mut statement=db.prepare(&format!("SELECT record_json,COALESCE(({FILTER}),0) FROM usage_events WHERE root_run_id=?9 AND sequence<=?1 ORDER BY sequence")).map_err(db_error)?;
    let rows = statement
        .query_map(params_from_iter(values), |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, bool>(1)?))
        })
        .map_err(db_error)?;
    rows.map(|row| {
        let (encoded, selected) = row.map_err(db_error)?;
        Ok(Record {
            event: decode(&encoded)?,
            selected,
        })
    })
    .collect()
}
