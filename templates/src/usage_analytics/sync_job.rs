//! Durable explicit sync targets. Raw history and upload acknowledgments have independent lifetimes.
use super::*;
use serde_json::json;

#[derive(Clone, Debug)]
pub struct Job {
    pub id: String,
    pub started_ms: i64,
    pub through: i64,
    pub history_days: Option<u32>,
    pub retry_at_ms: Option<i64>,
}
pub fn install(db: &Connection) -> Result<(), String> {
    db.execute_batch("CREATE TABLE IF NOT EXISTS usage_cloud_jobs(
        job_id TEXT PRIMARY KEY,binding TEXT NOT NULL,source_home_id TEXT NOT NULL,data_generation INTEGER NOT NULL,
        lifecycle_epoch INTEGER NOT NULL,consent_interval INTEGER NOT NULL,started_ms INTEGER NOT NULL,
        through_sequence INTEGER NOT NULL,history_days INTEGER,retry_at_ms INTEGER,finished INTEGER NOT NULL DEFAULT 0,outcome TEXT NOT NULL DEFAULT 'partial',sealed INTEGER NOT NULL DEFAULT 0 CHECK(sealed IN(0,1)));
        CREATE UNIQUE INDEX IF NOT EXISTS usage_cloud_active_job ON usage_cloud_jobs(binding,source_home_id,data_generation) WHERE finished=0;
        CREATE TABLE IF NOT EXISTS usage_cloud_job_members(job_id TEXT NOT NULL,event_id TEXT NOT NULL,sequence INTEGER NOT NULL,occurred_ms INTEGER NOT NULL,
        captured INTEGER NOT NULL DEFAULT 0 CHECK(captured IN(0,1)),state TEXT NOT NULL DEFAULT 'pending' CHECK(state IN('pending','uploaded','expired','excluded')),PRIMARY KEY(job_id,event_id));
        CREATE TABLE IF NOT EXISTS usage_cloud_terminal_events(binding TEXT NOT NULL,source_home_id TEXT NOT NULL,data_generation INTEGER NOT NULL,event_id TEXT NOT NULL,PRIMARY KEY(binding,event_id));
        CREATE INDEX IF NOT EXISTS usage_cloud_job_pending ON usage_cloud_job_members(job_id,state,occurred_ms,sequence);
        CREATE TRIGGER IF NOT EXISTS usage_cloud_job_target_immutable BEFORE UPDATE ON usage_cloud_jobs
        WHEN OLD.job_id IS NOT NEW.job_id OR OLD.binding IS NOT NEW.binding OR OLD.source_home_id IS NOT NEW.source_home_id OR OLD.data_generation IS NOT NEW.data_generation
        OR OLD.started_ms IS NOT NEW.started_ms
        OR OLD.through_sequence IS NOT NEW.through_sequence OR OLD.history_days IS NOT NEW.history_days OR (OLD.sealed=1 AND NEW.sealed!=1)
        BEGIN SELECT RAISE(ABORT,'immutable sync target'); END;
        CREATE TRIGGER IF NOT EXISTS usage_cloud_job_member_sealed BEFORE INSERT ON usage_cloud_job_members
        WHEN NOT EXISTS(SELECT 1 FROM usage_cloud_jobs j WHERE j.job_id=NEW.job_id AND j.sealed=0)
        BEGIN SELECT RAISE(ABORT,'sealed sync target'); END;
        CREATE TRIGGER IF NOT EXISTS usage_cloud_job_member_immutable BEFORE UPDATE ON usage_cloud_job_members
        WHEN OLD.job_id IS NOT NEW.job_id OR OLD.event_id IS NOT NEW.event_id OR OLD.sequence IS NOT NEW.sequence OR OLD.occurred_ms IS NOT NEW.occurred_ms
        OR (OLD.state!='pending' AND OLD.state IS NOT NEW.state)
        BEGIN SELECT RAISE(ABORT,'immutable sync member'); END;").map_err(failed)
}
pub fn active(db: &Connection, source: &Source) -> Result<Option<Job>, String> {
    let exists: bool = db
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='usage_cloud_jobs')",
            [],
            |r| r.get(0),
        )
        .map_err(failed)?;
    if !exists {
        return Ok(None);
    }
    let (b, h, g) = key(source);
    db.query_row("SELECT job_id,started_ms,through_sequence,history_days,retry_at_ms FROM usage_cloud_jobs WHERE binding=?1 AND source_home_id=?2 AND data_generation=?3 AND finished=0",params![b,h,g],|r|Ok(Job{id:r.get(0)?,started_ms:r.get(1)?,through:r.get(2)?,history_days:r.get(3)?,retry_at_ms:r.get(4)?})).optional().map_err(failed)
}
pub fn latest(db: &Connection, source: &Source) -> Result<Option<Job>, String> {
    let exists: bool = db
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='usage_cloud_jobs')",
            [],
            |r| r.get(0),
        )
        .map_err(failed)?;
    if !exists {
        return Ok(None);
    }
    let (b, h, g) = key(source);
    db.query_row("SELECT job_id,started_ms,through_sequence,history_days,retry_at_ms FROM usage_cloud_jobs WHERE binding=?1 AND source_home_id=?2 AND data_generation=?3 ORDER BY started_ms DESC,rowid DESC LIMIT 1",params![b,h,g],|r|Ok(Job{id:r.get(0)?,started_ms:r.get(1)?,through:r.get(2)?,history_days:r.get(3)?,retry_at_ms:r.get(4)?})).optional().map_err(failed)
}
pub fn terminal_count(db: &Connection, source: &Source) -> Result<i64, String> {
    let exists: bool = db
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='usage_cloud_terminal_events')",
            [],
            |r| r.get(0),
        )
        .map_err(failed)?;
    if !exists {
        return Ok(0);
    }
    let (b, h, _) = key(source);
    db.query_row(
        "SELECT COUNT(*) FROM usage_cloud_terminal_events WHERE binding=?1 AND source_home_id=?2",
        params![b, h],
        |r| r.get(0),
    )
    .map_err(failed)
}
pub fn start(
    db: &mut Connection,
    lease: &Lease,
    now: i64,
    history_days: Option<u32>,
) -> Result<Job, String> {
    start_inner(db, lease, now, history_days, true)
}
pub fn resume(db: &mut Connection, lease: &Lease, now: i64) -> Result<Job, String> {
    start_inner(db, lease, now, None, false)
}
fn start_inner(
    db: &mut Connection,
    lease: &Lease,
    now: i64,
    history_days: Option<u32>,
    explicit: bool,
) -> Result<Job, String> {
    if history_days.is_some_and(|days| days == 0 || days > 30) {
        return Err("History exceeds the 30-day plan limit".into());
    }
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(failed)?;
    check_lease(&tx, lease, now)?;
    install(&tx)?;
    let (_, provenance) = current(&tx, lease.source())?;
    authorized(&tx, lease.source(), &provenance)?;
    if let Some(job) = active(&tx, lease.source())? {
        let valid:bool=tx.query_row("SELECT lifecycle_epoch=?2 AND consent_interval=?3 FROM usage_cloud_jobs WHERE job_id=?1",params![job.id,provenance.lifecycle_epoch.get() as i64,provenance.consent_interval.get() as i64],|r|r.get(0)).map_err(failed)?;
        if !valid {
            if !explicit {
                return Err("Pending sync job requires explicit renewal of its consent".into());
            }
            // Only a new explicit sync reauthorizes transmission. The original
            // target and old ambiguous requests remain immutable and inspectable.
            tx.execute("UPDATE usage_cloud_jobs SET lifecycle_epoch=?2,consent_interval=?3 WHERE job_id=?1",params![job.id,provenance.lifecycle_epoch.get() as i64,provenance.consent_interval.get() as i64]).map_err(failed)?;
        }
        if history_days.is_some() && history_days != job.history_days {
            return Err("An active sync target already exists; resume it before selecting another history window".into());
        }
        tx.commit().map_err(failed)?;
        return Ok(job);
    }
    if !explicit {
        return Err("No explicit sync job is available to resume".into());
    }
    let job = Job {
        id: uuid::Uuid::new_v4().to_string(),
        started_ms: now,
        through: high(&tx)?,
        history_days,
        retry_at_ms: None,
    };
    let (b, h, g) = key(lease.source());
    tx.execute("INSERT INTO usage_cloud_jobs(job_id,binding,source_home_id,data_generation,lifecycle_epoch,consent_interval,started_ms,through_sequence,history_days) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",params![job.id,b,h,g,provenance.lifecycle_epoch.get() as i64,provenance.consent_interval.get() as i64,now,job.through,history_days]).map_err(failed)?;
    // The write transaction freezes exact committed identities, not just a cursor.
    // Occurrence time is anchored once; resuming never introduces a moving cutoff.
    let cutoff = history_days
        .map(|days| now.saturating_sub(i64::from(days) * 86_400_000))
        .unwrap_or(i64::MAX);
    tx.execute("INSERT INTO usage_cloud_job_members(job_id,event_id,sequence,occurred_ms)
        SELECT ?1,e.event_id,e.sequence,COALESCE(CAST((julianday(json_extract(e.record_json,'$.timestamp'))-2440587.5)*86400000 AS INTEGER),0)
        FROM usage_event_records e WHERE e.sequence<=?2 AND ?3>=0
        AND (EXISTS(SELECT 1 FROM usage_cloud_windows w WHERE w.binding=?4 AND w.source_home_id=?5 AND w.data_generation=?6 AND e.sequence>w.start_sequence AND (w.end_sequence IS NULL OR e.sequence<=w.end_sequence))
             OR (?7 IS NOT NULL AND CAST((julianday(json_extract(e.record_json,'$.timestamp'))-2440587.5)*86400000 AS INTEGER)>=?8))
        AND NOT EXISTS(SELECT 1 FROM usage_cloud_queue q WHERE q.binding=?4 AND q.source_home_id=?5 AND q.data_generation=?6 AND q.event_id=e.event_id AND q.retained=1)
        AND NOT EXISTS(SELECT 1 FROM usage_cloud_terminal_events q WHERE q.binding=?4 AND q.event_id=e.event_id)",params![job.id,job.through,now,b,h,g,history_days,cutoff]).map_err(failed)?;
    tx.execute(
        "UPDATE usage_cloud_jobs SET sealed=1 WHERE job_id=?1",
        [&job.id],
    )
    .map_err(failed)?;
    tx.commit().map_err(failed)?;
    Ok(job)
}
pub fn capture(db: &mut Connection, lease: &Lease, job: &Job, now: i64) -> Result<usize, String> {
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(failed)?;
    check_lease(&tx, lease, now)?;
    let (b, h, g) = key(lease.source());
    tx.execute("UPDATE usage_cloud_job_members SET state='expired' WHERE job_id=?1 AND state='pending' AND NOT EXISTS(SELECT 1 FROM usage_event_records e WHERE e.event_id=usage_cloud_job_members.event_id)",[&job.id]).map_err(failed)?;
    let rows = {
        let mut stmt=tx.prepare("SELECT e.sequence,e.event_id,e.record_json FROM usage_cloud_job_members m JOIN usage_event_records e USING(event_id) WHERE m.job_id=?1 AND m.state='pending' AND m.captured=0 ORDER BY m.occurred_ms,m.sequence LIMIT 100").map_err(failed)?;
        let rows = stmt
            .query_map([&job.id], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })
            .map_err(failed)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(failed)?;
        rows
    };
    for (sequence, event_id, text) in &rows {
        let raw: Value = serde_json::from_str(text).map_err(|_| invalid())?;
        let restored = raw.get("restored") == Some(&Value::Bool(true));
        let timestamp = raw
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(|text| {
                time::OffsetDateTime::parse(text, &time::format_description::well_known::Rfc3339)
                    .ok()
            })
            .and_then(|stamp| i64::try_from(stamp.unix_timestamp_nanos() / 1_000_000).ok());
        let has_hours: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='usage_retention_hours')",
                [],
                |r| r.get(0),
            )
            .map_err(failed)?;
        let captured_ineligible=has_hours && tx.query_row("SELECT EXISTS(SELECT 1 FROM usage_retention_hours WHERE event_id=?1 AND cloud_eligible=0)",[event_id],|r|r.get::<_,bool>(0)).map_err(failed)?;
        let excluded_before:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM usage_cloud_time_excluded WHERE binding=?1 AND source_home_id=?2 AND data_generation=?3 AND event_id=?4)",params![b,h,g,event_id],|r|r.get(0)).map_err(failed)?;
        let invalid_time = captured_ineligible
            || excluded_before
            || !timestamp
                .is_some_and(|stamp| stamp >= 0 && stamp <= job.started_ms.saturating_add(300_000));
        if restored || invalid_time {
            tx.execute("UPDATE usage_cloud_job_members SET captured=1,state='excluded' WHERE job_id=?1 AND event_id=?2",params![job.id,event_id]).map_err(failed)?;
            if restored {
                tx.execute("INSERT OR IGNORE INTO usage_cloud_blocked VALUES(?1,?2,?3,?4,?5,'origin_unverified')",params![b,h,g,event_id,sequence]).map_err(failed)?;
            } else {
                tx.execute(
                    "INSERT OR IGNORE INTO usage_cloud_time_excluded VALUES(?1,?2,?3,?4)",
                    params![b, h, g, event_id],
                )
                .map_err(failed)?;
            }
            continue;
        }
        let event = EventSubmission::from_allowed_text_with_timing(
            super::super::project(&tx, &raw)?,
            timing_from_local_capture(&raw).map_err(contract)?,
        )
        .map_err(contract)?;
        let member = event.member().map_err(contract)?;
        if member.event_id != *event_id {
            return Err(invalid());
        }
        tx.execute("INSERT OR IGNORE INTO usage_cloud_queue(binding,source_home_id,data_generation,event_id,sequence,original_text,original_sha256,semantic_hash_version,semantic_sha256,timing_json) VALUES(?1,?2,?3,?4,?5,?6,?7,1,?8,?9)",params![b,h,g,event_id,sequence,event.original_text,event.original_sha256.as_str(),event.semantic_sha256.as_str(),event.timing.as_ref().map(serde_json::to_string).transpose().map_err(failed)?]).map_err(failed)?;
        tx.execute(
            "UPDATE usage_cloud_job_members SET captured=1 WHERE job_id=?1 AND event_id=?2",
            params![job.id, event_id],
        )
        .map_err(failed)?;
    }
    tx.commit().map_err(failed)?;
    Ok(rows.len())
}
pub fn progress(db: &Connection, job: &Job) -> Result<Value, String> {
    let(total,pending,uploaded,expired,excluded):(i64,i64,i64,i64,i64)=db.query_row("SELECT COUNT(*),COALESCE(SUM(state='pending'),0),COALESCE(SUM(state='uploaded'),0),COALESCE(SUM(state='expired'),0),COALESCE(SUM(state='excluded'),0) FROM usage_cloud_job_members WHERE job_id=?1",[&job.id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).map_err(failed)?;
    let (outcome, finished): (String, bool) = db
        .query_row(
            "SELECT outcome,finished FROM usage_cloud_jobs WHERE job_id=?1",
            [&job.id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(failed)?;
    Ok(
        json!({"outcome":outcome,"finished":finished,"job_id":job.id,"started_at_ms":job.started_ms,"through_sequence":job.through.to_string(),"history_days":job.history_days,"target_events":total,"pending_events":pending,"uploaded_events":uploaded,"expired_events":expired,"excluded_events":excluded}),
    )
}
pub fn finish_pass(
    db: &Connection,
    job: &Job,
    retry_at: Option<i64>,
    outcome: &str,
) -> Result<Value, String> {
    let progress = progress(db, job)?;
    db.execute(
        "UPDATE usage_cloud_jobs SET retry_at_ms=?2,finished=?3,outcome=?4 WHERE job_id=?1",
        params![job.id, retry_at, progress["pending_events"] == 0, outcome],
    )
    .map_err(failed)?;
    self::progress(db, job)
}
/// An expiry response is not an acknowledgment. Retire the old compressed batch
/// and only remove the named identities from this job; all other members retry.
/// Terminal identities remain excluded across generations of the same account.
pub fn expire_batch_members(
    db: &mut Connection,
    lease: &Lease,
    job: &Job,
    batch: &UploadBatch,
    ids: &[String],
    now: i64,
) -> Result<(), String> {
    if ids.is_empty()
        || ids.iter().collect::<std::collections::BTreeSet<_>>().len() != ids.len()
        || ids
            .iter()
            .any(|id| !batch.members.iter().any(|m| &m.event_id == id))
    {
        return Err("Expired response changed batch membership".into());
    }
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(failed)?;
    check_lease(&tx, lease, now)?;
    let (b, h, g) = key(lease.source());
    for id in ids {
        tx.execute(
            "INSERT OR IGNORE INTO usage_cloud_terminal_events VALUES(?1,?2,?3,?4)",
            params![b, h, g, id],
        )
        .map_err(failed)?;
        tx.execute("UPDATE usage_cloud_job_members SET state='expired' WHERE job_id=?1 AND event_id=?2 AND state='pending'",params![job.id,id]).map_err(failed)?;
    }
    tx.execute("UPDATE usage_cloud_queue SET active_batch_id=NULL WHERE binding=?1 AND source_home_id=?2 AND data_generation=?3 AND active_batch_id=?4",params![b,h,g,batch.id.as_str()]).map_err(failed)?;
    tx.execute("UPDATE usage_cloud_expiry_guard SET active=1", [])
        .map_err(failed)?;
    tx.execute("DELETE FROM usage_cloud_upload_members WHERE binding=?1 AND source_home_id=?2 AND data_generation=?3 AND batch_id=?4",params![b,h,g,batch.id.as_str()]).map_err(failed)?;
    tx.execute("DELETE FROM usage_cloud_upload_batches WHERE binding=?1 AND source_home_id=?2 AND data_generation=?3 AND batch_id=?4",params![b,h,g,batch.id.as_str()]).map_err(failed)?;
    tx.execute("UPDATE usage_cloud_expiry_guard SET active=0", [])
        .map_err(failed)?;
    tx.commit().map_err(failed)
}
