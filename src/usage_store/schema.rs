//! Ordered transactional normalization and private recovery snapshots.
use super::{codec, db_error, disk, private_permissions, safe_path, SCHEMA_VERSION};
use rusqlite::{params, Connection, OpenFlags, OptionalExtension, TransactionBehavior};
use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

const V1_COLUMNS: &[(&str, &str, bool, i64)] = &[
    ("sequence", "INTEGER", false, 1),
    ("event_id", "TEXT", true, 0),
    ("root_run_id", "TEXT", false, 0),
    ("agent_run_id", "TEXT", false, 0),
    ("event_type", "TEXT", true, 0),
    ("created_at", "TEXT", true, 0),
    ("record_json", "TEXT", true, 0),
];

pub(super) fn validate_v1(db: &Connection) -> Result<(), String> {
    let mut statement = db
        .prepare("PRAGMA table_info(usage_events)")
        .map_err(db_error)?;
    let columns = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, bool>(3)?,
                row.get::<_, i64>(5)?,
            ))
        })
        .map_err(db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_error)?;
    if columns.len() != V1_COLUMNS.len()
        || columns.iter().zip(V1_COLUMNS).any(|(actual, expected)| {
            (actual.0.as_str(), actual.1.as_str(), actual.2, actual.3) != *expected
        })
    {
        return Err(
            "Usage database has an unexpected legacy structure; source left unchanged".into(),
        );
    }
    let ddl: String = db
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type='table' AND name='usage_events'",
            [],
            |row| row.get(0),
        )
        .map_err(db_error)?;
    let unique: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM pragma_index_list('usage_events') AS i WHERE i.[unique]=1 AND (SELECT COUNT(*) FROM pragma_index_info(i.name))=1 AND (SELECT name FROM pragma_index_info(i.name))='event_id')", [], |row| row.get(0)).map_err(db_error)?;
    let meta: Option<String> = db
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type='table' AND name='usage_meta'",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(db_error)?;
    if !ddl.to_ascii_uppercase().contains("AUTOINCREMENT") || !unique || meta.is_none() {
        return Err("Usage database legacy identity or metadata structure is incompatible; source left unchanged".into());
    }
    let meta_columns: i64 = db.query_row("SELECT COUNT(*) FROM pragma_table_info('usage_meta') WHERE (name='key' AND type='TEXT' AND pk=1) OR (name='value' AND type='TEXT' AND [notnull]=1)", [], |row| row.get(0)).map_err(db_error)?;
    let meta_total: i64 = db
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('usage_meta')",
            [],
            |row| row.get(0),
        )
        .map_err(db_error)?;
    if meta_columns != 2 || meta_total != 2 {
        return Err(
            "Usage database metadata structure is incompatible; source left unchanged".into(),
        );
    }
    let unexpected_triggers: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='trigger' AND tbl_name='usage_events')", [], |row| row.get(0)).map_err(db_error)?;
    if unexpected_triggers {
        return Err("Usage database has unexpected event triggers; source left unchanged".into());
    }
    Ok(())
}

pub(super) fn initialize(db: &Connection) -> Result<(), String> {
    db.execute_batch("CREATE TABLE usage_events(sequence INTEGER PRIMARY KEY AUTOINCREMENT,event_id TEXT UNIQUE NOT NULL,root_run_id TEXT,agent_run_id TEXT,event_type TEXT NOT NULL,created_at TEXT NOT NULL,envelope_json TEXT NOT NULL,residual_json TEXT NOT NULL DEFAULT '{}',codec_version INTEGER NOT NULL DEFAULT 1,agent_context_id INTEGER REFERENCES usage_contexts(context_id),provider_context_id INTEGER REFERENCES usage_contexts(context_id),attribution_context_id INTEGER REFERENCES usage_contexts(context_id),restored_attribution_context_id INTEGER REFERENCES usage_contexts(context_id),opaque_json TEXT);
        CREATE INDEX usage_events_root ON usage_events(root_run_id,sequence);
        CREATE INDEX usage_events_agent ON usage_events(agent_run_id,sequence);
        CREATE TABLE usage_meta(key TEXT PRIMARY KEY,value TEXT NOT NULL);").map_err(db_error)?;
    install(db)?;
    db.pragma_update(None, "user_version", SCHEMA_VERSION)
        .map_err(db_error)
}

fn install(db: &Connection) -> Result<(), String> {
    db.execute_batch("CREATE TABLE IF NOT EXISTS usage_contexts(context_id INTEGER PRIMARY KEY,codec_version INTEGER NOT NULL,kind TEXT NOT NULL,canonical_json TEXT NOT NULL,UNIQUE(codec_version,kind,canonical_json));
        CREATE TRIGGER IF NOT EXISTS usage_contexts_immutable BEFORE UPDATE ON usage_contexts BEGIN SELECT RAISE(ABORT,'usage contexts are immutable'); END;
        CREATE TRIGGER IF NOT EXISTS usage_events_immutable BEFORE UPDATE ON usage_events BEGIN SELECT RAISE(ABORT,'usage facts are immutable'); END;").map_err(db_error)?;
    db.execute_batch("CREATE INDEX IF NOT EXISTS usage_events_attribution_environment ON usage_contexts(json_extract(canonical_json,'$.environment.id')) WHERE kind='attribution';
        CREATE INDEX IF NOT EXISTS usage_events_attribution_package ON usage_contexts(json_extract(canonical_json,'$.package.source'),json_extract(canonical_json,'$.package.id')) WHERE kind='attribution';
        CREATE INDEX IF NOT EXISTS usage_events_attribution_location ON usage_contexts(json_extract(canonical_json,'$.environment.id'),json_extract(canonical_json,'$.package_location.path')) WHERE kind='attribution';
        CREATE INDEX IF NOT EXISTS usage_events_attribution_agent ON usage_contexts(json_extract(canonical_json,'$.agent.source'),json_extract(canonical_json,'$.agent.id')) WHERE kind='attribution';
        CREATE INDEX IF NOT EXISTS usage_events_attribution_workspace ON usage_contexts(json_extract(canonical_json,'$.workspace.environment_id'),json_extract(canonical_json,'$.workspace.path')) WHERE kind='attribution';").map_err(db_error)?;
    db.execute_batch("CREATE INDEX IF NOT EXISTS usage_events_context_agent ON usage_events(agent_context_id);
        CREATE INDEX IF NOT EXISTS usage_events_context_provider ON usage_events(provider_context_id);
        CREATE INDEX IF NOT EXISTS usage_events_context_attribution ON usage_events(attribution_context_id);
        CREATE INDEX IF NOT EXISTS usage_events_context_restored ON usage_events(restored_attribution_context_id);
        CREATE TRIGGER IF NOT EXISTS usage_events_context_cleanup AFTER DELETE ON usage_events BEGIN
          DELETE FROM usage_contexts WHERE context_id IN (OLD.agent_context_id,OLD.provider_context_id,OLD.attribution_context_id,OLD.restored_attribution_context_id)
            AND NOT EXISTS(SELECT 1 FROM usage_events WHERE agent_context_id=usage_contexts.context_id OR provider_context_id=usage_contexts.context_id OR attribution_context_id=usage_contexts.context_id OR restored_attribution_context_id=usage_contexts.context_id);
        END;").map_err(db_error)?;
    db.execute_batch(&codec::view_sql()).map_err(db_error)?;
    Ok(())
}

pub(super) fn context_ids(
    db: &Connection,
    encoded: &codec::Encoded,
) -> Result<[Option<i64>; 4], String> {
    let mut ids = [None; 4];
    for (index, value) in encoded.contexts.iter().enumerate() {
        if let Some(value) = value {
            db.execute("INSERT INTO usage_contexts(codec_version,kind,canonical_json) VALUES(?1,?2,?3) ON CONFLICT(codec_version,kind,canonical_json) DO NOTHING", params![codec::VERSION, codec::CONTEXTS[index], value]).map_err(db_error)?;
            ids[index] = Some(db.query_row("SELECT context_id FROM usage_contexts WHERE codec_version=?1 AND kind=?2 AND canonical_json=?3 COLLATE BINARY", params![codec::VERSION, codec::CONTEXTS[index], value], |row| row.get(0)).map_err(db_error)?);
        }
    }
    Ok(ids)
}

fn headroom(db: &Connection, available: u64) -> Result<(), String> {
    let pages = u64::try_from(
        db.pragma_query_value(None, "page_count", |row| row.get::<_, i64>(0))
            .map_err(db_error)?,
    )
    .map_err(|_| "Usage database page count is invalid")?;
    let size = u64::try_from(
        db.pragma_query_value(None, "page_size", |row| row.get::<_, i64>(0))
            .map_err(db_error)?,
    )
    .map_err(|_| "Usage database page size is invalid")?;
    // Reserve the snapshot, rewritten pages, WAL and context/index growth, plus
    // the same 256MiB application reserve used by capture policy.
    let required = pages
        .checked_mul(size)
        .and_then(|n| n.checked_mul(6))
        .and_then(|n| n.checked_add(256 * 1024 * 1024))
        .ok_or("Usage migration size overflowed")?;
    if available < required {
        return Err(format!("Usage migration requires {required} free bytes; {available} available; source left unchanged"));
    }
    Ok(())
}

fn recovery_image(path: &Path) -> Result<PathBuf, String> {
    let dir = path
        .parent()
        .ok_or("Usage database path has no parent")?
        .join("recovery");
    safe_path(&dir, true)?;
    fs::create_dir_all(&dir).map_err(|_| "Cannot create private usage recovery directory")?;
    private_permissions(&dir, true)?;
    let image = dir.join(format!("usage-v1-{}.sqlite3", uuid::Uuid::new_v4()));
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options
        .open(&image)
        .map_err(|_| "Cannot reserve usage recovery image")?;
    private_permissions(&image, false)?;
    drop(file);
    // VACUUM INTO reads one committed SQLite snapshot, including WAL. The
    // caller holds the writer lock throughout, so no old writer can advance it.
    let source = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .map_err(db_error)?;
    source
        .busy_timeout(Duration::from_millis(250))
        .map_err(db_error)?;
    source
        .execute(
            "VACUUM INTO ?1",
            [image.to_str().ok_or("Usage recovery path is not UTF-8")?],
        )
        .map_err(db_error)?;
    let image_db = Connection::open_with_flags(
        &image,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .map_err(db_error)?;
    let check: String = image_db
        .query_row("PRAGMA quick_check", [], |row| row.get(0))
        .map_err(db_error)?;
    if check != "ok" {
        return Err("Usage recovery image integrity check failed; source left unchanged".into());
    }
    drop(image_db);
    // Windows requires a writable handle to flush file buffers. Reopen the
    // existing verified image without creating or truncating its contents.
    fs::OpenOptions::new()
        .write(true)
        .open(&image)
        .and_then(|file| file.sync_all())
        .map_err(|_| "Cannot sync usage recovery image")?;
    // The directory entry must also survive a crash before schema commit.
    #[cfg(unix)]
    fs::File::open(&dir)
        .and_then(|file| file.sync_all())
        .map_err(|_| "Cannot sync usage recovery directory")?;
    Ok(image)
}

pub(super) fn preflight(db: &Connection, path: &Path) -> Result<(), String> {
    headroom(
        db,
        disk::available(path.parent().ok_or("Usage database path has no parent")?)?,
    )
}

pub(super) fn migrate(connection: &mut Connection, path: &Path) -> Result<(), String> {
    let tx = connection
        .transaction_with_behavior(TransactionBehavior::Exclusive)
        .map_err(db_error)?;
    let version: u32 = tx
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(db_error)?;
    if version == SCHEMA_VERSION {
        tx.commit().map_err(db_error)?;
        return Ok(());
    }
    if version != 1 {
        return Err("Usage database schema changed; source left unchanged".into());
    }
    validate_v1(&tx)?;
    preflight(&tx, path)?;
    let recovery = recovery_image(path)?;
    tx.execute_batch("DROP TRIGGER IF EXISTS usage_backup_projection_min_version;
        DROP INDEX IF EXISTS usage_events_attribution_environment;
        DROP INDEX IF EXISTS usage_events_attribution_package;
        DROP INDEX IF EXISTS usage_events_attribution_location;
        DROP INDEX IF EXISTS usage_events_attribution_agent;
        DROP INDEX IF EXISTS usage_events_attribution_workspace;
        ALTER TABLE usage_events RENAME COLUMN record_json TO envelope_json;
        ALTER TABLE usage_events ADD COLUMN residual_json TEXT NOT NULL DEFAULT '{}';
        ALTER TABLE usage_events ADD COLUMN codec_version INTEGER NOT NULL DEFAULT 1;
        ALTER TABLE usage_events ADD COLUMN agent_context_id INTEGER REFERENCES usage_contexts(context_id);
        ALTER TABLE usage_events ADD COLUMN provider_context_id INTEGER REFERENCES usage_contexts(context_id);
        ALTER TABLE usage_events ADD COLUMN attribution_context_id INTEGER REFERENCES usage_contexts(context_id);
        ALTER TABLE usage_events ADD COLUMN restored_attribution_context_id INTEGER REFERENCES usage_contexts(context_id);
        ALTER TABLE usage_events ADD COLUMN opaque_json TEXT;
        CREATE TABLE usage_contexts(context_id INTEGER PRIMARY KEY,codec_version INTEGER NOT NULL,kind TEXT NOT NULL,canonical_json TEXT NOT NULL,UNIQUE(codec_version,kind,canonical_json));").map_err(db_error)?;
    let mut after = 0i64;
    loop {
        let rows: Vec<(i64, String)> = {
            let mut statement = tx.prepare("SELECT sequence,envelope_json FROM usage_events WHERE sequence>?1 ORDER BY sequence LIMIT 100").map_err(db_error)?;
            let rows = statement
                .query_map([after], |row| Ok((row.get(0)?, row.get(1)?)))
                .map_err(db_error)?
                .collect::<Result<_, _>>()
                .map_err(db_error)?;
            rows
        };
        if rows.is_empty() {
            break;
        }
        for (sequence, raw) in rows {
            let encoded = codec::encode(&raw)?;
            let ids = context_ids(&tx, &encoded)?;
            tx.execute("UPDATE usage_events SET envelope_json=?2,residual_json=?3,agent_context_id=?4,provider_context_id=?5,attribution_context_id=?6,restored_attribution_context_id=?7,opaque_json=?8 WHERE sequence=?1", params![sequence, encoded.envelope, encoded.residual, ids[0], ids[1], ids[2], ids[3], encoded.opaque]).map_err(db_error)?;
            after = sequence;
        }
    }
    install(&tx)?;
    super::install_backup_guard(&tx)?;
    tx.execute(
        "INSERT INTO usage_meta(key,value) VALUES('schema_v2_recovery_image',?1)",
        [recovery
            .to_str()
            .ok_or("Usage recovery path is not UTF-8")?],
    )
    .map_err(db_error)?;
    tx.pragma_update(None, "user_version", SCHEMA_VERSION)
        .map_err(db_error)?;
    let broken: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_foreign_key_check)",
            [],
            |row| row.get(0),
        )
        .map_err(db_error)?;
    if broken {
        return Err("Usage migration foreign-key validation failed; source left unchanged".into());
    }
    tx.commit().map_err(db_error)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn insufficient_space_stops_before_migration() {
        let db = Connection::open_in_memory().unwrap();
        assert!(headroom(&db, 0)
            .unwrap_err()
            .contains("source left unchanged"));
    }
}

#[cfg(test)]
mod migration_tests {
    use super::*;
    use serde_json::{json, Value};

    fn legacy() -> (PathBuf, Connection) {
        let dir = std::env::temp_dir().join(format!("usage-migration-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let db = Connection::open(dir.join("usage.sqlite3")).unwrap();
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;
            CREATE TABLE usage_events(sequence INTEGER PRIMARY KEY AUTOINCREMENT,event_id TEXT UNIQUE NOT NULL,root_run_id TEXT,agent_run_id TEXT,event_type TEXT NOT NULL,created_at TEXT NOT NULL,record_json TEXT NOT NULL);
            CREATE INDEX usage_events_root ON usage_events(root_run_id,sequence);
            CREATE INDEX usage_events_agent ON usage_events(agent_run_id,sequence);
            CREATE TABLE usage_meta(key TEXT PRIMARY KEY,value TEXT NOT NULL);
            CREATE TABLE usage_backup_projection(event_id TEXT PRIMARY KEY REFERENCES usage_events(event_id) ON DELETE CASCADE,record_json TEXT NOT NULL);
            CREATE TABLE usage_backup_queue(event_id TEXT NOT NULL REFERENCES usage_events(event_id) ON DELETE CASCADE,binding TEXT NOT NULL,generation INTEGER NOT NULL,acknowledged INTEGER NOT NULL DEFAULT 0,PRIMARY KEY(event_id,binding,generation));
            CREATE TABLE usage_backup_ids(kind TEXT NOT NULL,local_value TEXT NOT NULL,opaque_id TEXT NOT NULL,PRIMARY KEY(kind,local_value));
            CREATE TABLE usage_backup_windows(window_id INTEGER PRIMARY KEY,binding TEXT NOT NULL,generation INTEGER NOT NULL,start_sequence INTEGER NOT NULL,end_sequence INTEGER,last_scan INTEGER NOT NULL);
            CREATE TABLE usage_backup_backoff(binding TEXT NOT NULL,generation INTEGER NOT NULL,failures INTEGER NOT NULL,next_attempt INTEGER NOT NULL,PRIMARY KEY(binding,generation));
            PRAGMA user_version=1;").unwrap();
        (dir, db)
    }

    fn append(db: &Connection, index: i64, context: Value) -> Value {
        let event = json!({"schema_version":1,"event_id":format!("event-{index}"),"root_run_id":"same-run","event_type":"provider_request_completed","timestamp":"2026-10-27T00:00:00Z","duration_ms":u64::MAX,"usage":{"input_tokens":u64::MAX,"detail":{"null":null}},"agent":context,"provider":null,"attribution":{"schema_version":1,"environment":{"id":"env"}},"restored_attribution":false,"status":null,"custom":[1,true,null,{"unsigned":u64::MAX}]});
        db.execute("INSERT INTO usage_events(sequence,event_id,root_run_id,event_type,created_at,record_json) VALUES(?1,?2,'same-run','provider_request_completed','2026-10-27T00:00:00Z',?3)", params![index,format!("event-{index}"),event.to_string()]).unwrap();
        event
    }

    #[test]
    fn migration_keeps_wal_facts_sequences_old_backup_bytes_and_recovery() {
        let (dir, old) = legacy();
        let mut expected = Vec::new();
        for index in 1..=205 {
            expected.push(append(
                &old,
                index,
                json!({"revision":index % 2,"wide":u64::MAX}),
            ));
        }
        append(&old, 500, json!({"deleted":true}));
        old.execute("DELETE FROM usage_events WHERE sequence=500", [])
            .unwrap();
        let projection = " { \"schema_version\" : 2, \"opaque\" : null } ";
        old.execute(
            "INSERT INTO usage_backup_projection VALUES('event-1',?1)",
            [projection],
        )
        .unwrap();
        old.execute_batch(
            "INSERT INTO usage_backup_queue VALUES('event-1','binding',7,1);
            INSERT INTO usage_backup_ids VALUES('root','same-run','opaque-root');
            INSERT INTO usage_backup_windows VALUES(1,'binding',7,0,200,197);
            INSERT INTO usage_backup_backoff VALUES('binding',7,4,1999999999);
            INSERT INTO usage_meta VALUES('unrelated','preserved');",
        )
        .unwrap();
        // An already-open, prepared legacy writer must not append after switch.
        let mut old_insert = old.prepare("INSERT INTO usage_events(event_id,event_type,created_at,record_json) VALUES('stale','x','now','{}')").unwrap();
        let readonly = super::super::open_at(&dir, false).unwrap().unwrap();
        assert_eq!(
            readonly
                .pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            readonly
                .query_row("SELECT COUNT(*) FROM usage_event_records", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            205
        );
        assert!(!dir.join("recovery").exists());
        drop(readonly);
        let migrated = super::super::open_at(&dir, true).unwrap().unwrap();
        assert!(old_insert.execute([]).is_err());
        drop(old_insert);
        let actual = migrated
            .prepare("SELECT record_json FROM usage_event_records ORDER BY sequence")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .map(|row| serde_json::from_str::<Value>(&row.unwrap()).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(actual, expected);
        assert_eq!(
            migrated
                .query_row(
                    "SELECT COUNT(*) FROM usage_contexts WHERE kind='agent'",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            2
        );
        assert_eq!(
            migrated
                .query_row("SELECT record_json FROM usage_backup_projection", [], |r| r
                    .get::<_, String>(0))
                .unwrap(),
            projection
        );
        assert_eq!(
            migrated
                .query_row(
                    "SELECT binding||':'||generation||':'||acknowledged FROM usage_backup_queue",
                    [],
                    |r| r.get::<_, String>(0)
                )
                .unwrap(),
            "binding:7:1"
        );
        assert_eq!(
            migrated
                .query_row("SELECT opaque_id FROM usage_backup_ids", [], |r| r
                    .get::<_, String>(0))
                .unwrap(),
            "opaque-root"
        );
        assert_eq!(
            migrated
                .query_row("SELECT last_scan FROM usage_backup_windows", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            197
        );
        assert_eq!(
            migrated
                .query_row("SELECT next_attempt FROM usage_backup_backoff", [], |r| r
                    .get::<_, i64>(
                    0
                ))
                .unwrap(),
            1999999999
        );
        assert_eq!(
            migrated
                .query_row(
                    "SELECT value FROM usage_meta WHERE key='unrelated'",
                    [],
                    |r| r.get::<_, String>(0)
                )
                .unwrap(),
            "preserved"
        );
        let snapshot: String = migrated
            .query_row(
                "SELECT value FROM usage_meta WHERE key='schema_v2_recovery_image'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let metadata = fs::metadata(&snapshot).unwrap();
        assert!(metadata.len() > 0);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
        }
        let snapshot =
            Connection::open_with_flags(snapshot, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        assert_eq!(
            snapshot
                .pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            snapshot
                .query_row("SELECT COUNT(*) FROM usage_events", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            205
        );
        let recovered: String = snapshot
            .query_row(
                "SELECT record_json FROM usage_events WHERE sequence=1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&recovered).unwrap(),
            expected[0]
        );
        let accepted = json!({"event_id":"after-migration","event_type":"x","timestamp":"now","agent":{"revision":3}});
        super::super::insert_event(&migrated, &accepted).unwrap();
        assert_eq!(
            migrated
                .query_row(
                    "SELECT sequence FROM usage_events WHERE event_id='after-migration'",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            501
        );
        assert!(migrated
            .execute("UPDATE usage_contexts SET canonical_json='{}'", [])
            .is_err());
        assert!(migrated
            .execute("UPDATE usage_events SET envelope_json='{}'", [])
            .is_err());
        // Reopening never restores the older image over accepted new facts.
        drop(migrated);
        let reopened = super::super::open_at(&dir, true).unwrap().unwrap();
        assert_eq!(
            reopened
                .query_row("SELECT COUNT(*) FROM usage_events", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            206
        );
        assert_eq!(
            snapshot
                .query_row("SELECT COUNT(*) FROM usage_events", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            205
        );
        drop(reopened);
        drop(snapshot);
        drop(old);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn interrupted_schema_switch_rolls_back_and_retry_keeps_source() {
        let (dir, old) = legacy();
        let original = append(&old, 1, json!({"version":1}));
        // Deliberate conflicting schema object fails after row transformation.
        old.execute_batch(
            "CREATE VIEW usage_event_records AS SELECT record_json FROM usage_events;",
        )
        .unwrap();
        assert!(super::super::open_at(&dir, true).is_err());
        assert_eq!(
            old.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
        let raw: String = old
            .query_row("SELECT record_json FROM usage_events", [], |r| r.get(0))
            .unwrap();
        assert_eq!(serde_json::from_str::<Value>(&raw).unwrap(), original);
        assert_eq!(
            old.query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name='usage_contexts'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
        old.execute_batch("DROP VIEW usage_event_records").unwrap();
        let db = super::super::open_at(&dir, true).unwrap().unwrap();
        assert_eq!(
            db.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
                .unwrap(),
            2
        );
        drop(db);
        drop(old);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn old_reader_snapshot_cannot_upgrade_to_writer_after_migration() {
        let (dir, old) = legacy();
        append(&old, 1, json!({"revision":1}));
        old.execute_batch("BEGIN").unwrap();
        let _: String = old
            .query_row("SELECT record_json FROM usage_events", [], |row| row.get(0))
            .unwrap();
        let migrated = super::super::open_at(&dir, true).unwrap().unwrap();
        assert!(old.execute("INSERT INTO usage_events(event_id,event_type,created_at,record_json) VALUES('stale','x','now','{}')",[]).is_err());
        old.execute_batch("ROLLBACK").unwrap();
        assert_eq!(
            old.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
                .unwrap(),
            2
        );
        assert_eq!(
            migrated
                .query_row("SELECT COUNT(*) FROM usage_events", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
        drop(migrated);
        drop(old);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn migration_reports_bounded_contention_then_preserves_source_on_retry() {
        let (dir, old) = legacy();
        let expected = append(&old, 1, json!({"revision":1}));
        let original: String = old
            .query_row("SELECT record_json FROM usage_events", [], |row| row.get(0))
            .unwrap();
        old.execute_batch("BEGIN EXCLUSIVE").unwrap();
        let started = std::time::Instant::now();
        assert!(super::super::open_or_busy_for_test(&dir).is_none());
        let elapsed = started.elapsed();
        println!("held migration writer returned Busy in {elapsed:?}");
        assert!(elapsed < Duration::from_secs(2));
        assert!(!dir.join("recovery").exists());
        assert_eq!(
            old.pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            old.query_row("SELECT record_json FROM usage_events", [], |row| row
                .get::<_, String>(0))
                .unwrap(),
            original
        );
        old.execute_batch("ROLLBACK").unwrap();
        let db = super::super::open_at(&dir, true).unwrap().unwrap();
        let stored: String = db
            .query_row("SELECT record_json FROM usage_event_records", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(serde_json::from_str::<Value>(&stored).unwrap(), expected);
        let images: Vec<_> = fs::read_dir(dir.join("recovery"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        assert_eq!(images.len(), 1);
        let recovery =
            Connection::open_with_flags(&images[0], OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        assert_eq!(
            recovery
                .query_row("PRAGMA quick_check", [], |row| row.get::<_, String>(0))
                .unwrap(),
            "ok"
        );
        assert_eq!(
            recovery
                .pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            recovery
                .query_row("SELECT record_json FROM usage_events", [], |row| row
                    .get::<_, String>(0))
                .unwrap(),
            original
        );
        drop(recovery);
        drop(db);
        drop(old);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn concurrent_migrators_recheck_locked_version_and_share_one_image() {
        let (dir, old) = legacy();
        let expected_legacy = append(&old, 1, json!({"revision":1}));
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(4));
        let threads: Vec<_> = (0..4).map(|index| {
            let dir=dir.clone(); let barrier=barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                let event = json!({"event_id":format!("new-{index}"),"event_type":"x","timestamp":"now"});
                let Some(db) = super::super::open_or_busy_for_test(&dir) else {
                    return Some(event);
                };
                super::super::insert_event(&db, &event).unwrap();
                None
            })
        }).collect();
        let attempts: Vec<_> = threads
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect();
        assert!(
            attempts.iter().any(Option::is_none),
            "one migrator must complete"
        );
        // A bounded Busy is not a schema failure. Retry only after the winning
        // migration and every other initial attempt have released their locks.
        for event in attempts.into_iter().flatten() {
            let db = super::super::open_at(&dir, true).unwrap().unwrap();
            super::super::insert_event(&db, &event).unwrap();
        }
        let db = super::super::open_at(&dir, false).unwrap().unwrap();
        assert_eq!(
            db.query_row("SELECT COUNT(*) FROM usage_events", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            5
        );
        let stored_legacy: String = db
            .query_row(
                "SELECT record_json FROM usage_event_records WHERE event_id='event-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&stored_legacy).unwrap(),
            expected_legacy
        );
        for index in 0..4 {
            let stored: String = db
                .query_row(
                    "SELECT record_json FROM usage_event_records WHERE event_id=?1",
                    [format!("new-{index}")],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(
                serde_json::from_str::<Value>(&stored).unwrap(),
                json!({"event_id":format!("new-{index}"),"event_type":"x","timestamp":"now"})
            );
        }
        assert_eq!(fs::read_dir(dir.join("recovery")).unwrap().count(), 1);
        drop(db);
        drop(old);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn writer_lock_timeout_and_unexpected_schema_leave_legacy_unchanged() {
        let (dir, old) = legacy();
        append(&old, 1, json!(null));
        old.execute_batch("BEGIN IMMEDIATE").unwrap();
        let start = std::time::Instant::now();
        assert!(super::super::open_at(&dir, true).is_err());
        assert!(start.elapsed() < Duration::from_secs(2));
        assert!(!dir.join("recovery").exists());
        old.execute_batch("ROLLBACK; ALTER TABLE usage_events ADD COLUMN unexpected TEXT")
            .unwrap();
        assert!(super::super::open_at(&dir, true).is_err());
        assert_eq!(
            old.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert!(!dir.join("recovery").exists());
        drop(old);
        fs::remove_dir_all(dir).unwrap();
    }
}
