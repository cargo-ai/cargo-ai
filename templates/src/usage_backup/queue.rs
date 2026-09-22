//! Account-bound selections in the history database, with no network transactions.
use super::{projection, Binding};
use crate::usage_store::{self, db_error};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde_json::{json, Value};

fn generation(binding: &Binding) -> Result<i64, String> {
    i64::try_from(binding.generation)
        .ok()
        .filter(|generation| *generation > 0)
        .ok_or_else(|| "Backup generation is invalid".into())
}

pub(super) fn initialize(db: &Connection) -> Result<(), String> {
    db.execute_batch("CREATE TABLE IF NOT EXISTS usage_backup_ids(kind TEXT NOT NULL,local_value TEXT NOT NULL,opaque_id TEXT NOT NULL,PRIMARY KEY(kind,local_value));
        CREATE TABLE IF NOT EXISTS usage_backup_windows(window_id INTEGER PRIMARY KEY,binding TEXT NOT NULL,generation INTEGER NOT NULL,start_sequence INTEGER NOT NULL,end_sequence INTEGER,last_scan INTEGER NOT NULL,CHECK(last_scan>=start_sequence),CHECK(end_sequence IS NULL OR end_sequence>=last_scan));
        CREATE UNIQUE INDEX IF NOT EXISTS usage_backup_open_window ON usage_backup_windows(binding,generation) WHERE end_sequence IS NULL;
        CREATE INDEX IF NOT EXISTS usage_backup_window_binding ON usage_backup_windows(binding,generation,last_scan,end_sequence);
        CREATE TABLE IF NOT EXISTS usage_backup_backoff(binding TEXT NOT NULL,generation INTEGER NOT NULL,failures INTEGER NOT NULL,next_attempt INTEGER NOT NULL,PRIMARY KEY(binding,generation));
        CREATE TABLE IF NOT EXISTS usage_backup_projection(event_id TEXT PRIMARY KEY REFERENCES usage_events(event_id) ON DELETE CASCADE,record_json TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS usage_backup_queue(event_id TEXT NOT NULL REFERENCES usage_events(event_id) ON DELETE CASCADE,binding TEXT NOT NULL,generation INTEGER NOT NULL,acknowledged INTEGER NOT NULL DEFAULT 0,PRIMARY KEY(event_id,binding,generation));
        CREATE TABLE IF NOT EXISTS usage_backup_lease(singleton INTEGER PRIMARY KEY CHECK(singleton=1),token TEXT NOT NULL,expires_at INTEGER NOT NULL);").map_err(db_error)
}
pub(super) fn open() -> Result<Connection, String> {
    let db = usage_store::open_database(true)?.ok_or("Usage database unavailable")?;
    initialize(&db)?;
    Ok(db)
}
pub(super) fn high(db: &Connection) -> Result<i64, String> {
    db.query_row(
        "SELECT COALESCE((SELECT seq FROM sqlite_sequence WHERE name='usage_events'),0)",
        [],
        |r| r.get(0),
    )
    .map_err(db_error)
}
fn exists(db: &Connection, table: &str) -> Result<bool, String> {
    db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
        [table],
        |r| r.get(0),
    )
    .map_err(db_error)
}

pub(super) fn status() -> Result<Value, String> {
    let mut result = json!({"pending_records":0,"acknowledged_records":0,"unscanned_records":0,"account_queues":[],"account_queues_truncated":false,"snapshot":"0"});
    if let Some(db) = usage_store::open_database(false)? {
        result["snapshot"] = json!(high(&db)?.to_string());
        if exists(&db, "usage_backup_queue")? {
            let (pending,acknowledged):(i64,i64)=db.query_row("SELECT COALESCE(SUM(acknowledged=0),0),COALESCE(SUM(acknowledged=1),0) FROM usage_backup_queue",[],|r|Ok((r.get(0)?,r.get(1)?))).map_err(db_error)?;
            let mut statement=db.prepare("SELECT binding,generation,SUM(acknowledged=0),SUM(acknowledged=1) FROM usage_backup_queue GROUP BY binding,generation ORDER BY binding,generation LIMIT 101").map_err(db_error)?;
            let rows = statement
                .query_map([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, i64>(2)?,
                        r.get::<_, i64>(3)?,
                    ))
                })
                .map_err(db_error)?;
            let mut queues = Vec::new();
            for row in rows {
                let (b, g, p, a) = row.map_err(db_error)?;
                if queues.len() == 100 {
                    result["account_queues_truncated"] = json!(true);
                    break;
                }
                queues.push(json!({"account_binding":b,"generation":g,"pending_records":p,"acknowledged_records":a}));
            }
            result["pending_records"] = json!(pending);
            result["acknowledged_records"] = json!(acknowledged);
            result["account_queues"] = json!(queues);
        }
        if exists(&db, "usage_backup_windows")? {
            let unscanned:i64=db.query_row("SELECT COUNT(*) FROM usage_events e WHERE COALESCE(json_extract(e.record_json,'$.restored'),0)!=1 AND EXISTS(SELECT 1 FROM usage_backup_windows w WHERE e.sequence>w.last_scan AND e.sequence<=COALESCE(w.end_sequence,?1) AND NOT EXISTS(SELECT 1 FROM usage_backup_queue q WHERE q.event_id=e.event_id AND q.binding=w.binding AND q.generation=w.generation))",[high(&db)?],|r|r.get(0)).map_err(db_error)?;
            result["unscanned_records"] = json!(unscanned);
        }
    }
    Ok(result)
}

pub(super) fn enable(binding: &Binding, preserve_active: bool) -> Result<i64, String> {
    let mut db = open()?;
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error)?;
    let boundary = enable_at(&tx, binding, preserve_active)?;
    tx.commit().map_err(db_error)?;
    Ok(boundary)
}
fn enable_at(db: &Connection, binding: &Binding, preserve_active: bool) -> Result<i64, String> {
    if preserve_active {
        let boundary:Option<i64>=db.query_row("SELECT start_sequence FROM usage_backup_windows WHERE binding=?1 AND generation=?2 AND end_sequence IS NULL",params![binding.account,generation(binding)?],|r|r.get(0)).optional().map_err(db_error)?;
        if let Some(boundary) = boundary {
            return Ok(boundary);
        }
    }
    let boundary = high(db)?;
    // An existing interval must be closed at the actual pause, not at a later
    // re-enable, or facts collected without consent could enter its range.
    let open:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM usage_backup_windows WHERE binding=?1 AND generation=?2 AND end_sequence IS NULL)",params![binding.account,generation(binding)?],|r|r.get(0)).map_err(db_error)?;
    if open {
        return Err(
            "Backup has an unclosed consent interval; disable it before enabling again".into(),
        );
    }
    db.execute("INSERT INTO usage_backup_windows(binding,generation,start_sequence,last_scan) VALUES(?1,?2,?3,?3)",params![binding.account,generation(binding)?,boundary]).map_err(db_error)?;
    Ok(boundary)
}
pub(super) fn pause(binding: &Binding) -> Result<(), String> {
    let mut db = open()?;
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error)?;
    pause_at(&tx, binding)?;
    tx.commit().map_err(db_error)
}
fn pause_at(db: &Connection, binding: &Binding) -> Result<(), String> {
    db.execute("UPDATE usage_backup_windows SET end_sequence=?3 WHERE binding=?1 AND generation=?2 AND end_sequence IS NULL",params![binding.account,generation(binding)?,high(db)?]).map_err(db_error)?;
    Ok(())
}

pub(super) fn auto_ready(binding: &Binding) -> Result<bool, String> {
    let db = open()?;
    ready_at(
        &db,
        binding,
        time::OffsetDateTime::now_utc().unix_timestamp(),
    )
}
fn ready_at(db: &Connection, binding: &Binding, now: i64) -> Result<bool, String> {
    let next: Option<i64> = db
        .query_row(
            "SELECT next_attempt FROM usage_backup_backoff WHERE binding=?1 AND generation=?2",
            params![binding.account, generation(binding)?],
            |r| r.get(0),
        )
        .optional()
        .map_err(db_error)?;
    Ok(next.is_none_or(|next| next <= now))
}
pub(super) fn defer(binding: &Binding) -> Result<(), String> {
    let db = open()?;
    defer_at(
        &db,
        binding,
        time::OffsetDateTime::now_utc().unix_timestamp(),
    )
}
fn defer_at(db: &Connection, binding: &Binding, now: i64) -> Result<(), String> {
    db.execute("INSERT INTO usage_backup_backoff(binding,generation,failures,next_attempt) VALUES(?1,?2,1,?3+30) ON CONFLICT(binding,generation) DO UPDATE SET failures=MIN(failures+1,8),next_attempt=?3+MIN(30*(1 << MIN(failures,7)),3600)",params![binding.account,generation(binding)?,now]).map_err(db_error)?;
    Ok(())
}
pub(super) fn reset_backoff(binding: &Binding) -> Result<(), String> {
    let db = open()?;
    reset_backoff_at(&db, binding)
}
fn reset_backoff_at(db: &Connection, binding: &Binding) -> Result<(), String> {
    db.execute(
        "DELETE FROM usage_backup_backoff WHERE binding=?1 AND generation=?2",
        params![binding.account, generation(binding)?],
    )
    .map_err(db_error)?;
    Ok(())
}

fn select(
    db: &Connection,
    binding: &Binding,
    records: &[(i64, String, String)],
) -> Result<usize, String> {
    let mut added = 0;
    for (_, event_id, encoded) in records {
        let event: Value =
            serde_json::from_str(encoded).map_err(|_| "Stored usage record is invalid")?;
        if event["restored"] == true {
            continue;
        }
        let stored: Option<String> = db
            .query_row(
                "SELECT record_json FROM usage_backup_projection WHERE event_id=?1",
                [event_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(db_error)?;
        if stored.is_none() {
            let projection = projection::project(db, &event)?;
            db.execute(
                "INSERT INTO usage_backup_projection(event_id,record_json) VALUES(?1,?2)",
                params![event_id, projection.to_string()],
            )
            .map_err(db_error)?;
        }
        added+=db.execute("INSERT OR IGNORE INTO usage_backup_queue(event_id,binding,generation) VALUES(?1,?2,?3)",params![event_id,binding.account,generation(binding)?]).map_err(db_error)?;
    }
    Ok(added)
}
fn records(
    db: &Connection,
    after: i64,
    through: i64,
) -> Result<Vec<(i64, String, String)>, String> {
    let mut statement=db.prepare("SELECT sequence,event_id,record_json FROM usage_events WHERE sequence>?1 AND sequence<=?2 ORDER BY sequence LIMIT 100").map_err(db_error)?;
    let rows = statement
        .query_map(params![after, through], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .map_err(db_error)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(db_error)
}
pub(super) fn capture(binding: &Binding) -> Result<(), String> {
    let mut db = open()?;
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error)?;
    capture_at(&tx, binding)?;
    tx.commit().map_err(db_error)
}
fn capture_at(db: &Connection, binding: &Binding) -> Result<(), String> {
    let consent: bool = db
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM usage_backup_windows WHERE binding=?1 AND generation=?2)",
            params![binding.account, generation(binding)?],
            |r| r.get(0),
        )
        .map_err(db_error)?;
    if !consent {
        return Err("Backup consent has no local boundary; enable backup explicitly".into());
    }
    let high = high(db)?;
    let mut statement=db.prepare("SELECT e.sequence,e.event_id,e.record_json FROM usage_events e JOIN usage_backup_windows w ON e.sequence>w.last_scan AND e.sequence<=COALESCE(w.end_sequence,?3) WHERE w.binding=?1 AND w.generation=?2 ORDER BY e.sequence LIMIT 100").map_err(db_error)?;
    let rows = statement
        .query_map(params![binding.account, generation(binding)?, high], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })
        .map_err(db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_error)?;
    select(db, binding, &rows)?;
    let scanned = if rows.len() < 100 {
        high
    } else {
        rows.last().unwrap().0
    };
    db.execute("UPDATE usage_backup_windows SET last_scan=MIN(COALESCE(end_sequence,?3),?3) WHERE binding=?1 AND generation=?2 AND last_scan<?3",params![binding.account,generation(binding)?,scanned]).map_err(db_error)?;
    Ok(())
}
pub(super) fn include_history(
    binding: &Binding,
    through: i64,
    confirmed: bool,
) -> Result<usize, String> {
    let started = std::time::Instant::now();
    let mut db = open()?;
    if through < 0 || through > high(&db)? {
        return Err("History snapshot is invalid".into());
    }
    let mut after = 0;
    let mut count = 0;
    loop {
        if started.elapsed() >= std::time::Duration::from_secs(30) {
            return Err(if confirmed {
                format!("Historical selection reached its 30-second budget after queueing {count} records; selections remain account-bound. Preview again to select any remainder")
            } else {
                "Historical preview reached its 30-second budget; no records were selected".into()
            });
        }
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error)?;
        let rows = records(&tx, after, through)?;
        if rows.is_empty() {
            break;
        }
        if confirmed {
            count += select(&tx, binding, &rows)?;
        } else {
            for (_, event_id, encoded) in &rows {
                let event: Value =
                    serde_json::from_str(encoded).map_err(|_| "Stored usage record is invalid")?;
                if event["restored"] == true {
                    continue;
                }
                let selected:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM usage_backup_queue WHERE event_id=?1 AND binding=?2 AND generation=?3)",params![event_id,binding.account,generation(binding)?],|r|r.get(0)).map_err(db_error)?;
                if !selected {
                    count += 1;
                }
            }
        }
        after = rows.last().unwrap().0;
        tx.commit().map_err(db_error)?;
    }
    Ok(count)
}
pub(super) fn batch(binding: &Binding) -> Result<Vec<Value>, String> {
    let db = open()?;
    let mut statement=db.prepare("SELECT p.record_json FROM usage_backup_queue q JOIN usage_backup_projection p USING(event_id) JOIN usage_events e USING(event_id) WHERE q.binding=?1 AND q.generation=?2 AND q.acknowledged=0 ORDER BY e.sequence LIMIT 100").map_err(db_error)?;
    let rows = statement
        .query_map(params![binding.account, generation(binding)?], |r| {
            r.get::<_, String>(0)
        })
        .map_err(db_error)?;
    let mut result = Vec::new();
    let mut bytes = 0;
    for row in rows {
        let encoded = row.map_err(db_error)?;
        if bytes + encoded.len() + 1 > 240 * 1024 {
            break;
        }
        bytes += encoded.len() + 1;
        let record =
            serde_json::from_str(&encoded).map_err(|_| "Queued backup metadata is invalid")?;
        projection::validate_cloud_record(&record)?;
        result.push(record);
    }
    Ok(result)
}
pub(super) fn acknowledge(binding: &Binding, ids: &[String]) -> Result<(), String> {
    let mut db = open()?;
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error)?;
    for id in ids {
        tx.execute("UPDATE usage_backup_queue SET acknowledged=1 WHERE event_id=?1 AND binding=?2 AND generation=?3",params![id,binding.account,generation(binding)?]).map_err(db_error)?;
    }
    tx.commit().map_err(db_error)
}
pub(super) fn restore(records: &[Value]) -> Result<usize, String> {
    if records.len() > 100 {
        return Err("Backup page exceeds its record limit".into());
    }
    let mut db = open()?;
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error)?;
    let mut inserted = 0;
    for record in records {
        let event_id = record["event_id"]
            .as_str()
            .ok_or("Backup event lacks identity")?;
        let existing: Option<String> = tx
            .query_row(
                "SELECT record_json FROM usage_events WHERE event_id=?1",
                [event_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(db_error)?;
        if let Some(encoded) = existing {
            let local: Value =
                serde_json::from_str(&encoded).map_err(|_| "Stored usage record is invalid")?;
            let cached: Option<String> = tx
                .query_row(
                    "SELECT record_json FROM usage_backup_projection WHERE event_id=?1",
                    [event_id],
                    |r| r.get(0),
                )
                .optional()
                .map_err(db_error)?;
            let projection = if let Some(cached) = cached {
                serde_json::from_str(&cached).map_err(|_| "Stored backup projection is invalid")?
            } else {
                projection::project(&tx, &local)?
            };
            if projection != *record {
                return Err(
                    "Restored usage event conflicts with an existing fact; originals preserved"
                        .into(),
                );
            }
        } else {
            let event = projection::restore(record)?;
            usage_store::insert_event(&tx, &event)?;
            inserted += 1;
            tx.execute(
                "INSERT INTO usage_backup_projection(event_id,record_json) VALUES(?1,?2)",
                params![event_id, record.to_string()],
            )
            .map_err(db_error)?;
        }
    }
    tx.commit().map_err(db_error)?;
    Ok(inserted)
}

pub(super) struct Lease {
    db: Connection,
    token: String,
}
impl Lease {
    pub(super) fn acquire() -> Result<Self, String> {
        let db = open()?;
        let token = uuid::Uuid::new_v4().to_string();
        let now = time::OffsetDateTime::now_utc().unix_timestamp();
        let changed=db.execute("INSERT INTO usage_backup_lease(singleton,token,expires_at) VALUES(1,?1,?2) ON CONFLICT(singleton) DO UPDATE SET token=excluded.token,expires_at=excluded.expires_at WHERE usage_backup_lease.expires_at<=?3",params![token,now+40,now]).map_err(db_error)?;
        if changed != 1 {
            return Err("Another backup operation is active; try again later".into());
        }
        Ok(Self { db, token })
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        let _ = self.db.execute(
            "DELETE FROM usage_backup_lease WHERE singleton=1 AND token=?1",
            [&self.token],
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn db() -> Connection {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("PRAGMA foreign_keys=ON;CREATE TABLE usage_events(sequence INTEGER PRIMARY KEY AUTOINCREMENT,event_id TEXT UNIQUE NOT NULL,root_run_id TEXT,agent_run_id TEXT,event_type TEXT,created_at TEXT,record_json TEXT);").unwrap();
        initialize(&db).unwrap();
        db
    }
    fn event() -> Value {
        json!({"schema_version":1,"event_id":format!("cai_event_{}",uuid::Uuid::new_v4()),"root_run_id":format!("cai_run_{}",uuid::Uuid::new_v4()),"agent_run_id":format!("cai_agent_run_{}",uuid::Uuid::new_v4()),"event_type":"provider_request_completed","timestamp":"2026-09-22T00:00:00Z","provider":{"server":"typesafe","profile":"private customer name","requested_model":"jev-1.13.0"},"agent":{"project_root":"/private/customer/path"},"usage":{"input_tokens":u64::MAX,"output_tokens":2},"error":{"message":"secret payload"},"prompt":"never upload","status":"failed"})
    }
    #[test]
    fn projection_is_private_unsigned_and_stable() {
        let db = db();
        let event = event();
        let a = projection::project(&db, &event).unwrap();
        let b = projection::project(&db, &event).unwrap();
        assert_eq!(a, b);
        assert_eq!(a["tokens"]["input_tokens"], u64::MAX);
        let text = a.to_string();
        for private in [
            "customer",
            "/private/",
            "secret payload",
            "never upload",
            "prompt",
        ] {
            assert!(!text.contains(private));
        }
        let restored = projection::restore(&a).unwrap();
        assert_eq!(projection::project(&db, &restored).unwrap(), a);
        assert_eq!(restored["usage"]["input_tokens"], u64::MAX);
    }
    #[test]
    fn queue_account_and_epoch_are_immutable_and_local_delete_cascades() {
        let db = db();
        let event = event();
        usage_store::insert_event(&db, &event).unwrap();
        let records = records(&db, 0, 1).unwrap();
        let a = Binding {
            account: uuid::Uuid::new_v4().to_string(),
            generation: 1,
        };
        let b = Binding {
            account: uuid::Uuid::new_v4().to_string(),
            generation: 1,
        };
        assert_eq!(select(&db, &a, &records).unwrap(), 1);
        assert_eq!(select(&db, &a, &records).unwrap(), 0);
        assert_eq!(select(&db, &b, &records).unwrap(), 1);
        let count: i64 = db
            .query_row("SELECT COUNT(*) FROM usage_backup_queue", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 2);
        db.execute("DELETE FROM usage_events", []).unwrap();
        let count: i64 = db
            .query_row("SELECT COUNT(*) FROM usage_backup_queue", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    fn binding() -> Binding {
        Binding {
            account: uuid::Uuid::new_v4().to_string(),
            generation: 1,
        }
    }
    fn append(db: &Connection) -> String {
        let event = event();
        usage_store::insert_event(db, &event).unwrap();
        event["event_id"].as_str().unwrap().to_owned()
    }
    fn queued(db: &Connection, binding: &Binding) -> std::collections::BTreeSet<String> {
        db.prepare("SELECT event_id FROM usage_backup_queue WHERE binding=?1 AND generation=?2")
            .unwrap()
            .query_map(
                params![binding.account, generation(binding).unwrap()],
                |r| r.get(0),
            )
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }
    #[test]
    fn repeated_enable_preserves_offline_eligibility_and_pause_excludes_new_records() {
        let db = db();
        let binding = binding();
        let historical = append(&db);
        assert_eq!(enable_at(&db, &binding, false).unwrap(), 1);
        let before_pause = append(&db);
        assert_eq!(enable_at(&db, &binding, true).unwrap(), 1);
        pause_at(&db, &binding).unwrap();
        let paused = append(&db);
        // Retrying disable must retain the original pause boundary.
        pause_at(&db, &binding).unwrap();
        assert_eq!(enable_at(&db, &binding, false).unwrap(), 3);
        let after_resume = append(&db);
        capture_at(&db, &binding).unwrap();
        assert_eq!(
            queued(&db, &binding),
            [before_pause, after_resume].into_iter().collect()
        );
        assert!(!queued(&db, &binding).contains(&historical));
        assert!(!queued(&db, &binding).contains(&paused));
        capture_at(&db, &binding).unwrap();
        assert_eq!(queued(&db, &binding).len(), 2);
    }
    #[test]
    fn closed_backlogs_and_new_generations_stay_account_bound() {
        let db = db();
        let a = binding();
        let b = binding();
        enable_at(&db, &a, false).unwrap();
        let a_only = append(&db);
        pause_at(&db, &a).unwrap();
        enable_at(&db, &b, false).unwrap();
        let b_only = append(&db);
        pause_at(&db, &b).unwrap();
        let newer = Binding {
            account: a.account.clone(),
            generation: 2,
        };
        enable_at(&db, &newer, false).unwrap();
        let new_only = append(&db);
        capture_at(&db, &a).unwrap();
        capture_at(&db, &b).unwrap();
        capture_at(&db, &newer).unwrap();
        assert_eq!(queued(&db, &a), [a_only].into_iter().collect());
        assert_eq!(queued(&db, &b), [b_only].into_iter().collect());
        assert_eq!(queued(&db, &newer), [new_only].into_iter().collect());
    }
    #[test]
    fn consent_boundaries_never_move_backward_after_local_deletion() {
        let db = db();
        let binding = binding();
        append(&db);
        enable_at(&db, &binding, false).unwrap();
        append(&db);
        db.execute("DELETE FROM usage_events", []).unwrap();
        assert_eq!(high(&db).unwrap(), 2);
        pause_at(&db, &binding).unwrap();
        append(&db);
        assert_eq!(enable_at(&db, &binding, false).unwrap(), 3);
        let current = append(&db);
        capture_at(&db, &binding).unwrap();
        assert_eq!(queued(&db, &binding), [current].into_iter().collect());
    }
    #[test]
    fn capture_is_bounded_and_never_selects_restored_facts() {
        let db = db();
        let binding = binding();
        enable_at(&db, &binding, false).unwrap();
        let mut restored = event();
        restored["restored"] = json!(true);
        usage_store::insert_event(&db, &restored).unwrap();
        for _ in 0..102 {
            append(&db);
        }
        capture_at(&db, &binding).unwrap();
        assert_eq!(queued(&db, &binding).len(), 99);
        capture_at(&db, &binding).unwrap();
        assert_eq!(queued(&db, &binding).len(), 102);
        assert!(!queued(&db, &binding).contains(restored["event_id"].as_str().unwrap()));
    }
    #[test]
    fn backoff_is_persistent_bounded_and_scoped_to_account_generation() {
        let db = db();
        let binding = binding();
        let other = Binding {
            account: binding.account.clone(),
            generation: 2,
        };
        assert!(ready_at(&db, &binding, 100).unwrap());
        defer_at(&db, &binding, 100).unwrap();
        assert!(!ready_at(&db, &binding, 129).unwrap());
        assert!(ready_at(&db, &binding, 130).unwrap());
        defer_at(&db, &binding, 130).unwrap();
        assert!(!ready_at(&db, &binding, 189).unwrap());
        assert!(ready_at(&db, &binding, 190).unwrap());
        for _ in 0..20 {
            defer_at(&db, &binding, 200).unwrap();
        }
        assert!(!ready_at(&db, &binding, 3799).unwrap());
        assert!(ready_at(&db, &binding, 3800).unwrap());
        assert!(ready_at(&db, &other, 200).unwrap());
        let failures: i64 = db
            .query_row("SELECT failures FROM usage_backup_backoff", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(failures, 8);
        reset_backoff_at(&db, &binding).unwrap();
        assert!(ready_at(&db, &binding, 200).unwrap());
    }
}
