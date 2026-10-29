//! Explicit local-detail expiry and conservative capture storage limits.
use super::{db_error, disk, private_permissions, safe_path};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
};

const POLICY_KEY: &str = "local_retention_policy_v2";
#[cfg(any(cargo_ai_cli, test))]
const LEGACY_POLICY_KEY: &str = "local_retention_policy_v1";
#[cfg(any(cargo_ai_cli, test))]
const CURSOR_KEY: &str = "local_retention_scan_after_v1";
const INDEX_CURSOR_KEY: &str = "local_retention_index_after_v2";
const PASS_LIMIT: usize = 1000;
const MAX_PASSES: usize = 64;
const HOUR_MS: i64 = 3_600_000;
const BUDGET: u64 = 512 * 1024 * 1024;
const FREE_RESERVE: u64 = 256 * 1024 * 1024;
const PROJECTION_ALLOWANCE: u64 = 2 * 16 * 1024;
const PAGE_GROWTH_ALLOWANCE: u64 = 2 * 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Policy {
    pub(crate) schema_version: u8,
    pub(crate) days: u8,
    pub(crate) enabled: bool,
    pub(crate) local_detail_loss_acknowledged: bool,
    pub(crate) unselected_loss_acknowledged: bool,
}
impl Default for Policy {
    fn default() -> Self {
        Self {
            schema_version: 2,
            days: 7,
            enabled: false,
            local_detail_loss_acknowledged: false,
            unselected_loss_acknowledged: false,
        }
    }
}
impl Policy {
    fn validate(&self) -> Result<(), String> {
        if self.schema_version != 2 || !matches!(self.days, 1 | 7 | 30) {
            return Err("Local retention requires schema version 2 and 1, 7 or 30 days".into());
        }
        if self.enabled
            && (!self.local_detail_loss_acknowledged || !self.unselected_loss_acknowledged)
        {
            return Err("Local retention requires explicit acknowledgment that expiry permanently removes richer local details; export them first if needed".into());
        }
        Ok(())
    }
}

fn policy(db: &Connection) -> Result<Policy, String> {
    let text: Option<String> = db
        .query_row(
            "SELECT value FROM usage_meta WHERE key=?1",
            [POLICY_KEY],
            |row| row.get(0),
        )
        .optional()
        .map_err(db_error)?;
    let policy = match text {
        None => Policy::default(),
        Some(text) => serde_json::from_str(&text)
            .map_err(|_| "Local retention policy is invalid; history preserved")?,
    };
    policy.validate()?;
    Ok(policy)
}

/// Explicit selection changes only local retention, never either upload consent.
#[cfg(any(cargo_ai_cli, test))]
pub(crate) fn configure(next: Policy) -> Result<Value, String> {
    next.validate()?;
    let mut db = super::open_database(true)?.ok_or("Usage database unavailable")?;
    configure_at(&mut db, &next)?;
    Ok(
        json!({"schema_version":2,"policy":next,"configured":true,"deleted_records":0,"upload_consent_changed":false,"local_detail_loss":loss_disclosure()}),
    )
}
#[cfg(any(cargo_ai_cli, test))]
fn configure_at(db: &mut Connection, next: &Policy) -> Result<(), String> {
    next.validate()?;
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error)?;
    tx.execute("INSERT INTO usage_meta(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value", params![POLICY_KEY,serde_json::to_string(next).map_err(|_| "Cannot encode local retention policy")?]).map_err(db_error)?;
    install_hours(&tx)?;
    tx.execute("DELETE FROM usage_meta WHERE key=?1", [CURSOR_KEY])
        .map_err(db_error)?;
    tx.commit().map_err(db_error)
}

fn loss_disclosure() -> Value {
    json!({"permanent":true,"full_local_restoration_from_cloud":false,"local_fields_removed":["local labels","local paths","unknown local extensions","other event details excluded from sanitized projections"],"export_command":"usage export","pending_uploads_expire":true,"expiry_is_acknowledgment":false,"explicit_rolling_policy_version":2})
}

fn exists(db: &Connection, table: &str) -> Result<bool, String> {
    db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
        [table],
        |row| row.get(0),
    )
    .map_err(db_error)
}

/// Sidecar metadata never changes original event timestamps, JSON or identities.
/// Invalid and future clocks use their first observation hour, permanently.
fn install_hours(db: &Connection) -> Result<(), String> {
    db.execute_batch("CREATE TABLE IF NOT EXISTS usage_retention_hours(event_id TEXT PRIMARY KEY REFERENCES usage_events(event_id) ON DELETE CASCADE,sequence INTEGER NOT NULL UNIQUE,hour_ms INTEGER NOT NULL,cloud_eligible INTEGER NOT NULL CHECK(cloud_eligible IN(0,1)));
        CREATE INDEX IF NOT EXISTS usage_retention_hour_expiry ON usage_retention_hours(hour_ms,sequence);
        CREATE TABLE IF NOT EXISTS usage_retention_retry_hours(event_id TEXT PRIMARY KEY,hour_ms INTEGER NOT NULL,cloud_eligible INTEGER NOT NULL);
        CREATE INDEX IF NOT EXISTS usage_retention_retry_expiry ON usage_retention_retry_hours(hour_ms);
        DROP TRIGGER IF EXISTS usage_retention_hour_capture;").map_err(db_error)
}
fn event_hour(created: &str, now: i64) -> (i64, bool) {
    let valid =
        time::OffsetDateTime::parse(created, &time::format_description::well_known::Rfc3339)
            .ok()
            .and_then(|t| i64::try_from(t.unix_timestamp_nanos() / 1_000_000).ok())
            .filter(|t| *t >= 0 && *t <= now.saturating_add(300_000));
    (valid.unwrap_or(now) / HOUR_MS * HOUR_MS, valid.is_some())
}
/// Capture and historical backfill share the same strict timestamp validator.
pub(super) fn index_capture(
    db: &Connection,
    id: &str,
    sequence: i64,
    created: &str,
) -> Result<(), String> {
    if !exists(db, "usage_retention_hours")? {
        return Ok(());
    }
    let now = i64::try_from(time::OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000)
        .map_err(|_| "Local retention time is invalid")?;
    let (hour, valid) = event_hour(created, now);
    db.execute(
        "DELETE FROM usage_retention_retry_hours WHERE event_id=?1",
        [id],
    )
    .map_err(db_error)?;
    db.execute(
        "INSERT OR IGNORE INTO usage_retention_hours VALUES(?1,?2,?3,?4)",
        params![id, sequence, hour, valid],
    )
    .map_err(db_error)?;
    db.execute(
        "UPDATE usage_meta SET value=?2 WHERE key=?1 AND CAST(value AS INTEGER)=?3",
        params![INDEX_CURSOR_KEY, sequence.to_string(), sequence - 1],
    )
    .map_err(db_error)?;
    Ok(())
}
fn cutoff(policy: &Policy, now: i64) -> i64 {
    now / HOUR_MS * HOUR_MS - i64::from(policy.days) * 24 * HOUR_MS
}
#[derive(Default)]
struct Scan {
    examined: usize,
    invalid_timestamp: usize,
    eligible: Vec<String>,
    complete: bool,
    indexing_pending: bool,
    expired_without_ack: usize,
}
fn scan(db: &Connection, policy: &Policy, now: i64) -> Result<Scan, String> {
    if now < 0 {
        return Err("Local retention time is invalid".into());
    }
    // A passive preview uses the index when installed; it never creates/backfills it.
    if !exists(db, "usage_retention_hours")? {
        return Ok(Scan {
            complete: false,
            indexing_pending: true,
            ..Scan::default()
        });
    }
    let mut indexing_pending:bool=db.query_row("SELECT COALESCE((SELECT MAX(sequence) FROM usage_events),0)>COALESCE((SELECT CAST(value AS INTEGER) FROM usage_meta WHERE key=?1),0)",[INDEX_CURSOR_KEY],|r|r.get(0)).map_err(db_error)?;
    if crate::usage_analytics::queue::installed(db)? {
        indexing_pending |= db.query_row("SELECT EXISTS(SELECT 1 FROM usage_cloud_queue q WHERE NOT EXISTS(SELECT 1 FROM usage_events e WHERE e.event_id=q.event_id) AND NOT EXISTS(SELECT 1 FROM usage_retention_retry_hours h WHERE h.event_id=q.event_id))",[],|r|r.get::<_,bool>(0)).map_err(db_error)?;
    }
    let mut stmt=db.prepare("SELECT event_id,cloud_eligible FROM (SELECT event_id,hour_ms,cloud_eligible FROM usage_retention_hours WHERE hour_ms<?1 UNION ALL SELECT event_id,hour_ms,cloud_eligible FROM usage_retention_retry_hours WHERE hour_ms<?1) ORDER BY hour_ms,event_id LIMIT ?2").map_err(db_error)?;
    let rows = stmt
        .query_map(params![cutoff(policy, now), PASS_LIMIT as i64], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, bool>(1)?))
        })
        .map_err(db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_error)?;
    let mut scan = Scan {
        examined: rows.len(),
        complete: rows.len() < PASS_LIMIT && !indexing_pending,
        indexing_pending,
        ..Scan::default()
    };
    for (id, valid) in rows {
        if !valid {
            scan.invalid_timestamp += 1;
        }
        if !crate::usage_analytics::queue::retained_event(db, &id)? {
            scan.expired_without_ack += 1;
        }
        scan.eligible.push(id);
    }
    Ok(scan)
}
fn report(policy: &Policy, scan: &Scan, deleted: usize) -> Value {
    json!({"schema_version":2,"policy":policy,"retention_consent_required":!policy.enabled,"local_detail_loss":loss_disclosure(),"scan_limit":PASS_LIMIT,"examined_records":scan.examined,"age_eligible_records":scan.eligible.len(),"custody_blocked_records":0,"unselected_loss_blocked_records":0,"invalid_timestamp_records":scan.invalid_timestamp,"eligible_records":if policy.enabled {scan.eligible.len()} else {0},"deleted_records":deleted,"expired_without_acknowledgment_records":if policy.enabled {scan.expired_without_ack} else {0},"coverage_incomplete":scan.expired_without_ack>0,"scan_complete":scan.complete,"maintenance_pending":policy.enabled && !scan.complete,"capture_paused":policy.enabled && !scan.complete,"upload_paused":policy.enabled && !scan.complete,"indexing_pending":scan.indexing_pending,"next_scan_after":"0","counts_scope":"this maintenance invocation; expired coverage total is persisted separately"})
}
#[cfg(any(cargo_ai_cli, test))]
pub(crate) fn preview(now_ms: i64) -> Result<Value, String> {
    if now_ms < 0 {
        return Err("Local retention time is invalid".into());
    }
    match super::open_database(false)? {
        None => Ok(report(
            &Policy::default(),
            &Scan {
                complete: true,
                ..Scan::default()
            },
            0,
        )),
        Some(db) => preview_at(&db, now_ms),
    }
}
#[cfg(any(cargo_ai_cli, test))]
fn preview_at(db: &Connection, now_ms: i64) -> Result<Value, String> {
    let policy = policy(db)?;
    let mut result = report(&policy, &scan(db, &policy, now_ms)?, 0);
    result["legacy_policy_requires_reselection"] = json!(
        db.query_row(
            "SELECT EXISTS(SELECT 1 FROM usage_meta WHERE key=?1)",
            [LEGACY_POLICY_KEY],
            |r| r.get::<_, bool>(0)
        )
        .map_err(db_error)?
            && !policy.enabled
    );
    let total: Option<String> = db
        .query_row(
            "SELECT value FROM usage_meta WHERE key='rolling_expired_without_ack_total_v2'",
            [],
            |r| r.get(0),
        )
        .optional()
        .map_err(db_error)?;
    result["expired_without_acknowledgment_total"] =
        json!(total.and_then(|s| s.parse::<u64>().ok()).unwrap_or(0));
    Ok(result)
}
/// Drain expired logical hours through short transactions. A finite budget leaves
/// an explicit pause; it never lets capture/upload race ahead of unfinished work.
pub(crate) fn apply(now_ms: i64) -> Result<Value, String> {
    if now_ms < 0 {
        return Err("Local retention time is invalid".into());
    }
    let Some(reader) = super::open_database(false)? else {
        return Ok(report(
            &Policy::default(),
            &Scan {
                complete: true,
                ..Scan::default()
            },
            0,
        ));
    };
    let selected = policy(&reader)?;
    if !selected.enabled {
        return Ok(report(
            &selected,
            &Scan {
                complete: true,
                ..Scan::default()
            },
            0,
        ));
    }
    drop(reader);
    let db = super::open_database(true)?.ok_or("Usage database unavailable")?;
    maintain_connection(&db, now_ms)
}

pub(super) fn before_capture(db: &Connection) -> Result<(), String> {
    if !db.is_autocommit() {
        return Ok(());
    }
    if !policy(db)?.enabled {
        return Ok(());
    }
    let now = i64::try_from(time::OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000)
        .map_err(|_| "Local retention time is invalid")?;
    let result = maintain_connection(db, now)?;
    if result["maintenance_pending"] == true {
        paused(db, "retention_maintenance");
        return Err("Usage capture paused while expired history maintenance catches up; application execution can continue".into());
    }
    Ok(())
}
fn maintain_connection(db: &Connection, now: i64) -> Result<Value, String> {
    let start = std::time::Instant::now();
    let mut deleted = 0u64;
    let mut expired = 0u64;
    let mut result;
    let mut passes = 0;
    loop {
        result = apply_connection(db, now)?;
        deleted += result["deleted_records"].as_u64().unwrap_or(0);
        expired += result["expired_without_acknowledgment_records"]
            .as_u64()
            .unwrap_or(0);
        passes += 1;
        if result["maintenance_pending"] != true
            || passes >= MAX_PASSES
            || start.elapsed() >= std::time::Duration::from_millis(250)
        {
            break;
        }
    }
    result["deleted_records"] = json!(deleted);
    result["expired_without_acknowledgment_records"] = json!(expired);
    result["coverage_incomplete"] = json!(expired > 0);
    result["maintenance_passes"] = json!(passes);
    Ok(result)
}
#[cfg(test)]
fn apply_at(db: &mut Connection, now: i64) -> Result<Value, String> {
    apply_connection(db, now)
}
fn apply_connection(db: &Connection, now: i64) -> Result<Value, String> {
    if now < 0 {
        return Err("Local retention time is invalid".into());
    }
    let tx = rusqlite::Transaction::new_unchecked(db, TransactionBehavior::Immediate)
        .map_err(db_error)?;
    // A waiting writer may have changed consent or the window before this lock.
    let policy = policy(&tx)?;
    if !policy.enabled {
        return Ok(report(
            &policy,
            &Scan {
                complete: true,
                ..Scan::default()
            },
            0,
        ));
    }
    install_hours(&tx)?;
    let after: i64 = tx
        .query_row(
            "SELECT CAST(value AS INTEGER) FROM usage_meta WHERE key=?1",
            [INDEX_CURSOR_KEY],
            |r| r.get(0),
        )
        .optional()
        .map_err(db_error)?
        .unwrap_or(0);
    let rows = {
        let mut stmt=tx.prepare("SELECT sequence,event_id,created_at FROM usage_events WHERE sequence>?1 ORDER BY sequence LIMIT ?2").map_err(db_error)?;
        let rows = stmt
            .query_map(params![after, PASS_LIMIT as i64], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })
            .map_err(db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_error)?;
        rows
    };
    let next = if rows.len() < PASS_LIMIT {
        crate::usage_analytics::queue::high(&tx)?
    } else {
        rows.last().map(|row| row.0).unwrap_or(after)
    };
    for (seq, id, created) in rows {
        let (hour, valid) = event_hour(&created, now);
        tx.execute(
            "INSERT OR IGNORE INTO usage_retention_hours VALUES(?1,?2,?3,?4)",
            params![id, seq, hour, valid],
        )
        .map_err(db_error)?;
    }
    tx.execute("INSERT INTO usage_meta(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",params![INDEX_CURSOR_KEY,next.to_string()]).map_err(db_error)?;
    if crate::usage_analytics::queue::installed(&tx)? {
        let retries = {
            let mut stmt=tx.prepare("SELECT event_id,original_text FROM usage_cloud_queue q WHERE NOT EXISTS(SELECT 1 FROM usage_events e WHERE e.event_id=q.event_id) AND NOT EXISTS(SELECT 1 FROM usage_retention_retry_hours h WHERE h.event_id=q.event_id) GROUP BY event_id ORDER BY event_id LIMIT ?1").map_err(db_error)?;
            let rows = stmt
                .query_map([PASS_LIMIT as i64], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                })
                .map_err(db_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(db_error)?;
            rows
        };
        for (id, text) in retries {
            let stamp = serde_json::from_str::<Value>(&text)
                .ok()
                .and_then(|v| v["occurred_at_ms"].as_i64())
                .filter(|t| *t >= 0 && *t <= now.saturating_add(300_000));
            tx.execute(
                "INSERT OR IGNORE INTO usage_retention_retry_hours VALUES(?1,?2,?3)",
                params![
                    id,
                    stamp.unwrap_or(now) / HOUR_MS * HOUR_MS,
                    stamp.is_some()
                ],
            )
            .map_err(db_error)?;
        }
    }
    let mut scan = scan(&tx, &policy, now)?;
    crate::usage_analytics::queue::expire_events(&tx, &scan.eligible, now)?;
    let mut deleted = 0;
    for id in &scan.eligible {
        deleted += tx
            .execute("DELETE FROM usage_events WHERE event_id=?1", [id])
            .map_err(db_error)?;
        tx.execute(
            "DELETE FROM usage_retention_retry_hours WHERE event_id=?1",
            [id],
        )
        .map_err(db_error)?;
    }
    tx.execute("INSERT INTO usage_meta(key,value) VALUES('rolling_expired_without_ack_total_v2',?1) ON CONFLICT(key) DO UPDATE SET value=CAST(CAST(value AS INTEGER)+CAST(excluded.value AS INTEGER) AS TEXT)",[scan.expired_without_ack.to_string()]).map_err(db_error)?;
    scan.complete=!scan.indexing_pending && !tx.query_row("SELECT EXISTS(SELECT 1 FROM usage_retention_hours WHERE hour_ms<?1) OR EXISTS(SELECT 1 FROM usage_retention_retry_hours WHERE hour_ms<?1)",[cutoff(&policy,now)],|r|r.get::<_,bool>(0)).map_err(db_error)?;
    tx.commit().map_err(db_error)?;
    Ok(report(&policy, &scan, deleted))
}

#[derive(Clone, Copy)]
struct Limits {
    budget: u64,
    reserve: u64,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            budget: BUDGET,
            reserve: FREE_RESERVE,
        }
    }
}
#[derive(Default)]
struct Storage {
    #[cfg(any(cargo_ai_cli, test))]
    active: u64,
    #[cfg(any(cargo_ai_cli, test))]
    reusable: u64,
    #[cfg(any(cargo_ai_cli, test))]
    wal: u64,
    used: u64,
    free: Option<u64>,
}
fn directory(db: &Connection) -> Option<PathBuf> {
    db.path()
        .filter(|p| !p.is_empty())
        .and_then(|p| Path::new(p).parent())
        .map(Path::to_path_buf)
}
fn storage(db: &Connection) -> Result<Storage, String> {
    let number = |name| -> Result<u64, String> {
        u64::try_from(
            db.pragma_query_value(None, name, |row| row.get::<_, i64>(0))
                .map_err(db_error)?,
        )
        .map_err(|_| "Usage storage page count is invalid".into())
    };
    let pages = number("page_count")?;
    let reusable = number("freelist_count")?;
    let size = number("page_size")?;
    let active = pages
        .checked_sub(reusable)
        .and_then(|n| n.checked_mul(size))
        .ok_or("Usage storage size overflowed")?;
    #[cfg(any(cargo_ai_cli, test))]
    let reusable = reusable
        .checked_mul(size)
        .ok_or("Usage reusable storage size overflowed")?;
    let wal = match db.path().filter(|p| !p.is_empty()) {
        None => 0,
        Some(path) => match fs::symlink_metadata(format!("{path}-wal")) {
            Ok(meta) if meta.is_file() && !meta.file_type().is_symlink() => meta.len(),
            Ok(_) => return Err("Usage WAL is not a regular file".into()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
            Err(_) => return Err("Cannot inspect usage WAL storage".into()),
        },
    };
    let used = active
        .checked_add(wal)
        .ok_or("Usage storage total overflowed")?;
    let path = directory(db).unwrap_or_else(std::env::temp_dir);
    let free = disk::available(&path).ok();
    Ok(Storage {
        #[cfg(any(cargo_ai_cli, test))]
        active,
        #[cfg(any(cargo_ai_cli, test))]
        reusable,
        #[cfg(any(cargo_ai_cli, test))]
        wal,
        used,
        free,
    })
}
fn growth(raw_bytes: usize) -> Result<u64, String> {
    u64::try_from(raw_bytes)
        .ok()
        .and_then(|n| n.checked_mul(3))
        .and_then(|n| n.checked_add(PROJECTION_ALLOWANCE + PAGE_GROWTH_ALLOWANCE))
        .ok_or("Usage capture size overflowed".into())
}
fn failure(storage: &Storage, growth: u64, limits: Limits) -> Option<&'static str> {
    if storage
        .used
        .checked_add(growth)
        .is_none_or(|n| n > limits.budget)
    {
        return Some("storage_budget");
    }
    match storage.free {
        None => Some("free_space_unavailable"),
        Some(free)
            if free
                .checked_sub(growth)
                .is_none_or(|remaining| remaining < limits.reserve) =>
        {
            Some("free_space_reserve")
        }
        _ => None,
    }
}
fn marker(db: &Connection, name: &str, text: &str) {
    let Some(dir) = directory(db) else { return };
    let path = dir.join(name);
    if safe_path(&path, false).is_ok() && fs::write(&path, text.as_bytes()).is_ok() {
        let _ = private_permissions(&path, false);
    }
}
fn paused(db: &Connection, code: &str) {
    marker(
        db,
        "capture-paused",
        match code {
            "storage_budget" => "storage_budget\n",
            "retention_maintenance" => "retention_maintenance\n",
            "free_space_reserve" => "free_space_reserve\n",
            _ => "free_space_unavailable\n",
        },
    );
    marker(
        db,
        "capture-incomplete",
        "Some usage facts could not be persisted. Totals may be incomplete.\n",
    );
}
fn resumed(db: &Connection) {
    let Some(dir) = directory(db) else { return };
    let path = dir.join("capture-paused");
    if path.is_file() && safe_path(&path, false).is_ok() {
        marker(
            db,
            "capture-resumed",
            "Capture resumed after a storage pause; earlier usage coverage remains incomplete.\n",
        );
        let _ = fs::remove_file(path);
    }
}

/// Called only for a new immutable fact, inside its write transaction. Existing
/// pages include all actual queues/projections; prospective growth reserves both
/// pipelines plus index/page splits without scanning historical payloads.
pub(super) fn check_capture(db: &Connection, raw_bytes: usize) -> Result<(), String> {
    let selected = policy(db)?;
    if selected.enabled {
        let now = i64::try_from(time::OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000)
            .map_err(|_| "Local retention time is invalid")?;
        if !scan(db, &selected, now)?.complete {
            paused(db, "retention_maintenance");
            return Err("Usage capture paused while expired history maintenance catches up; application execution can continue".into());
        }
    }
    let observed = storage(db).map_err(|_| {
        paused(db, "free_space_unavailable");
        "Usage capture paused because storage could not be verified; retained history is preserved"
            .to_string()
    })?;
    let limits = Limits::default();
    #[cfg(test)]
    let (observed, limits) = TEST_LIMITS.with(|slot| match slot.borrow().as_ref() {
        Some((test_limits, test_free)) => (
            Storage {
                free: *test_free,
                ..observed
            },
            *test_limits,
        ),
        None => (observed, limits),
    });
    let growth = growth(raw_bytes)?;
    if let Some(code) = failure(&observed, growth, limits) {
        paused(db, code);
        return Err(match code {"storage_budget"=>"Usage capture paused at the local storage budget; retained history is preserved", "free_space_reserve"=>"Usage capture paused to preserve free space for control operations; retained history is preserved",_=>"Usage capture paused because free space could not be verified; retained history is preserved"}.into());
    }
    if observed.used >= limits.budget.saturating_mul(4) / 5 {
        marker(
            db,
            "capture-budget-warning",
            "Usage storage is at or above 80% of its capture budget.\n",
        );
        static WARNED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        if !WARNED.swap(true, std::sync::atomic::Ordering::Relaxed) {
            eprintln!("Usage storage is at or above 80% of its capture budget; review local retention or export history.");
        }
    }
    Ok(())
}

pub(super) fn capture_committed(db: &Connection) {
    resumed(db);
}

#[cfg(any(cargo_ai_cli, test))]
fn last_pause(dir: Option<&Path>) -> Option<String> {
    use std::io::Read;
    let path = dir?.join("capture-paused");
    safe_path(&path, false).ok()?;
    let mut text = String::new();
    fs::File::open(path)
        .ok()?
        .take(64)
        .read_to_string(&mut text)
        .ok()?;
    matches!(
        text.trim(),
        "storage_budget"
            | "free_space_reserve"
            | "free_space_unavailable"
            | "retention_maintenance"
    )
    .then(|| text.trim().to_owned())
}

#[cfg(any(cargo_ai_cli, test))]
fn storage_report(storage: &Storage, dir: Option<&Path>) -> Result<Value, String> {
    let current = failure(storage, growth(0)?, Limits::default());
    let last = last_pause(dir);
    Ok(
        json!({"used_storage_upper_bound_bytes":storage.used,"active_sqlite_bytes":storage.active,"reusable_sqlite_bytes":storage.reusable,"physical_wal_bytes":storage.wal,"budget_bytes":BUDGET,"warning_threshold_percent":80,"warning":storage.used>=BUDGET*4/5,"free_space_bytes":storage.free,"free_space_reserve_bytes":FREE_RESERVE,"prospective_projection_allowance_bytes":PROJECTION_ALLOWANCE,"prospective_page_allowance_bytes":PAGE_GROWTH_ALLOWANCE,"capture_paused":current.is_some()||last.is_some(),"pause_reason":current,"last_pause_reason":last,"ready_to_resume":current.is_none(),"coverage_gap_recorded":dir.is_some_and(|d|d.join("capture-incomplete").is_file()),"resumed_after_gap":dir.is_some_and(|d|d.join("capture-resumed").is_file()),"capture_budget_excludes":["reusable database pages","recovery images","SQLite SHM"],"measurement":"SQLite active pages plus physical WAL; includes actual payload/projection/queue storage and may count WAL copies twice. Prospective allowance is per new fact, not escrow for all existing unscanned events. Queue growth can pause later capture; protected data is never removed for the budget. Recovery images remain preserved separately and consume physical free space."}),
    )
}

#[cfg(any(cargo_ai_cli, test))]
pub(crate) fn status(now_ms: i64) -> Result<Value, String> {
    if now_ms < 0 {
        return Err("Local retention time is invalid".into());
    }
    let Some(db) = super::open_database(false)? else {
        let mut result = report(
            &Policy::default(),
            &Scan {
                complete: true,
                ..Scan::default()
            },
            0,
        );
        let mut path = super::directory();
        while !path.is_dir() {
            if !path.pop() {
                break;
            }
        }
        let observed = Storage {
            free: disk::available(&path).ok(),
            ..Storage::default()
        };
        result["storage"] = storage_report(&observed, None)?;
        return Ok(result);
    };
    let mut result = preview_at(&db, now_ms)?;
    let observed = storage(&db)?;
    result["storage"] = storage_report(&observed, directory(&db).as_deref())?;
    Ok(result)
}

#[cfg(test)]
thread_local! { static TEST_LIMITS: std::cell::RefCell<Option<(Limits,Option<u64>)>> = const {std::cell::RefCell::new(None)}; }

#[cfg(all(test, cargo_ai_cli))]
#[path = "migration_benchmark_tests.rs"]
mod migration_benchmark_tests;
#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage_analytics::{contract::*, queue};
    use crate::usage_store::insert_event;

    const NOW: i64 = 1_793_145_600_000; // 2026-10-28T00:00:00Z
    fn id(index: u64) -> String {
        format!("00000000-0000-4000-8000-{index:012}")
    }
    fn event(index: u64, timestamp: &str) -> Value {
        json!({"schema_version":1,"event_id":id(index),"root_run_id":id(9001),"agent_run_id":id(9002),"event_type":"provider_request_completed","timestamp":timestamp,"usage":{"input_tokens":5},"agent":{"private_path":"/synthetic/private"},"extension":{"local_only":true}})
    }
    fn db() -> Connection {
        let db = Connection::open_in_memory().unwrap();
        super::super::initialize_test_database(&db);
        db
    }
    fn enabled(unselected: bool) -> Policy {
        Policy {
            enabled: true,
            local_detail_loss_acknowledged: true,
            unselected_loss_acknowledged: unselected,
            ..Policy::default()
        }
    }
    fn count(db: &Connection) -> i64 {
        db.query_row("SELECT COUNT(*) FROM usage_events", [], |row| row.get(0))
            .unwrap()
    }
    fn old_queue(db: &Connection) {
        db.execute_batch("CREATE TABLE usage_backup_queue(event_id TEXT NOT NULL REFERENCES usage_events(event_id) ON DELETE CASCADE,binding TEXT NOT NULL,generation INTEGER NOT NULL,acknowledged INTEGER NOT NULL,PRIMARY KEY(event_id,binding,generation)); CREATE TABLE usage_backup_windows(window_id INTEGER PRIMARY KEY,binding TEXT NOT NULL,generation INTEGER NOT NULL,start_sequence INTEGER NOT NULL,end_sequence INTEGER,last_scan INTEGER NOT NULL);").unwrap();
    }
    fn source() -> Source {
        Source {
            account_binding: CanonicalUuid::try_from(id(9003)).unwrap(),
            source_home_id: CanonicalUuid::try_from(id(9004)).unwrap(),
            data_generation: Counter::try_from(1).unwrap(),
        }
    }
    fn provenance() -> Provenance {
        Provenance {
            lifecycle_epoch: LifecycleEpoch::try_from(1).unwrap(),
            consent_interval: Counter::try_from(1).unwrap(),
        }
    }
    fn projection(_: &Connection, event: &Value) -> Result<String, String> {
        let occurred = time::OffsetDateTime::parse(
            event["timestamp"].as_str().unwrap(),
            &time::format_description::well_known::Rfc3339,
        )
        .unwrap()
        .unix_timestamp()
            * 1000;
        Ok(json!({"schema_version":1,"normalization_version":1,"event_id":event["event_id"],"root_run_id":id(9001),"agent_run_id":id(9002),"event_type":"provider_request","event_kind":"provider_request_completed","occurred_at_ms":occurred,"tokens":{"input_tokens":5,"output_tokens":null}}).to_string())
    }
    #[test]
    fn policy_committed_while_expiry_waits_on_writer_is_reloaded_before_deletion() {
        use std::sync::mpsc::{self, SyncSender};
        thread_local! {static WAITING:std::cell::RefCell<Option<SyncSender<()>>>=const {std::cell::RefCell::new(None)};}
        fn waiting_for_writer(attempt: i32) -> bool {
            WAITING.with(|slot| {
                if let Some(sender) = slot.borrow_mut().take() {
                    let _ = sender.send(());
                }
            });
            std::thread::sleep(std::time::Duration::from_millis(1));
            attempt < 5000
        }
        for next in [
            Policy {
                enabled: false,
                ..enabled(true)
            },
            Policy {
                days: 30,
                ..enabled(true)
            },
        ] {
            let fixture = Fixture::new();
            let mut writer = super::super::open_at(&fixture.0, true).unwrap().unwrap();
            insert_event(&writer, &event(1, "2026-10-10T00:00:00Z")).unwrap();
            configure_at(&mut writer, &enabled(true)).unwrap();
            let reader = Connection::open(fixture.0.join("usage.sqlite3")).unwrap();
            reader.pragma_update(None, "foreign_keys", true).unwrap();
            reader.busy_handler(Some(waiting_for_writer)).unwrap();
            let tx = writer
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .unwrap();
            next.validate().unwrap();
            tx.execute(
                "UPDATE usage_meta SET value=?2 WHERE key=?1",
                params![POLICY_KEY, serde_json::to_string(&next).unwrap()],
            )
            .unwrap();
            let (send, receive) = mpsc::sync_channel(1);
            let worker = std::thread::spawn(move || {
                WAITING.with(|slot| *slot.borrow_mut() = Some(send));
                apply_connection(&reader, NOW)
            });
            receive
                .recv_timeout(std::time::Duration::from_secs(2))
                .expect("expiry must actually wait behind the policy writer");
            tx.commit().unwrap();
            let result = worker.join().unwrap().unwrap();
            assert_eq!(
                result["deleted_records"], 0,
                "a newer consent/window must govern expiry after the writer lock is acquired"
            );
            assert_eq!(result["policy"], serde_json::to_value(&next).unwrap());
            assert_eq!(count(&writer), 1);
        }
    }
    #[test]
    fn old_policy_and_backup_material_require_explicit_new_loss_consent() {
        let mut db = db();
        old_queue(&db);
        insert_event(&db, &event(1, "2026-10-01T00:00:00Z")).unwrap();
        db.execute(
            "INSERT INTO usage_backup_queue VALUES(?1,'legacy',1,0)",
            [id(1)],
        )
        .unwrap();
        let mut legacy = enabled(true);
        legacy.schema_version = 1;
        db.execute(
            "INSERT INTO usage_meta VALUES(?1,?2)",
            params![LEGACY_POLICY_KEY, serde_json::to_string(&legacy).unwrap()],
        )
        .unwrap();
        assert_eq!(apply_at(&mut db, NOW).unwrap()["deleted_records"], 0);
        assert_eq!(count(&db), 1);
        assert!(configure_at(&mut db, &legacy).is_err());
        assert!(configure_at(&mut db, &enabled(false)).is_err());
        assert_eq!(
            db.query_row("SELECT COUNT(*) FROM usage_backup_queue", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
        configure_at(&mut db, &enabled(true)).unwrap();
        let result = apply_at(&mut db, NOW).unwrap();
        assert_eq!(result["deleted_records"], 1);
        assert_eq!(result["expired_without_acknowledgment_records"], 1);
        assert_eq!(
            db.query_row("SELECT COUNT(*) FROM usage_backup_queue", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
    #[test]
    fn one_seven_thirty_days_keep_current_partial_hour_and_exact_boundary() {
        for days in [1, 7, 30] {
            let mut db = db();
            let now = NOW + 37 * 60_000;
            let boundary = now / HOUR_MS * HOUR_MS - i64::from(days) * 24 * HOUR_MS;
            for (id, stamp) in [(1, boundary - 1), (2, boundary), (3, now)] {
                let stamp =
                    time::OffsetDateTime::from_unix_timestamp_nanos(i128::from(stamp) * 1_000_000)
                        .unwrap()
                        .format(&time::format_description::well_known::Rfc3339)
                        .unwrap();
                insert_event(&db, &event(id, &stamp)).unwrap();
            }
            configure_at(
                &mut db,
                &Policy {
                    days,
                    ..enabled(true)
                },
            )
            .unwrap();
            let preview = preview_at(&db, now).unwrap();
            assert!(preview["indexing_pending"].as_bool().unwrap());
            assert_eq!(count(&db), 3);
            assert_eq!(apply_at(&mut db, now).unwrap()["deleted_records"], 1);
            assert_eq!(count(&db), 2);
        }
    }
    #[test]
    fn unfinished_maintenance_pauses_nested_capture_and_fresh_append_drains_expiry_first() {
        let mut db = db();
        for i in 1..=1101 {
            insert_event(&db, &event(i, "2026-10-01T00:00:00Z")).unwrap();
        }
        configure_at(&mut db, &enabled(true)).unwrap();
        assert_eq!(apply_at(&mut db, NOW).unwrap()["maintenance_pending"], true);
        let stamp = time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap();
        db.execute_batch("BEGIN IMMEDIATE").unwrap();
        assert!(insert_event(&db, &event(5000, &stamp))
            .unwrap_err()
            .contains("maintenance catches up"));
        db.execute_batch("COMMIT").unwrap();
        assert_eq!(count(&db), 101);
        insert_event(&db, &event(5000, &stamp)).unwrap();
        assert_eq!(count(&db), 1);
    }
    #[test]
    fn restart_keeps_fixed_invalid_anchor_and_unknown_raw_fields() {
        let fixture = Fixture::new();
        let mut db = super::super::open_at(&fixture.0, true).unwrap().unwrap();
        let raw = event(1, "invalid-clock");
        insert_event(&db, &raw).unwrap();
        configure_at(&mut db, &enabled(true)).unwrap();
        apply_at(&mut db, NOW).unwrap();
        drop(db);
        let mut reopened = super::super::open_at(&fixture.0, true).unwrap().unwrap();
        let actual: String = reopened
            .query_row("SELECT record_json FROM usage_event_records", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(serde_json::from_str::<Value>(&actual).unwrap(), raw);
        assert_eq!(
            reopened
                .query_row("SELECT hour_ms FROM usage_retention_hours", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            NOW
        );
        assert_eq!(
            apply_at(&mut reopened, NOW + 8 * 24 * HOUR_MS).unwrap()["deleted_records"],
            1
        );
    }
    #[test]
    fn invalid_and_future_clocks_have_finite_persisted_anchor_and_no_cloud_eligibility() {
        let mut db = db();
        insert_event(&db, &event(1, "bad-date")).unwrap();
        insert_event(&db, &event(2, "2099-01-01T00:00:00Z")).unwrap();
        configure_at(&mut db, &enabled(true)).unwrap();
        assert_eq!(apply_at(&mut db, NOW).unwrap()["deleted_records"], 0);
        assert_eq!(
            db.query_row(
                "SELECT COUNT(*) FROM usage_retention_hours WHERE hour_ms=?1 AND cloud_eligible=0",
                [NOW],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            2
        );
        assert_eq!(
            apply_at(&mut db, NOW + 8 * 24 * HOUR_MS).unwrap()["deleted_records"],
            2
        );
    }
    #[test]
    fn large_inactivity_catchup_drains_short_transactions_before_capture() {
        let mut db = db();
        for i in 1..=2501 {
            insert_event(&db, &event(i, "2026-10-01T00:00:00Z")).unwrap();
        }
        configure_at(&mut db, &enabled(true)).unwrap();
        let first = apply_at(&mut db, NOW).unwrap();
        assert_eq!(first["deleted_records"], 1000);
        assert_eq!(first["maintenance_pending"], true);
        let rest = maintain_connection(&db, NOW).unwrap();
        assert_eq!(rest["deleted_records"], 1501);
        assert_eq!(rest["maintenance_pending"], false);
        assert_eq!(count(&db), 0);
        assert_eq!(
            preview_at(&db, NOW).unwrap()["expired_without_acknowledgment_total"],
            2501
        );
    }
    #[test]
    fn expiry_discards_mixed_frozen_batch_requeues_survivor_and_fences_late_ack() {
        let mut db = db();
        queue::initialize(&mut db).unwrap();
        queue::enable(&mut db, &source(), &provenance(), 0).unwrap();
        insert_event(&db, &event(1, "2026-10-01T00:00:00Z")).unwrap();
        insert_event(&db, &event(2, "2026-10-27T00:00:00Z")).unwrap();
        queue::capture(&mut db, &source(), projection).unwrap();
        let lease = queue::acquire_lease(&mut db, &source(), NOW, queue::MAX_LEASE_MS)
            .unwrap()
            .unwrap();
        let batch = queue::prepare_batch(&mut db, &lease, NOW).unwrap().unwrap();
        configure_at(&mut db, &enabled(true)).unwrap();
        assert_eq!(apply_at(&mut db, NOW).unwrap()["deleted_records"], 1);
        assert!(
            queue::acknowledge_uploaded(&mut db, &lease, batch.id(), NOW)
                .unwrap_err()
                .contains("expired")
        );
        assert_eq!(count(&db), 1);
        let new_lease = queue::acquire_lease(&mut db, &source(), NOW, queue::MAX_LEASE_MS)
            .unwrap()
            .unwrap();
        let survivor = queue::prepare_batch(&mut db, &new_lease, NOW)
            .unwrap()
            .unwrap();
        assert_ne!(batch.id(), survivor.id());
        assert_eq!(survivor.members().len(), 1);
        assert_eq!(survivor.members()[0].event_id, id(2));
        queue::acknowledge_uploaded(&mut db, &new_lease, survivor.id(), NOW).unwrap();
        assert_eq!(count(&db), 1);
        assert!(queue::retained_event(&db, &id(2)).unwrap());
        assert_eq!(
            apply_at(&mut db, NOW + 8 * 24 * HOUR_MS).unwrap()["deleted_records"],
            1
        );
        assert_eq!(queue::status(&db, &source()).unwrap().pending, 0);
        assert!(!queue::retained_event(&db, &id(2)).unwrap());
    }
    #[test]
    fn selected_retention_expires_orphaned_retry_material_from_old_raw_cleanup() {
        let mut db = db();
        queue::initialize(&mut db).unwrap();
        queue::enable(&mut db, &source(), &provenance(), 0).unwrap();
        insert_event(&db, &event(1, "2026-10-01T00:00:00Z")).unwrap();
        queue::capture(&mut db, &source(), projection).unwrap();
        db.execute("DELETE FROM usage_events", []).unwrap();
        configure_at(&mut db, &enabled(true)).unwrap();
        apply_at(&mut db, NOW).unwrap();
        assert_eq!(queue::status(&db, &source()).unwrap().pending, 0);
    }
    struct Override;
    impl Override {
        fn new(budget: u64, free: Option<u64>) -> Self {
            TEST_LIMITS.with(|slot| {
                *slot.borrow_mut() = Some((
                    Limits {
                        budget,
                        reserve: 100,
                    },
                    free,
                ))
            });
            Self
        }
    }
    impl Drop for Override {
        fn drop(&mut self) {
            TEST_LIMITS.with(|slot| *slot.borrow_mut() = None);
        }
    }
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let dir =
                std::env::temp_dir().join(format!("usage-retention-{}", uuid::Uuid::new_v4()));
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    #[test]
    fn passive_frontdoors_create_nothing_and_explicit_configuration_changes_no_upload_consent() {
        struct Selected(Option<std::ffi::OsString>);
        impl Drop for Selected {
            fn drop(&mut self) {
                // Home-selecting tests run serially in the isolated test runner.
                unsafe {
                    match self.0.take() {
                        Some(value) => std::env::set_var("CARGO_AI_HOME", value),
                        None => std::env::remove_var("CARGO_AI_HOME"),
                    }
                }
            }
        }
        let fixture = Fixture::new();
        let selected = fixture.0.join("selected");
        let _selected = Selected(std::env::var_os("CARGO_AI_HOME"));
        // No other test may read process-wide Home while it is selected here.
        unsafe { std::env::set_var("CARGO_AI_HOME", &selected) };
        assert_eq!(status(NOW).unwrap()["policy"]["enabled"], false);
        assert_eq!(preview(NOW).unwrap()["deleted_records"], 0);
        assert_eq!(apply(NOW).unwrap()["deleted_records"], 0);
        assert!(!selected.exists());
        fs::create_dir(&selected).unwrap();
        let config="unknown_setting = 'preserved'\n[usage]\ntracking = true\nbackup_enabled = true\nbackup_account_binding = 'old-account'\nbackup_generation = 5\n";
        fs::write(selected.join("config.toml"), config).unwrap();
        configure(enabled(true)).unwrap();
        assert_eq!(
            fs::read_to_string(selected.join("config.toml")).unwrap(),
            config
        );
        let db = super::super::open_database(false).unwrap().unwrap();
        assert!(!exists(&db, "usage_cloud_state").unwrap());
        assert!(!exists(&db, "usage_backup_windows").unwrap());
        assert_eq!(apply(NOW).unwrap()["deleted_records"], 0);
        let status = status(NOW).unwrap();
        assert_eq!(status["policy"]["days"], 7);
        assert_eq!(status["storage"]["budget_bytes"], BUDGET);
    }

    #[test]
    fn capture_budget_preserves_duplicates_controls_and_history_then_resumes_with_gap() {
        let fixture = Fixture::new();
        let mut db = super::super::open_at(&fixture.0, true).unwrap().unwrap();
        let first = event(1, "2026-10-01T00:00:00Z");
        insert_event(&db, &first).unwrap();
        let next = event(2, "2026-10-01T00:00:00Z");
        let used = storage(&db).unwrap().used;
        let bound = used + growth(next.to_string().len()).unwrap() - 1;
        {
            let _limits = Override::new(bound, Some(u64::MAX));
            assert!(!insert_event(&db, &first).unwrap());
            assert!(insert_event(&db, &next)
                .unwrap_err()
                .contains("storage budget"));
            assert_eq!(count(&db), 1);
            configure_at(&mut db, &enabled(true)).unwrap();
        }
        assert!(fixture.0.join("capture-paused").is_file());
        assert!(fixture.0.join("capture-incomplete").is_file());
        insert_event(&db, &next).unwrap();
        assert_eq!(count(&db), 1);
        assert!(!fixture.0.join("capture-paused").exists());
        assert!(fixture.0.join("capture-resumed").is_file());
        assert!(fixture.0.join("capture-incomplete").is_file());
    }
    #[test]
    fn capture_warns_at_eighty_percent_and_never_deletes_an_oversized_store() {
        let fixture = Fixture::new();
        let db = super::super::open_at(&fixture.0, true).unwrap().unwrap();
        let mut large = event(1, "2026-10-01T00:00:00Z");
        large["extension"] = json!("x".repeat(5 * 1024 * 1024));
        insert_event(&db, &large).unwrap();
        let used = storage(&db).unwrap().used;
        let next = event(2, "2026-10-01T00:00:00Z");
        {
            let _limits = Override::new(used * 5 / 4, Some(u64::MAX));
            insert_event(&db, &next).unwrap();
        }
        assert!(fixture.0.join("capture-budget-warning").is_file());
        {
            let _limits = Override::new(1, Some(u64::MAX));
            assert!(insert_event(&db, &event(3, "2026-10-01T00:00:00Z")).is_err());
            assert_eq!(count(&db), 2);
        }
        let original: String = db
            .query_row(
                "SELECT record_json FROM usage_event_records WHERE event_id=?1",
                [id(1)],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(serde_json::from_str::<Value>(&original).unwrap(), large);
    }

    #[test]
    fn free_space_failure_pauses_capture_without_restricting_control_writes() {
        let fixture = Fixture::new();
        let mut db = super::super::open_at(&fixture.0, true).unwrap().unwrap();
        let event = event(1, "2026-10-01T00:00:00Z");
        {
            let _limits = Override::new(u64::MAX, None);
            assert!(insert_event(&db, &event)
                .unwrap_err()
                .contains("could not be verified"));
            configure_at(&mut db, &enabled(true)).unwrap();
        }
        {
            let _limits = Override::new(
                u64::MAX,
                Some(growth(event.to_string().len()).unwrap() + 99),
            );
            assert!(insert_event(&db, &event)
                .unwrap_err()
                .contains("free space"));
            assert_eq!(count(&db), 0);
        }
        insert_event(&db, &event).unwrap();
        assert_eq!(count(&db), 1);
    }
    #[test]
    fn freed_sqlite_pages_are_reusable_without_truncating_the_old_store() {
        let fixture = Fixture::new();
        let db = super::super::open_at(&fixture.0, true).unwrap().unwrap();
        let mut large = event(1, "2026-10-01T00:00:00Z");
        large["local_extension"] = json!("x".repeat(2 * 1024 * 1024));
        insert_event(&db, &large).unwrap();
        db.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)").unwrap();
        let before = storage(&db).unwrap().active;
        db.execute("DELETE FROM usage_events", []).unwrap();
        db.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)").unwrap();
        let after = storage(&db).unwrap();
        assert!(after.active < before);
        assert!(after.reusable > 0);
        assert!(fs::metadata(fixture.0.join("usage.sqlite3")).unwrap().len() > after.active);
        let next = event(2, "2026-10-01T00:00:00Z");
        let _limits = Override::new(
            after.used + growth(next.to_string().len()).unwrap() + 1,
            Some(u64::MAX),
        );
        insert_event(&db, &next).unwrap();
        assert_eq!(count(&db), 1);
    }
}
