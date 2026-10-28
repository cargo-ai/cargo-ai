//! Explicit local-detail expiry and conservative capture storage limits.
use super::{db_error, disk, private_permissions, safe_path};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
};

const POLICY_KEY: &str = "local_retention_policy_v1";
const CURSOR_KEY: &str = "local_retention_scan_after_v1";
const PASS_LIMIT: usize = 100;
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
            schema_version: 1,
            days: 7,
            enabled: false,
            local_detail_loss_acknowledged: false,
            unselected_loss_acknowledged: false,
        }
    }
}
impl Policy {
    fn validate(&self) -> Result<(), String> {
        if self.schema_version != 1 || !matches!(self.days, 1 | 7 | 30) {
            return Err("Local retention requires schema version 1 and 1, 7 or 30 days".into());
        }
        if self.enabled && !self.local_detail_loss_acknowledged {
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
        json!({"schema_version":1,"policy":next,"configured":true,"deleted_records":0,"upload_consent_changed":false,"local_detail_loss":loss_disclosure()}),
    )
}
#[cfg(any(cargo_ai_cli, test))]
fn configure_at(db: &mut Connection, next: &Policy) -> Result<(), String> {
    next.validate()?;
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error)?;
    tx.execute("INSERT INTO usage_meta(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value", params![POLICY_KEY,serde_json::to_string(next).map_err(|_| "Cannot encode local retention policy")?]).map_err(db_error)?;
    tx.execute("DELETE FROM usage_meta WHERE key=?1", [CURSOR_KEY])
        .map_err(db_error)?;
    tx.commit().map_err(db_error)
}

fn loss_disclosure() -> Value {
    json!({"permanent":true,"full_local_restoration_from_cloud":false,"local_fields_removed":["local labels","local paths","unknown local extensions","other event details excluded from sanitized projections"],"export_command":"usage export","old_backup_acknowledgment_is_new_retained_custody":false,"unselected_loss_acknowledgment_required_without_new_retained_copy":true})
}

fn exists(db: &Connection, table: &str) -> Result<bool, String> {
    db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
        [table],
        |row| row.get(0),
    )
    .map_err(db_error)
}
fn old_protected(db: &Connection, id: &str, sequence: i64) -> Result<bool, String> {
    if exists(db,"usage_backup_queue")? && db.query_row("SELECT EXISTS(SELECT 1 FROM usage_backup_queue WHERE event_id=?1 AND acknowledged=0)", [id], |row|row.get::<_,bool>(0)).map_err(db_error)? { return Ok(true); }
    if exists(db,"usage_backup_windows")? && db.query_row("SELECT EXISTS(SELECT 1 FROM usage_backup_windows WHERE ?1>start_sequence AND ?1>last_scan AND (end_sequence IS NULL OR ?1<=end_sequence))", [sequence], |row|row.get::<_,bool>(0)).map_err(db_error)? { return Ok(true); }
    Ok(false)
}

#[derive(Default)]
struct Scan {
    examined: usize,
    age_eligible: usize,
    custody_blocked: usize,
    unselected_loss_blocked: usize,
    invalid_timestamp: usize,
    eligible: Vec<String>,
    next_after: i64,
    complete: bool,
}
fn scan(db: &Connection, policy: &Policy, now_ms: i64) -> Result<Scan, String> {
    if now_ms < 0 {
        return Err("Local retention time is invalid".into());
    }
    let after: Option<String> = db
        .query_row(
            "SELECT value FROM usage_meta WHERE key=?1",
            [CURSOR_KEY],
            |row| row.get(0),
        )
        .optional()
        .map_err(db_error)?;
    let after = after
        .as_deref()
        .unwrap_or("0")
        .parse::<i64>()
        .ok()
        .filter(|n| *n >= 0)
        .ok_or("Local retention cursor is invalid; history preserved")?;
    let mut statement = db.prepare("SELECT sequence,event_id,created_at FROM usage_events WHERE sequence>?1 ORDER BY sequence LIMIT ?2").map_err(db_error)?;
    let rows = statement
        .query_map(params![after, PASS_LIMIT as i64], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_error)?;
    let cutoff = (i128::from(now_ms) - i128::from(policy.days) * 86_400_000) * 1_000_000;
    let mut scan = Scan {
        examined: rows.len(),
        complete: rows.len() < PASS_LIMIT,
        ..Scan::default()
    };
    for (sequence, id, created) in rows {
        scan.next_after = sequence;
        let Ok(time) =
            time::OffsetDateTime::parse(&created, &time::format_description::well_known::Rfc3339)
        else {
            scan.invalid_timestamp += 1;
            continue;
        };
        if time.unix_timestamp_nanos() >= cutoff {
            continue;
        }
        scan.age_eligible += 1;
        if old_protected(db, &id, sequence)?
            || crate::usage_analytics::queue::protected_event(db, &id, sequence)?
        {
            scan.custody_blocked += 1;
            continue;
        }
        // Old acknowledgments cannot prove a current copy after old-path clear.
        // Only exact new retained custody exempts the explicit local-only loss choice.
        if !policy.unselected_loss_acknowledged
            && !crate::usage_analytics::queue::retained_event(db, &id)?
        {
            scan.unselected_loss_blocked += 1;
            continue;
        }
        scan.eligible.push(id);
    }
    if scan.complete {
        scan.next_after = 0;
    }
    Ok(scan)
}
fn report(policy: &Policy, scan: &Scan, deleted: usize) -> Value {
    json!({"schema_version":1,"policy":policy,"retention_consent_required":!policy.enabled,"local_detail_loss":loss_disclosure(),"scan_limit":PASS_LIMIT,"examined_records":scan.examined,"age_eligible_records":scan.age_eligible,"custody_blocked_records":scan.custody_blocked,"unselected_loss_blocked_records":scan.unselected_loss_blocked,"invalid_timestamp_records":scan.invalid_timestamp,"eligible_records":if policy.enabled {scan.eligible.len()} else {0},"deleted_records":deleted,"scan_complete":scan.complete,"next_scan_after":scan.next_after.to_string(),"counts_scope":"this bounded scan; not whole history"})
}

/// Read-only views never initialize upload tables, migrate, expire or advance cursors.
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
    Ok(report(&policy, &scan(db, &policy, now_ms)?, 0))
}

/// One bounded operation; a fresh write transaction rechecks policy and both custodians.
pub(crate) fn apply(now_ms: i64) -> Result<Value, String> {
    if now_ms < 0 {
        return Err("Local retention time is invalid".into());
    }
    // Active-operation catch-up must remain passive until this Home has made
    // its explicit retention choice, including when usage tracking is off.
    let Some(reader) = super::open_database(false)? else {
        return Ok(report(&Policy::default(), &Scan::default(), 0));
    };
    let selected = policy(&reader)?;
    if !selected.enabled {
        return Ok(report(&selected, &Scan::default(), 0));
    }
    drop(reader);
    let mut db = super::open_database(true)?.ok_or("Usage database unavailable")?;
    apply_at(&mut db, now_ms)
}
fn apply_at(db: &mut Connection, now_ms: i64) -> Result<Value, String> {
    if now_ms < 0 {
        return Err("Local retention time is invalid".into());
    }
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error)?;
    let policy = policy(&tx)?;
    if !policy.enabled {
        return Ok(report(&policy, &Scan::default(), 0));
    }
    let scan = scan(&tx, &policy, now_ms)?;
    let mut deleted = 0;
    for id in &scan.eligible {
        let sequence: Option<i64> = tx
            .query_row(
                "SELECT sequence FROM usage_events WHERE event_id=?1",
                [id],
                |row| row.get(0),
            )
            .optional()
            .map_err(db_error)?;
        if let Some(sequence) = sequence {
            if old_protected(&tx, id, sequence)?
                || crate::usage_analytics::queue::protected_event(&tx, id, sequence)?
            {
                return Err("Usage custody changed; local history preserved".into());
            }
            deleted += tx
                .execute("DELETE FROM usage_events WHERE event_id=?1", [id])
                .map_err(db_error)?;
        }
    }
    tx.execute("INSERT INTO usage_meta(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",params![CURSOR_KEY,scan.next_after.to_string()]).map_err(db_error)?;
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
        "storage_budget" | "free_space_reserve" | "free_space_unavailable"
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
    #[test]
    fn explicit_policy_preserves_upgraded_history_and_requires_both_loss_choices() {
        let mut db = db();
        insert_event(&db, &event(1, "2026-10-01T00:00:00Z")).unwrap();
        assert_eq!(policy(&db).unwrap(), Policy::default());
        assert_eq!(apply_at(&mut db, NOW).unwrap()["deleted_records"], 0);
        assert_eq!(count(&db), 1);
        let mut rejected = enabled(false);
        rejected.local_detail_loss_acknowledged = false;
        assert!(configure_at(&mut db, &rejected).is_err());
        assert!(configure_at(
            &mut db,
            &Policy {
                days: 0,
                ..enabled(true)
            }
        )
        .is_err());
        for days in [1, 7, 30] {
            configure_at(
                &mut db,
                &Policy {
                    days,
                    ..enabled(false)
                },
            )
            .unwrap();
        }
        configure_at(&mut db, &enabled(false)).unwrap();
        let preview = preview_at(&db, NOW).unwrap();
        assert_eq!(preview["unselected_loss_blocked_records"], 1);
        assert_eq!(
            preview["local_detail_loss"]["full_local_restoration_from_cloud"],
            false
        );
        assert_eq!(preview["eligible_records"], 0);
        assert_eq!(apply_at(&mut db, NOW).unwrap()["deleted_records"], 0);
        configure_at(&mut db, &enabled(true)).unwrap();
        assert_eq!(apply_at(&mut db, NOW).unwrap()["deleted_records"], 1);
    }
    #[test]
    fn strict_age_boundary_invalid_dates_and_preview_are_nonmutating() {
        let mut db = db();
        for (index, stamp) in [
            (1, "2026-10-20T23:59:59.999999999Z"),
            (2, "2026-10-21T00:00:00Z"),
            (3, "2026-10-21T00:00:00.000000001Z"),
            (4, "2020-01-01"),
        ] {
            insert_event(&db, &event(index, stamp)).unwrap();
        }
        configure_at(&mut db, &enabled(true)).unwrap();
        let before: i64 = db
            .query_row("SELECT COUNT(*) FROM sqlite_master", [], |r| r.get(0))
            .unwrap();
        let preview = preview_at(&db, NOW).unwrap();
        assert_eq!(preview["eligible_records"], 1);
        assert_eq!(preview["invalid_timestamp_records"], 1);
        assert_eq!(count(&db), 4);
        assert_eq!(
            db.query_row(
                "SELECT COUNT(*) FROM usage_meta WHERE key=?1",
                [CURSOR_KEY],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
        assert_eq!(
            db.query_row("SELECT COUNT(*) FROM sqlite_master", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            before
        );
        assert!(!exists(&db, "usage_cloud_state").unwrap());
        assert!(!exists(&db, "usage_backup_windows").unwrap());
        assert_eq!(apply_at(&mut db, NOW).unwrap()["deleted_records"], 1);
        assert_eq!(count(&db), 3);
    }
    #[test]
    fn closed_unscanned_windows_and_disabled_pending_queues_cannot_be_waived() {
        let mut db = db();
        old_queue(&db);
        for i in 1..=251 {
            insert_event(&db, &event(i, "2026-10-01T00:00:00Z")).unwrap();
        }
        db.execute(
            "INSERT INTO usage_backup_windows VALUES(1,'disabled-account',1,0,100,0)",
            [],
        )
        .unwrap();
        db.execute(
            "INSERT INTO usage_backup_queue VALUES(?1,'other-account',2,0)",
            [id(1)],
        )
        .unwrap();
        configure_at(&mut db, &enabled(true)).unwrap();
        let first = apply_at(&mut db, NOW).unwrap();
        assert_eq!(first["examined_records"], 100);
        assert_eq!(first["custody_blocked_records"], 100);
        assert_eq!(first["deleted_records"], 0);
        let second = apply_at(&mut db, NOW).unwrap();
        assert_eq!(second["deleted_records"], 100);
        let third = apply_at(&mut db, NOW).unwrap();
        assert_eq!(third["deleted_records"], 51);
        assert_eq!(count(&db), 100);
        assert_eq!(third["next_scan_after"], "0");
    }
    #[test]
    fn apply_rechecks_new_custody_after_preview_and_partial_schema_fails_closed() {
        let mut db = db();
        insert_event(&db, &event(1, "2026-10-01T00:00:00Z")).unwrap();
        configure_at(&mut db, &enabled(true)).unwrap();
        assert_eq!(preview_at(&db, NOW).unwrap()["eligible_records"], 1);
        old_queue(&db);
        db.execute(
            "INSERT INTO usage_backup_queue VALUES(?1,'new-obligation',1,0)",
            [id(1)],
        )
        .unwrap();
        assert_eq!(apply_at(&mut db, NOW).unwrap()["deleted_records"], 0);
        assert_eq!(count(&db), 1);
        db.execute("UPDATE usage_backup_queue SET acknowledged=1", [])
            .unwrap();
        db.execute("CREATE TABLE usage_cloud_state(incomplete TEXT)", [])
            .unwrap();
        assert!(apply_at(&mut db, NOW).is_err());
        assert_eq!(count(&db), 1);
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
        Ok(json!({"schema_version":1,"normalization_version":1,"event_id":event["event_id"],"root_run_id":id(9001),"agent_run_id":id(9002),"event_type":"provider_request","event_kind":"provider_request_completed","occurred_at_ms":1,"tokens":{"input_tokens":5,"output_tokens":null}}).to_string())
    }
    fn retain(db: &mut Connection) {
        let lease = queue::acquire_lease(db, &source(), 100, queue::MAX_LEASE_MS)
            .unwrap()
            .unwrap();
        let batch = queue::prepare_batch(db, &lease, 100).unwrap().unwrap();
        let admission = AdmissionCommitment {
            protocol_version: Version,
            source: batch.source().clone(),
            provenance: batch.provenance().clone(),
            batch_id: batch.id().clone(),
            ordered_membership_sha256: batch.membership().clone(),
            manifest_sha256: Sha256Hash::of(b"synthetic exact authenticated manifest"),
        };
        queue::begin_attempt(db, &lease, batch.id(), 100).unwrap();
        queue::record_admission(db, &lease, batch.id(), &admission, 100).unwrap();
        let receipt = Receipt {
            protocol_version: Version,
            source: admission.source,
            provenance: admission.provenance,
            batch_id: admission.batch_id,
            manifest_sha256: admission.manifest_sha256,
            ordered_membership_sha256: admission.ordered_membership_sha256,
            custody: Custody::Retained,
            members: batch.members().to_vec(),
        };
        queue::acknowledge_retained(db, &lease, batch.id(), &receipt, 100).unwrap();
        queue::release_lease(db, &lease, 100).unwrap();
    }
    #[test]
    fn exact_new_receipt_frees_only_new_custody_and_disable_preserves_both_obligations() {
        let mut db = db();
        old_queue(&db);
        queue::initialize(&mut db).unwrap();
        queue::enable(&mut db, &source(), &provenance(), 0).unwrap();
        insert_event(&db, &event(1, "2026-10-01T00:00:00Z")).unwrap();
        configure_at(&mut db, &enabled(false)).unwrap();
        // A future interval is an obligation even before projection/scanning.
        assert_eq!(
            apply_at(&mut db, NOW).unwrap()["custody_blocked_records"],
            1
        );
        queue::capture(&mut db, &source(), projection).unwrap();
        db.execute(
            "INSERT INTO usage_backup_queue VALUES(?1,'old-account',1,0)",
            [id(1)],
        )
        .unwrap();
        retain(&mut db);
        queue::disable(&mut db, &source()).unwrap();
        assert_eq!(
            apply_at(&mut db, NOW).unwrap()["custody_blocked_records"],
            1
        );
        db.execute("UPDATE usage_backup_queue SET acknowledged=1", [])
            .unwrap();
        assert_eq!(apply_at(&mut db, NOW).unwrap()["deleted_records"], 1);
        assert_eq!(count(&db), 0);
        assert!(queue::retained_event(&db, &id(1)).unwrap());
    }
    #[test]
    fn old_ack_is_not_current_new_copy_and_new_quarantine_stays_protected() {
        let mut db = db();
        old_queue(&db);
        queue::initialize(&mut db).unwrap();
        queue::enable(&mut db, &source(), &provenance(), 0).unwrap();
        insert_event(&db, &event(1, "2026-10-01T00:00:00Z")).unwrap();
        queue::capture(&mut db, &source(), projection).unwrap();
        queue::quarantine_event(&mut db, &source(), &id(1)).unwrap();
        queue::disable(&mut db, &source()).unwrap();
        db.execute(
            "INSERT INTO usage_backup_queue VALUES(?1,'old-account',1,1)",
            [id(1)],
        )
        .unwrap();
        configure_at(&mut db, &enabled(true)).unwrap();
        assert_eq!(
            apply_at(&mut db, NOW).unwrap()["custody_blocked_records"],
            1
        );
        let mut old_only = super::tests::db();
        old_queue(&old_only);
        insert_event(&old_only, &event(2, "2026-10-01T00:00:00Z")).unwrap();
        old_only
            .execute(
                "INSERT INTO usage_backup_queue VALUES(?1,'old-account',1,1)",
                [id(2)],
            )
            .unwrap();
        configure_at(&mut old_only, &enabled(false)).unwrap();
        assert_eq!(
            apply_at(&mut old_only, NOW).unwrap()["unselected_loss_blocked_records"],
            1
        );
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
        assert_eq!(count(&db), 2);
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
    fn cleared_new_copy_requires_local_only_loss_choice_again() {
        let mut db = db();
        queue::initialize(&mut db).unwrap();
        queue::enable(&mut db, &source(), &provenance(), 0).unwrap();
        insert_event(&db, &event(1, "2026-10-01T00:00:00Z")).unwrap();
        queue::capture(&mut db, &source(), projection).unwrap();
        retain(&mut db);
        queue::mark_cleared(&mut db, &source()).unwrap();
        configure_at(&mut db, &enabled(false)).unwrap();
        assert_eq!(
            apply_at(&mut db, NOW).unwrap()["unselected_loss_blocked_records"],
            1
        );
        assert_eq!(count(&db), 1);
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
