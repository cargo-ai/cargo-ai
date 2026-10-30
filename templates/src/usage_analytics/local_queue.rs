//! Shared local retention maintenance for CLI and generated runtimes.
use rusqlite::{params, Connection};
fn failed(_: impl std::fmt::Display) -> String {
    "Usage cloud database operation failed".into()
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

pub fn high(db: &Connection) -> Result<i64, String> {
    db.query_row(
        "SELECT COALESCE((SELECT seq FROM sqlite_sequence WHERE name='usage_events'),0)",
        [],
        |row| row.get(0),
    )
    .map_err(failed)
}

pub(super) fn has_clear_intent_column(db: &Connection) -> Result<bool, String> {
    db.query_row("SELECT EXISTS(SELECT 1 FROM pragma_table_info('usage_cloud_state') WHERE name='clear_intent_json')", [], |row| row.get(0)).map_err(failed)
}

/// Reports a recorded acknowledgment, not current cloud retention or analytical
/// visibility. An explicit clear invalidates that generation's recorded copy.
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

pub(super) fn install_expiry_guards(db: &Connection) -> Result<(), String> {
    db.execute_batch("CREATE INDEX IF NOT EXISTS usage_cloud_queue_event ON usage_cloud_queue(event_id);
        CREATE INDEX IF NOT EXISTS usage_cloud_member_event ON usage_cloud_batch_members(event_id);
        CREATE INDEX IF NOT EXISTS usage_cloud_blocked_event ON usage_cloud_blocked(event_id);
        CREATE TABLE IF NOT EXISTS usage_cloud_expiry_guard(active INTEGER NOT NULL CHECK(active IN(0,1)));
        INSERT INTO usage_cloud_expiry_guard SELECT 0 WHERE NOT EXISTS(SELECT 1 FROM usage_cloud_expiry_guard);
        DROP TRIGGER IF EXISTS usage_cloud_member_no_delete;
        DROP TRIGGER IF EXISTS usage_cloud_batch_no_delete;
        DROP TRIGGER IF EXISTS usage_cloud_projection_no_delete;
        CREATE TRIGGER usage_cloud_member_no_delete BEFORE DELETE ON usage_cloud_batch_members WHEN (SELECT active FROM usage_cloud_expiry_guard)=0 AND EXISTS(SELECT 1 FROM usage_cloud_batches b WHERE b.binding=OLD.binding AND b.source_home_id=OLD.source_home_id AND b.data_generation=OLD.data_generation AND b.batch_id=OLD.batch_id AND b.sealed=1) BEGIN SELECT RAISE(ABORT,'sealed usage cloud membership'); END;
        CREATE TRIGGER usage_cloud_batch_no_delete BEFORE DELETE ON usage_cloud_batches WHEN OLD.sealed=1 AND (SELECT active FROM usage_cloud_expiry_guard)=0 BEGIN SELECT RAISE(ABORT,'immutable usage cloud batch'); END;
        CREATE TRIGGER usage_cloud_projection_no_delete BEFORE DELETE ON usage_cloud_queue WHEN (SELECT active FROM usage_cloud_expiry_guard)=0 BEGIN SELECT RAISE(ABORT,'immutable usage cloud projection'); END;").map_err(failed)
}

#[derive(Default, Debug)]
pub struct ExpiryCounts {
    pub expired_queue_records: usize,
    pub expired_without_ack: usize,
}
/// Called within the retention write transaction after explicit rolling consent.
/// Whole frozen batches are discarded, never rewritten. Surviving identities are
/// released for a new batch and active senders are fenced before payload removal.
pub fn expire_events(
    db: &Connection,
    event_ids: &[String],
    _now: i64,
) -> Result<ExpiryCounts, String> {
    let mut counts = ExpiryCounts::default();
    if event_ids.is_empty() || !installed(db)? {
        return Ok(counts);
    }
    if db.is_autocommit() {
        return Err("Usage expiry requires a write transaction".into());
    }
    install_expiry_guards(db)?;
    db.execute("UPDATE usage_cloud_expiry_guard SET active=1", [])
        .map_err(failed)?;
    for id in event_ids {
        let batches = {
            let mut stmt=db.prepare("SELECT DISTINCT binding,source_home_id,data_generation,batch_id FROM usage_cloud_batch_members WHERE event_id=?1").map_err(failed)?;
            let rows = stmt
                .query_map([id], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, i64>(2)?,
                        r.get::<_, String>(3)?,
                    ))
                })
                .map_err(failed)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(failed)?;
            rows
        };
        for (b, h, g, batch) in batches {
            db.execute("UPDATE usage_cloud_schedule SET lease_token=NULL,lease_expires_ms=NULL WHERE binding=?1 AND source_home_id=?2 AND data_generation=?3",params![b,h,g]).map_err(failed)?;
            db.execute("UPDATE usage_cloud_queue SET active_batch_id=NULL WHERE binding=?1 AND source_home_id=?2 AND data_generation=?3 AND active_batch_id=?4",params![b,h,g,batch]).map_err(failed)?;
            db.execute("DELETE FROM usage_cloud_batch_members WHERE binding=?1 AND source_home_id=?2 AND data_generation=?3 AND batch_id=?4",params![b,h,g,batch]).map_err(failed)?;
            db.execute("DELETE FROM usage_cloud_batches WHERE binding=?1 AND source_home_id=?2 AND data_generation=?3 AND batch_id=?4",params![b,h,g,batch]).map_err(failed)?;
        }
        let upload_table:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='usage_cloud_upload_members' AND type='table')",[],|r|r.get(0)).map_err(failed)?;
        if upload_table {
            let batches = {
                let mut stmt=db.prepare("SELECT DISTINCT binding,source_home_id,data_generation,batch_id FROM usage_cloud_upload_members WHERE event_id=?1").map_err(failed)?;
                let rows = stmt
                    .query_map([id], |r| {
                        Ok((
                            r.get::<_, String>(0)?,
                            r.get::<_, String>(1)?,
                            r.get::<_, i64>(2)?,
                            r.get::<_, String>(3)?,
                        ))
                    })
                    .map_err(failed)?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(failed)?;
                rows
            };
            for (b, h, g, batch) in batches {
                db.execute("UPDATE usage_cloud_schedule SET lease_token=NULL,lease_expires_ms=NULL WHERE binding=?1 AND source_home_id=?2 AND data_generation=?3",params![b,h,g]).map_err(failed)?;
                db.execute("UPDATE usage_cloud_queue SET active_batch_id=NULL WHERE binding=?1 AND source_home_id=?2 AND data_generation=?3 AND active_batch_id=?4",params![b,h,g,batch]).map_err(failed)?;
                db.execute("DELETE FROM usage_cloud_upload_members WHERE binding=?1 AND source_home_id=?2 AND data_generation=?3 AND batch_id=?4",params![b,h,g,batch]).map_err(failed)?;
                db.execute("DELETE FROM usage_cloud_upload_batches WHERE binding=?1 AND source_home_id=?2 AND data_generation=?3 AND batch_id=?4",params![b,h,g,batch]).map_err(failed)?;
            }
        }
        counts.expired_without_ack += db
            .query_row(
                "SELECT COUNT(*) FROM usage_cloud_queue WHERE event_id=?1 AND retained=0",
                [id],
                |r| r.get::<_, i64>(0),
            )
            .map_err(failed)? as usize;
        counts.expired_queue_records += db
            .execute("DELETE FROM usage_cloud_queue WHERE event_id=?1", [id])
            .map_err(failed)?;
        db.execute("DELETE FROM usage_cloud_blocked WHERE event_id=?1", [id])
            .map_err(failed)?;
        let time_table:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='usage_cloud_time_excluded' AND type='table')",[],|r|r.get(0)).map_err(failed)?;
        if time_table {
            db.execute(
                "DELETE FROM usage_cloud_time_excluded WHERE event_id=?1",
                [id],
            )
            .map_err(failed)?;
        }
    }
    db.execute("UPDATE usage_cloud_expiry_guard SET active=0", [])
        .map_err(failed)?;
    Ok(counts)
}
