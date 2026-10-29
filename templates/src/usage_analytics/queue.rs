//! Local immutable selections and custody; every transaction ends before network I/O.
use super::contract::*;
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde_json::Value;
#[path = "local_queue.rs"]
mod local;
pub use local::{expire_events, high, installed, retained_event};
use local::{has_clear_intent_column, install_expiry_guards};

#[cfg(test)]
pub const HOUR_MS: i64 = 3_600_000;
pub const MAX_LEASE_MS: i64 = 60_000;
#[cfg(test)]
pub const LANDING_RECORD_ALLOWANCE: usize = 1536;
#[cfg(test)]
pub const LANDING_ENVELOPE_ALLOWANCE: usize = 4096;
#[cfg(test)]
pub(super) const MAX_ACCOUNT_DOCUMENT_BYTES: usize = 300 * 1024;
/// Strict bounded decoding of retained legacy clear-intent documents.
#[cfg(test)]
pub(super) fn parse_account_document(text: &str) -> Result<Value, ContractError> {
    if text.len() >= MAX_ACCOUNT_DOCUMENT_BYTES {
        return Err(ContractError::BoundsExceeded);
    }
    super::contract::parse_exact_json(text).map_err(|_| ContractError::InvalidRequest)
}
#[cfg(test)]
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

/// Upload metadata has an independent lifetime. Explicit rolling maintenance
/// coordinates its expiry with raw history and whole immutable retry batches.
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
        admission_json TEXT,receipt_json TEXT,readmitted_from TEXT,resend_hint_json TEXT,
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
    let has_hint: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM pragma_table_info('usage_cloud_batches') WHERE name='resend_hint_json')", [], |row| row.get(0)).map_err(failed)?;
    if !has_hint {
        tx.execute_batch("ALTER TABLE usage_cloud_batches ADD COLUMN resend_hint_json TEXT;")
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
    install_expiry_guards(&tx)?;
    tx.execute_batch("CREATE TABLE IF NOT EXISTS usage_cloud_time_excluded(binding TEXT NOT NULL,source_home_id TEXT NOT NULL,data_generation INTEGER NOT NULL,event_id TEXT NOT NULL,PRIMARY KEY(binding,source_home_id,data_generation,event_id));").map_err(failed)?;
    tx.commit().map_err(failed)
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
    invalidate_payload_required(&tx, source)?;
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
    invalidate_payload_required(&tx, source)?;
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

#[derive(Debug, Default, PartialEq, Eq)]
pub struct CaptureProgress {
    pub examined: usize,
    pub added: usize,
}

#[cfg(test)]
pub fn capture<F>(db: &mut Connection, source: &Source, project: F) -> Result<usize, String>
where
    F: Fn(&Connection, &Value) -> Result<String, String>,
{
    capture_progress(db, source, project).map(|progress| progress.added)
}

/// Project at most 100 newest events from one consented window. Per-event
/// membership preserves gaps and open-hour additions across restarts. Callback
/// failures roll back all selection progress. Return progress only after commit;
/// examined rows include persisted origin and time exclusions.
pub fn capture_progress<F>(
    db: &mut Connection,
    source: &Source,
    project: F,
) -> Result<CaptureProgress, String>
where
    F: Fn(&Connection, &Value) -> Result<String, String>,
{
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(failed)?;
    let (b, h, g) = key(source);
    let boundary = high(&tx)?;
    let window:Option<(String,i64,i64,bool)>=tx.query_row("SELECT window_id,start_sequence,COALESCE(end_sequence,?4),historical FROM usage_cloud_windows w WHERE binding=?1 AND source_home_id=?2 AND data_generation=?3 AND EXISTS(SELECT 1 FROM usage_event_records e WHERE e.sequence>w.start_sequence AND e.sequence<=COALESCE(w.end_sequence,?4) AND NOT EXISTS(SELECT 1 FROM usage_cloud_queue q WHERE q.binding=?1 AND q.source_home_id=?2 AND q.data_generation=?3 AND q.event_id=e.event_id) AND NOT EXISTS(SELECT 1 FROM usage_cloud_blocked q WHERE q.binding=?1 AND q.source_home_id=?2 AND q.data_generation=?3 AND q.event_id=e.event_id) AND NOT EXISTS(SELECT 1 FROM usage_cloud_time_excluded q WHERE q.binding=?1 AND q.source_home_id=?2 AND q.data_generation=?3 AND q.event_id=e.event_id)) ORDER BY COALESCE(end_sequence,?4) DESC,window_id LIMIT 1",params![b,h,g,boundary],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional().map_err(failed)?;
    let Some((window, after, through, _historical)) = window else {
        return Ok(CaptureProgress::default());
    };
    let rows = {
        let indexed:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='usage_retention_hours' AND type='table')",[],|r|r.get(0)).map_err(failed)?;
        let order = if indexed {
            "(SELECT hour_ms FROM usage_retention_hours h WHERE h.event_id=e.event_id) DESC,e.sequence DESC"
        } else {
            "e.sequence DESC"
        };
        let sql=format!("SELECT sequence,event_id,record_json FROM usage_event_records e WHERE sequence>?1 AND sequence<=?2 AND NOT EXISTS(SELECT 1 FROM usage_cloud_queue q WHERE q.binding=?3 AND q.source_home_id=?4 AND q.data_generation=?5 AND q.event_id=e.event_id) AND NOT EXISTS(SELECT 1 FROM usage_cloud_blocked q WHERE q.binding=?3 AND q.source_home_id=?4 AND q.data_generation=?5 AND q.event_id=e.event_id) AND NOT EXISTS(SELECT 1 FROM usage_cloud_time_excluded q WHERE q.binding=?3 AND q.source_home_id=?4 AND q.data_generation=?5 AND q.event_id=e.event_id) ORDER BY {order} LIMIT 100");
        let mut stmt = tx.prepare(&sql).map_err(failed)?;
        let records = stmt
            .query_map(params![after, through, b, h, g], |r| {
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
        let valid_time = raw
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(|stamp| {
                time::OffsetDateTime::parse(stamp, &time::format_description::well_known::Rfc3339)
                    .ok()
            })
            .is_some_and(|stamp| {
                stamp.unix_timestamp() >= 0
                    && stamp <= time::OffsetDateTime::now_utc() + time::Duration::minutes(5)
            });
        let invalid_anchor:bool=if tx.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='usage_retention_hours' AND type='table')",[],|r|r.get(0)).map_err(failed)? {
            tx.query_row("SELECT EXISTS(SELECT 1 FROM usage_retention_hours WHERE event_id=?1 AND cloud_eligible=0)",[event_id],|r|r.get(0)).map_err(failed)?
        } else {false};
        if !valid_time || invalid_anchor {
            tx.execute(
                "INSERT OR IGNORE INTO usage_cloud_time_excluded VALUES(?1,?2,?3,?4)",
                params![b, h, g, event_id],
            )
            .map_err(failed)?;
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
    // Per-event membership is authoritative. A high cursor only summarizes the
    // observed boundary; gaps and later additions remain independently eligible.
    tx.execute(
        "UPDATE usage_cloud_windows SET last_scan=MAX(last_scan,?2) WHERE window_id=?1",
        params![window, through],
    )
    .map_err(failed)?;
    tx.commit().map_err(failed)?;
    Ok(CaptureProgress {
        examined: rows.len(),
        added,
    })
}

/// Legacy custody inspection; explicit rolling retention does not use this gate.
/// Disable, failures and quarantine never imply acknowledgment.
#[cfg(test)]
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

/// Persist the exact confirmation before dispatch, close consent and fence any
/// active sender. Unknown outcomes preserve all selected local history. Later
/// explicit previews for the same generation preserve every earlier attempt.
#[cfg(any(cargo_ai_cli, test))]
#[cfg(test)]
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
    invalidate_payload_required(&tx, source)?;
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
    invalidate_payload_required(&tx, source)?;
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
    tx.execute("UPDATE usage_cloud_batches SET resend_hint_json=NULL WHERE binding=?1 AND data_generation<?2",
        params![binding.as_str(), generation.get() as i64]).map_err(failed)?;
    tx.execute(
        "UPDATE usage_cloud_state SET enabled=0,cleared=1 WHERE binding=?1 AND data_generation<?2",
        params![binding.as_str(), generation.get() as i64],
    )
    .map_err(failed)?;
    tx.execute("UPDATE usage_cloud_windows SET end_sequence=?3 WHERE binding=?1 AND data_generation<?2 AND end_sequence IS NULL",params![binding.as_str(),generation.get() as i64,boundary]).map_err(failed)?;
    tx.execute("UPDATE usage_cloud_schedule SET lease_token=NULL,lease_expires_ms=NULL WHERE binding=?1 AND data_generation<?2",params![binding.as_str(),generation.get() as i64]).map_err(failed)?;
    tx.commit().map_err(failed)
}

#[cfg(test)]
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
    pub time_ineligible: i64,
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
            time_ineligible: 0,
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
    let time_excluded_installed:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='usage_cloud_time_excluded' AND type='table')",[],|r|r.get(0)).map_err(failed)?;
    let time_ineligible = if time_excluded_installed {
        db.query_row("SELECT COUNT(*) FROM usage_cloud_time_excluded WHERE binding=?1 AND source_home_id=?2 AND data_generation=?3",params![b,h,g],|r|r.get(0)).map_err(failed)?
    } else {
        0
    };
    let unscanned_sql="SELECT COUNT(*) FROM usage_event_records e WHERE EXISTS(SELECT 1 FROM usage_cloud_windows w WHERE w.binding=?1 AND w.source_home_id=?2 AND w.data_generation=?3 AND e.sequence>w.start_sequence AND (w.end_sequence IS NULL OR e.sequence<=w.end_sequence)) AND NOT EXISTS(SELECT 1 FROM usage_cloud_queue q WHERE q.binding=?1 AND q.source_home_id=?2 AND q.data_generation=?3 AND q.event_id=e.event_id) AND NOT EXISTS(SELECT 1 FROM usage_cloud_blocked q WHERE q.binding=?1 AND q.source_home_id=?2 AND q.data_generation=?3 AND q.event_id=e.event_id) AND NOT EXISTS(SELECT 1 FROM usage_cloud_time_excluded q WHERE q.binding=?1 AND q.source_home_id=?2 AND q.data_generation=?3 AND q.event_id=e.event_id)";
    let unscanned_sql = if time_excluded_installed {
        unscanned_sql.to_owned()
    } else {
        unscanned_sql.replace(" AND NOT EXISTS(SELECT 1 FROM usage_cloud_time_excluded q WHERE q.binding=?1 AND q.source_home_id=?2 AND q.data_generation=?3 AND q.event_id=e.event_id)","")
    };
    let unscanned = db
        .query_row(&unscanned_sql, params![b, h, g], |r| r.get(0))
        .map_err(failed)?;
    let (mut unknown_batches,buffered_batches,terminal_unretained_batches):(i64,i64,i64)=db.query_row("SELECT COALESCE(SUM(outcome='unknown'),0),COALESCE(SUM(outcome='buffered'),0),COALESCE(SUM(outcome='terminal_unretained'),0) FROM usage_cloud_batches WHERE binding=?1 AND source_home_id=?2 AND data_generation=?3",params![b,h,g],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).map_err(failed)?;
    let upload_installed:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='usage_cloud_upload_batches' AND type='table')",[],|r|r.get(0)).map_err(failed)?;
    if upload_installed {
        unknown_batches+=db.query_row("SELECT COUNT(*) FROM usage_cloud_upload_batches WHERE binding=?1 AND source_home_id=?2 AND data_generation=?3 AND attempts>0 AND acknowledged=0",params![b,h,g],|r|r.get::<_,i64>(0)).map_err(failed)?;
    }
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
        time_ineligible,
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
    #[cfg(test)]
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
#[cfg(test)]
pub struct FrozenBatch {
    source: Source,
    provenance: Provenance,
    id: CanonicalUuid,
    request_json: String,
    members: Vec<ReceiptMember>,
    membership: Sha256Hash,
}
#[cfg(test)]
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

#[cfg(test)]
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

#[cfg(test)]
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
#[cfg(test)]
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
        let mut stmt=tx.prepare("SELECT original_text,timing_json FROM usage_cloud_queue WHERE binding=?1 AND source_home_id=?2 AND data_generation=?3 AND retained=0 AND quarantined=0 AND active_batch_id IS NULL ORDER BY sequence DESC LIMIT 100").map_err(failed)?;
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
#[cfg(test)]
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

// Active queue operations lazily upgrade metadata for already-enabled Homes.
fn ensure_resend_hint(db: &Connection) -> Result<(), String> {
    let present: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM pragma_table_info('usage_cloud_batches') WHERE name='resend_hint_json')", [], |row| row.get(0)).map_err(failed)?;
    if !present {
        db.execute_batch("ALTER TABLE usage_cloud_batches ADD COLUMN resend_hint_json TEXT;")
            .map_err(failed)?;
    }
    Ok(())
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg(test)]
struct ResendHint {
    source: Source,
    provenance: Provenance,
    batch_id: CanonicalUuid,
    request_sha256: Sha256Hash,
    attempts: i64,
}

/// This scheduling decision is armed only by a verified authenticated directive.
/// It carries no provider token or permission to bypass current server checks.
#[cfg(test)]
pub fn defer_payload_required(
    db: &mut Connection,
    lease: &Lease,
    batch_id: &CanonicalUuid,
    admission: &AdmissionCommitment,
    now: i64,
) -> Result<(), String> {
    ensure_resend_hint(db)?;
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(failed)?;
    admission_at(&tx, lease, batch_id, admission, now, false)?;
    let batch = load_batch(&tx, &lease.source, batch_id)?;
    authorized(&tx, &lease.source, &batch.provenance)?;
    let (b, h, g) = key(&lease.source);
    let attempts: i64 = tx.query_row("SELECT attempts FROM usage_cloud_batches WHERE binding=?1 AND source_home_id=?2 AND data_generation=?3 AND batch_id=?4 AND outcome!='retained'",
        params![b,h,g,batch_id.as_str()], |row| row.get(0)).map_err(failed)?;
    if attempts <= 0 {
        return Err(invalid());
    }
    let hint = ResendHint {
        source: batch.source.clone(),
        provenance: batch.provenance.clone(),
        batch_id: batch.id.clone(),
        request_sha256: Sha256Hash::of(batch.request_json.as_bytes()),
        attempts,
    };
    let text = serde_json::to_string(&hint).map_err(|_| invalid())?;
    let changed = tx.execute("UPDATE usage_cloud_batches SET resend_hint_json=?5 WHERE binding=?1 AND source_home_id=?2 AND data_generation=?3 AND batch_id=?4",
        params![b,h,g,batch_id.as_str(),text]).map_err(failed)?;
    if changed != 1 {
        return Err(invalid());
    }
    tx.commit().map_err(failed)
}

/// Atomically consumes a matching one-shot decision with the next unknown attempt.
/// A crash or lost response after consumption always returns to Receipt recovery.
#[cfg(test)]
pub fn consume_payload_required(
    db: &mut Connection,
    lease: &Lease,
    batch_id: &CanonicalUuid,
    now: i64,
) -> Result<bool, String> {
    ensure_resend_hint(db)?;
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(failed)?;
    check_lease(&tx, lease, now)?;
    let batch = load_batch(&tx, &lease.source, batch_id)?;
    authorized(&tx, &lease.source, &batch.provenance)?;
    let (b, h, g) = key(&lease.source);
    let (text,attempts,outcome): (Option<String>,i64,String) = tx.query_row(
        "SELECT resend_hint_json,attempts,outcome FROM usage_cloud_batches WHERE binding=?1 AND source_home_id=?2 AND data_generation=?3 AND batch_id=?4",
        params![b,h,g,batch_id.as_str()], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?))).map_err(failed)?;
    let hint = text
        .as_deref()
        .and_then(|value| serde_json::from_str::<ResendHint>(value).ok());
    let matches = hint.is_some_and(|hint| {
        hint.source == batch.source
            && hint.provenance == batch.provenance
            && hint.batch_id == batch.id
            && hint.request_sha256 == Sha256Hash::of(batch.request_json.as_bytes())
            && hint.attempts == attempts
            && attempts > 0
            && outcome != "retained"
    });
    let quarantined: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM usage_cloud_batch_members m JOIN usage_cloud_queue q USING(binding,source_home_id,data_generation,event_id) WHERE m.binding=?1 AND m.source_home_id=?2 AND m.data_generation=?3 AND m.batch_id=?4 AND q.quarantined=1)",
        params![b,h,g,batch_id.as_str()], |row| row.get(0)).map_err(failed)?;
    if !matches || quarantined {
        tx.execute("UPDATE usage_cloud_batches SET resend_hint_json=NULL WHERE binding=?1 AND source_home_id=?2 AND data_generation=?3 AND batch_id=?4",
            params![b,h,g,batch_id.as_str()]).map_err(failed)?;
        tx.commit().map_err(failed)?;
        return Ok(false);
    }
    let changed = tx.execute("UPDATE usage_cloud_batches SET resend_hint_json=NULL,attempts=attempts+1,outcome='unknown' WHERE binding=?1 AND source_home_id=?2 AND data_generation=?3 AND batch_id=?4 AND outcome!='retained'",
        params![b,h,g,batch_id.as_str()]).map_err(failed)?;
    if changed != 1 {
        return Err(invalid());
    }
    tx.commit().map_err(failed)?;
    Ok(true)
}

fn invalidate_payload_required(db: &Connection, source: &Source) -> Result<(), String> {
    ensure_resend_hint(db)?;
    let (b, h, g) = key(source);
    db.execute("UPDATE usage_cloud_batches SET resend_hint_json=NULL WHERE binding=?1 AND source_home_id=?2 AND data_generation=?3",
        params![b,h,g]).map_err(failed)?;
    Ok(())
}

/// Must commit before the first network dispatch. Unknown remains retryable and
/// is never converted to success by timeout, lease expiry or elapsed time.
#[cfg(test)]
pub fn begin_attempt(
    db: &mut Connection,
    lease: &Lease,
    batch_id: &CanonicalUuid,
    now: i64,
) -> Result<(), String> {
    ensure_resend_hint(db)?;
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
    let changed=tx.execute("UPDATE usage_cloud_batches SET resend_hint_json=NULL,attempts=attempts+1,outcome=CASE WHEN outcome='retained' THEN 'retained' ELSE 'unknown' END WHERE binding=?1 AND source_home_id=?2 AND data_generation=?3 AND batch_id=?4 AND outcome!='retained'",params![b,h,g,batch_id.as_str()]).map_err(failed)?;
    if changed != 1 {
        return Err(invalid());
    }
    tx.commit().map_err(failed)
}

#[cfg(test)]
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
#[cfg(test)]
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
#[cfg(test)]
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
#[cfg(test)]
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

#[cfg(test)]
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
#[cfg(test)]
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
#[cfg(test)]
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

/// The caller verifies the authenticated Elastic acknowledgment for the exact
/// source, batch and membership before this local progress update. Analytical
/// visibility is separate. Raw in-window history is never deleted by an ACK.
pub fn acknowledge_uploaded(
    db: &mut Connection,
    lease: &Lease,
    batch_id: &CanonicalUuid,
    now: i64,
) -> Result<(), String> {
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(failed)?;
    let (b, h, g) = key(&lease.source);
    let v2_table:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='usage_cloud_upload_batches' AND type='table')",[],|r|r.get(0)).map_err(failed)?;
    if v2_table && tx.query_row("SELECT EXISTS(SELECT 1 FROM usage_cloud_upload_batches WHERE binding=?1 AND source_home_id=?2 AND data_generation=?3 AND batch_id=?4)",params![b,h,g,batch_id.as_str()],|r|r.get::<_,bool>(0)).map_err(failed)? {
        check_lease(&tx,lease,now)?;
        let batch=load_upload_batch(&tx,&lease.source,batch_id)?;
        authorized(&tx,&lease.source,&batch.provenance)?;
        for member in &batch.members {
            let changed=tx.execute("UPDATE usage_cloud_queue SET retained=1,active_batch_id=NULL WHERE binding=?1 AND source_home_id=?2 AND data_generation=?3 AND event_id=?4 AND original_sha256=?5 AND semantic_sha256=?6 AND active_batch_id=?7",params![b,h,g,member.event_id,member.original_sha256.as_str(),member.semantic_sha256.as_str(),batch_id.as_str()]).map_err(failed)?;
            if changed!=1 {return Err(invalid());}
        }
        // Per-event progress survives, while acknowledged compressed retry copies
        // no longer consume the local pending budget. Original facts stay intact.
        tx.execute("UPDATE usage_cloud_expiry_guard SET active=1",[]).map_err(failed)?;
        tx.execute("DELETE FROM usage_cloud_upload_members WHERE binding=?1 AND source_home_id=?2 AND data_generation=?3 AND batch_id=?4",params![b,h,g,batch_id.as_str()]).map_err(failed)?;
        tx.execute("DELETE FROM usage_cloud_upload_batches WHERE binding=?1 AND source_home_id=?2 AND data_generation=?3 AND batch_id=?4",params![b,h,g,batch_id.as_str()]).map_err(failed)?;
        tx.execute("UPDATE usage_cloud_expiry_guard SET active=0",[]).map_err(failed)?;
        return tx.commit().map_err(failed);
    }
    #[cfg(test)]
    {
        let present:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM usage_cloud_batches WHERE binding=?1 AND source_home_id=?2 AND data_generation=?3 AND batch_id=?4)",params![b,h,g,batch_id.as_str()],|r|r.get(0)).map_err(failed)?;
        if !present {
            return Err("Usage upload was acknowledged upstream after its local batch expired; no local history was resurrected".into());
        }
        check_lease(&tx, lease, now)?;
        let batch = load_batch(&tx, &lease.source, batch_id)?;
        authorized(&tx, &lease.source, &batch.provenance)?;
        for member in &batch.members {
            let changed=tx.execute("UPDATE usage_cloud_queue SET retained=1 WHERE binding=?1 AND source_home_id=?2 AND data_generation=?3 AND event_id=?4 AND original_sha256=?5 AND semantic_sha256=?6 AND active_batch_id=?7",params![b,h,g,member.event_id,member.original_sha256.as_str(),member.semantic_sha256.as_str(),batch_id.as_str()]).map_err(failed)?;
            if changed != 1 {
                return Err(invalid());
            }
        }
        tx.execute("UPDATE usage_cloud_batches SET outcome='retained' WHERE binding=?1 AND source_home_id=?2 AND data_generation=?3 AND batch_id=?4",params![b,h,g,batch_id.as_str()]).map_err(failed)?;
        return tx.commit().map_err(failed);
    }
    #[cfg(not(test))]
    Err("Usage upload was acknowledged upstream after its local batch expired; no local history was resurrected".into())
}

/// Frozen explicit-upload bytes and exact per-event progress survive process exit.
pub struct UploadBatch {
    source: Source,
    provenance: Provenance,
    id: CanonicalUuid,
    request_json: String,
    members: Vec<ReceiptMember>,
    membership: Sha256Hash,
}
impl UploadBatch {
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
fn install_upload_batches(db: &Connection) -> Result<(), String> {
    db.execute_batch("CREATE TABLE IF NOT EXISTS usage_cloud_upload_batches(binding TEXT NOT NULL,source_home_id TEXT NOT NULL,data_generation INTEGER NOT NULL,batch_id TEXT NOT NULL,lifecycle_epoch INTEGER NOT NULL,consent_interval INTEGER NOT NULL,request_json TEXT NOT NULL CHECK(length(CAST(request_json AS BLOB))<=12582912),membership_sha256 TEXT NOT NULL,members_json TEXT NOT NULL,sealed INTEGER NOT NULL DEFAULT 0 CHECK(sealed IN(0,1)),acknowledged INTEGER NOT NULL DEFAULT 0 CHECK(acknowledged IN(0,1)),attempts INTEGER NOT NULL DEFAULT 0,PRIMARY KEY(binding,source_home_id,data_generation,batch_id));
        CREATE TABLE IF NOT EXISTS usage_cloud_upload_members(binding TEXT NOT NULL,source_home_id TEXT NOT NULL,data_generation INTEGER NOT NULL,batch_id TEXT NOT NULL,event_id TEXT NOT NULL,ordinal INTEGER NOT NULL CHECK(ordinal>=0 AND ordinal<4096),PRIMARY KEY(binding,source_home_id,data_generation,batch_id,event_id),UNIQUE(binding,source_home_id,data_generation,batch_id,ordinal),FOREIGN KEY(binding,source_home_id,data_generation,batch_id) REFERENCES usage_cloud_upload_batches(binding,source_home_id,data_generation,batch_id),FOREIGN KEY(binding,source_home_id,data_generation,event_id) REFERENCES usage_cloud_queue(binding,source_home_id,data_generation,event_id));
        CREATE INDEX IF NOT EXISTS usage_cloud_upload_event ON usage_cloud_upload_members(event_id);
        CREATE TRIGGER IF NOT EXISTS usage_cloud_upload_batch_immutable BEFORE UPDATE ON usage_cloud_upload_batches WHEN OLD.binding IS NOT NEW.binding OR OLD.source_home_id IS NOT NEW.source_home_id OR OLD.data_generation IS NOT NEW.data_generation OR OLD.batch_id IS NOT NEW.batch_id OR OLD.lifecycle_epoch IS NOT NEW.lifecycle_epoch OR OLD.consent_interval IS NOT NEW.consent_interval OR OLD.request_json IS NOT NEW.request_json OR OLD.membership_sha256 IS NOT NEW.membership_sha256 OR OLD.members_json IS NOT NEW.members_json OR (OLD.sealed=1 AND NEW.sealed!=1) OR OLD.acknowledged>NEW.acknowledged BEGIN SELECT RAISE(ABORT,'immutable usage upload batch'); END;
        CREATE TRIGGER IF NOT EXISTS usage_cloud_upload_member_sealed BEFORE INSERT ON usage_cloud_upload_members WHEN EXISTS(SELECT 1 FROM usage_cloud_upload_batches b WHERE b.binding=NEW.binding AND b.source_home_id=NEW.source_home_id AND b.data_generation=NEW.data_generation AND b.batch_id=NEW.batch_id AND b.sealed=1) BEGIN SELECT RAISE(ABORT,'sealed usage upload membership'); END;
        CREATE TRIGGER IF NOT EXISTS usage_cloud_upload_member_immutable BEFORE UPDATE ON usage_cloud_upload_members BEGIN SELECT RAISE(ABORT,'immutable usage upload membership'); END;
        CREATE TRIGGER IF NOT EXISTS usage_cloud_upload_batch_no_delete BEFORE DELETE ON usage_cloud_upload_batches WHEN (SELECT active FROM usage_cloud_expiry_guard)=0 BEGIN SELECT RAISE(ABORT,'immutable usage upload batch'); END;
        CREATE TRIGGER IF NOT EXISTS usage_cloud_upload_member_no_delete BEFORE DELETE ON usage_cloud_upload_members WHEN (SELECT active FROM usage_cloud_expiry_guard)=0 BEGIN SELECT RAISE(ABORT,'immutable usage upload membership'); END;").map_err(failed)
}
/// Retire a bounded set of now-ineligible immutable requests, never their raw
/// records or per-event acknowledgments. Surviving members get a fresh batch.
pub fn retire_uploads_before(
    db: &mut Connection,
    source: &Source,
    cutoff_ms: i64,
    _now: i64,
) -> Result<usize, String> {
    if !installed(db)? {
        return Ok(0);
    }
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(failed)?;
    let (_, provenance) = current(&tx, source)?;
    authorized(&tx, source, &provenance)?;
    install_upload_batches(&tx)?;
    let (b, h, g) = key(source);
    let batches = {
        let mut statement = tx.prepare("SELECT DISTINCT m.batch_id FROM usage_cloud_upload_members m JOIN usage_cloud_queue q USING(binding,source_home_id,data_generation,event_id) WHERE m.binding=?1 AND m.source_home_id=?2 AND m.data_generation=?3 AND CAST(json_extract(q.original_text,'$.occurred_at_ms') AS INTEGER)<?4 LIMIT 100").map_err(failed)?;
        let rows = statement
            .query_map(params![b, h, g, cutoff_ms], |row| row.get::<_, String>(0))
            .map_err(failed)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(failed)?;
        rows
    };
    if !batches.is_empty() {
        tx.execute("UPDATE usage_cloud_schedule SET lease_token=NULL,lease_expires_ms=NULL WHERE binding=?1 AND source_home_id=?2 AND data_generation=?3",params![b,h,g]).map_err(failed)?;
        tx.execute("UPDATE usage_cloud_expiry_guard SET active=1", [])
            .map_err(failed)?;
        for id in &batches {
            tx.execute("UPDATE usage_cloud_queue SET active_batch_id=NULL WHERE binding=?1 AND source_home_id=?2 AND data_generation=?3 AND active_batch_id=?4",params![b,h,g,id]).map_err(failed)?;
            tx.execute("DELETE FROM usage_cloud_upload_members WHERE binding=?1 AND source_home_id=?2 AND data_generation=?3 AND batch_id=?4",params![b,h,g,id]).map_err(failed)?;
            tx.execute("DELETE FROM usage_cloud_upload_batches WHERE binding=?1 AND source_home_id=?2 AND data_generation=?3 AND batch_id=?4",params![b,h,g,id]).map_err(failed)?;
        }
        tx.execute("UPDATE usage_cloud_expiry_guard SET active=0", [])
            .map_err(failed)?;
    }
    tx.commit().map_err(failed)?;
    Ok(batches.len())
}

/// Passive coverage, computed from durable per-event state and current policy.
pub fn cloud_window_ineligible(
    db: &Connection,
    source: &Source,
    cutoff_ms: i64,
) -> Result<i64, String> {
    if !installed(db)? {
        return Ok(0);
    }
    let (b, h, g) = key(source);
    db.query_row("SELECT COUNT(*) FROM usage_cloud_queue WHERE binding=?1 AND source_home_id=?2 AND data_generation=?3 AND retained=0 AND CAST(json_extract(original_text,'$.occurred_at_ms') AS INTEGER)<?4",params![b,h,g,cutoff_ms],|row|row.get(0)).map_err(failed)
}

fn load_upload_batch(
    db: &Connection,
    source: &Source,
    id: &CanonicalUuid,
) -> Result<UploadBatch, String> {
    let (b, h, g) = key(source);
    let (epoch,interval,text,hash,members):(i64,i64,String,String,String)=db.query_row("SELECT lifecycle_epoch,consent_interval,request_json,membership_sha256,members_json FROM usage_cloud_upload_batches WHERE binding=?1 AND source_home_id=?2 AND data_generation=?3 AND batch_id=?4 AND sealed=1",params![b,h,g,id.as_str()],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).map_err(failed)?;
    Ok(UploadBatch {
        source: source.clone(),
        provenance: Provenance {
            lifecycle_epoch: LifecycleEpoch::try_from(epoch as u64).map_err(contract)?,
            consent_interval: Counter::try_from(interval as u64).map_err(contract)?,
        },
        id: id.clone(),
        request_json: text,
        membership: Sha256Hash::try_from(hash).map_err(contract)?,
        members: serde_json::from_str(&members).map_err(|_| invalid())?,
    })
}
/// The trusted codec builder chooses an accepted prefix after compression and
/// final envelope-size checks. Stored bytes are replayed verbatim after an
/// unknown response. No completed-hour flag can hide a late event or a gap.
#[cfg(test)]
pub fn prepare_upload_batch<F>(
    db: &mut Connection,
    lease: &Lease,
    now: i64,
    build: F,
) -> Result<Option<UploadBatch>, String>
where
    F: FnOnce(
        &Source,
        &Provenance,
        &CanonicalUuid,
        &[EventSubmission],
    ) -> Result<(String, usize, Sha256Hash), String>,
{
    prepare_upload_batch_since(db, lease, now, 0, build)
}

/// Cloud eligibility is independent of the local rolling window. Old pending
/// identities remain local and visible; they cannot hold newer uploads hostage.
pub fn prepare_upload_batch_since<F>(
    db: &mut Connection,
    lease: &Lease,
    now: i64,
    cutoff_ms: i64,
    build: F,
) -> Result<Option<UploadBatch>, String>
where
    F: FnOnce(
        &Source,
        &Provenance,
        &CanonicalUuid,
        &[EventSubmission],
    ) -> Result<(String, usize, Sha256Hash), String>,
{
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(failed)?;
    check_lease(&tx, lease, now)?;
    let (_, provenance) = current(&tx, &lease.source)?;
    authorized(&tx, &lease.source, &provenance)?;
    install_upload_batches(&tx)?;
    let (b, h, g) = key(&lease.source);
    tx.execute("UPDATE usage_cloud_queue SET active_batch_id=NULL WHERE binding=?1 AND source_home_id=?2 AND data_generation=?3 AND active_batch_id IN(SELECT batch_id FROM usage_cloud_upload_batches WHERE binding=?1 AND source_home_id=?2 AND data_generation=?3 AND (lifecycle_epoch!=?4 OR consent_interval!=?5))",params![b,h,g,provenance.lifecycle_epoch.get() as i64,provenance.consent_interval.get() as i64]).map_err(failed)?;
    let prior:Option<String>=tx.query_row("SELECT b.batch_id FROM usage_cloud_upload_batches b WHERE b.binding=?1 AND b.source_home_id=?2 AND b.data_generation=?3 AND b.lifecycle_epoch=?4 AND b.consent_interval=?5 AND b.acknowledged=0 AND b.sealed=1 AND NOT EXISTS(SELECT 1 FROM usage_cloud_upload_members m JOIN usage_cloud_queue q USING(binding,source_home_id,data_generation,event_id) WHERE m.binding=b.binding AND m.source_home_id=b.source_home_id AND m.data_generation=b.data_generation AND m.batch_id=b.batch_id AND CAST(json_extract(q.original_text,'$.occurred_at_ms') AS INTEGER)<?6) ORDER BY b.rowid LIMIT 1",params![b,h,g,provenance.lifecycle_epoch.get() as i64,provenance.consent_interval.get() as i64,cutoff_ms],|r|r.get(0)).optional().map_err(failed)?;
    if let Some(id) = prior {
        let batch = load_upload_batch(
            &tx,
            &lease.source,
            &CanonicalUuid::try_from(id).map_err(contract)?,
        )?;
        tx.commit().map_err(failed)?;
        return Ok(Some(batch));
    }
    let rows = {
        let mut stmt=tx.prepare("SELECT original_text,timing_json FROM (SELECT original_text,timing_json,sequence,CAST(json_extract(original_text,'$.occurred_at_ms') AS INTEGER)/3600000 AS event_hour,SUM(length(CAST(original_text AS BLOB))+2048+length(COALESCE(timing_json,''))) OVER (ORDER BY CAST(json_extract(original_text,'$.occurred_at_ms') AS INTEGER)/3600000 DESC,sequence DESC) AS candidate_bytes FROM usage_cloud_queue WHERE binding=?1 AND source_home_id=?2 AND data_generation=?3 AND retained=0 AND quarantined=0 AND active_batch_id IS NULL AND CAST(json_extract(original_text,'$.occurred_at_ms') AS INTEGER)>=?4) WHERE candidate_bytes<=8388608 ORDER BY event_hour DESC,sequence DESC LIMIT 4096").map_err(failed)?;
        let rows = stmt
            .query_map(params![b, h, g, cutoff_ms], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
            })
            .map_err(failed)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(failed)?;
        rows
    };
    let mut events = Vec::new();
    let mut bytes = 0usize;
    for (text, timing) in rows {
        let event = EventSubmission::from_allowed_text_with_timing(
            text,
            timing
                .map(|text| serde_json::from_str(&text).map_err(|_| invalid()))
                .transpose()?,
        )
        .map_err(contract)?;
        let encoded = serde_json::to_vec(&event).map_err(|_| invalid())?.len();
        if bytes + encoded + events.len() + 2 > 8 * 1024 * 1024 {
            break;
        }
        bytes += encoded;
        events.push(event);
    }
    if events.is_empty() {
        return Ok(None);
    }
    let id = CanonicalUuid::try_from(uuid::Uuid::new_v4().to_string()).map_err(contract)?;
    let (text, count, membership) = build(&lease.source, &provenance, &id, &events)?;
    if count == 0 || count > events.len() || text.len() > 12 * 1024 * 1024 {
        return Err("Usage upload codec returned invalid bounded membership".into());
    }
    let members = events[..count]
        .iter()
        .map(EventSubmission::member)
        .collect::<Result<Vec<_>, _>>()
        .map_err(contract)?;
    tx.execute("INSERT INTO usage_cloud_upload_batches(binding,source_home_id,data_generation,batch_id,lifecycle_epoch,consent_interval,request_json,membership_sha256,members_json) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",params![b,h,g,id.as_str(),provenance.lifecycle_epoch.get() as i64,provenance.consent_interval.get() as i64,text,membership.as_str(),serde_json::to_string(&members).map_err(|_|invalid())?]).map_err(failed)?;
    for (ordinal, member) in members.iter().enumerate() {
        let changed=tx.execute("UPDATE usage_cloud_queue SET active_batch_id=?5 WHERE binding=?1 AND source_home_id=?2 AND data_generation=?3 AND event_id=?4 AND active_batch_id IS NULL AND retained=0 AND quarantined=0 AND original_sha256=?6 AND semantic_sha256=?7",params![b,h,g,member.event_id,id.as_str(),member.original_sha256.as_str(),member.semantic_sha256.as_str()]).map_err(failed)?;
        if changed != 1 {
            return Err(invalid());
        }
        tx.execute(
            "INSERT INTO usage_cloud_upload_members VALUES(?1,?2,?3,?4,?5,?6)",
            params![b, h, g, id.as_str(), member.event_id, ordinal as i64],
        )
        .map_err(failed)?;
    }
    tx.execute("UPDATE usage_cloud_upload_batches SET sealed=1 WHERE binding=?1 AND source_home_id=?2 AND data_generation=?3 AND batch_id=?4",params![b,h,g,id.as_str()]).map_err(failed)?;
    let batch = load_upload_batch(&tx, &lease.source, &id)?;
    tx.commit().map_err(failed)?;
    Ok(Some(batch))
}
pub fn begin_upload_attempt(
    db: &mut Connection,
    lease: &Lease,
    batch_id: &CanonicalUuid,
    now: i64,
) -> Result<(), String> {
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(failed)?;
    check_lease(&tx, lease, now)?;
    let batch = load_upload_batch(&tx, &lease.source, batch_id)?;
    authorized(&tx, &lease.source, batch.provenance())?;
    let (b, h, g) = key(&lease.source);
    tx.execute("UPDATE usage_cloud_upload_batches SET attempts=attempts+1 WHERE binding=?1 AND source_home_id=?2 AND data_generation=?3 AND batch_id=?4 AND acknowledged=0",params![b,h,g,batch_id.as_str()]).map_err(failed)?;
    tx.commit().map_err(failed)
}

const MAX_UPLOAD_CLEAR_ATTEMPTS: usize = 100;
const MAX_UPLOAD_CLEAR_INTENT_BYTES: usize = 128 * 1024;
fn upload_clear_attempts(
    text: &str,
    source: &Source,
) -> Result<Vec<super::elastic::Request>, String> {
    if text.len() > MAX_UPLOAD_CLEAR_INTENT_BYTES {
        return Err("Usage upload clear confirmation history exceeds its bound; preserved for reconciliation".into());
    }
    let attempts:Vec<super::elastic::Request>=serde_json::from_str(text).map_err(|_|"Saved usage upload clear confirmations are invalid or belong to another protocol; preserved for reconciliation")?;
    if attempts.is_empty() || attempts.len()>MAX_UPLOAD_CLEAR_ATTEMPTS || attempts.iter().any(|attempt|!matches!(attempt,super::elastic::Request::ClearConfirm{source:saved,..} if saved==source)) {
        return Err("Saved usage upload clear confirmations do not match this source; preserved for reconciliation".into());
    }
    Ok(attempts)
}

/// Freeze the exact protocol-2 confirmation before dispatch. Closing consent and
/// invalidating the source lease prevents uploads while a clear result is unknown.
/// Later previews append bounded attempts without losing earlier confirmations.
pub fn begin_upload_clear(
    db: &mut Connection,
    request: &super::elastic::Request,
) -> Result<(), String> {
    let super::elastic::Request::ClearConfirm {
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
    let (cleared,prior):(bool,Option<String>)=tx.query_row("SELECT cleared,clear_intent_json FROM usage_cloud_state WHERE binding=?1 AND source_home_id=?2 AND data_generation=?3",params![b,h,g],|r|Ok((r.get(0)?,r.get(1)?))).map_err(failed)?;
    if cleared {
        return Err("Usage cloud generation was already cleared".into());
    }
    let mut attempts = prior
        .map(|text| upload_clear_attempts(&text, source))
        .transpose()?
        .unwrap_or_default();
    if !attempts.contains(request) {
        if attempts.len() == MAX_UPLOAD_CLEAR_ATTEMPTS {
            return Err("Usage upload clear attempt history is full; reconcile a preserved exact confirmation".into());
        }
        attempts.push(request.clone());
    }
    let encoded = serde_json::to_string(&attempts).map_err(failed)?;
    if encoded.len() > MAX_UPLOAD_CLEAR_INTENT_BYTES {
        return Err("Usage upload clear confirmation history exceeds its bound; preserved for reconciliation".into());
    }
    let boundary = high(&tx)?;
    invalidate_payload_required(&tx, source)?;
    tx.execute("UPDATE usage_cloud_state SET enabled=0,clear_intent_json=?4 WHERE binding=?1 AND source_home_id=?2 AND data_generation=?3",params![b,h,g,encoded]).map_err(failed)?;
    tx.execute("UPDATE usage_cloud_windows SET end_sequence=?4 WHERE binding=?1 AND source_home_id=?2 AND data_generation=?3 AND end_sequence IS NULL",params![b,h,g,boundary]).map_err(failed)?;
    tx.execute("UPDATE usage_cloud_schedule SET lease_token=NULL,lease_expires_ms=NULL WHERE binding=?1 AND source_home_id=?2 AND data_generation=?3",params![b,h,g]).map_err(failed)?;
    tx.commit().map_err(failed)
}

/// Passive restart recovery returns the latest frozen protocol-2 confirmation.
/// The caller must retry these exact fields or reconcile authenticated generation
/// advancement; a missing response never authorizes resuming the old upload source.
pub fn pending_upload_clear(
    db: &Connection,
    source: &Source,
) -> Result<Option<super::elastic::Request>, String> {
    if !installed(db)? || !has_clear_intent_column(db)? {
        return Ok(None);
    }
    let (b, h, g) = key(source);
    let saved:Option<(bool,Option<String>)>=db.query_row("SELECT cleared,clear_intent_json FROM usage_cloud_state WHERE binding=?1 AND source_home_id=?2 AND data_generation=?3",params![b,h,g],|r|Ok((r.get(0)?,r.get(1)?))).optional().map_err(failed)?;
    let Some((false, Some(text))) = saved else {
        return Ok(None);
    };
    Ok(upload_clear_attempts(&text, source)?.pop())
}

/// Use only after verifying the authenticated exact-source clear result or a
/// same-binding generation advance. Old raw and per-event progress are retained;
/// consent and all old-source upload leases remain fenced. Enabling a negotiated
/// new generation starts after the current high-water and does not select history.
#[cfg(any(cargo_ai_cli, test))]
pub fn finish_upload_clear(db: &mut Connection, source: &Source) -> Result<(), String> {
    mark_cleared(db, source)
}
