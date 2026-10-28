use super::{contract::*, queue::*};
use rusqlite::{params, Connection};
use serde_json::{json, Value};

fn uuid(index: u64) -> CanonicalUuid {
    CanonicalUuid::try_from(format!("00000000-0000-4000-8000-{index:012}")).unwrap()
}
fn counter(value: u64) -> Counter {
    Counter::try_from(value).unwrap()
}
fn source() -> Source {
    Source {
        account_binding: uuid(1),
        source_home_id: uuid(2),
        data_generation: counter(1),
    }
}
fn provenance(interval: u64) -> Provenance {
    Provenance {
        lifecycle_epoch: counter(1).into(),
        consent_interval: counter(interval),
    }
}
fn db() -> Connection {
    let mut db = Connection::open_in_memory().unwrap();
    db.execute_batch("PRAGMA foreign_keys=ON; CREATE TABLE usage_events(sequence INTEGER PRIMARY KEY AUTOINCREMENT,event_id TEXT UNIQUE NOT NULL,record_json TEXT NOT NULL); CREATE VIEW usage_event_records AS SELECT * FROM usage_events;").unwrap();
    initialize(&mut db).unwrap();
    db
}
fn projection(index: u64) -> Value {
    json!({"schema_version":1,"normalization_version":1,"event_id":uuid(index+100).as_str(),
        "root_run_id":uuid(10).as_str(),"agent_run_id":uuid(11).as_str(),
        "event_type":"provider_request","event_kind":"provider_request_completed","occurred_at_ms":1,
        "tokens":{"input_tokens":u64::MAX,"output_tokens":null}})
}
fn insert(db: &Connection, index: u64, restored: bool) -> String {
    let p = projection(index);
    let id = p["event_id"].as_str().unwrap().to_owned();
    let event = json!({"event_id":id,"root_run_id":p["root_run_id"],"agent_run_id":p["agent_run_id"],
        "event_type":"provider_request_completed","timestamp":"2026-10-27T00:00:00Z","backup_projection":p,"restored":restored});
    db.execute(
        "INSERT INTO usage_events(event_id,record_json) VALUES(?1,?2)",
        params![id, event.to_string()],
    )
    .unwrap();
    id
}
fn prepared() -> (Connection, Lease, FrozenBatch) {
    let mut db = db();
    enable(&mut db, &source(), &provenance(1), 100).unwrap();
    insert(&db, 1, false);
    insert(&db, 2, false);
    capture(&mut db, &source(), super::project).unwrap();
    let lease = acquire_lease(&mut db, &source(), 100, MAX_LEASE_MS)
        .unwrap()
        .unwrap();
    let batch = prepare_batch(&mut db, &lease, 100).unwrap().unwrap();
    (db, lease, batch)
}
fn commitment(batch: &FrozenBatch) -> AdmissionCommitment {
    let commitment = AdmissionCommitment {
        protocol_version: Version,
        source: batch.source().clone(),
        provenance: batch.provenance().clone(),
        batch_id: batch.id().clone(),
        ordered_membership_sha256: batch.membership().clone(),
        manifest_sha256: Sha256Hash::of(b"authenticated opaque server manifest"),
    };
    AdmissionCommitment::parse(&serde_json::to_string(&commitment).unwrap()).unwrap()
}
fn receipt(batch: &FrozenBatch, custody: Custody) -> Receipt {
    let c = commitment(batch);
    let receipt = Receipt {
        protocol_version: Version,
        source: c.source,
        provenance: c.provenance,
        batch_id: c.batch_id,
        manifest_sha256: c.manifest_sha256,
        ordered_membership_sha256: c.ordered_membership_sha256,
        custody,
        members: batch.members().to_vec(),
    };
    Receipt::parse(&serde_json::to_string(&receipt).unwrap()).unwrap()
}

#[test]
fn future_windows_exclude_disabled_gap_and_history_is_explicit() {
    let mut db = db();
    assert_eq!(enable(&mut db, &source(), &provenance(1), 0).unwrap(), 0);
    let first = insert(&db, 1, false);
    disable(&mut db, &source()).unwrap();
    let gap = insert(&db, 2, false);
    assert!(enable(&mut db, &source(), &provenance(1), 10).is_err());
    assert_eq!(enable(&mut db, &source(), &provenance(2), 10).unwrap(), 2);
    let third = insert(&db, 3, false);
    assert!(protected_event(&db, &first, 1).unwrap());
    assert!(!protected_event(&db, &gap, 2).unwrap());
    assert_eq!(capture(&mut db, &source(), super::project).unwrap(), 1);
    assert_eq!(capture(&mut db, &source(), super::project).unwrap(), 1);
    assert_eq!(status(&db, &source()).unwrap().pending, 2);
    assert!(!protected_event(&db, &gap, 2).unwrap());
    assert!(protected_event(&db, &third, 3).unwrap());
    select_history(&mut db, &source(), &provenance(2), 2).unwrap();
    assert!(protected_event(&db, &gap, 2).unwrap());
    capture(&mut db, &source(), super::project).unwrap();
    assert_eq!(status(&db, &source()).unwrap().pending, 3);
}

#[test]
fn bounded_capture_rolls_back_callback_failure_and_blocks_only_unverified_origins() {
    let mut db = db();
    enable(&mut db, &source(), &provenance(1), 0).unwrap();
    let restored = insert(&db, 1, true);
    let valid = insert(&db, 2, false);
    assert!(capture(&mut db, &source(), |_, _| Err(
        "projection unavailable".into()
    ))
    .is_err());
    assert_eq!(status(&db, &source()).unwrap().unscanned, 2);
    assert_eq!(status(&db, &source()).unwrap().origin_unverified, 0);
    assert_eq!(capture(&mut db, &source(), super::project).unwrap(), 1);
    let observed = status(&db, &source()).unwrap();
    assert_eq!(observed.origin_unverified, 1);
    assert_eq!(observed.unscanned, 0);
    assert!(protected_event(&db, &restored, 1).unwrap());
    assert!(protected_event(&db, &valid, 2).unwrap());
    assert_eq!(
        db.query_row(
            "SELECT COUNT(*) FROM usage_cloud_queue WHERE event_id=?1",
            [restored],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
}

#[test]
fn capture_and_freeze_obey_record_and_exact_wire_byte_bounds() {
    let mut db = db();
    enable(&mut db, &source(), &provenance(1), 0).unwrap();
    for index in 1..=101 {
        insert(&db, index, false);
    }
    let padded = |_: &Connection, raw: &Value| {
        let mut text = raw["backup_projection"].to_string();
        text.push_str(&" ".repeat(MAX_EVENT_BYTES - text.len()));
        Ok(text)
    };
    assert_eq!(capture(&mut db, &source(), padded).unwrap(), 100);
    assert_eq!(status(&db, &source()).unwrap().unscanned, 1);
    let lease = acquire_lease(&mut db, &source(), 0, MAX_LEASE_MS)
        .unwrap()
        .unwrap();
    let batch = prepare_batch(&mut db, &lease, 0).unwrap().unwrap();
    assert!(batch.request_json().len() < MAX_BATCH_BYTES);
    assert!(
        batch.request_json().len()
            + LANDING_RECORD_ALLOWANCE * batch.members().len()
            + LANDING_ENVELOPE_ALLOWANCE
            < MAX_BATCH_BYTES
    );
    assert!(batch.members().len() < 100);
    assert!(!batch.members().is_empty());
    Request::parse(batch.request_json()).unwrap();
    assert_eq!(capture(&mut db, &source(), padded).unwrap(), 1);
}

#[test]
fn unknown_dispatch_retries_the_same_frozen_bytes_and_old_backup_is_untouched() {
    let (mut db, lease, batch) = prepared();
    let event = batch.members()[0].event_id.clone();
    db.execute("INSERT INTO usage_backup_queue(event_id,binding,generation,acknowledged) VALUES(?1,'old-binding',1,0)",[event]).unwrap();
    begin_attempt(&mut db, &lease, batch.id(), 100).unwrap();
    assert_eq!(status(&db, &source()).unwrap().unknown_batches, 1);
    release_lease(&mut db, &lease, 101).unwrap();
    let lease = acquire_lease(&mut db, &source(), 102, MAX_LEASE_MS)
        .unwrap()
        .unwrap();
    let retried = prepare_batch(&mut db, &lease, 102).unwrap().unwrap();
    assert_eq!(batch.request_json(), retried.request_json());
    assert_eq!(batch.id(), retried.id());
    assert_eq!(
        db.query_row("SELECT acknowledged FROM usage_backup_queue", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn terminal_rejection_survives_disable_and_reinitialization_without_releasing_payload() {
    let (mut db, lease, batch) = prepared();
    begin_attempt(&mut db, &lease, batch.id(), 100).unwrap();
    let mut wrong = commitment(&batch);
    wrong.batch_id = uuid(99);
    assert!(record_terminal_unretained(&mut db, &lease, batch.id(), &wrong, 100).is_err());
    record_terminal_unretained(&mut db, &lease, batch.id(), &commitment(&batch), 100).unwrap();
    assert_eq!(
        status(&db, &source()).unwrap().terminal_unretained_batches,
        1
    );
    release_lease(&mut db, &lease, 101).unwrap();
    disable(&mut db, &source()).unwrap();
    initialize(&mut db).unwrap();
    let observed = status(&db, &source()).unwrap();
    assert_eq!(observed.terminal_unretained_batches, 1);
    assert_eq!(observed.pending, 2);
    assert_eq!(observed.retained, 0);
    for (index, member) in batch.members().iter().enumerate() {
        assert!(protected_event(&db, &member.event_id, index as i64 + 1).unwrap());
    }
    let stored: String = db
        .query_row("SELECT request_json FROM usage_cloud_batches", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(stored, batch.request_json());
    assert!(db.execute("DELETE FROM usage_cloud_queue", []).is_err());
}

#[test]
fn retained_ack_requires_authenticated_commitment_and_exact_members_and_preserves_payload() {
    let (mut db, lease, batch) = prepared();
    begin_attempt(&mut db, &lease, batch.id(), 100).unwrap();
    let retained = receipt(&batch, Custody::Retained);
    assert!(acknowledge_retained(&mut db, &lease, batch.id(), &retained, 100).is_err());
    let mut bad = commitment(&batch);
    bad.source.source_home_id = uuid(88);
    assert!(record_admission(&mut db, &lease, batch.id(), &bad, 100).is_err());
    record_buffered(&mut db, &lease, batch.id(), &commitment(&batch), 100).unwrap();
    assert_eq!(status(&db, &source()).unwrap().pending, 2);
    assert!(acknowledge_retained(
        &mut db,
        &lease,
        batch.id(),
        &receipt(&batch, Custody::Buffered),
        100
    )
    .is_err());
    let mut changed = retained.clone();
    changed.members.reverse();
    assert!(acknowledge_retained(&mut db, &lease, batch.id(), &changed, 100).is_err());
    changed = retained.clone();
    changed.manifest_sha256 = Sha256Hash::of(b"receipt supplied own hash");
    assert!(acknowledge_retained(&mut db, &lease, batch.id(), &changed, 100).is_err());
    acknowledge_retained(&mut db, &lease, batch.id(), &retained, 100).unwrap();
    assert_eq!(status(&db, &source()).unwrap().retained, 2);
    let id = &batch.members()[0].event_id;
    assert!(!protected_event(&db, id, 1).unwrap());
    db.execute("DELETE FROM usage_events", []).unwrap();
    assert_eq!(
        db.query_row("SELECT COUNT(*) FROM usage_cloud_queue", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        2
    );
    assert_eq!(
        db.query_row(
            "SELECT COUNT(*) FROM usage_cloud_batches WHERE receipt_json IS NOT NULL",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
}

#[test]
fn lease_tokens_expiry_disable_and_clock_recovery_fence_stale_acknowledgments() {
    let (mut db, lease, batch) = prepared();
    assert_eq!(lease.source(), &source());
    assert_eq!(lease.expires_ms(), 60_100);
    assert!(acquire_lease(&mut db, &source(), 101, MAX_LEASE_MS)
        .unwrap()
        .is_none());
    begin_attempt(&mut db, &lease, batch.id(), 100).unwrap();
    record_admission(&mut db, &lease, batch.id(), &commitment(&batch), 100).unwrap();
    assert!(release_lease(&mut db, &lease, 60_100).is_err());
    let replacement = acquire_lease(&mut db, &source(), 60_100, MAX_LEASE_MS)
        .unwrap()
        .unwrap();
    assert!(acknowledge_retained(
        &mut db,
        &lease,
        batch.id(),
        &receipt(&batch, Custody::Retained),
        60_100
    )
    .is_err());
    disable(&mut db, &source()).unwrap();
    assert!(begin_attempt(&mut db, &replacement, batch.id(), 60_101).is_err());
    assert!(protected_event(&db, &batch.members()[0].event_id, 1).unwrap());
    assert!(acquire_lease(&mut db, &source(), 60_101, MAX_LEASE_MS)
        .unwrap()
        .is_none());
}

#[test]
fn renewed_permission_requires_explicit_readmission_with_stable_events_and_old_attempt() {
    let (mut db, lease, old) = prepared();
    begin_attempt(&mut db, &lease, old.id(), 100).unwrap();
    disable(&mut db, &source()).unwrap();
    let next = Provenance {
        lifecycle_epoch: counter(2).into(),
        consent_interval: counter(2),
    };
    enable(&mut db, &source(), &next, 200).unwrap();
    let lease = acquire_lease(&mut db, &source(), 200, MAX_LEASE_MS)
        .unwrap()
        .unwrap();
    assert!(prepare_batch(&mut db, &lease, 200).unwrap().is_none());
    assert!(begin_attempt(&mut db, &lease, old.id(), 200).is_err());
    let new = readmit_batch(&mut db, &lease, old.id(), 200).unwrap();
    assert_ne!(old.id(), new.id());
    assert_eq!(old.members(), new.members());
    assert_eq!(old.membership(), new.membership());
    assert_eq!(new.provenance(), &next);
    assert_eq!(status(&db, &source()).unwrap().unknown_batches, 1);
    assert_eq!(
        db.query_row(
            "SELECT attempts FROM usage_cloud_batches WHERE batch_id=?1",
            [old.id().as_str()],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
}

#[test]
fn immutable_guards_reject_payload_member_and_commitment_replacement() {
    let (mut db, lease, batch) = prepared();
    assert!(db
        .execute("UPDATE usage_cloud_batches SET request_json='{}'", [])
        .is_err());
    assert!(db
        .execute(
            "UPDATE usage_cloud_batch_members SET original_sha256='changed'",
            []
        )
        .is_err());
    assert!(db
        .execute("UPDATE usage_cloud_queue SET original_text='changed'", [])
        .is_err());
    assert!(db
        .execute("DELETE FROM usage_cloud_batch_members", [])
        .is_err());
    assert!(db.execute("DELETE FROM usage_cloud_batches", []).is_err());
    assert!(db.execute("DELETE FROM usage_cloud_queue", []).is_err());
    begin_attempt(&mut db, &lease, batch.id(), 100).unwrap();
    record_admission(&mut db, &lease, batch.id(), &commitment(&batch), 100).unwrap();
    let mut changed = commitment(&batch);
    changed.manifest_sha256 = Sha256Hash::of(b"replacement");
    assert!(record_admission(&mut db, &lease, batch.id(), &changed, 100).is_err());
}

#[test]
fn scheduler_persists_hourly_due_bounded_backoff_and_clock_jump_catchup() {
    let (mut db, lease, _) = prepared();
    assert!(due(&mut db, &source(), 100).unwrap());
    assert_eq!(
        schedule_next(&mut db, &lease, 100, true).unwrap(),
        HOUR_MS + 100
    );
    assert!(!due(&mut db, &source(), 101).unwrap());
    assert_eq!(schedule_next(&mut db, &lease, 102, false).unwrap(), 30_102);
    assert_eq!(schedule_next(&mut db, &lease, 103, false).unwrap(), 60_103);
    assert!(due(&mut db, &source(), 0).unwrap());
    let replacement = acquire_lease(&mut db, &source(), 0, 1000).unwrap().unwrap();
    assert!(release_lease(&mut db, &lease, 0).is_err());
    release_lease(&mut db, &replacement, 1).unwrap();
}

#[test]
fn every_binding_generation_and_quarantine_obligation_protects_retention() {
    let (mut db, lease, batch) = prepared();
    let id = batch.members()[0].event_id.clone();
    quarantine_event(&mut db, &source(), &id).unwrap();
    disable(&mut db, &source()).unwrap();
    assert!(protected_event(&db, &id, 1).unwrap());
    let other = Source {
        data_generation: counter(2),
        ..source()
    };
    enable(&mut db, &other, &provenance(1), 100).unwrap();
    select_history(&mut db, &other, &provenance(1), 2).unwrap();
    assert!(protected_event(&db, &batch.members()[1].event_id, 2).unwrap());
    assert!(release_lease(&mut db, &lease, 101).is_err());
}

#[test]
fn passive_retention_probes_never_install_or_assume_an_incomplete_schema() {
    let db = Connection::open_in_memory().unwrap();
    assert!(!installed(&db).unwrap());
    assert!(!protected_event(&db, "event", 1).unwrap());
    assert!(!retained_event(&db, "event").unwrap());
    assert_eq!(status(&db, &source()).unwrap().pending, 0);
    assert_eq!(
        db.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table'",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    db.execute("CREATE TABLE usage_cloud_state(dummy TEXT)", [])
        .unwrap();
    assert!(protected_event(&db, "event", 1).is_err());
    assert!(retained_event(&db, "event").is_err());
}

#[test]
fn account_envelope_syntax_rejects_duplicate_fields_before_value_decoding() {
    assert!(
        parse_account_document(r#"{"action":"usage_analytics","action":"usage_analytics"}"#)
            .is_err()
    );
    assert!(parse_account_document(
        r#"{"credentials":{"access_token":"one","access_token":"two"}}"#
    )
    .is_err());
    let exact =
        parse_account_document(r#"{"usage_analytics":{"tokens":18446744073709551615}}"#).unwrap();
    assert_eq!(exact["usage_analytics"]["tokens"].as_u64(), Some(u64::MAX));
    assert!(parse_account_document(&" ".repeat(MAX_ACCOUNT_DOCUMENT_BYTES)).is_err());
    assert_eq!(LifecycleEpoch::try_from(0).unwrap().get(), 0);
    assert!(Counter::try_from(0).is_err());
}

#[test]
fn explicit_clear_invalidates_known_copy_and_stale_generation_without_discarding_pending() {
    let (mut db, lease, batch) = prepared();
    begin_attempt(&mut db, &lease, batch.id(), 100).unwrap();
    record_admission(&mut db, &lease, batch.id(), &commitment(&batch), 100).unwrap();
    acknowledge_retained(
        &mut db,
        &lease,
        batch.id(),
        &receipt(&batch, Custody::Retained),
        100,
    )
    .unwrap();
    let retained = &batch.members()[0].event_id;
    disable(&mut db, &source()).unwrap();
    assert!(retained_event(&db, retained).unwrap());
    enable(&mut db, &source(), &provenance(2), 200).unwrap();
    let pending = insert(&db, 3, false);
    mark_cleared(&mut db, &source()).unwrap();
    assert!(!retained_event(&db, retained).unwrap());
    assert!(protected_event(&db, &pending, 3).unwrap());
    let observed = status(&db, &source()).unwrap();
    assert!(observed.cleared);
    assert_eq!(observed.retained, 0);
    assert!(!observed.enabled);
    assert!(enable(&mut db, &source(), &provenance(3), 300).is_err());
    assert!(acquire_lease(&mut db, &source(), 300, MAX_LEASE_MS)
        .unwrap()
        .is_none());
    assert_eq!(
        db.query_row("SELECT COUNT(*) FROM usage_cloud_queue", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        2
    );
    let other = Source {
        data_generation: counter(2),
        ..source()
    };
    enable(&mut db, &other, &provenance(1), 300).unwrap();
    assert!(!status(&db, &other).unwrap().cleared);
}

#[test]
fn shared_connection_leases_serialize_simultaneous_process_candidates() {
    let path = std::env::temp_dir().join(format!(
        "cargo-ai-cloud-lease-{}.sqlite3",
        uuid::Uuid::new_v4()
    ));
    let mut first = Connection::open(&path).unwrap();
    first.execute_batch("CREATE TABLE usage_events(sequence INTEGER PRIMARY KEY AUTOINCREMENT,event_id TEXT UNIQUE,record_json TEXT); CREATE VIEW usage_event_records AS SELECT * FROM usage_events;").unwrap();
    initialize(&mut first).unwrap();
    enable(&mut first, &source(), &provenance(1), 100).unwrap();
    let second = Connection::open(&path).unwrap();
    first
        .busy_timeout(std::time::Duration::from_secs(2))
        .unwrap();
    second
        .busy_timeout(std::time::Duration::from_secs(2))
        .unwrap();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let handles: Vec<_> = [first, second]
        .into_iter()
        .map(|mut connection| {
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                acquire_lease(&mut connection, &source(), 100, MAX_LEASE_MS)
                    .unwrap()
                    .is_some()
            })
        })
        .collect();
    assert_eq!(
        handles
            .into_iter()
            .map(|thread| usize::from(thread.join().unwrap()))
            .sum::<usize>(),
        1
    );
    std::fs::remove_file(path).unwrap();
}

#[test]
fn timing_is_closed_original_bound_and_does_not_rewrite_projection_identity() {
    let mut value = projection(90);
    value["elapsed_ms"] = json!(42);
    let text = value.to_string();
    let legacy = EventSubmission::from_allowed_text(text.clone()).unwrap();
    let timed = EventSubmission::from_allowed_text_with_timing(
        text.clone(),
        Some(TimingSupplement::monotonic(42).unwrap()),
    )
    .unwrap();
    assert_eq!(legacy.original_text, timed.original_text);
    assert_eq!(legacy.original_sha256, timed.original_sha256);
    assert_eq!(legacy.semantic_sha256, timed.semantic_sha256);
    assert_eq!(
        serde_json::to_value(legacy.member().unwrap())
            .unwrap()
            .as_object()
            .unwrap()
            .len(),
        4
    );
    assert!(legacy.member().unwrap().timing_sha256.is_none());
    assert!(timed.member().unwrap().timing_sha256.is_some());
    assert_ne!(
        ordered_membership_sha256(&source(), &[legacy.member().unwrap()]).unwrap(),
        ordered_membership_sha256(&source(), &[timed.member().unwrap()]).unwrap()
    );
    let formatted =
        EventSubmission::from_allowed_text_with_timing(format!(" {text} "), timed.timing.clone())
            .unwrap();
    assert_eq!(formatted.semantic_sha256, timed.semantic_sha256);
    assert_ne!(
        formatted.member().unwrap().timing_sha256,
        timed.member().unwrap().timing_sha256
    );
    assert!(EventSubmission::from_allowed_text_with_timing(
        text,
        Some(TimingSupplement::monotonic(43).unwrap())
    )
    .is_err());
    for invalid in [
        json!({"version":2,"basis":"monotonic_elapsed","elapsed_ms":42}),
        json!({"version":1,"basis":"wall_clock","elapsed_ms":42}),
        json!({"version":1,"basis":"monotonic_elapsed","elapsed_ms":42,"server_clock":true}),
        json!({"version":1,"basis":"monotonic_elapsed","elapsed_ms":1.5}),
    ] {
        assert!(serde_json::from_value::<TimingSupplement>(invalid).is_err());
    }
    assert!(timing_from_local_capture(
        &json!({"duration_ms":42,"timestamp_basis":"observed_request_start_and_completion"})
    )
    .unwrap()
    .is_none());
    assert!(timing_from_local_capture(&json!({"restored":true,"elapsed_timing":{"version":1,"basis":"monotonic_elapsed","elapsed_ms":42}})).unwrap().is_none());
}

#[test]
fn timing_survives_file_reopen_and_exact_receipt_controls_custody() {
    let path = std::env::temp_dir().join(format!("usage-timing-{}.sqlite", uuid::Uuid::new_v4()));
    let mut db = Connection::open(&path).unwrap();
    db.execute_batch("CREATE TABLE usage_events(sequence INTEGER PRIMARY KEY AUTOINCREMENT,event_id TEXT UNIQUE NOT NULL,record_json TEXT NOT NULL);CREATE VIEW usage_event_records AS SELECT * FROM usage_events;").unwrap();
    initialize(&mut db).unwrap();
    enable(&mut db, &source(), &provenance(1), 100).unwrap();
    let mut p = projection(91);
    p["elapsed_ms"] = json!(3);
    let id = p["event_id"].as_str().unwrap().to_owned();
    let raw = json!({"event_id":id,"root_run_id":p["root_run_id"],"agent_run_id":p["agent_run_id"],"event_type":"provider_request_completed","timestamp":"1900-01-01T00:00:00Z","duration_ms":3,"backup_projection":p,"elapsed_timing":{"version":1,"basis":"monotonic_elapsed","elapsed_ms":3}});
    db.execute(
        "INSERT INTO usage_events(event_id,record_json) VALUES(?1,?2)",
        params![id, raw.to_string()],
    )
    .unwrap();
    capture(&mut db, &source(), super::project).unwrap();
    let lease = acquire_lease(&mut db, &source(), 100, MAX_LEASE_MS)
        .unwrap()
        .unwrap();
    let batch = prepare_batch(&mut db, &lease, 100).unwrap().unwrap();
    let before = batch.request_json().to_owned();
    assert!(batch.members()[0].timing_sha256.is_some());
    assert!(db
        .execute("UPDATE usage_cloud_queue SET timing_json=NULL", [])
        .is_err());
    begin_attempt(&mut db, &lease, batch.id(), 100).unwrap();
    record_admission(&mut db, &lease, batch.id(), &commitment(&batch), 100).unwrap();
    let mut missing = receipt(&batch, Custody::Retained);
    missing.members[0].timing_sha256 = None;
    assert!(acknowledge_retained(&mut db, &lease, batch.id(), &missing, 100).is_err());
    assert!(protected_event(&db, &id, 1).unwrap());
    drop(db);
    let mut db = Connection::open(&path).unwrap();
    initialize(&mut db).unwrap();
    let now = MAX_LEASE_MS + 101;
    let lease = acquire_lease(&mut db, &source(), now, MAX_LEASE_MS)
        .unwrap()
        .unwrap();
    let recovered = prepare_batch(&mut db, &lease, now).unwrap().unwrap();
    assert_eq!(before, recovered.request_json());
    acknowledge_retained(
        &mut db,
        &lease,
        recovered.id(),
        &receipt(&recovered, Custody::Retained),
        now,
    )
    .unwrap();
    assert!(!protected_event(&db, &id, 1).unwrap());
    assert_eq!(status(&db, &source()).unwrap().retained, 1);
    drop(db);
    std::fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn completion_based_timer_wakes_when_real_queue_due_after_nonzero_pass() {
    use std::{
        sync::{Arc, Mutex},
        time::{Duration, Instant},
    };
    struct State {
        db: Connection,
        settled: Option<(Instant, i64)>,
        observations: Vec<bool>,
        completed: usize,
    }
    let mut db = db();
    enable(&mut db, &source(), &provenance(1), 100).unwrap();
    let state = Arc::new(Mutex::new(State {
        db,
        settled: None,
        observations: Vec::new(),
        completed: 0,
    }));
    let (done, ready) = tokio::sync::oneshot::channel();
    let done = Arc::new(Mutex::new(Some(done)));
    let observed = state.clone();
    let period = Duration::from_millis(60);
    super::scheduler::scope_with(
        async {
            tokio::time::timeout(Duration::from_secs(3), ready)
                .await
                .unwrap()
                .unwrap();
        },
        period,
        move || {
            let state = state.clone();
            let done = done.clone();
            async move {
                let attempt = {
                    let mut state = state.lock().unwrap();
                    let now = state
                        .settled
                        .map(|(at, clock)| {
                            clock.saturating_add(
                                (at.elapsed().as_nanos() * HOUR_MS as u128 / period.as_nanos())
                                    .min(i64::MAX as u128) as i64,
                            )
                        })
                        .unwrap_or(100);
                    let due = due(&mut state.db, &source(), now).unwrap();
                    state.observations.push(due);
                    if due {
                        Some((
                            acquire_lease(&mut state.db, &source(), now, MAX_LEASE_MS)
                                .unwrap()
                                .unwrap(),
                            now,
                        ))
                    } else {
                        None
                    }
                };
                if let Some((lease, start)) = attempt {
                    tokio::time::sleep(Duration::from_millis(25)).await;
                    let mut state = state.lock().unwrap();
                    let finish = start + 25;
                    assert_eq!(
                        schedule_next(&mut state.db, &lease, finish, true).unwrap(),
                        finish + HOUR_MS
                    );
                    release_lease(&mut state.db, &lease, finish).unwrap();
                    state.settled = Some((Instant::now(), finish));
                    state.completed += 1;
                    if state.completed == 2 {
                        if let Some(done) = done.lock().unwrap().take() {
                            done.send(()).unwrap();
                        }
                    }
                }
            }
        },
    )
    .await;
    let state = observed.lock().unwrap();
    assert_eq!(
        &state.observations[..2],
        &[true, true],
        "the first scheduled wake after a nonzero pass must reach the persisted due time"
    );
    assert_eq!(state.completed, 2);
}

#[test]
fn clear_intent_preserves_exact_attempt_and_protects_retained_and_pending_history() {
    let (mut db, lease, batch) = prepared();
    begin_attempt(&mut db, &lease, batch.id(), 100).unwrap();
    record_admission(&mut db, &lease, batch.id(), &commitment(&batch), 100).unwrap();
    acknowledge_retained(
        &mut db,
        &lease,
        batch.id(),
        &receipt(&batch, Custody::Retained),
        100,
    )
    .unwrap();
    let retained = batch.members()[0].event_id.clone();
    let pending = insert(&db, 3, false);
    capture(&mut db, &source(), super::project).unwrap();
    let request = Request::ClearConfirm {
        protocol_version: Version,
        source: source(),
        lifecycle_epoch: provenance(1).lifecycle_epoch,
        preview_id: uuid(70),
        preview_sha256: Sha256Hash::of(b"exact preview"),
    };
    begin_clear(&mut db, &request).unwrap();
    assert!(clear_outcome_unknown(&db, &source()).unwrap());
    assert!(!retained_event(&db, &retained).unwrap());
    assert!(protected_event(&db, &retained, 1).unwrap());
    assert!(protected_event(&db, &pending, 3).unwrap());
    assert!(!status(&db, &source()).unwrap().enabled);
    assert_eq!(status(&db, &source()).unwrap().retained, 0);
    assert!(release_lease(&mut db, &lease, 100).is_err());
    assert!(enable(&mut db, &source(), &provenance(2), 200).is_err());
    begin_clear(&mut db, &request).unwrap();
    let mut changed = request.clone();
    if let Request::ClearConfirm { preview_sha256, .. } = &mut changed {
        *preview_sha256 = Sha256Hash::of(b"changed preview");
    }
    begin_clear(&mut db, &changed).unwrap();
    let frozen: String = db
        .query_row(
            "SELECT clear_intent_json FROM usage_cloud_state",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Vec<Request>>(&frozen).unwrap(),
        vec![request, changed]
    );
    mark_cleared(&mut db, &source()).unwrap();
    assert!(!clear_outcome_unknown(&db, &source()).unwrap());
    assert!(!retained_event(&db, &retained).unwrap());
    assert!(!protected_event(&db, &retained, 1).unwrap());
    assert!(protected_event(&db, &pending, 3).unwrap());
    assert_eq!(
        db.query_row(
            "SELECT COUNT(*) FROM usage_cloud_queue WHERE original_text IS NOT NULL",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        3
    );
}

#[test]
fn existing_queue_upgrades_clear_intent_metadata_without_rewriting_custody() {
    let (mut db, lease, batch) = prepared();
    begin_attempt(&mut db, &lease, batch.id(), 100).unwrap();
    record_admission(&mut db, &lease, batch.id(), &commitment(&batch), 100).unwrap();
    acknowledge_retained(
        &mut db,
        &lease,
        batch.id(),
        &receipt(&batch, Custody::Retained),
        100,
    )
    .unwrap();
    let before: String = db
        .query_row("SELECT request_json FROM usage_cloud_batches", [], |row| {
            row.get(0)
        })
        .unwrap();
    db.execute_batch("ALTER TABLE usage_cloud_state DROP COLUMN clear_intent_json;")
        .unwrap();
    assert!(retained_event(&db, &batch.members()[0].event_id).unwrap());
    initialize(&mut db).unwrap();
    assert!(!clear_outcome_unknown(&db, &source()).unwrap());
    assert!(retained_event(&db, &batch.members()[0].event_id).unwrap());
    assert_eq!(
        db.query_row("SELECT request_json FROM usage_cloud_batches", [], |row| {
            row.get::<_, String>(0)
        })
        .unwrap(),
        before
    );
}

#[test]
fn generation_reconciliation_invalidates_copies_left_under_older_unselected_sources() {
    let (mut db, lease, batch) = prepared();
    begin_attempt(&mut db, &lease, batch.id(), 100).unwrap();
    record_admission(&mut db, &lease, batch.id(), &commitment(&batch), 100).unwrap();
    acknowledge_retained(
        &mut db,
        &lease,
        batch.id(),
        &receipt(&batch, Custody::Retained),
        100,
    )
    .unwrap();
    let mut newer = source();
    newer.data_generation = counter(2);
    enable(&mut db, &newer, &provenance(2), 100).unwrap();
    let mut other = source();
    other.account_binding = uuid(90);
    enable(&mut db, &other, &provenance(1), 100).unwrap();
    assert!(retained_event(&db, &batch.members()[0].event_id).unwrap());
    reconcile_generation(&mut db, &newer.account_binding, newer.data_generation).unwrap();
    assert!(!retained_event(&db, &batch.members()[0].event_id).unwrap());
    assert!(status(&db, &source()).unwrap().cleared);
    assert!(!status(&db, &newer).unwrap().cleared);
    assert!(!status(&db, &other).unwrap().cleared);
    assert!(status(&db, &other).unwrap().enabled);
    assert_eq!(
        db.query_row(
            "SELECT COUNT(*) FROM usage_cloud_batches WHERE receipt_json IS NOT NULL",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
}
