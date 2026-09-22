//! Device-local, immutable usage facts shared by the CLI and generated agents.
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

pub(crate) const SCHEMA_VERSION: u32 = 1;
pub(crate) const TRACKING_ENV: &str = "CARGO_AI_USAGE_TRACKING";

pub(crate) fn directory() -> PathBuf {
    crate::config::loader::config_path()
        .parent()
        .unwrap_or(Path::new("."))
        .join("usage")
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub(crate) struct UsageSettings {
    pub(crate) tracking: bool,
    pub(crate) backup_enabled: bool,
    pub(crate) backup_account_binding: Option<String>,
    pub(crate) backup_generation: Option<u64>,
}
impl Default for UsageSettings {
    fn default() -> Self {
        Self {
            tracking: true,
            backup_enabled: false,
            backup_account_binding: None,
            backup_generation: None,
        }
    }
}

pub(crate) fn settings() -> Result<UsageSettings, String> {
    let path = crate::config::loader::config_path();
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(UsageSettings::default())
        }
        Err(_) => {
            return Err("Cannot read usage settings; configuration was left unchanged".into())
        }
    };
    let config: toml::Value = toml::from_str(&text)
        .map_err(|_| "Cannot parse usage settings; configuration was left unchanged")?;
    match config.get("usage") {
        None => Ok(UsageSettings::default()),
        Some(value) => value
            .clone()
            .try_into()
            .map_err(|_| "Usage settings are invalid".into()),
    }
}

pub(crate) fn tracking_enabled() -> Result<bool, String> {
    if let Ok(value) = std::env::var(TRACKING_ENV) {
        return match value.trim().to_ascii_lowercase().as_str() {
            "off" | "false" | "0" => Ok(false),
            "on" | "true" | "1" => Ok(true),
            _ => Err("CARGO_AI_USAGE_TRACKING must be on or off".into()),
        };
    }
    Ok(settings()?.tracking)
}

fn safe_path(path: &Path, directory: bool) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(meta)
            if meta.file_type().is_symlink()
                || (directory && !meta.is_dir())
                || (!directory && !meta.is_file()) =>
        {
            Err(
                "Usage storage requires regular files in a local directory; symlinks are refused"
                    .into(),
            )
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err("Cannot inspect usage storage".into()),
    }
}

fn private_permissions(path: &Path, directory: bool) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            path,
            fs::Permissions::from_mode(if directory { 0o700 } else { 0o600 }),
        )
        .map_err(|_| "Cannot secure usage storage permissions")?;
    }
    #[cfg(not(unix))]
    let _ = (path, directory);
    Ok(())
}

pub(crate) fn open_database(create: bool) -> Result<Option<Connection>, String> {
    open_at(&directory(), create)
}

fn open_at(dir: &Path, create: bool) -> Result<Option<Connection>, String> {
    let path = dir.join("usage.sqlite3");
    safe_path(dir, true)?;
    for suffix in ["", "-wal", "-shm"] {
        safe_path(&dir.join(format!("usage.sqlite3{suffix}")), false)?;
    }
    if !create && !path.exists() {
        return Ok(None);
    }
    if create {
        fs::create_dir_all(dir).map_err(|_| "Cannot create usage storage directory")?;
        private_permissions(dir, true)?;
    }
    // Resolve platform aliases such as macOS /var without following a usage
    // directory or database link: those owned entries were checked above.
    let path = fs::canonicalize(dir)
        .map_err(|_| "Cannot resolve usage storage directory")?
        .join("usage.sqlite3");
    let flags = if create {
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE
    } else {
        OpenFlags::SQLITE_OPEN_READ_ONLY
    };
    let mut connection =
        Connection::open_with_flags(&path, flags | OpenFlags::SQLITE_OPEN_NOFOLLOW)
            .map_err(db_error)?;
    connection
        .pragma_update(None, "foreign_keys", true)
        .map_err(db_error)?;
    connection
        .busy_timeout(Duration::from_millis(250))
        .map_err(db_error)?;
    let version: u32 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(db_error)?;
    if version > SCHEMA_VERSION {
        return Err("Usage database belongs to a newer incompatible schema; left unchanged".into());
    }
    if create {
        // Concurrent first openers may return SQLITE_BUSY from journal-mode
        // activation without invoking SQLite's busy handler. Retry only this
        // transition within the same bounded wait used for ordinary writes.
        connection.busy_timeout(Duration::ZERO).map_err(db_error)?;
        let deadline = std::time::Instant::now() + Duration::from_millis(250);
        loop {
            match connection.pragma_update(None, "journal_mode", "WAL") {
                Ok(()) => break,
                Err(rusqlite::Error::SqliteFailure(error, _))
                    if matches!(
                        error.code,
                        rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
                    ) && std::time::Instant::now() < deadline =>
                {
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(error) => return Err(db_error(error)),
            }
        }
        connection
            .busy_timeout(Duration::from_millis(250))
            .map_err(db_error)?;
        connection
            .pragma_update(None, "synchronous", "FULL")
            .map_err(db_error)?;
        connection
            .pragma_update(None, "wal_autocheckpoint", 100)
            .map_err(db_error)?;
        if version == 0 {
            let transaction = connection
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                .map_err(db_error)?;
            let locked_version: u32 = transaction
                .pragma_query_value(None, "user_version", |row| row.get(0))
                .map_err(db_error)?;
            if locked_version == 0 {
                transaction.execute_batch("CREATE TABLE IF NOT EXISTS usage_events(sequence INTEGER PRIMARY KEY AUTOINCREMENT,event_id TEXT UNIQUE NOT NULL,root_run_id TEXT,agent_run_id TEXT,event_type TEXT NOT NULL,created_at TEXT NOT NULL,record_json TEXT NOT NULL); CREATE INDEX IF NOT EXISTS usage_events_root ON usage_events(root_run_id,sequence); CREATE INDEX IF NOT EXISTS usage_events_agent ON usage_events(agent_run_id,sequence); CREATE TABLE IF NOT EXISTS usage_meta(key TEXT PRIMARY KEY,value TEXT NOT NULL); PRAGMA user_version=1;").map_err(db_error)?;
            } else if locked_version != SCHEMA_VERSION {
                return Err("Usage database schema changed; left unchanged".into());
            }
            transaction.commit().map_err(db_error)?;
        }
        private_permissions(&path, false)?;
    } else if version != SCHEMA_VERSION {
        return Err("Usage database requires migration by a current runtime before reading".into());
    }
    Ok(Some(connection))
}

pub(crate) fn db_error(error: rusqlite::Error) -> String {
    format!("Usage database operation failed: {error}")
}

pub(crate) fn insert_event(connection: &Connection, event: &Value) -> Result<bool, String> {
    let required = |key| {
        event
            .get(key)
            .and_then(Value::as_str)
            .ok_or_else(|| format!("Usage record missing {key}"))
    };
    let event_id = required("event_id")?;
    let encoded = serde_json::to_string(event).map_err(|_| "Cannot encode usage record")?;
    let existing: Option<String> = connection
        .query_row(
            "SELECT record_json FROM usage_events WHERE event_id=?1",
            [event_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(db_error)?;
    if let Some(existing) = existing {
        return if existing == encoded {
            Ok(false)
        } else {
            Err("Usage event identity conflict; original fact preserved".into())
        };
    }
    let changed = connection.execute("INSERT INTO usage_events(event_id,root_run_id,agent_run_id,event_type,created_at,record_json) VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(event_id) DO NOTHING", params![event_id,event["root_run_id"].as_str(),event["agent_run_id"].as_str(),required("event_type")?,required("timestamp")?,encoded]).map_err(db_error)?;
    if changed == 0 {
        let existing: String = connection
            .query_row(
                "SELECT record_json FROM usage_events WHERE event_id=?1",
                [event_id],
                |row| row.get(0),
            )
            .map_err(db_error)?;
        if existing != encoded {
            return Err("Usage event identity conflict; original fact preserved".into());
        }
    }
    Ok(changed != 0)
}

pub(crate) fn persist(event: &Value) -> Result<(), String> {
    let connection = open_database(true)?.ok_or("Usage database unavailable")?;
    insert_event(&connection, event)?;
    Ok(())
}

pub(crate) fn mark_incomplete() {
    // A fixed-size marker is coverage evidence, never a second event store.
    let dir = directory();
    let marker = dir.join("capture-incomplete");
    if safe_path(&dir, true).is_ok() && safe_path(&marker, false).is_ok() && dir.is_dir() {
        let _ = fs::write(
            &marker,
            b"Some usage facts could not be persisted. Totals may be incomplete.\n",
        );
        let _ = private_permissions(&marker, false);
    }
}

pub(crate) fn incomplete() -> bool {
    directory().join("capture-incomplete").exists()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn fixture() -> PathBuf {
        std::env::temp_dir().join(format!("usage-store-{}", uuid::Uuid::now_v7()))
    }
    fn event(id: &str) -> Value {
        json!({"event_id":id,"root_run_id":"root","event_type":"provider_request_completed","timestamp":"2026-09-22T00:00:00Z","usage":{"input_tokens":u64::MAX}})
    }
    #[test]
    fn immutable_facts_survive_reopen_and_conflicts() {
        let dir = fixture();
        assert!(open_at(&dir, false).unwrap().is_none());
        assert!(!dir.exists());
        let db = open_at(&dir, true).unwrap().unwrap();
        assert!(insert_event(&db, &event("one")).unwrap());
        assert!(!insert_event(&db, &event("one")).unwrap());
        let mut changed = event("one");
        changed["usage"] = Value::Null;
        assert!(insert_event(&db, &changed).is_err());
        drop(db);
        let db = open_at(&dir, false).unwrap().unwrap();
        let stored: String = db
            .query_row("SELECT record_json FROM usage_events", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&stored).unwrap(),
            event("one")
        );
        drop(db);
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn future_schema_is_refused_without_writes() {
        let dir = fixture();
        let db = open_at(&dir, true).unwrap().unwrap();
        db.pragma_update(None, "user_version", 99).unwrap();
        drop(db);
        assert!(open_at(&dir, true).is_err());
        let db = Connection::open(dir.join("usage.sqlite3")).unwrap();
        assert_eq!(
            db.pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))
                .unwrap(),
            99
        );
        drop(db);
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn concurrent_writers_and_reader_preserve_counts() {
        let dir = fixture();
        drop(open_at(&dir, true).unwrap());
        let threads: Vec<_> = (0..8)
            .map(|i| {
                let dir = dir.clone();
                std::thread::spawn(move || {
                    let db = open_at(&dir, true).unwrap().unwrap();
                    for j in 0..20 {
                        insert_event(&db, &event(&format!("{i}-{j}"))).unwrap();
                    }
                })
            })
            .collect();
        let read = open_at(&dir, false).unwrap().unwrap();
        let _: i64 = read
            .query_row("SELECT COUNT(*) FROM usage_events", [], |r| r.get(0))
            .unwrap();
        for thread in threads {
            thread.join().unwrap();
        }
        assert_eq!(
            read.query_row("SELECT COUNT(*) FROM usage_events", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            160
        );
        drop(read);
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn busy_write_is_bounded_and_rollback_preserves_data() {
        let dir = fixture();
        let mut db = open_at(&dir, true).unwrap().unwrap();
        insert_event(&db, &event("saved")).unwrap();
        let other = open_at(&dir, true).unwrap().unwrap();
        let transaction = db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .unwrap();
        transaction.execute("DELETE FROM usage_events", []).unwrap();
        let start = std::time::Instant::now();
        assert!(insert_event(&other, &event("blocked")).is_err());
        assert!(start.elapsed() < Duration::from_secs(2));
        drop(transaction);
        assert_eq!(
            other
                .query_row("SELECT COUNT(*) FROM usage_events", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
        drop(other);
        drop(db);
        fs::remove_dir_all(dir).unwrap();
    }
}

#[cfg(test)]
mod failure_tests {
    use super::*;
    use serde_json::json;
    fn fixture() -> PathBuf {
        std::env::temp_dir().join(format!("usage-failure-{}", uuid::Uuid::now_v7()))
    }
    #[test]
    fn concurrent_first_use_initializes_one_compatible_schema() {
        let dir = fixture();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
        let threads:Vec<_>=(0..8).map(|index| {let dir=dir.clone();let barrier=barrier.clone();std::thread::spawn(move || {barrier.wait();let db=open_at(&dir,true).unwrap().unwrap();insert_event(&db,&json!({"event_id":format!("event-{index}"),"event_type":"agent_run_started","timestamp":"2026-09-22T00:00:00Z"})).unwrap();})}).collect();
        for thread in threads {
            thread.join().unwrap();
        }
        let db = open_at(&dir, false).unwrap().unwrap();
        assert_eq!(
            db.query_row("SELECT COUNT(*) FROM usage_events", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            8
        );
        assert_eq!(
            db.pragma_query_value(None, "journal_mode", |row| row.get::<_, String>(0))
                .unwrap(),
            "wal"
        );
        drop(db);
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn malformed_database_is_not_replaced() {
        let dir = fixture();
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("usage.sqlite3");
        fs::write(&path, b"preserve invalid database for recovery").unwrap();
        assert!(open_at(&dir, true).is_err());
        assert!(open_at(&dir, false).is_err());
        assert_eq!(
            fs::read(&path).unwrap(),
            b"preserve invalid database for recovery"
        );
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn full_database_rejects_write_and_preserves_prior_commit() {
        let dir = fixture();
        let db = open_at(&dir, true).unwrap().unwrap();
        let first = json!({"event_id":"first","event_type":"agent_run_started","timestamp":"2026-09-22T00:00:00Z"});
        insert_event(&db, &first).unwrap();
        let pages: i64 = db
            .pragma_query_value(None, "page_count", |row| row.get(0))
            .unwrap();
        db.pragma_update(None, "max_page_count", pages).unwrap();
        let large = json!({"event_id":"large","event_type":"agent_run_started","timestamp":"2026-09-22T00:00:00Z","agent":{"name":"x".repeat(65536)}});
        assert!(insert_event(&db, &large).is_err());
        assert_eq!(
            db.query_row("SELECT COUNT(*) FROM usage_events", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
        drop(db);
        fs::remove_dir_all(dir).unwrap();
    }
    #[cfg(unix)]
    #[test]
    fn symlink_database_is_refused_without_touching_target() {
        let dir = fixture();
        fs::create_dir_all(&dir).unwrap();
        let target = dir.join("preserved");
        fs::write(&target, b"unchanged").unwrap();
        std::os::unix::fs::symlink(&target, dir.join("usage.sqlite3")).unwrap();
        assert!(open_at(&dir, true).is_err());
        assert!(open_at(&dir, false).is_err());
        assert_eq!(fs::read(target).unwrap(), b"unchanged");
        fs::remove_dir_all(dir).unwrap();
    }
    #[cfg(unix)]
    #[test]
    fn storage_has_private_permissions_and_readonly_writes_fail() {
        use std::os::unix::fs::PermissionsExt;
        let dir = fixture();
        let db = open_at(&dir, true).unwrap().unwrap();
        let path = dir.join("usage.sqlite3");
        assert_eq!(
            fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        drop(db);
        let db = open_at(&dir, false).unwrap().unwrap();
        assert!(insert_event(&db,&json!({"event_id":"read-only","event_type":"agent_run_started","timestamp":"2026-09-22T00:00:00Z"})).is_err());
        drop(db);
        fs::remove_dir_all(dir).unwrap();
    }
}
