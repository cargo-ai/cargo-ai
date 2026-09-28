//! Query-local scratch keeps complete snapshot calculations off the process heap.
use super::{dimension, incomplete, key, restored, selection};
use crate::usage_store::db_error;
use clap::ArgMatches;
use rusqlite::{params, Connection};
use serde_json::{json, Map, Value};

const BATCH: usize = 512;

pub(super) struct Scratch<'a> {
    db: &'a Connection,
}

impl<'a> Scratch<'a> {
    pub(super) fn prepare(
        db: &'a Connection,
        snapshot: i64,
        args: &ArgMatches,
        groups: &[String],
    ) -> Result<Self, String> {
        db.execute_batch("CREATE TEMP TABLE v2_roots(root_run_id TEXT PRIMARY KEY) WITHOUT ROWID;
            CREATE TEMP TABLE v2_context(sequence INTEGER PRIMARY KEY,root_run_id TEXT NOT NULL,agent_run_id TEXT,operation_id TEXT,event_type TEXT NOT NULL,attempt_key TEXT,selected INTEGER NOT NULL,record_json TEXT NOT NULL,duration_text TEXT,incomplete INTEGER NOT NULL);
            CREATE INDEX v2_context_root ON v2_context(root_run_id);
            CREATE INDEX v2_context_attempt ON v2_context(root_run_id,attempt_key,event_type);
            CREATE TEMP TABLE v2_requests(sequence INTEGER PRIMARY KEY,root_run_id TEXT NOT NULL,group_key TEXT NOT NULL);
            CREATE INDEX v2_requests_group ON v2_requests(group_key,root_run_id);
            CREATE TEMP TABLE v2_seen_starts(root_run_id TEXT NOT NULL,attempt_key TEXT NOT NULL,PRIMARY KEY(root_run_id,attempt_key)) WITHOUT ROWID;")
            .map_err(db_error)?;
        selection::v2_populate_roots(db, snapshot, args)?;
        let mut after = 0;
        loop {
            let events = selection::v2_context_page(db, snapshot, after, args, BATCH)?;
            let size = events.len();
            for (sequence, event, selected) in events {
                let root = event["root_run_id"].as_str().unwrap_or("");
                let attempt = event["attempt_id"]
                    .as_str()
                    .or_else(|| event["operation_id"].as_str());
                let duration = event["duration_ms"].as_u64().map(|value| value.to_string());
                db.execute("INSERT INTO temp.v2_context(sequence,root_run_id,agent_run_id,operation_id,event_type,attempt_key,selected,record_json,duration_text,incomplete) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)", params![sequence,root,event["agent_run_id"].as_str(),event["operation_id"].as_str(),event["event_type"].as_str().unwrap_or(""),attempt,selected, event.to_string(),duration,incomplete(&event)]).map_err(db_error)?;
                after = sequence;
            }
            if size < BATCH {
                break;
            }
        }
        let mut statement = db.prepare("SELECT sequence,root_run_id,event_type,attempt_key,record_json FROM temp.v2_context WHERE selected=1 AND event_type IN ('provider_request_started','provider_request_completed') ORDER BY sequence").map_err(db_error)?;
        let mut rows = statement.query([]).map_err(db_error)?;
        while let Some(row) = rows.next().map_err(db_error)? {
            let sequence: i64 = row.get(0).map_err(db_error)?;
            let root: String = row.get(1).map_err(db_error)?;
            let kind: String = row.get(2).map_err(db_error)?;
            let attempt: Option<String> = row.get(3).map_err(db_error)?;
            let encoded: String = row.get(4).map_err(db_error)?;
            if kind == "provider_request_started" {
                if let Some(attempt) = attempt.as_deref() {
                    let completed: i64 = db.query_row("SELECT EXISTS(SELECT 1 FROM temp.v2_context WHERE root_run_id=?1 AND attempt_key=?2 AND event_type='provider_request_completed')", params![root,attempt], |row| row.get(0)).map_err(db_error)?;
                    if completed != 0 {
                        continue;
                    }
                    if db.execute("INSERT OR IGNORE INTO temp.v2_seen_starts(root_run_id,attempt_key) VALUES(?1,?2)", params![root,attempt]).map_err(db_error)? == 0 { continue; }
                }
            }
            let event: Value = serde_json::from_str(&encoded)
                .map_err(|_| "Stored usage record is invalid; history was left unchanged")?;
            let group_key = key(&event, groups).to_string();
            db.execute(
                "INSERT INTO temp.v2_requests(sequence,root_run_id,group_key) VALUES(?1,?2,?3)",
                params![sequence, root, group_key],
            )
            .map_err(db_error)?;
        }
        Ok(Self { db })
    }

    fn context_count(&self, group: Option<&str>, expression: &str) -> Result<i64, String> {
        let sql = format!("WITH scoped AS (SELECT c.* FROM temp.v2_context c WHERE (?1 IS NULL OR c.root_run_id IN (SELECT r.root_run_id FROM temp.v2_requests r WHERE r.group_key=?1))) SELECT {expression} FROM scoped");
        self.db
            .query_row(&sql, [group], |row| row.get(0))
            .map_err(db_error)
    }

    fn difference_count(
        &self,
        group: Option<&str>,
        field: &str,
        started: &str,
        completed: &str,
    ) -> Result<i64, String> {
        let start_filter = if started == "*" {
            String::new()
        } else {
            format!(" AND event_type='{started}'")
        };
        let sql = format!("WITH scoped AS (SELECT c.* FROM temp.v2_context c WHERE (?1 IS NULL OR c.root_run_id IN (SELECT r.root_run_id FROM temp.v2_requests r WHERE r.group_key=?1))) SELECT COUNT(*) FROM (SELECT {field} FROM scoped WHERE {field} IS NOT NULL{start_filter} GROUP BY {field} EXCEPT SELECT {field} FROM scoped WHERE {field} IS NOT NULL AND event_type='{completed}' GROUP BY {field})");
        self.db
            .query_row(&sql, [group], |row| row.get(0))
            .map_err(db_error)
    }

    fn percentile(
        &self,
        group: Option<&str>,
        kind: &str,
        request_only: bool,
    ) -> Result<Value, String> {
        let source = if request_only {
            "temp.v2_context c JOIN temp.v2_requests r ON r.sequence=c.sequence"
        } else {
            "temp.v2_context c"
        };
        let scope = if request_only {
            "(?1 IS NULL OR r.group_key=?1)"
        } else {
            "(?1 IS NULL OR c.root_run_id IN (SELECT r.root_run_id FROM temp.v2_requests r WHERE r.group_key=?1))"
        };
        let count_sql = format!("SELECT COUNT(*) FROM {source} WHERE {scope} AND c.event_type=?2 AND c.duration_text IS NOT NULL");
        let count: i64 = self
            .db
            .query_row(&count_sql, params![group, kind], |row| row.get(0))
            .map_err(db_error)?;
        if count == 0 {
            return Ok(json!({"count":0,"p50_ms":null,"p95_ms":null}));
        }
        let at = |percent: i64| -> Result<u64, String> {
            let offset = (count * percent + 99) / 100 - 1;
            let sql = format!("SELECT c.duration_text FROM {source} WHERE {scope} AND c.event_type=?2 AND c.duration_text IS NOT NULL ORDER BY LENGTH(c.duration_text),c.duration_text LIMIT 1 OFFSET ?3");
            let text: String = self
                .db
                .query_row(&sql, params![group, kind, offset], |row| row.get(0))
                .map_err(db_error)?;
            text.parse()
                .map_err(|_| "Stored usage duration is invalid".into())
        };
        Ok(json!({"count":count,"p50_ms":at(50)?,"p95_ms":at(95)?}))
    }

    pub(super) fn summary(&self, group: Option<&str>) -> Result<Value, String> {
        let mut tokens = Map::new();
        let mut unknown = Map::new();
        let mut sums = [0u64; 3];
        let mut missing = [0u64; 3];
        let mut request_count = 0u64;
        let mut unfinished = 0u64;
        let mut failures = 0u64;
        let mut retries = 0u64;
        let mut statement = self.db.prepare("SELECT c.record_json FROM temp.v2_requests r JOIN temp.v2_context c ON c.sequence=r.sequence WHERE (?1 IS NULL OR r.group_key=?1)").map_err(db_error)?;
        let mut rows = statement.query([group]).map_err(db_error)?;
        while let Some(row) = rows.next().map_err(db_error)? {
            let encoded: String = row.get(0).map_err(db_error)?;
            let event: Value = serde_json::from_str(&encoded)
                .map_err(|_| "Stored usage record is invalid; history was left unchanged")?;
            request_count = request_count.checked_add(1).ok_or("Usage count overflow")?;
            let completed = event["event_type"] == "provider_request_completed";
            if !completed {
                unfinished += 1;
            }
            if event["status"] == "failed" {
                failures += 1;
            }
            if event["attempt_index"].as_u64().unwrap_or(1) > 1 {
                retries += 1;
            }
            for (index, field) in ["input_tokens", "output_tokens", "total_tokens"]
                .iter()
                .enumerate()
            {
                if let Some(value) = event["usage"][field].as_u64().filter(|_| completed) {
                    sums[index] = sums[index].checked_add(value).ok_or(
                        "Usage aggregate overflow; inspect individual exact integer facts",
                    )?;
                } else {
                    missing[index] += 1;
                }
            }
        }
        for (index, field) in ["input_tokens", "output_tokens", "total_tokens"]
            .iter()
            .enumerate()
        {
            tokens.insert((*field).into(), json!(sums[index]));
            unknown.insert((*field).into(), json!(missing[index]));
        }
        let roots = self.context_count(group, "COUNT(DISTINCT root_run_id)")?;
        let agents = self.context_count(group, "COUNT(DISTINCT agent_run_id)")?;
        let tool_count =
            self.context_count(group, "COALESCE(SUM(event_type='tool_run_started'),0)")?;
        let tool_completed =
            self.context_count(group, "COALESCE(SUM(event_type='tool_run_completed'),0)")?;
        let capture_count = self.context_count(group, "COALESCE(SUM(incomplete),0)")?;
        let incomplete_roots =
            self.difference_count(group, "root_run_id", "*", "root_run_completed")?;
        let incomplete_agents =
            self.difference_count(group, "agent_run_id", "*", "agent_run_completed")?;
        let incomplete_tools = self.difference_count(
            group,
            "operation_id",
            "tool_run_started",
            "tool_run_completed",
        )?;
        Ok(json!({
            "tokens":tokens,"unknown_request_counts":unknown,
            "capture_incomplete":capture_count>0,"capture_incomplete_event_count":capture_count,
            "request_count":request_count,"unfinished_request_count":unfinished,
            "root_run_count":roots,"agent_run_count":agents,
            "tool_count":tool_count,"tool_completed_count":tool_completed,
            "incomplete_run_count":incomplete_roots,"incomplete_agent_run_count":incomplete_agents,"incomplete_tool_count":incomplete_tools,
            "error_count":failures,"error_rate":{"numerator":failures,"denominator":request_count},
            "retry_rate":{"numerator":retries,"denominator":request_count},
            "latency":{"provider":self.percentile(group,"provider_request_completed",true)?,
                "run_wall_clock":self.percentile(group,"root_run_completed",false)?,
                "tool":self.percentile(group,"tool_run_completed",false)?}
        }))
    }

    pub(super) fn attribution_coverage(&self, groups: &[String]) -> Result<Value, String> {
        let mut known = vec![0u64; groups.len()];
        let mut legacy = 0u64;
        let mut unsupported = 0u64;
        let mut count = 0u64;
        let mut statement = self.db.prepare("SELECT c.record_json FROM temp.v2_requests r JOIN temp.v2_context c ON c.sequence=r.sequence").map_err(db_error)?;
        let mut rows = statement.query([]).map_err(db_error)?;
        while let Some(row) = rows.next().map_err(db_error)? {
            let encoded: String = row.get(0).map_err(db_error)?;
            let event: Value = serde_json::from_str(&encoded)
                .map_err(|_| "Stored usage record is invalid; history was left unchanged")?;
            count += 1;
            if event["attribution"].is_null() && event["restored_attribution"].is_null() {
                legacy += 1;
            } else if event["attribution"]["schema_version"] != 1
                && restored::attribution(&event).is_none()
            {
                unsupported += 1;
            }
            for (index, name) in groups.iter().enumerate() {
                let value = dimension(&event, name);
                let available = if value.is_null() {
                    false
                } else if value["namespace"]["kind"] == "usage_backup" {
                    matches!(value["state"].as_str(), Some("known" | "none"))
                } else {
                    match name.as_str() {
                        "environment" | "package" | "agent" => value["id"].as_str().is_some(),
                        "package_location" => {
                            value["path"].as_str().is_some()
                                && value["environment_id"].as_str().is_some()
                        }
                        "workspace" => {
                            (value["path"].as_str().is_some() || value["source"] == "none")
                                && value["environment_id"].as_str().is_some()
                        }
                        "package_revision" => {
                            value["version"].as_str().is_some()
                                || value["content_digest"].as_str().is_some()
                                || value["hosted_version_id"].as_str().is_some()
                        }
                        _ => true,
                    }
                };
                if available {
                    known[index] += 1;
                }
            }
        }
        let mut dimensions = Map::new();
        for (index, name) in groups.iter().enumerate() {
            dimensions.insert(name.clone(),json!({"known_request_count":known[index],"unknown_request_count":count-known[index],"legacy_request_count":legacy}));
        }
        Ok(
            json!({"request_count":count,"legacy_request_count":legacy,"unsupported_schema_request_count":unsupported,"dimensions":dimensions}),
        )
    }

    pub(super) fn group_count(&self) -> Result<i64, String> {
        self.db
            .query_row(
                "SELECT COUNT(DISTINCT group_key) FROM temp.v2_requests",
                [],
                |row| row.get(0),
            )
            .map_err(db_error)
    }

    pub(super) fn group_page(
        &self,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<(String, Value)>, String> {
        let mut statement = self.db.prepare("SELECT DISTINCT group_key FROM temp.v2_requests ORDER BY group_key LIMIT ?1 OFFSET ?2").map_err(db_error)?;
        let rows = statement
            .query_map(params![limit as i64, offset as i64], |row| {
                row.get::<_, String>(0)
            })
            .map_err(db_error)?;
        rows.map(|row| {
            let encoded = row.map_err(db_error)?;
            let key = serde_json::from_str(&encoded).map_err(|_| "Invalid grouped usage key")?;
            Ok((encoded, key))
        })
        .collect()
    }

    pub(super) fn capture_incomplete(&self) -> Result<bool, String> {
        Ok(self.context_count(None, "COALESCE(SUM(incomplete),0)")? > 0)
    }
}
