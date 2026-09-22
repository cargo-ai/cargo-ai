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
