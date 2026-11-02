use super::*;
use queue::*;
fn uuid(index: u64) -> CanonicalUuid {
    CanonicalUuid::try_from(format!("00000000-0000-4000-8000-{index:012}")).unwrap()
}
fn source() -> Source {
    Source {
        account_binding: uuid(1),
        source_home_id: uuid(2),
        data_generation: Counter::try_from(1).unwrap(),
    }
}
fn provenance() -> Provenance {
    Provenance {
        lifecycle_epoch: LifecycleEpoch::try_from(0).unwrap(),
        consent_interval: Counter::try_from(1).unwrap(),
    }
}
fn db() -> Connection {
    let mut db = Connection::open_in_memory().unwrap();
    db.execute_batch("PRAGMA foreign_keys=ON; CREATE TABLE usage_events(sequence INTEGER PRIMARY KEY AUTOINCREMENT,event_id TEXT UNIQUE NOT NULL,record_json TEXT NOT NULL);CREATE VIEW usage_event_records AS SELECT * FROM usage_events;").unwrap();
    initialize(&mut db).unwrap();
    db
}
fn insert(db: &Connection, index: u64, age_ms: i64) -> String {
    let id = uuid(index + 100).as_str().to_owned();
    let time =
        time::OffsetDateTime::from_unix_timestamp_nanos(i128::from(now_ms() - age_ms) * 1_000_000)
            .unwrap()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap();
    let raw = json!({"schema_version":1,"event_id":id,"root_run_id":uuid(10),"agent_run_id":uuid(11),"event_type":"provider_request_completed","timestamp":time,"usage":{"input_tokens":1}});
    db.execute(
        "INSERT INTO usage_events(event_id,record_json) VALUES(?1,?2)",
        params![id, raw.to_string()],
    )
    .unwrap();
    id
}
fn leased(db: &mut Connection) -> Lease {
    queue::enable(db, &source(), &provenance(), now_ms()).unwrap();
    queue::acquire_lease(db, &source(), now_ms(), MAX_LEASE_MS)
        .unwrap()
        .unwrap()
}
#[test]
fn job_freezes_exact_committed_membership_and_window_anchor_across_resume() {
    let mut db = db();
    insert(&db, 1, 86_400_000 * 5);
    let lease = leased(&mut db);
    insert(&db, 2, 2000);
    let anchor = now_ms();
    let job = job::start(&mut db, &lease, anchor, Some(30)).unwrap();
    assert_eq!(job::progress(&db, &job).unwrap()["target_events"], 2);
    insert(&db, 3, 0);
    let resumed = job::start(&mut db, &lease, anchor + 10_000, None).unwrap();
    assert_eq!(resumed.id, job.id);
    assert_eq!(resumed.started_ms, anchor);
    assert_eq!(resumed.through, 2);
    assert_eq!(job::progress(&db, &resumed).unwrap()["target_events"], 2);
    assert!(job::start(&mut db, &lease, anchor, Some(7)).is_err());
    assert!(job::start(&mut db, &lease, anchor, Some(31)).is_err());
    assert!(db
        .execute("UPDATE usage_cloud_jobs SET through_sequence=99", [])
        .is_err());
    assert!(db.execute("INSERT INTO usage_cloud_job_members(job_id,event_id,sequence,occurred_ms) VALUES(?1,'new-arrival',99,0)",[&job.id]).is_err());
    assert!(db
        .execute("UPDATE usage_cloud_job_members SET event_id='changed'", [])
        .is_err());
}
#[test]
fn future_only_selection_excludes_pre_enable_history_and_sends_oldest_first() {
    let mut db = db();
    insert(&db, 1, 10_000);
    let lease = leased(&mut db);
    let newer = insert(&db, 2, 1000);
    let older = insert(&db, 3, 2000);
    let job = job::start(&mut db, &lease, now_ms(), None).unwrap();
    job::capture(&mut db, &lease, &job, now_ms()).unwrap();
    let batch = prepare_job_upload(&mut db, &lease, now_ms(), &job.id, build_upload)
        .unwrap()
        .unwrap();
    assert_eq!(
        batch
            .members()
            .iter()
            .map(|m| m.event_id.clone())
            .collect::<Vec<_>>(),
        vec![older, newer]
    );
    let retried = prepare_job_upload(&mut db, &lease, now_ms(), &job.id, build_upload)
        .unwrap()
        .unwrap();
    assert_eq!(retried.request_json(), batch.request_json());
    acknowledge_uploaded(&mut db, &lease, batch.id(), now_ms()).unwrap();
    let progress = job::finish_pass(&db, &job, None, None, "fully_synced").unwrap();
    assert_eq!(progress["uploaded_events"], 2);
    assert_eq!(progress["pending_events"], 0);
    assert_eq!(high(&db).unwrap(), 3);
    assert!(job::active(&db, &source()).unwrap().is_none());
}
#[test]
fn independent_expiry_reports_uncaptured_members_without_changing_target() {
    let mut db = db();
    let lease = leased(&mut db);
    let expired = insert(&db, 1, 2000);
    insert(&db, 2, 1000);
    let job = job::start(&mut db, &lease, now_ms(), None).unwrap();
    let tx = db.transaction().unwrap();
    expire_events(&tx, std::slice::from_ref(&expired), now_ms()).unwrap();
    tx.execute("DELETE FROM usage_events WHERE event_id=?1", [expired])
        .unwrap();
    tx.commit().unwrap();
    job::capture(&mut db, &lease, &job, now_ms()).unwrap();
    let progress = job::progress(&db, &job).unwrap();
    assert_eq!(progress["target_events"], 2);
    assert_eq!(progress["expired_events"], 1);
    assert_eq!(progress["pending_events"], 1);
}
#[test]
fn environment_lease_fences_concurrent_senders_and_stale_acks() {
    let mut db = db();
    let lease = leased(&mut db);
    assert!(acquire_lease(&mut db, &source(), now_ms(), MAX_LEASE_MS)
        .unwrap()
        .is_none());
    insert(&db, 1, 0);
    let job = job::start(&mut db, &lease, now_ms(), None).unwrap();
    job::capture(&mut db, &lease, &job, now_ms()).unwrap();
    let batch = prepare_job_upload(&mut db, &lease, now_ms(), &job.id, build_upload)
        .unwrap()
        .unwrap();
    queue::disable(&mut db, &source()).unwrap();
    assert!(acknowledge_uploaded(&mut db, &lease, batch.id(), now_ms()).is_err());
    assert_eq!(job::progress(&db, &job).unwrap()["uploaded_events"], 0);
}
#[test]
fn partial_remote_expiry_preserves_remaining_job_and_rejects_foreign_ids() {
    let mut db = db();
    let lease = leased(&mut db);
    let expired = insert(&db, 1, 2000);
    let kept = insert(&db, 2, 1000);
    let job = job::start(&mut db, &lease, now_ms(), None).unwrap();
    job::capture(&mut db, &lease, &job, now_ms()).unwrap();
    let batch = prepare_job_upload(&mut db, &lease, now_ms(), &job.id, build_upload)
        .unwrap()
        .unwrap();
    assert!(job::expire_batch_members(
        &mut db,
        &lease,
        &job,
        &batch,
        &[uuid(999).as_str().to_owned()],
        now_ms()
    )
    .is_err());
    job::expire_batch_members(&mut db, &lease, &job, &batch, &[expired], now_ms()).unwrap();
    let remaining = prepare_job_upload(&mut db, &lease, now_ms(), &job.id, build_upload)
        .unwrap()
        .unwrap();
    assert_eq!(remaining.members().len(), 1);
    assert_eq!(remaining.members()[0].event_id, kept);
    assert_ne!(remaining.id(), batch.id());
    assert_eq!(high(&db).unwrap(), 2);
}
#[test]
fn capture_transactions_are_bounded_and_do_not_admit_new_arrivals() {
    let mut db = db();
    let lease = leased(&mut db);
    for n in 0..203 {
        insert(&db, n, 1000 + n as i64);
    }
    let job = job::start(&mut db, &lease, now_ms(), None).unwrap();
    insert(&db, 999, 0);
    assert_eq!(job::capture(&mut db, &lease, &job, now_ms()).unwrap(), 100);
    assert_eq!(job::capture(&mut db, &lease, &job, now_ms()).unwrap(), 100);
    assert_eq!(job::capture(&mut db, &lease, &job, now_ms()).unwrap(), 3);
    assert_eq!(job::capture(&mut db, &lease, &job, now_ms()).unwrap(), 0);
}

#[test]
fn retry_reservations_survive_restart_and_fence_a_crashed_fourth_attempt() {
    let mut db = db();
    let mut lease = leased(&mut db);
    insert(&db, 1, 1000);
    let job = job::start(&mut db, &lease, now_ms(), None).unwrap();
    job::capture(&mut db, &lease, &job, now_ms()).unwrap();
    let batch = prepare_job_upload(&mut db, &lease, now_ms(), &job.id, build_upload)
        .unwrap()
        .unwrap();
    for expected in 1..=4 {
        let now = now_ms();
        assert_eq!(
            reserve_upload_attempt(&mut db, &lease, &job.id, &batch, now, 10_000).unwrap(),
            expected
        );
        assert!(reserve_upload_attempt(&mut db, &lease, &job.id, &batch, now + 1, 10_000).is_err());
        if expected < 4 {
            pause_upload(
                &mut db,
                &lease,
                &job.id,
                &batch,
                Some(now),
                "retry_backoff",
                "outcome_unknown",
                now,
            )
            .unwrap();
        }
    }
    let restored = job::active(&db, &source()).unwrap().unwrap();
    assert_eq!(restored.retry_reason.as_deref(), Some("retry_cooldown"));
    assert!(restored.retry_at_ms.unwrap() - now_ms() >= 309_000);
    let original = batch.request_json().to_owned();
    let due = restored.retry_at_ms.unwrap();
    // A new process may acquire a new lease after the old token expires.
    lease = acquire_lease(&mut db, &source(), due, MAX_LEASE_MS)
        .unwrap()
        .unwrap();
    assert_eq!(
        reserve_upload_attempt(&mut db, &lease, &job.id, &batch, due, 10_000).unwrap(),
        1
    );
    assert_eq!(
        prepare_job_upload(&mut db, &lease, due, &job.id, build_upload)
            .unwrap()
            .unwrap()
            .request_json(),
        original
    );
}

#[test]
fn retry_jitter_has_the_full_bounded_support() {
    for (attempt, cap) in [(1, 1000), (2, 2000), (3, 4000)] {
        let values = (0..2000)
            .map(|_| retry_delay_ms(attempt))
            .collect::<Vec<_>>();
        assert!(values.iter().all(|value| (0..=cap).contains(value)));
        assert!(values.iter().any(|value| *value < cap / 4));
        assert!(values.iter().any(|value| *value > cap * 3 / 4));
    }
}

#[test]
fn small_events_fill_the_count_bound_and_flush_an_exact_tail() {
    let mut db = db();
    let lease = leased(&mut db);
    for n in 0..4100 {
        insert(&db, n, 10_000 - n as i64);
    }
    let job = job::start(&mut db, &lease, now_ms(), None).unwrap();
    while job::capture(&mut db, &lease, &job, now_ms()).unwrap() > 0 {}
    let batch = prepare_job_upload(&mut db, &lease, now_ms(), &job.id, build_upload)
        .unwrap()
        .unwrap();
    assert_eq!(batch.members().len(), elastic::MAX_BATCH_EVENTS);
    let request = batch.request_json().parse::<elastic::Request>().unwrap();
    let elastic::Request::Upload { batch: encoded, .. } = request else {
        panic!("expected upload")
    };
    assert_eq!(elastic::decode_batch(&encoded).unwrap().len(), 4096);
    assert!((encoded.decoded_bytes as usize) <= elastic::MAX_DECODED_BYTES);
    acknowledge_uploaded(&mut db, &lease, batch.id(), now_ms()).unwrap();
    let tail = prepare_job_upload(&mut db, &lease, now_ms(), &job.id, build_upload)
        .unwrap()
        .unwrap();
    assert_eq!(tail.members().len(), 4);
    assert!(batch
        .members()
        .iter()
        .all(|member| !tail.members().contains(member)));
}

#[test]
fn older_retry_metadata_opens_passively_and_upgrades_without_changing_frozen_bytes() {
    let mut db = db();
    let lease = leased(&mut db);
    insert(&db, 1, 1000);
    let job = job::start(&mut db, &lease, now_ms(), None).unwrap();
    job::capture(&mut db, &lease, &job, now_ms()).unwrap();
    let frozen = prepare_job_upload(&mut db, &lease, now_ms(), &job.id, build_upload)
        .unwrap()
        .unwrap()
        .request_json()
        .to_owned();
    db.execute_batch("ALTER TABLE usage_cloud_jobs DROP COLUMN retry_reason; ALTER TABLE usage_cloud_upload_batches DROP COLUMN retry_reason; ALTER TABLE usage_cloud_upload_batches DROP COLUMN retry_at_ms; ALTER TABLE usage_cloud_upload_batches DROP COLUMN retry_attempts;").unwrap();
    let passive = job::latest(&db, &source()).unwrap().unwrap();
    assert_eq!(passive.id, job.id);
    assert!(passive.retry_reason.is_none());
    assert!(!db.query_row("SELECT EXISTS(SELECT 1 FROM pragma_table_info('usage_cloud_jobs') WHERE name='retry_reason')",[],|r|r.get::<_,bool>(0)).unwrap());
    let resumed = job::resume(&mut db, &lease, now_ms()).unwrap();
    assert_eq!(resumed.id, job.id);
    assert_eq!(
        prepare_job_upload(&mut db, &lease, now_ms(), &job.id, build_upload)
            .unwrap()
            .unwrap()
            .request_json(),
        frozen
    );
}
