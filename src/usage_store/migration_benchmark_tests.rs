//! Optional credential-free copied-history qualification. Inputs are explicit;
//! normal Homes and network clients are never consulted by this test harness.
use super::super::{open_at, SCHEMA_VERSION};
use super::{configure_at, maintain_connection, policy, Policy};
use crate::usage_analytics::{
    contract::{
        CanonicalUuid, Counter, EventSubmission, LifecycleEpoch, Provenance, Sha256Hash, Source,
    },
    elastic::{self, Request, Version},
    queue,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use rusqlite::{params, Connection, OpenFlags};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    process::Command,
    time::{Instant, SystemTime, UNIX_EPOCH},
};
type Proof<T> = Result<T, &'static str>;
const ROW_GROUP: i64 = 100;
struct Config {
    source: PathBuf,
    expected_sha: String,
    expected_events: u64,
    expected_raw_bytes: u64,
    as_of_ms: i64,
    exports: Option<PathBuf>,
}
fn check(valid: bool, label: &'static str) -> Proof<()> {
    if valid {
        Ok(())
    } else {
        Err(label)
    }
}
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn file_hash(path: &Path) -> Proof<String> {
    let mut file = fs::File::open(path).map_err(|_| "input-open")?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 128 * 1024];
    loop {
        let n = file.read(&mut buffer).map_err(|_| "input-read")?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}
fn private_mode(path: &Path, directory: bool) -> Proof<()> {
    let _ = (path, directory);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            path,
            fs::Permissions::from_mode(if directory { 0o700 } else { 0o600 }),
        )
        .map_err(|_| "private-permissions")?;
    }
    Ok(())
}
struct PrivateDir {
    path: PathBuf,
    keep: bool,
}
impl PrivateDir {
    fn new(path: PathBuf) -> Proof<Self> {
        check(
            path.is_absolute() && !path.exists(),
            "new-absolute-private-directory-required",
        )?;
        fs::create_dir(&path).map_err(|_| "new-private-directory")?;
        private_mode(&path, true)?;
        Ok(Self { path, keep: false })
    }
    fn temporary() -> Proof<Self> {
        Self::new(
            std::env::temp_dir().join(format!("usage-history-proof-{}", uuid::Uuid::new_v4())),
        )
    }
    fn clean(&mut self) -> Proof<()> {
        fs::remove_dir_all(&self.path).map_err(|_| "private-copy-cleanup")?;
        self.keep = true;
        Ok(())
    }
}
impl Drop for PrivateDir {
    fn drop(&mut self) {
        if !self.keep {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}
fn write_bytes_private(path: &Path, bytes: &[u8]) -> Proof<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|_| "new-private-output-required")?;
    private_mode(path, false)?;
    file.write_all(bytes).map_err(|_| "output-write")?;
    file.sync_all().map_err(|_| "output-sync")
}
fn write_private(path: &Path, value: &Value) -> Proof<()> {
    write_bytes_private(
        path,
        &serde_json::to_vec_pretty(value).map_err(|_| "output-json")?,
    )
}
fn identity(path: &Path) -> Proof<Value> {
    let metadata = fs::symlink_metadata(path).map_err(|_| "source-metadata")?;
    check(
        metadata.is_file() && !metadata.file_type().is_symlink(),
        "regular-source-no-symlink",
    )?;
    let mut value = json!({"bytes":metadata.len(),"modified_ns":metadata.modified().map_err(|_|"source-time")?.duration_since(UNIX_EPOCH).map_err(|_|"source-time")?.as_nanos().to_string(),"sha256":file_hash(path)?});
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        value["inode"] = json!(metadata.ino());
        value["device"] = json!(metadata.dev());
        value["mode"] = json!(metadata.mode());
        value["ctime_seconds"] = json!(metadata.ctime());
        value["ctime_nanoseconds"] = json!(metadata.ctime_nsec());
    }
    value["readonly_permission"] = json!(metadata.permissions().readonly());
    Ok(value)
}
fn source_identity(path: &Path) -> Proof<Value> {
    let mut result = json!({"database":identity(path)?});
    for suffix in ["-wal", "-shm", "-journal"] {
        let side = PathBuf::from(format!("{}{suffix}", path.to_str().ok_or("source-utf8")?));
        result[suffix] = if side.exists() {
            identity(&side)?
        } else {
            Value::Null
        };
    }
    check(
        result["-wal"].is_null() || result["-wal"]["bytes"] == 0,
        "committed-standalone-source-required",
    )?;
    Ok(result)
}
fn immutable(path: &Path) -> Proof<Connection> {
    let canonical = fs::canonicalize(path).map_err(|_| "canonical-source-path")?;
    let mut url = reqwest::Url::from_file_path(&canonical).map_err(|_| "absolute-source-uri")?;
    url.query_pairs_mut()
        .append_pair("mode", "ro")
        .append_pair("immutable", "1");
    Connection::open_with_flags(
        url.as_str(),
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_URI
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .map_err(|_| "immutable-source-open")
}
fn frame(hasher: &mut Sha256, bytes: &[u8]) {
    hasher.update((bytes.len() as u64).to_le_bytes());
    hasher.update(bytes);
}
fn records(db: &Connection, legacy: bool) -> Proof<Value> {
    let table = if legacy {
        "usage_events"
    } else {
        "usage_event_records"
    };
    let mut after = 0i64;
    let mut count = 0u64;
    let mut raw_bytes = 0u64;
    let mut semantic_bytes = 0u64;
    let mut hasher = Sha256::new();
    let mut invalid_json = 0u64;
    let mut invalid_time = 0u64;
    let mut min_ms = None::<i64>;
    let mut max_ms = None::<i64>;
    loop {
        let mut stmt=db.prepare(&format!("SELECT sequence,event_id,root_run_id,agent_run_id,event_type,created_at,record_json FROM {table} WHERE sequence>?1 ORDER BY sequence LIMIT ?2")).map_err(|_|"record-query")?;
        let rows = stmt
            .query_map(params![after, ROW_GROUP], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, String>(6)?,
                ))
            })
            .map_err(|_| "record-read")?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| "record-read")?;
        if rows.is_empty() {
            break;
        }
        for (seq, id, root, agent, kind, created, text) in rows {
            after = seq;
            count += 1;
            raw_bytes += text.len() as u64;
            let canonical = match serde_json::from_str::<Value>(&text) {
                Ok(value) => {
                    semantic_bytes += value.to_string().len() as u64;
                    json!({"valid_json":value})
                }
                Err(_) => {
                    invalid_json += 1;
                    json!({"malformed_bytes_sha256":hash(text.as_bytes())})
                }
            };
            let record = json!([seq, id, root, agent, kind, created, canonical]);
            frame(
                &mut hasher,
                &serde_json::to_vec(&record).map_err(|_| "record-canonical")?,
            );
            if let Ok(time) = time::OffsetDateTime::parse(
                &created,
                &time::format_description::well_known::Rfc3339,
            ) {
                let ms = i64::try_from(time.unix_timestamp_nanos() / 1_000_000)
                    .map_err(|_| "timestamp-bound")?;
                min_ms = Some(min_ms.map_or(ms, |old| old.min(ms)));
                max_ms = Some(max_ms.map_or(ms, |old| old.max(ms)));
            } else {
                invalid_time += 1;
            }
        }
    }
    Ok(
        json!({"events":count,"raw_json_bytes":raw_bytes,"canonical_json_bytes":semantic_bytes,"last_sequence":after,"all_records_semantics_sha256":format!("{:x}",hasher.finalize()),"invalid_json_events":invalid_json,"invalid_time_events":invalid_time,"minimum_event_time_ms":min_ms,"maximum_event_time_ms":max_ms}),
    )
}
fn legacy_tables(db: &Connection) -> Proof<Value> {
    use rusqlite::types::ValueRef;
    let mut result = json!({});
    for table in [
        "usage_meta",
        "usage_backup_projection",
        "usage_backup_queue",
        "usage_backup_ids",
        "usage_backup_windows",
        "usage_backup_backoff",
        "usage_backup_lease",
    ] {
        let exists: bool = db
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
                [table],
                |r| r.get(0),
            )
            .map_err(|_| "legacy-table-inventory")?;
        if !exists {
            result[table] = Value::Null;
            continue;
        }
        let cols: i64 = db
            .query_row(
                &format!("SELECT count(*) FROM pragma_table_info('{table}')"),
                [],
                |r| r.get(0),
            )
            .map_err(|_| "legacy-column-inventory")?;
        let order = (1..=cols)
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join(",");
        let where_clause = if table == "usage_meta" {
            " WHERE key!='schema_v2_recovery_image'"
        } else {
            ""
        };
        let mut stmt = db
            .prepare(&format!(
                "SELECT * FROM {table}{where_clause} ORDER BY {order}"
            ))
            .map_err(|_| "legacy-state-query")?;
        let mut rows = stmt.query([]).map_err(|_| "legacy-state-read")?;
        let mut count = 0u64;
        let mut hasher = Sha256::new();
        while let Some(row) = rows.next().map_err(|_| "legacy-state-read")? {
            count += 1;
            for i in 0..cols {
                let value = row.get_ref(i as usize).map_err(|_| "legacy-cell")?;
                match value {
                    ValueRef::Null => frame(&mut hasher, b"null"),
                    ValueRef::Integer(n) => {
                        frame(&mut hasher, b"integer");
                        frame(&mut hasher, &n.to_le_bytes());
                    }
                    ValueRef::Real(n) => {
                        frame(&mut hasher, b"real");
                        frame(&mut hasher, &n.to_bits().to_le_bytes());
                    }
                    ValueRef::Text(text) => {
                        frame(&mut hasher, b"text");
                        frame(&mut hasher, text);
                    }
                    ValueRef::Blob(bytes) => {
                        frame(&mut hasher, b"blob");
                        frame(&mut hasher, bytes);
                    }
                }
            }
        }
        result[table] = json!({"rows":count,"columns":cols,"typed_contents_sha256":format!("{:x}",hasher.finalize())});
    }
    result["event_sequence_high_water"] =
        json!(queue::high(db).map_err(|_| "legacy-sequence-high-water")?);
    Ok(result)
}
fn footprint(db: &Connection, dir: &Path) -> Proof<Value> {
    let page: i64 = db
        .pragma_query_value(None, "page_size", |r| r.get(0))
        .map_err(|_| "page-size")?;
    let count: i64 = db
        .pragma_query_value(None, "page_count", |r| r.get(0))
        .map_err(|_| "page-count")?;
    let free: i64 = db
        .pragma_query_value(None, "freelist_count", |r| r.get(0))
        .map_err(|_| "freelist-count")?;
    let mut result =
        json!({"active_sqlite_bytes":(count-free)*page,"reusable_sqlite_bytes":free*page});
    for suffix in ["", "-wal", "-shm"] {
        let p = dir.join(format!("usage.sqlite3{suffix}"));
        result[if suffix.is_empty() {
            "database_file_bytes"
        } else {
            suffix
        }] = json!(fs::metadata(p).map(|m| m.len()).unwrap_or(0));
    }
    Ok(result)
}
fn sample() -> Value {
    let output = Command::new(if cfg!(windows) { "ps" } else { "/bin/ps" })
        .args([
            "-o",
            "rss=",
            "-o",
            "time=",
            "-p",
            &std::process::id().to_string(),
        ])
        .output();
    let Ok(output) = output else {
        return json!({"available":false});
    };
    let Ok(text) = std::str::from_utf8(&output.stdout) else {
        return json!({"available":false});
    };
    let mut fields = text.split_whitespace();
    let rss = fields.next().and_then(|s| s.parse::<u64>().ok());
    let cpu = fields.next().and_then(|s| {
        let mut seconds = 0f64;
        for part in s.split(':') {
            seconds = seconds * 60.0 + part.parse::<f64>().ok()?;
        }
        Some(seconds)
    });
    json!({"available":rss.is_some()&&cpu.is_some(),"rss_bytes":rss.map(|n|n*1024),"process_cpu_seconds":cpu})
}
fn elapsed(start: Instant, before: &Value, after: &Value) -> Value {
    json!({"wall_seconds":start.elapsed().as_secs_f64(),"process_cpu_seconds_delta":before["process_cpu_seconds"].as_f64().zip(after["process_cpu_seconds"].as_f64()).map(|(a,b)|(b-a).max(0.0)),"rss_after_bytes":after["rss_bytes"]})
}
fn now_ms() -> Proof<i64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "system-clock")?
        .as_millis()
        .try_into()
        .map_err(|_| "system-clock")
}
fn id(n: u64) -> CanonicalUuid {
    format!("00000000-0000-4000-8000-{n:012}")
        .try_into()
        .unwrap()
}
fn source() -> Source {
    Source {
        account_binding: id(1),
        source_home_id: id(2),
        data_generation: Counter::try_from(1).unwrap(),
    }
}
fn provenance() -> Provenance {
    Provenance {
        lifecycle_epoch: LifecycleEpoch::try_from(0).unwrap(),
        consent_interval: Counter::try_from(1).unwrap(),
    }
}
fn encode(
    source: &Source,
    provenance: &Provenance,
    batch_id: &CanonicalUuid,
    events: &[EventSubmission],
    target: usize,
) -> Result<(String, usize, Sha256Hash), String> {
    let (batch, count) =
        elastic::encode_batch(events, target).map_err(|_| "bounded compression failed")?;
    let members = events[..count]
        .iter()
        .map(EventSubmission::member)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "event membership failed")?;
    let membership =
        elastic::membership_sha256(source, &members).map_err(|_| "batch membership failed")?;
    let request = Request::Upload {
        protocol_version: Version,
        source: source.clone(),
        provenance: provenance.clone(),
        batch_id: batch_id.clone(),
        ordered_membership_sha256: membership.clone(),
        batch,
        retry_count: 0,
    };
    Ok((
        serde_json::to_string(&request).map_err(|_| "request encoding failed")?,
        count,
        membership,
    ))
}
fn benchmark(dir: &Path, as_of: i64, target: usize, exports: Option<&Path>) -> Proof<Value> {
    let mut db = open_at(dir, true)
        .map_err(|_| "benchmark-open")?
        .ok_or("benchmark-database")?;
    let raw_before = records(&db, false)?;
    let cutoff = as_of.saturating_sub(7 * 86_400_000).max(0);
    let expected:i64=db.query_row("SELECT count(*) FROM usage_cloud_queue WHERE retained=0 AND CAST(json_extract(original_text,'$.occurred_at_ms') AS INTEGER)>=?1",[cutoff],|r|r.get(0)).map_err(|_|"eligible-count")?;
    let expected = u64::try_from(expected).map_err(|_| "eligible-count-range")?;
    let before = sample();
    let start = Instant::now();
    let mut total_events = 0u64;
    let mut decoded_bytes = 0u64;
    let mut compressed_bytes = 0u64;
    let mut request_bytes = 0u64;
    let mut largest_envelope = 0usize;
    let mut batches = 0u64;
    let mut peak_sampled_rss = 0u64;
    let mut batch_evidence = Vec::new();
    let mut resumed = false;
    let mut encoding_wall_seconds = 0f64;
    loop {
        let wall = now_ms()?;
        let mut lease = queue::acquire_lease(&mut db, &source(), wall, queue::MAX_LEASE_MS)
            .map_err(|_| "benchmark-lease")?
            .ok_or("benchmark-lease-unavailable")?;
        let batch = queue::prepare_upload_batch_since(
            &mut db,
            &lease,
            wall,
            cutoff,
            |source, p, id, events| {
                let started = Instant::now();
                let result = encode(source, p, id, events, target);
                encoding_wall_seconds += started.elapsed().as_secs_f64();
                result
            },
        )
        .map_err(|_| "benchmark-prepare")?;
        let Some(mut batch) = batch else {
            queue::release_lease(&mut db, &lease, now_ms()?)
                .map_err(|_| "benchmark-release-empty")?;
            break;
        };
        queue::begin_upload_attempt(&mut db, &lease, batch.id(), now_ms()?)
            .map_err(|_| "benchmark-attempt")?;
        if !resumed {
            let frozen = batch.request_json().to_owned();
            queue::release_lease(&mut db, &lease, now_ms()?).map_err(|_| "restart-release")?;
            drop(db);
            db = open_at(dir, true)
                .map_err(|_| "restart-open")?
                .ok_or("restart-database")?;
            lease = queue::acquire_lease(&mut db, &source(), now_ms()?, queue::MAX_LEASE_MS)
                .map_err(|_| "restart-lease")?
                .ok_or("restart-lease-unavailable")?;
            batch = queue::prepare_upload_batch_since(
                &mut db,
                &lease,
                now_ms()?,
                cutoff,
                |_, _, _, _| Err("frozen retry unexpectedly rebuilt".into()),
            )
            .map_err(|_| "restart-frozen-request")?
            .ok_or("restart-batch")?;
            check(batch.request_json() == frozen, "restart-bytes-identical")?;
            resumed = true;
        }
        let request = batch
            .request_json()
            .parse::<Request>()
            .map_err(|_| "final-request-validate")?;
        let Request::Upload { batch: encoded, .. } = &request else {
            return Err("upload-shape");
        };
        let events = elastic::decode_batch(encoded).map_err(|_| "bounded-independent-decode")?;
        check(
            events.len() == batch.members().len() && encoded.event_count as usize == events.len(),
            "decoded-membership-count",
        )?;
        let members = events
            .iter()
            .map(EventSubmission::member)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| "decoded-members")?;
        check(members == batch.members(), "decoded-membership-exact")?;
        let compressed = STANDARD
            .decode(&encoded.data_base64)
            .map_err(|_| "base64")?;
        check(
            compressed.len() <= elastic::MAX_COMPRESSED_BYTES
                && encoded.decoded_bytes as usize <= elastic::MAX_DECODED_BYTES
                && events.len() <= elastic::MAX_BATCH_EVENTS,
            "all-codec-guards",
        )?;
        let envelope=serde_json::to_vec(&json!({"action":"usage_analytics","credentials":{"access_token":"x".repeat(16_384)},"usage_analytics":&request})).map_err(|_|"envelope")?;
        check(
            envelope.len() <= elastic::MAX_ACCOUNT_BYTES,
            "maximum-token-account-envelope-guard",
        )?;
        largest_envelope = largest_envelope.max(envelope.len());
        batches += 1;
        total_events += events.len() as u64;
        decoded_bytes += u64::from(encoded.decoded_bytes);
        compressed_bytes += compressed.len() as u64;
        request_bytes += batch.request_json().len() as u64;
        let name = format!("target-{target}-batch-{batches}.json");
        if let Some(export) = exports {
            write_bytes_private(&export.join(&name), batch.request_json().as_bytes())?;
        }
        batch_evidence.push(json!({"file":exports.map(|_|name),"events":events.len(),"decoded_bytes":encoded.decoded_bytes,"compressed_bytes":compressed.len(),"request_bytes":batch.request_json().len(),"request_sha256":hash(batch.request_json().as_bytes()),"membership_sha256":batch.membership(),"encoded_sha256":encoded.encoded_sha256}));
        // This is a local progress simulation, never a statement about real custody.
        queue::acknowledge_uploaded(&mut db, &lease, batch.id(), now_ms()?)
            .map_err(|_| "simulated-local-ack")?;
        queue::release_lease(&mut db, &lease, now_ms()?).map_err(|_| "benchmark-release")?;
        let resource = sample();
        peak_sampled_rss = peak_sampled_rss.max(resource["rss_bytes"].as_u64().unwrap_or(0));
    }
    let after = sample();
    let measurement = elapsed(start, &before, &after);
    let wall = measurement["wall_seconds"].as_f64().unwrap_or(0.0);
    check(total_events == expected, "full-eligible-catchup")?;
    let raw_after = records(&db, false)?;
    check(
        raw_before["events"] == raw_after["events"]
            && raw_before["all_records_semantics_sha256"]
                == raw_after["all_records_semantics_sha256"],
        "ack-preserves-all-local-raw",
    )?;
    let remaining:i64=db.query_row("SELECT count(*) FROM usage_cloud_queue WHERE retained=0 AND CAST(json_extract(original_text,'$.occurred_at_ms') AS INTEGER)>=?1",[cutoff],|r|r.get(0)).map_err(|_|"remaining-eligible")?;
    check(remaining == 0, "no-eligible-gap-left")?;
    Ok(
        json!({"compressed_target_bytes":target,"clock":"explicit isolated as-of; original event timestamps unchanged","as_of_ms":as_of,"cloud_cutoff_ms":cutoff,"eligible_events":expected,"acknowledged_locally_for_measurement":total_events,"real_uploads":0,"simulated_acknowledgments":true,"batches":batches,"decoded_logical_bytes":decoded_bytes,"compressed_wire_bytes":compressed_bytes,"serialized_request_bytes":request_bytes,"largest_maximum_token_account_envelope_bytes":largest_envelope,"events_per_second":if wall>0.0 {total_events as f64/wall} else {0.0},"logical_bytes_per_second":if wall>0.0 {decoded_bytes as f64/wall} else {0.0},"wire_bytes_per_second":if wall>0.0 {compressed_bytes as f64/wall} else {0.0},"encoding_wall_seconds":encoding_wall_seconds,"events_per_encoding_second":if encoding_wall_seconds>0.0 {total_events as f64/encoding_wall_seconds} else {0.0},"throughput_scope":"queue, incremental encoding, bounded round-trip validation and simulated progress; no network","measurement":measurement,"maximum_sampled_rss_bytes":peak_sampled_rss,"rss_limitation":"step samples, not a continuous peak measurement","restart_reused_frozen_bytes":resumed,"raw_preserved_after_ack":true,"storage":footprint(&db,dir)?,"batch_details":batch_evidence}),
    )
}
fn run(config: &Config) -> Proof<Value> {
    check(
        config.source.is_absolute()
            && config.expected_sha.len() == 64
            && config.expected_events > 0
            && config.expected_events <= 1_000_000
            && config.as_of_ms >= 0,
        "explicit-input-bounds",
    )?;
    let original_before = source_identity(&config.source)?;
    check(
        original_before["database"]["sha256"] == config.expected_sha,
        "frozen-input-sha256",
    )?;
    let mut work = PrivateDir::temporary()?;
    let mut exports = match &config.exports {
        Some(path) => {
            check(
                path.starts_with(std::env::temp_dir())
                    || path.starts_with("/private/tmp")
                    || path.starts_with("/tmp"),
                "exports-only-in-private-temporary-storage",
            )?;
            Some(PrivateDir::new(path.clone())?)
        }
        None => None,
    };
    let resource_before = sample();
    let outcome = (|| -> Proof<Value> {
        let original = immutable(&config.source)?;
        let version: u32 = original
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .map_err(|_| "source-schema")?;
        check(version == 1, "supported-legacy-source-version-required")?;
        let source_records = records(&original, true)?;
        check(
            source_records["events"] == config.expected_events
                && source_records["raw_json_bytes"] == config.expected_raw_bytes,
            "expected-frozen-count-and-bytes",
        )?;
        check(
            source_records["maximum_event_time_ms"]
                .as_i64()
                .is_none_or(|time| time <= config.as_of_ms.saturating_add(300_000)),
            "as-of-cannot-reinterpret-future-data",
        )?;
        let released = legacy_tables(&original)?;
        let dir = work.path.join("preserved-home");
        fs::create_dir(&dir).map_err(|_| "copy-home")?;
        private_mode(&dir, true)?;
        let copied = dir.join("usage.sqlite3");
        fs::copy(&config.source, &copied).map_err(|_| "copy-frozen-source")?;
        private_mode(&copied, false)?;
        check(
            file_hash(&copied)? == config.expected_sha,
            "copy-exact-source-bytes",
        )?;
        let sentinel = work.path.join("unrelated-local-data");
        fs::write(&sentinel, b"preserved unrelated local data").map_err(|_| "local-sentinel")?;
        private_mode(&sentinel, false)?;
        let before = sample();
        let start = Instant::now();
        let db = open_at(&dir, true)
            .map_err(|_| "supported-production-migration")?
            .ok_or("migrated-database")?;
        let migration_timing = elapsed(start, &before, &sample());
        check(
            db.pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))
                .map_err(|_| "migrated-version")?
                == SCHEMA_VERSION,
            "normalized-version",
        )?;
        let migrated = records(&db, false)?;
        check(
            source_records["all_records_semantics_sha256"]
                == migrated["all_records_semantics_sha256"]
                && source_records["events"] == migrated["events"],
            "full-raw-identity-timestamp-unknown-field-preservation",
        )?;
        check(
            legacy_tables(&db)? == released,
            "released-backup-state-preserved",
        )?;
        check(
            !policy(&db).map_err(|_| "migration-policy")?.enabled,
            "no-new-policy-or-cloud-consent-from-upgrade",
        )?;
        let recovery: String = db
            .query_row(
                "SELECT value FROM usage_meta WHERE key='schema_v2_recovery_image'",
                [],
                |r| r.get(0),
            )
            .map_err(|_| "recovery-image")?;
        let image = immutable(Path::new(&recovery))?;
        check(
            records(&image, true)?["all_records_semantics_sha256"]
                == source_records["all_records_semantics_sha256"]
                && legacy_tables(&image)? == released,
            "supported-recovery-image-preserves-released-data",
        )?;
        drop(image);
        let normalized_storage = footprint(&db, &dir)?;
        let residual_records:i64=db.query_row("SELECT count(*) FROM usage_events WHERE residual_json!='{}' OR opaque_json IS NOT NULL",[],|r|r.get(0)).map_err(|_|"unknown-extension-coverage")?;
        drop(db);
        let mut db = open_at(&dir, true)
            .map_err(|_| "restart-normalized-home")?
            .ok_or("restart-normalized-database")?;
        check(
            records(&db, false)?["all_records_semantics_sha256"]
                == source_records["all_records_semantics_sha256"]
                && legacy_tables(&db)? == released,
            "restart-preserves-full-history-and-old-backup-state",
        )?;
        check(
            !policy(&db).map_err(|_| "restart-policy")?.enabled,
            "restart-never-selects-new-retention",
        )?;
        // All destructive work below is confined to this explicitly selected copy.
        configure_at(
            &mut db,
            &Policy {
                schema_version: 2,
                days: 30,
                enabled: true,
                local_detail_loss_acknowledged: true,
                unselected_loss_acknowledged: true,
            },
        )
        .map_err(|_| "explicit-isolated-rolling-consent")?;
        let before = sample();
        let start = Instant::now();
        let mut invocations = 0u64;
        let mut expired = 0u64;
        let mut expired_unknown = 0u64;
        loop {
            let result = maintain_connection(&db, config.as_of_ms)
                .map_err(|_| "isolated-as-of-maintenance")?;
            invocations += 1;
            expired += result["deleted_records"].as_u64().unwrap_or(0);
            expired_unknown += result["expired_without_acknowledgment_records"]
                .as_u64()
                .unwrap_or(0);
            if result["maintenance_pending"] != true {
                break;
            }
            check(
                invocations <= config.expected_events + 2,
                "bounded-maintenance-progress",
            )?;
        }
        let retention = json!({"days":30,"as_of_ms":config.as_of_ms,"explicit_consent":true,"invocations":invocations,"expired_raw_events":expired,"expired_without_ack_or_unknown":expired_unknown,"measurement":elapsed(start,&before,&sample())});
        queue::initialize(&mut db).map_err(|_| "new-queue-initialize")?;
        queue::enable(&mut db, &source(), &provenance(), now_ms()?)
            .map_err(|_| "separate-synthetic-cloud-selection")?;
        let through = queue::high(&db).map_err(|_| "capture-high")?;
        queue::select_history(&mut db, &source(), &provenance(), through)
            .map_err(|_| "explicit-history-selection")?;
        let before = sample();
        let start = Instant::now();
        let mut passes = 0u64;
        let mut captured = 0u64;
        let mut examined = 0u64;
        loop {
            let progress =
                queue::capture_progress(&mut db, &source(), crate::usage_analytics::project)
                    .map_err(|_| "production-projection-capture")?;
            if progress.examined == 0 {
                break;
            }
            captured += progress.added as u64;
            examined += progress.examined as u64;
            passes += 1;
            check(
                progress.examined <= 100 && progress.added <= progress.examined,
                "bounded-capture-progress",
            )?;
            check(
                passes <= config.expected_events + 1,
                "capture-progress-bound",
            )?;
        }
        let status = queue::status(&db, &source()).map_err(|_| "capture-final-status")?;
        check(status.unscanned == 0, "capture-complete")?;
        let capture = json!({"passes":passes,"bounded_rows_per_capture":100,"examined_events":examined,"captured_projection_events":captured,"origin_unverified":status.origin_unverified,"invalid_time_excluded":status.time_ineligible,"measurement":elapsed(start,&before,&sample())});
        db.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
            .map_err(|_| "baseline-checkpoint")?;
        drop(db);
        let mut hypotheses = Vec::new();
        for target in [1_000_000usize, 2_000_000, 3_000_000] {
            let variant = work.path.join(format!("target-{target}"));
            fs::create_dir(&variant).map_err(|_| "variant-home")?;
            private_mode(&variant, true)?;
            let path = variant.join("usage.sqlite3");
            fs::copy(&copied, &path).map_err(|_| "variant-copy")?;
            private_mode(&path, false)?;
            hypotheses.push(benchmark(
                &variant,
                config.as_of_ms,
                target,
                exports.as_ref().map(|e| e.path.as_path()),
            )?);
            fs::remove_dir_all(variant).map_err(|_| "variant-cleanup")?;
        }
        check(
            fs::read(sentinel).map_err(|_| "sentinel-read")? == b"preserved unrelated local data",
            "unrelated-local-data-preserved",
        )?;
        let result = json!({"input_schema":version,"normalized_schema":SCHEMA_VERSION,"original_records":source_records,"migration_measurement":migration_timing,"normalized_storage":normalized_storage,"full_semantic_reconstruction":true,"identity_sequences_and_timestamps_preserved":true,"residual_or_opaque_records":residual_records,"released_backup_state_before_new_policy":released,"released_backup_state_preserved_through_migration_restart":true,"recovery_image_verified":true,"no_new_policy_before_explicit_selection":true,"retention_branch":retention,"projection_capture":capture,"compression_hypotheses":hypotheses,"unrelated_local_data_preserved":true});
        if let Some(export) = exports.as_ref() {
            write_private(
                &export.path.join("manifest.json"),
                &json!({"schema_version":1,"as_of_ms":config.as_of_ms,"account":"synthetic-local-history-benchmark","subject":"synthetic-local-history-benchmark","real_uploads":0,"targets":result["compression_hypotheses"]}),
            )?;
        }
        Ok(result)
    })();
    let original_after = source_identity(&config.source)?;
    check(
        original_before == original_after,
        "original-database-and-sidecars-unchanged",
    )?;
    let mut result = outcome?;
    work.clean()?;
    if let Some(export) = exports.as_mut() {
        export.keep = true;
        result["sanitized_temporary_export_directory"] = json!(export.path);
    }
    result["source_before"] = original_before;
    result["source_after"] = original_after;
    result["source_unchanged"] = json!(true);
    result["raw_disposable_copies_removed"] = json!(true);
    result["build_profile"] = json!(if cfg!(debug_assertions) {
        "debug_test"
    } else {
        "release_test"
    });
    result["target_arch"] = json!(std::env::consts::ARCH);
    result["target_os"] = json!(std::env::consts::OS);
    result["package_version"] = json!(env!("CARGO_PKG_VERSION"));
    result["resource_before"] = resource_before;
    result["resource_after"] = sample();
    result["limitations"]=json!(["Single host local warm-cache proof; no network, cloud custody or dashboard-readiness claim","CPU/RSS process samples include bounded harness verification; RSS is not a continuous high-water measurement","Legacy interrupted/rollback/concurrent-migrator proofs are separate unchanged supported-migration unit tests","Only aggregates/hashes persist; optional sanitized compressed exports are confined to explicit private temporary storage"]);
    Ok(result)
}
fn input(name: &str) -> Proof<String> {
    std::env::var(name).map_err(|_| "explicit-benchmark-input-required")
}
#[test]
#[ignore = "explicit authorized frozen-source paths/hash/count/as-of and new private aggregate output required; never use a normal Home"]
fn copied_history_migration_and_codec_benchmark() {
    let config = Config {
        source: PathBuf::from(input("CARGO_AI_MIGRATION_SOURCE_FILE").expect("explicit source")),
        expected_sha: input("CARGO_AI_MIGRATION_SOURCE_SHA256").expect("explicit frozen hash"),
        expected_events: input("CARGO_AI_MIGRATION_EXPECTED_EVENTS")
            .unwrap()
            .parse()
            .unwrap(),
        expected_raw_bytes: input("CARGO_AI_MIGRATION_EXPECTED_RAW_BYTES")
            .unwrap()
            .parse()
            .unwrap(),
        as_of_ms: input("CARGO_AI_MIGRATION_AS_OF_MS")
            .unwrap()
            .parse()
            .unwrap(),
        exports: std::env::var_os("CARGO_AI_MIGRATION_EXPORT_DIR").map(PathBuf::from),
    };
    let output =
        PathBuf::from(input("CARGO_AI_MIGRATION_OUTPUT_FILE").expect("new private output"));
    assert!(output.is_absolute() && !output.exists());
    let result = if sample()["available"] == true {
        run(&config)
    } else {
        Err("own-process-cpu-rss-sampling-unavailable")
    };
    let aggregate = match &result {
        Ok(value) => json!({"status":"passed","proof":value}),
        Err(error) => json!({"status":"failed","error":error,"real_uploads":0}),
    };
    write_private(&output, &aggregate).expect("new private aggregate evidence");
    assert!(
        result.is_ok(),
        "copied-history proof failed; inspect private aggregate evidence"
    );
}
#[test]
fn synthetic_copy_qualification_preserves_unknown_fields_and_iterates_beyond_one_capture_group() {
    let source_dir = PrivateDir::temporary().unwrap();
    let source = source_dir.path.join("synthetic.sqlite3");
    let db = Connection::open(&source).unwrap();
    db.execute_batch("CREATE TABLE usage_events(sequence INTEGER PRIMARY KEY AUTOINCREMENT,event_id TEXT UNIQUE NOT NULL,root_run_id TEXT,agent_run_id TEXT,event_type TEXT NOT NULL,created_at TEXT NOT NULL,record_json TEXT NOT NULL);CREATE INDEX usage_events_root ON usage_events(root_run_id,sequence);CREATE INDEX usage_events_agent ON usage_events(agent_run_id,sequence);CREATE TABLE usage_meta(key TEXT PRIMARY KEY,value TEXT NOT NULL);CREATE TABLE usage_backup_projection(event_id TEXT PRIMARY KEY REFERENCES usage_events(event_id) ON DELETE CASCADE,record_json TEXT NOT NULL);CREATE TABLE usage_backup_queue(event_id TEXT NOT NULL REFERENCES usage_events(event_id) ON DELETE CASCADE,binding TEXT NOT NULL,generation INTEGER NOT NULL,acknowledged INTEGER NOT NULL,PRIMARY KEY(event_id,binding,generation));CREATE TABLE usage_backup_ids(kind TEXT NOT NULL,local_value TEXT NOT NULL,opaque_id TEXT NOT NULL,PRIMARY KEY(kind,local_value));CREATE TABLE usage_backup_windows(window_id INTEGER PRIMARY KEY,binding TEXT NOT NULL,generation INTEGER NOT NULL,start_sequence INTEGER NOT NULL,end_sequence INTEGER,last_scan INTEGER NOT NULL);CREATE TABLE usage_backup_backoff(binding TEXT NOT NULL,generation INTEGER NOT NULL,failures INTEGER NOT NULL,next_attempt INTEGER NOT NULL,PRIMARY KEY(binding,generation));CREATE TABLE usage_backup_lease(singleton INTEGER PRIMARY KEY,token TEXT NOT NULL,expires_at INTEGER NOT NULL);PRAGMA user_version=1;").unwrap();
    let now = now_ms().unwrap();
    let stamp = time::OffsetDateTime::from_unix_timestamp_nanos(i128::from(now) * 1_000_000)
        .unwrap()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap();
    let mut bytes = 0;
    for n in 1..=257 {
        let record=json!({"schema_version":1,"event_id":id(n+100),"root_run_id":id(10),"agent_run_id":id(11),"event_type":"provider_request_completed","timestamp":stamp,"usage":{"input_tokens":u64::MAX},"unknown_extension":{"null":null,"wide":u64::MAX,"list":[false,1,"value"]},"agent":{"private_path":"/synthetic/private"}}).to_string();
        bytes += record.len() as u64;
        db.execute("INSERT INTO usage_events(event_id,root_run_id,agent_run_id,event_type,created_at,record_json) VALUES(?1,?2,?3,'provider_request_completed',?4,?5)",params![id(n+100).as_str(),id(10).as_str(),id(11).as_str(),stamp,record]).unwrap();
    }
    db.execute(
        "INSERT INTO usage_backup_ids VALUES('root','synthetic-local-root',?1)",
        [id(20).as_str()],
    )
    .unwrap();
    db.execute(
        "INSERT INTO usage_backup_queue VALUES(?1,'released-binding',1,0)",
        [id(101).as_str()],
    )
    .unwrap();
    db.execute(
        "INSERT INTO usage_meta VALUES('unrelated-local-meta','preserved')",
        [],
    )
    .unwrap();
    drop(db);
    private_mode(&source, false).unwrap();
    let config = Config {
        expected_sha: file_hash(&source).unwrap(),
        source,
        expected_events: 257,
        expected_raw_bytes: bytes,
        as_of_ms: now,
        exports: None,
    };
    let result = run(&config).unwrap();
    assert_eq!(result["original_records"]["events"], 257);
    assert_eq!(result["projection_capture"]["passes"], 3);
    assert_eq!(result["projection_capture"]["examined_events"], 257);
    for target in result["compression_hypotheses"].as_array().unwrap() {
        assert_eq!(target["eligible_events"], 257);
        assert_eq!(target["raw_preserved_after_ack"], true);
        assert_eq!(target["restart_reused_frozen_bytes"], true);
    }
    assert_eq!(result["source_unchanged"], true);
}
