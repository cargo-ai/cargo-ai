//! Local immutable selections and custody; every transaction ends before network I/O.
use super::contract::*;
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde_json::Value;

pub const HOUR_MS: i64 = 3_600_000;
pub const MAX_LEASE_MS: i64 = 60_000;
pub const LANDING_RECORD_ALLOWANCE: usize = 1536;
pub const LANDING_ENVELOPE_ALLOWANCE: usize = 4096;
fn within_landing_budget(bytes: usize, count: usize) -> bool {
    bytes
        .checked_add(LANDING_RECORD_ALLOWANCE * count + LANDING_ENVELOPE_ALLOWANCE)
        .is_some_and(|total| total < MAX_BATCH_BYTES)
}

fn failed(_: impl std::fmt::Display) -> String {
    "Usage cloud database operation failed".into()
}
fn invalid() -> String {
    "Usage cloud state or identity is invalid".into()
}
pub(crate) fn timing_from_local_capture(
    raw: &Value,
) -> Result<Option<TimingSupplement>, ContractError> {
    if raw.get("restored") == Some(&Value::Bool(true)) {
        return Ok(None);
    }
    raw.get("elapsed_timing")
        .map(|value| {
            serde_json::from_value(value.clone()).map_err(|_| ContractError::InvalidRecord)
        })
        .transpose()
}

fn contract(error: ContractError) -> String {
    error.to_string()
}
fn key(source: &Source) -> (&str, &str, i64) {
    (
        source.account_binding.as_str(),
        source.source_home_id.as_str(),
        source.data_generation.get() as i64,
    )
}

/// Passive callers never create queue state. A partial schema is an assessment
/// failure, so retention keeps the event instead of guessing away obligations.
pub fn installed(db: &Connection) -> Result<bool, String> {
    let count:i64=db.query_row("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN('usage_cloud_state','usage_cloud_windows','usage_cloud_blocked','usage_cloud_queue','usage_cloud_batches','usage_cloud_batch_members','usage_cloud_schedule')",[],|row|row.get(0)).map_err(failed)?;
    match count {
        0 => Ok(false),
        7 => Ok(true),
        _ => Err("Usage cloud schema is incomplete; preserve local events".into()),
    }
}

/// These tables deliberately have no cascading dependency on the local event
/// parent: retained projections and exact receipts survive local age expiry.
#[cfg(any(cargo_ai_cli, test))]
pub fn initialize(db: &mut Connection) -> Result<(), String> {
    db.pragma_update(None, "foreign_keys", true)
        .map_err(failed)?;
    let foreign_keys: bool = db
        .pragma_query_value(None, "foreign_keys", |row| row.get(0))
        .map_err(failed)?;
    if !foreign_keys {
        return Err(invalid());
    }
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(failed)?;
    tx.execute_batch("CREATE TABLE IF NOT EXISTS usage_cloud_state(
        binding TEXT NOT NULL,source_home_id TEXT NOT NULL,data_generation INTEGER NOT NULL CHECK(data_generation>0),
        enabled INTEGER NOT NULL CHECK(enabled IN(0,1)),lifecycle_epoch INTEGER NOT NULL CHECK(lifecycle_epoch>=0),consent_interval INTEGER NOT NULL CHECK(consent_interval>0),
        cleared INTEGER NOT NULL DEFAULT 0 CHECK(cleared IN(0,1)),clear_intent_json TEXT,CHECK(cleared=0 OR enabled=0),
        PRIMARY KEY(binding,source_home_id,data_generation));
    CREATE TABLE IF NOT EXISTS usage_cloud_windows(
        window_id TEXT PRIMARY KEY,binding TEXT NOT NULL,source_home_id TEXT NOT NULL,data_generation INTEGER NOT NULL,
        lifecycle_epoch INTEGER NOT NULL CHECK(lifecycle_epoch>=0),consent_interval INTEGER NOT NULL CHECK(consent_interval>0),
        historical INTEGER NOT NULL CHECK(historical IN(0,1)),start_sequence INTEGER NOT NULL CHECK(start_sequence>=0),end_sequence INTEGER,last_scan INTEGER NOT NULL,
        CHECK(last_scan>=start_sequence),CHECK(end_sequence IS NULL OR end_sequence>=last_scan),
        FOREIGN KEY(binding,source_home_id,data_generation) REFERENCES usage_cloud_state(binding,source_home_id,data_generation));
    CREATE UNIQUE INDEX IF NOT EXISTS usage_cloud_open_window ON usage_cloud_windows(binding,source_home_id,data_generation) WHERE end_sequence IS NULL;
    CREATE INDEX IF NOT EXISTS usage_cloud_window_scan ON usage_cloud_windows(binding,source_home_id,data_generation,last_scan,end_sequence);
    CREATE TABLE IF NOT EXISTS usage_cloud_blocked(
        binding TEXT NOT NULL,source_home_id TEXT NOT NULL,data_generation INTEGER NOT NULL,event_id TEXT NOT NULL,sequence INTEGER NOT NULL CHECK(sequence>0),
        reason TEXT NOT NULL CHECK(reason='origin_unverified'),PRIMARY KEY(binding,source_home_id,data_generation,event_id),
        FOREIGN KEY(binding,source_home_id,data_generation) REFERENCES usage_cloud_state(binding,source_home_id,data_generation));
    CREATE TABLE IF NOT EXISTS usage_cloud_queue(
        binding TEXT NOT NULL,source_home_id TEXT NOT NULL,data_generation INTEGER NOT NULL,event_id TEXT NOT NULL,sequence INTEGER NOT NULL CHECK(sequence>0),
        original_text TEXT NOT NULL CHECK(length(CAST(original_text AS BLOB))<=16384),original_sha256 TEXT NOT NULL,semantic_hash_version INTEGER NOT NULL CHECK(semantic_hash_version=1),semantic_sha256 TEXT NOT NULL,timing_json TEXT,
        retained INTEGER NOT NULL DEFAULT 0 CHECK(retained IN(0,1)),quarantined INTEGER NOT NULL DEFAULT 0 CHECK(quarantined IN(0,1)),active_batch_id TEXT,
        PRIMARY KEY(binding,source_home_id,data_generation,event_id),UNIQUE(binding,source_home_id,data_generation,sequence),
        FOREIGN KEY(binding,source_home_id,data_generation) REFERENCES usage_cloud_state(binding,source_home_id,data_generation));
    CREATE TABLE IF NOT EXISTS usage_cloud_batches(
        binding TEXT NOT NULL,source_home_id TEXT NOT NULL,data_generation INTEGER NOT NULL,batch_id TEXT NOT NULL,
        lifecycle_epoch INTEGER NOT NULL CHECK(lifecycle_epoch>=0),consent_interval INTEGER NOT NULL CHECK(consent_interval>0),
        request_json TEXT NOT NULL CHECK(length(CAST(request_json AS BLOB))<262144),ordered_membership_sha256 TEXT NOT NULL,
        sealed INTEGER NOT NULL CHECK(sealed IN(0,1)),attempts INTEGER NOT NULL DEFAULT 0 CHECK(attempts>=0),
        outcome TEXT NOT NULL DEFAULT 'never_submitted' CHECK(outcome IN('never_submitted','unknown','buffered','terminal_unretained','retained')),
        admission_json TEXT,receipt_json TEXT,readmitted_from TEXT,
        PRIMARY KEY(binding,source_home_id,data_generation,batch_id),
        FOREIGN KEY(binding,source_home_id,data_generation) REFERENCES usage_cloud_state(binding,source_home_id,data_generation));
    CREATE TABLE IF NOT EXISTS usage_cloud_batch_members(
        binding TEXT NOT NULL,source_home_id TEXT NOT NULL,data_generation INTEGER NOT NULL,batch_id TEXT NOT NULL,ordinal INTEGER NOT NULL CHECK(ordinal>=0 AND ordinal<100),event_id TEXT NOT NULL,
        original_sha256 TEXT NOT NULL,semantic_hash_version INTEGER NOT NULL CHECK(semantic_hash_version=1),semantic_sha256 TEXT NOT NULL,timing_sha256 TEXT,
        PRIMARY KEY(binding,source_home_id,data_generation,batch_id,ordinal),UNIQUE(binding,source_home_id,data_generation,batch_id,event_id),
        FOREIGN KEY(binding,source_home_id,data_generation,batch_id) REFERENCES usage_cloud_batches(binding,source_home_id,data_generation,batch_id),
        FOREIGN KEY(binding,source_home_id,data_generation,event_id) REFERENCES usage_cloud_queue(binding,source_home_id,data_generation,event_id));
    CREATE TABLE IF NOT EXISTS usage_cloud_schedule(
        binding TEXT NOT NULL,source_home_id TEXT NOT NULL,data_generation INTEGER NOT NULL,
        next_due_ms INTEGER NOT NULL CHECK(next_due_ms>=0),failures INTEGER NOT NULL DEFAULT 0 CHECK(failures BETWEEN 0 AND 8),last_clock_ms INTEGER NOT NULL CHECK(last_clock_ms>=0),
        lease_token TEXT,lease_expires_ms INTEGER,CHECK((lease_token IS NULL)=(lease_expires_ms IS NULL)),
        PRIMARY KEY(binding,source_home_id,data_generation),
        FOREIGN KEY(binding,source_home_id,data_generation) REFERENCES usage_cloud_state(binding,source_home_id,data_generation));
    ").map_err(failed)?;
    let has_clear: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM pragma_table_info('usage_cloud_state') WHERE name='clear_intent_json')", [], |row| row.get(0)).map_err(failed)?;
    if !has_clear {
        tx.execute_batch("ALTER TABLE usage_cloud_state ADD COLUMN clear_intent_json TEXT;")
            .map_err(failed)?;
    }
    // The unreleased optional metadata upgrade preserves frozen request bytes.
    let has_queue:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM pragma_table_info('usage_cloud_queue') WHERE name='timing_json')",[],|r|r.get(0)).map_err(failed)?;
    if !has_queue {
        tx.execute_batch("ALTER TABLE usage_cloud_queue ADD COLUMN timing_json TEXT;")
            .map_err(failed)?;
    }
    let has_members:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM pragma_table_info('usage_cloud_batch_members') WHERE name='timing_sha256')",[],|r|r.get(0)).map_err(failed)?;
    if !has_members {
        tx.execute_batch("ALTER TABLE usage_cloud_batch_members ADD COLUMN timing_sha256 TEXT;")
            .map_err(failed)?;
    }
    tx.execute_batch("DROP TRIGGER IF EXISTS usage_cloud_projection_immutable;
    CREATE TRIGGER IF NOT EXISTS usage_cloud_batch_immutable BEFORE UPDATE ON usage_cloud_batches
        WHEN OLD.binding IS NOT NEW.binding OR OLD.source_home_id IS NOT NEW.source_home_id OR OLD.data_generation IS NOT NEW.data_generation OR OLD.batch_id IS NOT NEW.batch_id
        OR OLD.lifecycle_epoch IS NOT NEW.lifecycle_epoch OR OLD.consent_interval IS NOT NEW.consent_interval OR OLD.request_json IS NOT NEW.request_json
        OR OLD.ordered_membership_sha256 IS NOT NEW.ordered_membership_sha256 OR OLD.readmitted_from IS NOT NEW.readmitted_from OR (OLD.sealed=1 AND NEW.sealed!=1)
        OR (OLD.admission_json IS NOT NULL AND OLD.admission_json IS NOT NEW.admission_json)
        OR (OLD.receipt_json IS NOT NULL AND OLD.receipt_json IS NOT NEW.receipt_json)
        BEGIN SELECT RAISE(ABORT,'immutable usage cloud batch'); END;
    CREATE TRIGGER IF NOT EXISTS usage_cloud_member_immutable BEFORE UPDATE ON usage_cloud_batch_members BEGIN SELECT RAISE(ABORT,'immutable usage cloud member'); END;
    CREATE TRIGGER IF NOT EXISTS usage_cloud_member_no_delete BEFORE DELETE ON usage_cloud_batch_members
        WHEN EXISTS(SELECT 1 FROM usage_cloud_batches b WHERE b.binding=OLD.binding AND b.source_home_id=OLD.source_home_id AND b.data_generation=OLD.data_generation AND b.batch_id=OLD.batch_id AND b.sealed=1)
        BEGIN SELECT RAISE(ABORT,'sealed usage cloud membership'); END;
    CREATE TRIGGER IF NOT EXISTS usage_cloud_batch_no_delete BEFORE DELETE ON usage_cloud_batches WHEN OLD.sealed=1 BEGIN SELECT RAISE(ABORT,'immutable usage cloud batch'); END;
    CREATE TRIGGER IF NOT EXISTS usage_cloud_projection_no_delete BEFORE DELETE ON usage_cloud_queue BEGIN SELECT RAISE(ABORT,'immutable usage cloud projection'); END;
    CREATE TRIGGER IF NOT EXISTS usage_cloud_member_sealed BEFORE INSERT ON usage_cloud_batch_members
        WHEN EXISTS(SELECT 1 FROM usage_cloud_batches b WHERE b.binding=NEW.binding AND b.source_home_id=NEW.source_home_id AND b.data_generation=NEW.data_generation AND b.batch_id=NEW.batch_id AND b.sealed=1)
        BEGIN SELECT RAISE(ABORT,'sealed usage cloud membership'); END;
    CREATE TRIGGER IF NOT EXISTS usage_cloud_projection_immutable BEFORE UPDATE ON usage_cloud_queue
        WHEN OLD.binding IS NOT NEW.binding OR OLD.source_home_id IS NOT NEW.source_home_id OR OLD.data_generation IS NOT NEW.data_generation OR OLD.event_id IS NOT NEW.event_id OR OLD.sequence IS NOT NEW.sequence
        OR OLD.original_text IS NOT NEW.original_text OR OLD.original_sha256 IS NOT NEW.original_sha256 OR OLD.semantic_hash_version IS NOT NEW.semantic_hash_version OR OLD.semantic_sha256 IS NOT NEW.semantic_sha256 OR OLD.timing_json IS NOT NEW.timing_json
        OR (OLD.retained=1 AND NEW.retained!=1)
        BEGIN SELECT RAISE(ABORT,'immutable usage cloud projection'); END;").map_err(failed)?;
    tx.commit().map_err(failed)
}

pub fn high(db: &Connection) -> Result<i64, String> {
    db.query_row(
        "SELECT COALESCE((SELECT seq FROM sqlite_sequence WHERE name='usage_events'),0)",
        [],
        |row| row.get(0),
    )
    .map_err(failed)
}

fn current(db: &Connection, source: &Source) -> Result<(bool, Provenance), String> {
    let (b, h, g) = key(source);
    let (enabled,epoch,interval):(bool,i64,i64)=db.query_row("SELECT enabled,lifecycle_epoch,consent_interval FROM usage_cloud_state WHERE binding=?1 AND source_home_id=?2 AND data_generation=?3",params![b,h,g],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).map_err(failed)?;
    Ok((
        enabled,
        Provenance {
            lifecycle_epoch: LifecycleEpoch::try_from(epoch as u64).map_err(contract)?,
            consent_interval: Counter::try_from(interval as u64).map_err(contract)?,
        },
    ))
}

fn authorized(db: &Connection, source: &Source, provenance: &Provenance) -> Result<(), String> {
    let (enabled, actual) = current(db, source)?;
    if !enabled || &actual != provenance {
        return Err("Usage cloud upload consent is not current".into());
    }
    Ok(())
}

fn enabled(db: &Connection, source: &Source) -> Result<bool, String> {
    if clear_outcome_unknown(db, source)? {
        return Ok(false);
    }
    let (b, h, g) = key(source);
    Ok(db.query_row("SELECT enabled=1 AND cleared=0 FROM usage_cloud_state WHERE binding=?1 AND source_home_id=?2 AND data_generation=?3",params![b,h,g],|row|row.get(0)).optional().map_err(failed)?.unwrap_or(false))
}

fn has_clear_intent_column(db: &Connection) -> Result<bool, String> {
    db.query_row("SELECT EXISTS(SELECT 1 FROM pragma_table_info('usage_cloud_state') WHERE name='clear_intent_json')", [], |row| row.get(0)).map_err(failed)
}

/// An unresolved dispatched clear cannot be treated as either retained custody
/// or permission to discard selected history. Older queues had no such intent.
pub fn clear_outcome_unknown(db: &Connection, source: &Source) -> Result<bool, String> {
    if !has_clear_intent_column(db)? {
        return Ok(false);
    }
    let (b, h, g) = key(source);
    db.query_row("SELECT clear_intent_json IS NOT NULL AND cleared=0 FROM usage_cloud_state WHERE binding=?1 AND source_home_id=?2 AND data_generation=?3", params![b,h,g], |row|row.get(0)).optional().map_err(failed).map(|value|value.unwrap_or(false))
}

/// Explicit future consent starts strictly after the current capture high-water.
#[cfg(any(cargo_ai_cli, test))]
pub fn enable(
    db: &mut Connection,
    source: &Source,
    provenance: &Provenance,
    now_ms: i64,
) -> Result<i64, String> {
    if now_ms < 0 {
        return Err(invalid());
    }
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(failed)?;
    let (b, h, g) = key(source);
    if clear_outcome_unknown(&tx, source)? {
        return Err("Usage cloud clear outcome is unknown; reconcile the exact confirmation or a newer generation before enabling".into());
    }
    let prior:Option<(bool,i64,i64,bool)>=tx.query_row("SELECT enabled,lifecycle_epoch,consent_interval,cleared FROM usage_cloud_state WHERE binding=?1 AND source_home_id=?2 AND data_generation=?3",params![b,h,g],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional().map_err(failed)?;
    if let Some((enabled, epoch, interval, cleared)) = prior {
        if cleared {
            return Err("Usage cloud generation was cleared; negotiate the new generation".into());
        }
        if enabled
            && epoch == provenance.lifecycle_epoch.get() as i64
            && interval == provenance.consent_interval.get() as i64
        {
            return tx.query_row("SELECT start_sequence FROM usage_cloud_windows WHERE binding=?1 AND source_home_id=?2 AND data_generation=?3 AND end_sequence IS NULL",params![b,h,g],|r|r.get(0)).map_err(failed);
        }
        if enabled
            || provenance.lifecycle_epoch.get() < (epoch as u64)
            || (provenance.lifecycle_epoch.get() == epoch as u64
                && provenance.consent_interval.get() <= (interval as u64))
        {
            return Err("Usage cloud enable requires a renewed permission interval".into());
        }
    }
    let boundary = high(&tx)?;
    tx.execute("INSERT INTO usage_cloud_state(binding,source_home_id,data_generation,enabled,lifecycle_epoch,consent_interval) VALUES(?1,?2,?3,1,?4,?5) ON CONFLICT(binding,source_home_id,data_generation) DO UPDATE SET enabled=1,lifecycle_epoch=?4,consent_interval=?5",params![b,h,g,provenance.lifecycle_epoch.get() as i64,provenance.consent_interval.get() as i64]).map_err(failed)?;
    tx.execute(
        "INSERT INTO usage_cloud_windows VALUES(?1,?2,?3,?4,?5,?6,0,?7,NULL,?7)",
        params![
            uuid::Uuid::new_v4().to_string(),
            b,
            h,
            g,
            provenance.lifecycle_epoch.get() as i64,
            provenance.consent_interval.get() as i64,
            boundary
        ],
    )
    .map_err(failed)?;
    tx.execute("INSERT INTO usage_cloud_schedule(binding,source_home_id,data_generation,next_due_ms,last_clock_ms) VALUES(?1,?2,?3,?4,?4) ON CONFLICT(binding,source_home_id,data_generation) DO UPDATE SET next_due_ms=?4,last_clock_ms=?4,lease_token=NULL,lease_expires_ms=NULL",params![b,h,g,now_ms]).map_err(failed)?;
    tx.commit().map_err(failed)?;
    Ok(boundary)
}

#[cfg(any(cargo_ai_cli, test))]
pub fn disable(db: &mut Connection, source: &Source) -> Result<(), String> {
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(failed)?;
    let (b, h, g) = key(source);
    let boundary = high(&tx)?;
    tx.execute("UPDATE usage_cloud_windows SET end_sequence=?4 WHERE binding=?1 AND source_home_id=?2 AND data_generation=?3 AND end_sequence IS NULL",params![b,h,g,boundary]).map_err(failed)?;
    tx.execute("UPDATE usage_cloud_state SET enabled=0 WHERE binding=?1 AND source_home_id=?2 AND data_generation=?3",params![b,h,g]).map_err(failed)?;
    tx.execute("UPDATE usage_cloud_schedule SET lease_token=NULL,lease_expires_ms=NULL WHERE binding=?1 AND source_home_id=?2 AND data_generation=?3",params![b,h,g]).map_err(failed)?;
    tx.commit().map_err(failed)
}

/// The caller has verified the exact preview hash and through-sequence boundary.
/// Selection is durable immediately; bounded capture can finish on later calls.
#[cfg(any(cargo_ai_cli, test))]
pub fn select_history(
    db: &mut Connection,
    source: &Source,
    provenance: &Provenance,
    through: i64,
) -> Result<(), String> {
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(failed)?;
    authorized(&tx, source, provenance)?;
    if through < 0 || through > high(&tx)? {
        return Err(invalid());
    }
    let (b, h, g) = key(source);
    tx.execute(
        "INSERT INTO usage_cloud_windows VALUES(?1,?2,?3,?4,?5,?6,1,0,?7,0)",
        params![
            uuid::Uuid::new_v4().to_string(),
            b,
            h,
            g,
            provenance.lifecycle_epoch.get() as i64,
            provenance.consent_interval.get() as i64,
            through
        ],
    )
    .map_err(failed)?;
    tx.commit().map_err(failed)
}

/// Select at most 100 rows from one consented window. Callback failures roll back
/// the scan cursor and preserve every unscanned custody obligation.
pub fn capture<F>(db: &mut Connection, source: &Source, project: F) -> Result<usize, String>
where
    F: Fn(&Connection, &Value) -> Result<String, String>,
{
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(failed)?;
    let (b, h, g) = key(source);
    let boundary = high(&tx)?;
    let window:Option<(String,i64,i64,bool)>=tx.query_row("SELECT window_id,last_scan,COALESCE(end_sequence,?4),historical FROM usage_cloud_windows WHERE binding=?1 AND source_home_id=?2 AND data_generation=?3 AND last_scan<COALESCE(end_sequence,?4) ORDER BY last_scan,window_id LIMIT 1",params![b,h,g,boundary],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional().map_err(failed)?;
    let Some((window, after, through, _historical)) = window else {
        return Ok(0);
    };
    let rows = {
        let mut stmt=tx.prepare("SELECT sequence,event_id,record_json FROM usage_event_records WHERE sequence>?1 AND sequence<=?2 ORDER BY sequence LIMIT 100").map_err(failed)?;
        let records = stmt
            .query_map(params![after, through], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })
            .map_err(failed)?;
        records.collect::<Result<Vec<_>, _>>().map_err(failed)?
    };
    let mut added = 0;
    for (sequence, event_id, text) in &rows {
        let exists:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM usage_cloud_queue WHERE binding=?1 AND source_home_id=?2 AND data_generation=?3 AND event_id=?4)",params![b,h,g,event_id],|r|r.get(0)).map_err(failed)?;
        if exists {
            continue;
        }
        let raw: Value = serde_json::from_str(text).map_err(|_| invalid())?;
        if raw.get("restored") == Some(&Value::Bool(true)) {
            tx.execute("INSERT OR IGNORE INTO usage_cloud_blocked VALUES(?1,?2,?3,?4,?5,'origin_unverified')",params![b,h,g,event_id,sequence]).map_err(failed)?;
            continue;
        }
        let event = EventSubmission::from_allowed_text_with_timing(
            project(&tx, &raw)?,
            timing_from_local_capture(&raw).map_err(contract)?,
        )
        .map_err(contract)?;
        let member = event.member().map_err(contract)?;
        if &member.event_id != event_id {
            return Err("Usage cloud projection changed stable event identity".into());
        }
        added+=tx.execute("INSERT INTO usage_cloud_queue(binding,source_home_id,data_generation,event_id,sequence,original_text,original_sha256,semantic_hash_version,semantic_sha256,timing_json) VALUES(?1,?2,?3,?4,?5,?6,?7,1,?8,?9)",params![b,h,g,event_id,sequence,event.original_text,event.original_sha256.as_str(),event.semantic_sha256.as_str(),event.timing.as_ref().map(serde_json::to_string).transpose().map_err(|_|invalid())?]).map_err(failed)?;
    }
    let scanned = if rows.len() < MAX_BATCH_EVENTS {
        through
    } else {
        rows.last().ok_or_else(invalid)?.0
    };
    tx.execute(
        "UPDATE usage_cloud_windows SET last_scan=?2 WHERE window_id=?1",
        params![window, scanned],
    )
    .map_err(failed)?;
    tx.commit().map_err(failed)?;
    Ok(added)
}

/// Any selected obligation in any account/Home/generation protects the local row.
/// Disable, failures and quarantine never imply retained custody.
pub fn protected_event(db: &Connection, event_id: &str, sequence: i64) -> Result<bool, String> {
    if !installed(db)? {
        return Ok(false);
    }
    let selected: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM usage_cloud_queue WHERE event_id=?1 AND retained=0) OR EXISTS(SELECT 1 FROM usage_cloud_blocked WHERE event_id=?1) OR EXISTS(SELECT 1 FROM usage_cloud_windows WHERE ?2>last_scan AND ?2>start_sequence AND (end_sequence IS NULL OR ?2<=end_sequence))",params![event_id,sequence],|r|r.get(0)).map_err(failed)?;
    if selected || !has_clear_intent_column(db)? {
        return Ok(selected);
    }
    db.query_row("SELECT EXISTS(SELECT 1 FROM usage_cloud_queue q JOIN usage_cloud_state s USING(binding,source_home_id,data_generation) WHERE q.event_id=?1 AND s.clear_intent_json IS NOT NULL AND s.cleared=0)",[event_id],|row|row.get(0)).map_err(failed)
}

/// Historical custody is known locally; upload disable does not erase it. A
/// separately authenticated explicit clear invalidates that generation's copy.
pub fn retained_event(db: &Connection, event_id: &str) -> Result<bool, String> {
    if !installed(db)? {
        return Ok(false);
    }
    let query = if has_clear_intent_column(db)? {
        "SELECT EXISTS(SELECT 1 FROM usage_cloud_queue q JOIN usage_cloud_state s USING(binding,source_home_id,data_generation) WHERE q.event_id=?1 AND q.retained=1 AND q.quarantined=0 AND s.cleared=0 AND s.clear_intent_json IS NULL)"
    } else {
        "SELECT EXISTS(SELECT 1 FROM usage_cloud_queue q JOIN usage_cloud_state s USING(binding,source_home_id,data_generation) WHERE q.event_id=?1 AND q.retained=1 AND q.quarantined=0 AND s.cleared=0)"
    };
    db.query_row(query, [event_id], |row| row.get(0))
        .map_err(failed)
}

/// Persist the exact confirmation before dispatch, close consent and fence any
/// active sender. Unknown outcomes preserve all selected local history. Later
/// explicit previews for the same generation preserve every earlier attempt.
#[cfg(any(cargo_ai_cli, test))]
pub fn begin_clear(db: &mut Connection, request: &Request) -> Result<(), String> {
    const MAX_ATTEMPTS: usize = 100;
    let Request::ClearConfirm {
        source,
        lifecycle_epoch,
        ..
    } = request
    else {
        return Err(invalid());
    };
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(failed)?;
    let (_, provenance) = current(&tx, source)?;
    if provenance.lifecycle_epoch != *lifecycle_epoch {
        return Err(invalid());
    }
    let (b, h, g) = key(source);
    let (cleared, prior):(bool,Option<String>)=tx.query_row("SELECT cleared,clear_intent_json FROM usage_cloud_state WHERE binding=?1 AND source_home_id=?2 AND data_generation=?3",params![b,h,g],|row|Ok((row.get(0)?,row.get(1)?))).map_err(failed)?;
    if cleared {
        return Err("Usage cloud generation was already cleared".into());
    }
    let mut attempts: Vec<Request> = match prior {
        None => Vec::new(),
        Some(text) => {
            let value = parse_account_document(&text).map_err(contract)?;
            if value.is_array() {
                serde_json::from_value(value).map_err(failed)?
            } else {
                vec![Request::parse(&text).map_err(contract)?]
            }
        }
    };
    if attempts.len() > MAX_ATTEMPTS
        || attempts
            .iter()
            .any(|attempt| !matches!(attempt,Request::ClearConfirm{source:old,..} if old==source))
    {
        return Err(invalid());
    }
    if !attempts.contains(request) {
        if attempts.len() == MAX_ATTEMPTS {
            return Err("Usage cloud clear attempt history is full; reconcile a preserved exact confirmation".into());
        }
        attempts.push(request.clone());
    }
    let encoded = serde_json::to_string(&attempts).map_err(failed)?;
    let boundary = high(&tx)?;
    tx.execute("UPDATE usage_cloud_state SET enabled=0,clear_intent_json=?4 WHERE binding=?1 AND source_home_id=?2 AND data_generation=?3",params![b,h,g,encoded]).map_err(failed)?;
    tx.execute("UPDATE usage_cloud_windows SET end_sequence=?4 WHERE binding=?1 AND source_home_id=?2 AND data_generation=?3 AND end_sequence IS NULL",params![b,h,g,boundary]).map_err(failed)?;
    tx.execute("UPDATE usage_cloud_schedule SET lease_token=NULL,lease_expires_ms=NULL WHERE binding=?1 AND source_home_id=?2 AND data_generation=?3",params![b,h,g]).map_err(failed)?;
    tx.commit().map_err(failed)
}

/// Call only after an authenticated explicit clear result for this exact source
/// and generation, or authenticated same-binding generation advance. Keep all
/// pending payloads, windows, confirmation intent and historical receipts.
#[cfg(any(cargo_ai_cli, test))]
pub fn mark_cleared(db: &mut Connection, source: &Source) -> Result<(), String> {
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(failed)?;
    let (b, h, g) = key(source);
    let boundary = high(&tx)?;
    let changed=tx.execute("UPDATE usage_cloud_state SET enabled=0,cleared=1 WHERE binding=?1 AND source_home_id=?2 AND data_generation=?3",params![b,h,g]).map_err(failed)?;
    if changed != 1 {
        return Err(invalid());
    }
    tx.execute("UPDATE usage_cloud_windows SET end_sequence=?4 WHERE binding=?1 AND source_home_id=?2 AND data_generation=?3 AND end_sequence IS NULL",params![b,h,g,boundary]).map_err(failed)?;
    tx.execute("UPDATE usage_cloud_schedule SET lease_token=NULL,lease_expires_ms=NULL WHERE binding=?1 AND source_home_id=?2 AND data_generation=?3",params![b,h,g]).map_err(failed)?;
    tx.commit().map_err(failed)
}

/// Authenticated status for this exact account binding proves every older
/// generation was logically cleared. Reconcile all saved sources, even when a
/// previous enable already selected the newer generation after a lost response.
#[cfg(any(cargo_ai_cli, test))]
pub fn reconcile_generation(
    db: &mut Connection,
    binding: &CanonicalUuid,
    generation: Counter,
) -> Result<(), String> {
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(failed)?;
    let boundary = high(&tx)?;
    tx.execute(
        "UPDATE usage_cloud_state SET enabled=0,cleared=1 WHERE binding=?1 AND data_generation<?2",
        params![binding.as_str(), generation.get() as i64],
    )
    .map_err(failed)?;
    tx.execute("UPDATE usage_cloud_windows SET end_sequence=?3 WHERE binding=?1 AND data_generation<?2 AND end_sequence IS NULL",params![binding.as_str(),generation.get() as i64,boundary]).map_err(failed)?;
    tx.execute("UPDATE usage_cloud_schedule SET lease_token=NULL,lease_expires_ms=NULL WHERE binding=?1 AND data_generation<?2",params![binding.as_str(),generation.get() as i64]).map_err(failed)?;
    tx.commit().map_err(failed)
}

pub fn quarantine_event(
    db: &mut Connection,
    source: &Source,
    event_id: &str,
) -> Result<(), String> {
    let (b, h, g) = key(source);
    let changed=db.execute("UPDATE usage_cloud_queue SET quarantined=1 WHERE binding=?1 AND source_home_id=?2 AND data_generation=?3 AND event_id=?4 AND retained=0",params![b,h,g,event_id]).map_err(failed)?;
    if changed != 1 {
        return Err(invalid());
    }
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Status {
    pub enabled: bool,
    pub cleared: bool,
    pub clear_outcome_unknown: bool,
    pub pending: i64,
    pub quarantined: i64,
    pub retained: i64,
    pub origin_unverified: i64,
    pub unscanned: i64,
    pub unknown_batches: i64,
    pub buffered_batches: i64,
    pub terminal_unretained_batches: i64,
}

pub fn status(db: &Connection, source: &Source) -> Result<Status, String> {
    if !installed(db)? {
        return Ok(Status {
            enabled: false,
            cleared: false,
            clear_outcome_unknown: false,
            pending: 0,
            quarantined: 0,
            retained: 0,
            origin_unverified: 0,
            unscanned: 0,
            unknown_batches: 0,
            buffered_batches: 0,
            terminal_unretained_batches: 0,
        });
    }
    let (b, h, g) = key(source);
    let enabled = enabled(db, source)?;
    let clear_outcome_unknown = clear_outcome_unknown(db, source)?;
    let cleared:bool=db.query_row("SELECT cleared FROM usage_cloud_state WHERE binding=?1 AND source_home_id=?2 AND data_generation=?3",params![b,h,g],|row|row.get(0)).optional().map_err(failed)?.unwrap_or(false);
    let (pending,retained,quarantined):(i64,i64,i64)=db.query_row("SELECT COALESCE(SUM(retained=0),0),COALESCE(SUM(retained=1),0),COALESCE(SUM(quarantined=1 AND retained=0),0) FROM usage_cloud_queue WHERE binding=?1 AND source_home_id=?2 AND data_generation=?3",params![b,h,g],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).map_err(failed)?;
    let origin_unverified=db.query_row("SELECT COUNT(*) FROM usage_cloud_blocked WHERE binding=?1 AND source_home_id=?2 AND data_generation=?3",params![b,h,g],|r|r.get(0)).map_err(failed)?;
    let unscanned=db.query_row("SELECT COUNT(*) FROM usage_event_records e WHERE EXISTS(SELECT 1 FROM usage_cloud_windows w WHERE w.binding=?1 AND w.source_home_id=?2 AND w.data_generation=?3 AND e.sequence>w.last_scan AND e.sequence>w.start_sequence AND (w.end_sequence IS NULL OR e.sequence<=w.end_sequence))",params![b,h,g],|r|r.get(0)).map_err(failed)?;
    let (unknown_batches,buffered_batches,terminal_unretained_batches):(i64,i64,i64)=db.query_row("SELECT COALESCE(SUM(outcome='unknown'),0),COALESCE(SUM(outcome='buffered'),0),COALESCE(SUM(outcome='terminal_unretained'),0) FROM usage_cloud_batches WHERE binding=?1 AND source_home_id=?2 AND data_generation=?3",params![b,h,g],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).map_err(failed)?;
    Ok(Status {
        enabled,
        cleared,
        clear_outcome_unknown,
        pending,
        quarantined,
        retained: if cleared || clear_outcome_unknown {
            0
        } else {
            retained
        },
        origin_unverified,
        unscanned,
        unknown_batches,
        buffered_batches,
        terminal_unretained_batches,
    })
}

#[derive(Clone, Debug)]
pub struct Lease {
    source: Source,
    token: String,
    expires_ms: i64,
}
impl Lease {
    pub fn source(&self) -> &Source {
        &self.source
    }
    pub fn expires_ms(&self) -> i64 {
        self.expires_ms
    }
}

fn check_lease(db: &Connection, lease: &Lease, now: i64) -> Result<(), String> {
    if now < 0 || lease.expires_ms <= now || lease.expires_ms.saturating_sub(now) > MAX_LEASE_MS {
        return Err("Usage cloud lease expired".into());
    }
    let (b, h, g) = key(&lease.source);
    let valid:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM usage_cloud_schedule WHERE binding=?1 AND source_home_id=?2 AND data_generation=?3 AND lease_token=?4 AND lease_expires_ms=?5)",params![b,h,g,lease.token,lease.expires_ms],|r|r.get(0)).map_err(failed)?;
    if !valid {
        return Err("Usage cloud lease was superseded".into());
    }
    Ok(())
}

/// Expired or clock-skewed leases are fenced by a new random token. No SQLite
/// transaction is retained by the returned value or during subsequent network I/O.
pub fn acquire_lease(
    db: &mut Connection,
    source: &Source,
    now: i64,
    ttl_ms: i64,
) -> Result<Option<Lease>, String> {
    if now < 0 || !(1..=MAX_LEASE_MS).contains(&ttl_ms) || now.checked_add(ttl_ms).is_none() {
        return Err(invalid());
    }
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(failed)?;
    if !enabled(&tx, source)? {
        return Ok(None);
    }
    let (b, h, g) = key(source);
    let expires:Option<i64>=tx.query_row("SELECT lease_expires_ms FROM usage_cloud_schedule WHERE binding=?1 AND source_home_id=?2 AND data_generation=?3",params![b,h,g],|r|r.get(0)).map_err(failed)?;
    if expires.is_some_and(|expires| expires > now && expires.saturating_sub(now) <= MAX_LEASE_MS) {
        return Ok(None);
    }
    let lease = Lease {
        source: source.clone(),
        token: uuid::Uuid::new_v4().to_string(),
        expires_ms: now + ttl_ms,
    };
    tx.execute("UPDATE usage_cloud_schedule SET lease_token=?4,lease_expires_ms=?5 WHERE binding=?1 AND source_home_id=?2 AND data_generation=?3",params![b,h,g,lease.token,lease.expires_ms]).map_err(failed)?;
    tx.commit().map_err(failed)?;
    Ok(Some(lease))
}

pub fn release_lease(db: &mut Connection, lease: &Lease, now: i64) -> Result<(), String> {
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(failed)?;
    check_lease(&tx, lease, now)?;
    let (b, h, g) = key(&lease.source);
    tx.execute("UPDATE usage_cloud_schedule SET lease_token=NULL,lease_expires_ms=NULL WHERE binding=?1 AND source_home_id=?2 AND data_generation=?3",params![b,h,g]).map_err(failed)?;
    tx.commit().map_err(failed)
}

#[derive(Clone, Debug)]
pub struct FrozenBatch {
    source: Source,
    provenance: Provenance,
    id: CanonicalUuid,
    request_json: String,
    members: Vec<ReceiptMember>,
    membership: Sha256Hash,
}
impl FrozenBatch {
    pub fn source(&self) -> &Source {
        &self.source
    }
    pub fn provenance(&self) -> &Provenance {
        &self.provenance
    }
    pub fn id(&self) -> &CanonicalUuid {
        &self.id
    }
    pub fn request_json(&self) -> &str {
        &self.request_json
    }
    pub fn members(&self) -> &[ReceiptMember] {
        &self.members
    }
    pub fn membership(&self) -> &Sha256Hash {
        &self.membership
    }
}

fn load_batch(db: &Connection, source: &Source, id: &CanonicalUuid) -> Result<FrozenBatch, String> {
    let (b, h, g) = key(source);
    let text:String=db.query_row("SELECT request_json FROM usage_cloud_batches WHERE binding=?1 AND source_home_id=?2 AND data_generation=?3 AND batch_id=?4 AND sealed=1",params![b,h,g,id.as_str()],|r|r.get(0)).map_err(failed)?;
    let Request::Sync {
        source: stored_source,
        provenance,
        batch_id,
        ordered_membership_sha256,
        events,
        ..
    } = Request::parse(&text).map_err(contract)?
    else {
        return Err(invalid());
    };
    if &stored_source != source || &batch_id != id {
        return Err(invalid());
    }
    let members = events
        .iter()
        .map(EventSubmission::member)
        .collect::<Result<Vec<_>, _>>()
        .map_err(contract)?;
    if super::contract::ordered_membership_sha256(source, &members).map_err(contract)?
        != ordered_membership_sha256
    {
        return Err(invalid());
    }
    Ok(FrozenBatch {
        source: source.clone(),
        provenance,
        id: id.clone(),
        request_json: text,
        members,
        membership: ordered_membership_sha256,
    })
}

fn freeze(
    db: &Connection,
    source: &Source,
    provenance: &Provenance,
    events: Vec<EventSubmission>,
    readmitted_from: Option<&str>,
) -> Result<FrozenBatch, String> {
    let id = CanonicalUuid::try_from(uuid::Uuid::new_v4().to_string()).map_err(contract)?;
    let members = events
        .iter()
        .map(EventSubmission::member)
        .collect::<Result<Vec<_>, _>>()
        .map_err(contract)?;
    let membership = ordered_membership_sha256(source, &members).map_err(contract)?;
    let request = Request::Sync {
        protocol_version: Version,
        source: source.clone(),
        provenance: provenance.clone(),
        batch_id: id.clone(),
        ordered_membership_sha256: membership.clone(),
        events,
    };
    let text = serde_json::to_string(&request).map_err(|_| invalid())?;
    Request::parse(&text).map_err(contract)?;
    if !within_landing_budget(text.len(), members.len()) {
        return Err("Usage cloud batch exceeds its landing metadata budget".into());
    }
    let (b, h, g) = key(source);
    db.execute("INSERT INTO usage_cloud_batches(binding,source_home_id,data_generation,batch_id,lifecycle_epoch,consent_interval,request_json,ordered_membership_sha256,sealed,readmitted_from) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,0,?9)",params![b,h,g,id.as_str(),provenance.lifecycle_epoch.get() as i64,provenance.consent_interval.get() as i64,text,membership.as_str(),readmitted_from]).map_err(failed)?;
    for (ordinal, member) in members.iter().enumerate() {
        db.execute(
            "INSERT INTO usage_cloud_batch_members(binding,source_home_id,data_generation,batch_id,ordinal,event_id,original_sha256,semantic_hash_version,semantic_sha256,timing_sha256) VALUES(?1,?2,?3,?4,?5,?6,?7,1,?8,?9)",
            params![
                b,
                h,
                g,
                id.as_str(),
                ordinal as i64,
                member.event_id,
                member.original_sha256.as_str(),
                member.semantic_sha256.as_str(),
                member.timing_sha256.as_ref().map(Sha256Hash::as_str)
            ],
        )
        .map_err(failed)?;
        let changed=db.execute("UPDATE usage_cloud_queue SET active_batch_id=?5 WHERE binding=?1 AND source_home_id=?2 AND data_generation=?3 AND event_id=?4 AND original_sha256=?6 AND semantic_sha256=?7 AND quarantined=0 AND ((?8 IS NULL AND active_batch_id IS NULL AND retained=0) OR active_batch_id=?8)",params![b,h,g,member.event_id,id.as_str(),member.original_sha256.as_str(),member.semantic_sha256.as_str(),readmitted_from]).map_err(failed)?;
        if changed != 1 {
            return Err(invalid());
        }
    }
    db.execute("UPDATE usage_cloud_batches SET sealed=1 WHERE binding=?1 AND source_home_id=?2 AND data_generation=?3 AND batch_id=?4",params![b,h,g,id.as_str()]).map_err(failed)?;
    load_batch(db, source, &id)
}

/// Reuses exact stored bytes after unknown responses. A changed permission epoch
/// never silently rewrites a frozen batch; use explicit readmission instead.
pub fn prepare_batch(
    db: &mut Connection,
    lease: &Lease,
    now: i64,
) -> Result<Option<FrozenBatch>, String> {
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(failed)?;
    check_lease(&tx, lease, now)?;
    let (_, provenance) = current(&tx, &lease.source)?;
    authorized(&tx, &lease.source, &provenance)?;
    let (b, h, g) = key(&lease.source);
    let prior:Option<String>=tx.query_row("SELECT b.batch_id FROM usage_cloud_batches b WHERE b.binding=?1 AND b.source_home_id=?2 AND b.data_generation=?3 AND b.lifecycle_epoch=?4 AND b.consent_interval=?5 AND b.sealed=1 AND b.outcome!='retained' AND NOT EXISTS(SELECT 1 FROM usage_cloud_batch_members m JOIN usage_cloud_queue q USING(binding,source_home_id,data_generation,event_id) WHERE m.binding=b.binding AND m.source_home_id=b.source_home_id AND m.data_generation=b.data_generation AND m.batch_id=b.batch_id AND q.quarantined=1) ORDER BY b.rowid LIMIT 1",params![b,h,g,provenance.lifecycle_epoch.get() as i64,provenance.consent_interval.get() as i64],|r|r.get(0)).optional().map_err(failed)?;
    if let Some(id) = prior {
        return load_batch(
            &tx,
            &lease.source,
            &CanonicalUuid::try_from(id).map_err(contract)?,
        )
        .map(Some);
    }
    let rows = {
        let mut stmt=tx.prepare("SELECT original_text,timing_json FROM usage_cloud_queue WHERE binding=?1 AND source_home_id=?2 AND data_generation=?3 AND retained=0 AND quarantined=0 AND active_batch_id IS NULL ORDER BY sequence LIMIT 100").map_err(failed)?;
        let records = stmt
            .query_map(params![b, h, g], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
            })
            .map_err(failed)?;
        records.collect::<Result<Vec<_>, _>>().map_err(failed)?
    };
    let mut events = Vec::new();
    for (text, timing) in rows {
        let timing = timing
            .map(|value| serde_json::from_str(&value).map_err(|_| invalid()))
            .transpose()?;
        events
            .push(EventSubmission::from_allowed_text_with_timing(text, timing).map_err(contract)?);
        let members = events
            .iter()
            .map(EventSubmission::member)
            .collect::<Result<Vec<_>, _>>()
            .map_err(contract)?;
        let candidate = Request::Sync {
            protocol_version: Version,
            source: lease.source.clone(),
            provenance: provenance.clone(),
            batch_id: CanonicalUuid::try_from("00000000-0000-4000-8000-000000000000".to_string())
                .map_err(contract)?,
            ordered_membership_sha256: ordered_membership_sha256(&lease.source, &members)
                .map_err(contract)?,
            events: events.clone(),
        };
        if !within_landing_budget(
            serde_json::to_vec(&candidate).map_err(|_| invalid())?.len(),
            members.len(),
        ) {
            events.pop();
            break;
        }
    }
    if events.is_empty() {
        return Ok(None);
    }
    let batch = freeze(&tx, &lease.source, &provenance, events, None)?;
    tx.commit().map_err(failed)?;
    Ok(Some(batch))
}

/// Explicit renewed permission re-admits the identical original members under a
/// new batch while preserving every old attempt and opaque commitment.
#[cfg(any(cargo_ai_cli, test))]
pub fn readmit_batch(
    db: &mut Connection,
    lease: &Lease,
    old_id: &CanonicalUuid,
    now: i64,
) -> Result<FrozenBatch, String> {
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(failed)?;
    check_lease(&tx, lease, now)?;
    let (_, provenance) = current(&tx, &lease.source)?;
    authorized(&tx, &lease.source, &provenance)?;
    let old = load_batch(&tx, &lease.source, old_id)?;
    if old.provenance == provenance {
        return Err("Usage cloud readmission requires changed explicit permission".into());
    }
    let Request::Sync { events, .. } = Request::parse(&old.request_json).map_err(contract)? else {
        return Err(invalid());
    };
    let next = freeze(
        &tx,
        &lease.source,
        &provenance,
        events,
        Some(old_id.as_str()),
    )?;
    tx.commit().map_err(failed)?;
    Ok(next)
}

/// Must commit before the first network dispatch. Unknown remains retryable and
/// is never converted to success by timeout, lease expiry or elapsed time.
pub fn begin_attempt(
    db: &mut Connection,
    lease: &Lease,
    batch_id: &CanonicalUuid,
    now: i64,
) -> Result<(), String> {
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(failed)?;
    check_lease(&tx, lease, now)?;
    let batch = load_batch(&tx, &lease.source, batch_id)?;
    authorized(&tx, &lease.source, &batch.provenance)?;
    let (b, h, g) = key(&lease.source);
    let quarantined:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM usage_cloud_batch_members m JOIN usage_cloud_queue q USING(binding,source_home_id,data_generation,event_id) WHERE m.binding=?1 AND m.source_home_id=?2 AND m.data_generation=?3 AND m.batch_id=?4 AND q.quarantined=1)",params![b,h,g,batch_id.as_str()],|row|row.get(0)).map_err(failed)?;
    if quarantined {
        return Err("Usage cloud batch includes quarantined records".into());
    }
    let changed=tx.execute("UPDATE usage_cloud_batches SET attempts=attempts+1,outcome=CASE WHEN outcome='retained' THEN 'retained' ELSE 'unknown' END WHERE binding=?1 AND source_home_id=?2 AND data_generation=?3 AND batch_id=?4 AND outcome!='retained'",params![b,h,g,batch_id.as_str()]).map_err(failed)?;
    if changed != 1 {
        return Err(invalid());
    }
    tx.commit().map_err(failed)
}

fn admission_at(
    db: &Connection,
    lease: &Lease,
    batch_id: &CanonicalUuid,
    admission: &AdmissionCommitment,
    now: i64,
    buffered: bool,
) -> Result<(), String> {
    check_lease(db, lease, now)?;
    let batch = load_batch(db, &lease.source, batch_id)?;
    admission
        .verify(&batch.source, &batch.provenance, &batch.id, &batch.members)
        .map_err(contract)?;
    let text = serde_json::to_string(admission).map_err(|_| invalid())?;
    let (b, h, g) = key(&lease.source);
    let changed=db.execute("UPDATE usage_cloud_batches SET admission_json=?5,outcome=CASE WHEN outcome='retained' THEN outcome WHEN ?6 THEN 'buffered' ELSE outcome END WHERE binding=?1 AND source_home_id=?2 AND data_generation=?3 AND batch_id=?4 AND attempts>0",params![b,h,g,batch_id.as_str(),text,buffered]).map_err(failed)?;
    if changed != 1 {
        return Err(invalid());
    }
    Ok(())
}

/// Input must be the authenticated admission response, including on recovery.
pub fn record_admission(
    db: &mut Connection,
    lease: &Lease,
    batch_id: &CanonicalUuid,
    admission: &AdmissionCommitment,
    now: i64,
) -> Result<(), String> {
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(failed)?;
    admission_at(&tx, lease, batch_id, admission, now, false)?;
    tx.commit().map_err(failed)
}
pub fn record_buffered(
    db: &mut Connection,
    lease: &Lease,
    batch_id: &CanonicalUuid,
    admission: &AdmissionCommitment,
    now: i64,
) -> Result<(), String> {
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(failed)?;
    admission_at(&tx, lease, batch_id, admission, now, true)?;
    tx.commit().map_err(failed)
}

/// An authenticated terminal disposition preserves every original byte and
/// keeps all rows protected. Only an exact retained receipt releases custody.
pub fn record_terminal_unretained(
    db: &mut Connection,
    lease: &Lease,
    batch_id: &CanonicalUuid,
    admission: &AdmissionCommitment,
    now: i64,
) -> Result<(), String> {
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(failed)?;
    admission_at(&tx, lease, batch_id, admission, now, false)?;
    let (b, h, g) = key(&lease.source);
    tx.execute("UPDATE usage_cloud_batches SET outcome='terminal_unretained' WHERE binding=?1 AND source_home_id=?2 AND data_generation=?3 AND batch_id=?4 AND outcome!='retained'",params![b,h,g,batch_id.as_str()]).map_err(failed)?;
    tx.commit().map_err(failed)
}

pub fn acknowledge_retained(
    db: &mut Connection,
    lease: &Lease,
    batch_id: &CanonicalUuid,
    receipt: &Receipt,
    now: i64,
) -> Result<(), String> {
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(failed)?;
    check_lease(&tx, lease, now)?;
    let batch = load_batch(&tx, &lease.source, batch_id)?;
    let (b, h, g) = key(&lease.source);
    let text:Option<String>=tx.query_row("SELECT admission_json FROM usage_cloud_batches WHERE binding=?1 AND source_home_id=?2 AND data_generation=?3 AND batch_id=?4",params![b,h,g,batch_id.as_str()],|r|r.get(0)).map_err(failed)?;
    let admission: AdmissionCommitment = serde_json::from_str(
        &text.ok_or("Usage cloud authenticated admission commitment is missing")?,
    )
    .map_err(|_| invalid())?;
    admission
        .verify(&batch.source, &batch.provenance, &batch.id, &batch.members)
        .map_err(contract)?;
    receipt
        .verify_retained(&ReceiptExpectation {
            source: batch.source.clone(),
            provenance: batch.provenance.clone(),
            batch_id: batch.id.clone(),
            manifest_sha256: admission.manifest_sha256,
            ordered_membership_sha256: batch.membership.clone(),
            members: batch.members.clone(),
        })
        .map_err(contract)?;
    for member in &batch.members {
        let changed=tx.execute("UPDATE usage_cloud_queue SET retained=1 WHERE binding=?1 AND source_home_id=?2 AND data_generation=?3 AND event_id=?4 AND original_sha256=?5 AND semantic_sha256=?6",params![b,h,g,member.event_id,member.original_sha256.as_str(),member.semantic_sha256.as_str()]).map_err(failed)?;
        if changed != 1 {
            return Err(invalid());
        }
    }
    let encoded = serde_json::to_string(receipt).map_err(|_| invalid())?;
    tx.execute("UPDATE usage_cloud_batches SET outcome='retained',receipt_json=COALESCE(receipt_json,?5) WHERE binding=?1 AND source_home_id=?2 AND data_generation=?3 AND batch_id=?4",params![b,h,g,batch_id.as_str(),encoded]).map_err(failed)?;
    tx.commit().map_err(failed)
}

/// Clamp persisted wall-clock deadlines to at most an hour from the observed
/// clock. Backward jumps cannot strand retry work behind an old future deadline.
pub fn due(db: &mut Connection, source: &Source, now: i64) -> Result<bool, String> {
    if now < 0 {
        return Err(invalid());
    }
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(failed)?;
    if !enabled(&tx, source)? {
        return Ok(false);
    }
    let (b, h, g) = key(source);
    let (next,last):(i64,i64)=tx.query_row("SELECT next_due_ms,last_clock_ms FROM usage_cloud_schedule WHERE binding=?1 AND source_home_id=?2 AND data_generation=?3",params![b,h,g],|r|Ok((r.get(0)?,r.get(1)?))).map_err(failed)?;
    let next = if now < last {
        now
    } else {
        next.min(now.saturating_add(HOUR_MS))
    };
    tx.execute("UPDATE usage_cloud_schedule SET next_due_ms=?4,last_clock_ms=?5 WHERE binding=?1 AND source_home_id=?2 AND data_generation=?3",params![b,h,g,next,now]).map_err(failed)?;
    tx.commit().map_err(failed)?;
    Ok(next <= now)
}

/// Persist a bounded retry or hourly success deadline while holding the fence.
pub fn schedule_next(
    db: &mut Connection,
    lease: &Lease,
    now: i64,
    success: bool,
) -> Result<i64, String> {
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(failed)?;
    check_lease(&tx, lease, now)?;
    let (b, h, g) = key(&lease.source);
    let failures:i64=tx.query_row("SELECT failures FROM usage_cloud_schedule WHERE binding=?1 AND source_home_id=?2 AND data_generation=?3",params![b,h,g],|r|r.get(0)).map_err(failed)?;
    let next_failures = if success { 0 } else { (failures + 1).min(8) };
    let delay = if success {
        HOUR_MS
    } else {
        (30_000i64 * (1i64 << (next_failures - 1))).min(HOUR_MS)
    };
    let next = now.saturating_add(delay);
    tx.execute("UPDATE usage_cloud_schedule SET failures=?4,next_due_ms=?5,last_clock_ms=?6 WHERE binding=?1 AND source_home_id=?2 AND data_generation=?3",params![b,h,g,next_failures,next,now]).map_err(failed)?;
    tx.commit().map_err(failed)?;
    Ok(next)
}
