//! Independent, explicitly consented cloud custody for immutable usage history.
#[cfg(test)]
#[path = "usage_analytics/client_tests.rs"]
mod client_tests;
#[cfg(any(cargo_ai_cli, test))]
#[path = "usage_analytics/contract.rs"]
pub(crate) mod contract;
#[cfg(any(cargo_ai_cli, test))]
#[path = "usage_analytics/elastic.rs"]
pub(crate) mod elastic;
#[cfg(any(cargo_ai_cli, test))]
#[path = "usage_analytics/queue.rs"]
pub(crate) mod queue;
#[cfg(not(any(cargo_ai_cli, test)))]
#[path = "usage_analytics/local_queue.rs"]
pub(crate) mod queue;
#[path = "usage_analytics/scheduler.rs"]
pub(crate) mod scheduler;
#[cfg(test)]
#[path = "usage_analytics/tests.rs"]
mod tests;
#[cfg(any(cargo_ai_cli, test))]
#[path = "usage_analytics/transport.rs"]
mod transport;
#[cfg(test)]
thread_local! { pub(crate) static TEST_ENDPOINT: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) }; }

#[cfg(any(cargo_ai_cli, test))]
use contract::*;
#[cfg(any(cargo_ai_cli, test))]
use elastic::{Request, Version};
#[cfg(any(cargo_ai_cli, test))]
use rusqlite::{params, Connection, OptionalExtension};
#[cfg(any(cargo_ai_cli, test))]
use serde::{Deserialize, Serialize};
#[cfg(any(cargo_ai_cli, test))]
use serde_json::{json, Value};
#[cfg(any(cargo_ai_cli, test))]
use std::time::{Duration, Instant};
// A previous experimental selection cannot grant this protocol's cloud consent.
#[cfg(any(cargo_ai_cli, test))]
const SELECTED: &str = "usage_cloud_selected_v2";
#[cfg(cargo_ai_cli)]
const HOME: &str = "usage_cloud_home_id_v1";
#[cfg(any(cargo_ai_cli, test))]
const DEADLINE: Duration = Duration::from_secs(15);
#[cfg(any(cargo_ai_cli, test))]
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
#[cfg(any(cargo_ai_cli, test))]
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Selection {
    pub(crate) source: Source,
    pub(crate) provenance: Provenance,
    pub(crate) cloud_retention_days: u32,
}
#[cfg(any(cargo_ai_cli, test))]
fn cloud_days(body: &Value) -> Result<u32, String> {
    body["policy"]["retention_days"]
        .as_u64()
        .and_then(|days| u32::try_from(days).ok())
        .filter(|days| (1..=3650).contains(days))
        .ok_or("Usage cloud returned an invalid age policy".into())
}
#[cfg(any(cargo_ai_cli, test))]
fn cloud_cutoff(selection: &Selection, now: i64) -> Result<i64, String> {
    if !(1..=3650).contains(&selection.cloud_retention_days) {
        return Err("Saved server cloud policy is invalid; pending history preserved".into());
    }
    Ok(now
        .saturating_sub(i64::from(selection.cloud_retention_days) * 86_400_000)
        .max(0))
}
pub(crate) fn now_ms() -> i64 {
    time::OffsetDateTime::now_utc()
        .unix_timestamp_nanos()
        .div_euclid(1_000_000)
        .clamp(0, i64::MAX as i128) as i64
}
#[cfg(any(cargo_ai_cli, test))]
pub(crate) fn project(db: &Connection, event: &Value) -> Result<String, String> {
    crate::usage_backup::analytics_projection(db, event)
}
#[cfg(any(cargo_ai_cli, test))]
fn encode(error: impl std::fmt::Display) -> String {
    error.to_string()
}
#[cfg(any(cargo_ai_cli, test))]
fn db_error(_: impl std::fmt::Display) -> String {
    "Usage cloud local persistence failed; pending history preserved".into()
}
#[cfg(any(cargo_ai_cli, test))]
fn open() -> Result<Connection, String> {
    crate::usage_store::open_database(true)?.ok_or("Usage history database unavailable".into())
}
#[cfg(any(cargo_ai_cli, test))]
fn read_selection(db: &Connection) -> Result<Option<Selection>, String> {
    let text: Option<String> = db
        .query_row(
            "SELECT value FROM usage_meta WHERE key=?1",
            [SELECTED],
            |row| row.get(0),
        )
        .optional()
        .map_err(db_error)?;
    text.map(|text| {
        let selection: Selection = serde_json::from_str(&text).map_err(|_| {
            "Saved usage cloud selection is invalid; it was not redirected".to_owned()
        })?;
        cloud_cutoff(&selection, now_ms())?;
        Ok(selection)
    })
    .transpose()
}
#[cfg(any(cargo_ai_cli, test))]
pub(crate) fn selected() -> Result<Option<Selection>, String> {
    let Some(db) = crate::usage_store::open_database(false)? else {
        return Ok(None);
    };
    read_selection(&db)
}
#[cfg(any(cargo_ai_cli, test))]
fn rolling_enabled(db: &Connection) -> Result<bool, String> {
    let text: Option<String> = db
        .query_row(
            "SELECT value FROM usage_meta WHERE key='local_retention_policy_v2'",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(db_error)?;
    let Some(text) = text else { return Ok(false) };
    let policy: crate::usage_store::retention::Policy =
        serde_json::from_str(&text).map_err(|_| "Local rolling history policy is invalid")?;
    Ok(policy.schema_version == 2
        && matches!(policy.days, 1 | 7 | 30)
        && policy.enabled
        && policy.local_detail_loss_acknowledged
        && policy.unselected_loss_acknowledged)
}
#[cfg(any(cargo_ai_cli, test))]
pub(crate) fn local_status() -> Result<Value, String> {
    let mut value = json!({"schema_version":2,"enabled":false,"cleared":false,"clear_outcome_unknown":false,"pending_clear_confirmation":null,"source":null,"provenance":null,"pending":0,"quarantined":0,"uploaded":0,"origin_unverified":0,"time_ineligible":0,"cloud_window_ineligible":0,"cloud_retention_days":null,"unscanned":0,"unknown_batches":0,"expired_without_acknowledgment_total":0,"retention_requires_exact_receipt":false,"processing_status":"not_observed","full_local_restoration_from_cloud":false});
    let Some(db) = crate::usage_store::open_database(false)? else {
        return Ok(value);
    };
    let expired: Option<String> = db
        .query_row(
            "SELECT value FROM usage_meta WHERE key='rolling_expired_without_ack_total_v2'",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(db_error)?;
    value["expired_without_acknowledgment_total"] =
        json!(expired.and_then(|s| s.parse::<u64>().ok()).unwrap_or(0));
    let Some(selection) = read_selection(&db)? else {
        return Ok(value);
    };
    let status = queue::status(&db, &selection.source)?;
    value["enabled"] =
        json!(status.enabled && rolling_enabled(&db)? && crate::usage_store::tracking_enabled()?);
    value["cleared"] = json!(status.cleared);
    value["clear_outcome_unknown"] = json!(status.clear_outcome_unknown);
    if let Some(Request::ClearConfirm {
        source,
        lifecycle_epoch,
        preview_id,
        preview_sha256,
        ..
    }) = queue::pending_upload_clear(&db, &selection.source)?
    {
        value["pending_clear_confirmation"] = json!({"source":source,"lifecycle_epoch":lifecycle_epoch,"preview_id":preview_id,"preview_sha256":preview_sha256});
    }
    value["source"] = json!(selection.source);
    value["cloud_retention_days"] = json!(selection.cloud_retention_days);
    value["provenance"] = json!(selection.provenance);
    value["pending"] = json!(status.pending);
    value["cloud_window_ineligible"] = json!(queue::cloud_window_ineligible(
        &db,
        &selection.source,
        cloud_cutoff(&selection, now_ms())?
    )?);
    value["quarantined"] = json!(status.quarantined);
    value["uploaded"] = json!(status.retained);
    value["origin_unverified"] = json!(status.origin_unverified);
    value["time_ineligible"] = json!(status.time_ineligible);
    value["unscanned"] = json!(status.unscanned);
    value["unknown_batches"] = json!(status.unknown_batches);
    Ok(value)
}
#[cfg(any(cargo_ai_cli, test))]
fn supported(body: &Value) -> Result<(), String> {
    if body["protocol_version"] != 2
        || body["capability"]["enabled"] != true
        || !body["capability"]["supported_protocol_versions"]
            .as_array()
            .is_some_and(|versions| versions.contains(&json!(2)))
    {
        return Err(
            "Usage cloud protocol 2 is not available; local consent was not enabled".into(),
        );
    }
    Ok(())
}
#[cfg(cargo_ai_cli)]
#[cfg(any(cargo_ai_cli, test))]
pub(crate) async fn remote_status() -> Result<Value, String> {
    transport::V2Transport::new(REQUEST_TIMEOUT)?
        .call(&Request::Status {
            protocol_version: Version,
        })
        .await
}
#[cfg(cargo_ai_cli)]
#[cfg(any(cargo_ai_cli, test))]
pub(crate) async fn enable(replace_home_id: Option<CanonicalUuid>) -> Result<Value, String> {
    tokio::time::timeout(DEADLINE, async {
        let mut db = open()?;
        if !rolling_enabled(&db)? || !crate::usage_store::tracking_enabled()? {
            return Err("Enable local history and explicitly select rolling policy version 2 before enabling cloud uploads".into());
        }
        let prior = read_selection(&db)?;
        let mut client = transport::V2Transport::new(REQUEST_TIMEOUT)?;
        let status = client.call(&Request::Status { protocol_version: Version }).await?;
        supported(&status)?;
        let binding = status["account_binding"].as_str().map(|s| CanonicalUuid::try_from(s.to_owned())).transpose().map_err(encode)?;
        if prior.as_ref().is_some_and(|prior| binding.as_ref() != Some(&prior.source.account_binding)) {
            return Err("Signed-in account differs; existing usage queues were not redirected".into());
        }
        queue::initialize(&mut db)?;
        if let Some(prior) = &prior {
            let generation = Counter::try_from(status["data_generation"].as_u64().ok_or("Usage cloud generation is missing")?).map_err(encode)?;
            if generation < prior.source.data_generation { return Err("Usage cloud returned an older generation".into()) }
            queue::reconcile_generation(&mut db, &prior.source.account_binding, generation)?;
            if generation == prior.source.data_generation && queue::clear_outcome_unknown(&db, &prior.source)? {
                return Err("Usage cloud clear outcome is unknown; reconcile the preserved exact confirmation".into());
            }
        }
        let home: Option<String> = db.query_row("SELECT value FROM usage_meta WHERE key=?1", [HOME], |row| row.get(0)).optional().map_err(db_error)?;
        let home = CanonicalUuid::try_from(home.unwrap_or_else(|| uuid::Uuid::new_v4().to_string())).map_err(encode)?;
        db.execute("INSERT OR IGNORE INTO usage_meta(key,value) VALUES(?1,?2)", params![HOME, home.as_str()]).map_err(db_error)?;
        let body = client.call(&Request::Enable { protocol_version: Version, source_home_id: home.clone(), replace_home_id }).await?;
        let source: Source = serde_json::from_value(body["source"].clone()).map_err(|_| "Usage cloud enable omitted its source")?;
        let provenance: Provenance = serde_json::from_value(body["provenance"].clone()).map_err(|_| "Usage cloud enable omitted its permission")?;
        let cloud_retention_days = cloud_days(&body)?;
        let generation_matches = status["data_generation"].as_u64().is_none_or(|g| g == source.data_generation.get());
        if body["protocol_version"] != 2 || body["enabled"] != true || source.source_home_id != home || binding.as_ref().is_some_and(|b| b != &source.account_binding) || !generation_matches || status["lifecycle_epoch"].as_u64() != Some(provenance.lifecycle_epoch.get()) {
            return Err("Usage cloud enable returned mismatched authority; reconcile remote consent before retrying".into());
        }
        if let Some(prior) = &prior {
            if prior.source != source || prior.provenance != provenance { queue::disable(&mut db, &prior.source)?; }
        }
        queue::reconcile_generation(&mut db, &source.account_binding, source.data_generation)?;
        let boundary = queue::enable(&mut db, &source, &provenance, now_ms())?;
        let selection = Selection { source: source.clone(), provenance: provenance.clone(), cloud_retention_days };
        db.execute("INSERT INTO usage_meta(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value", params![SELECTED, serde_json::to_string(&selection).map_err(encode)?]).map_err(db_error)?;
        Ok(json!({"schema_version":2,"enabled":true,"source":source,"provenance":provenance,"future_after_sequence":boundary.to_string(),"historical_records_selected":0,"local":local_status()?}))
    }).await.map_err(|_| "Usage cloud enable deadline elapsed; remote outcome may be unknown; inspect status before retrying".to_string())?
}
#[cfg(cargo_ai_cli)]
#[cfg(any(cargo_ai_cli, test))]
pub(crate) async fn disable() -> Result<Value, String> {
    let selection = selected()?.ok_or("Usage cloud is not enabled")?;
    queue::disable(&mut open()?, &selection.source)?;
    let body = transport::V2Transport::new(REQUEST_TIMEOUT)?
        .call(&Request::Disable {
            protocol_version: Version,
            source: selection.source.clone(),
            provenance: selection.provenance,
        })
        .await?;
    let returned: Source = serde_json::from_value(body["source"].clone())
        .map_err(|_| "Usage cloud disable omitted its source")?;
    if returned != selection.source || body["protocol_version"] != 2 || body["enabled"] != false {
        return Err("Remote disable requires reconciliation; local sends remain stopped".into());
    }
    Ok(
        json!({"schema_version":2,"enabled":false,"history_deleted":false,"pending_preserved":true,"remote":body,"local":local_status()?}),
    )
}
#[cfg(cargo_ai_cli)]
#[cfg(any(cargo_ai_cli, test))]
fn preview_hash(selection: &Selection, through: i64) -> Result<Sha256Hash, String> {
    canonical_hash("cargo-ai-usage-history-selection-v2", &(selection, through)).map_err(encode)
}
#[cfg(cargo_ai_cli)]
#[cfg(any(cargo_ai_cli, test))]
pub(crate) fn history_selection(
    through: Option<i64>,
    confirmation: Option<&str>,
) -> Result<Value, String> {
    let selection = selected()?.ok_or("Enable usage cloud explicitly before selecting history")?;
    let Some(db) = crate::usage_store::open_database(false)? else {
        return Err("Usage history unavailable".into());
    };
    let high = queue::high(&db)?;
    let through = through.unwrap_or(high);
    if through < 0 || through > high {
        return Err("Historical snapshot is invalid".into());
    }
    let hash = preview_hash(&selection, through)?;
    drop(db);
    if let Some(confirmation) = confirmation {
        if hash.as_str() != confirmation {
            return Err(
                "Historical selection preview no longer matches the current source and permission"
                    .into(),
            );
        }
        queue::select_history(
            &mut open()?,
            &selection.source,
            &selection.provenance,
            through,
        )?;
    }
    Ok(
        json!({"schema_version":2,"source":selection.source,"provenance":selection.provenance,"through_sequence":through.to_string(),"preview_sha256":hash,"selected":confirmation.is_some(),"upload_started":false,"restored_origin_unverified_records_remain_blocked":true}),
    )
}
#[cfg(any(cargo_ai_cli, test))]
fn build_upload(
    source: &Source,
    provenance: &Provenance,
    id: &CanonicalUuid,
    events: &[EventSubmission],
) -> Result<(String, usize, Sha256Hash), String> {
    let (batch, count) = elastic::encode_batch(events, elastic::DEFAULT_TARGET_BYTES).map_err(|_| "Usage upload contains an event exceeding its bounded codec limits; no event was truncated")?;
    let members = events[..count]
        .iter()
        .map(EventSubmission::member)
        .collect::<Result<Vec<_>, _>>()
        .map_err(encode)?;
    let membership = elastic::membership_sha256(source, &members).map_err(encode)?;
    let request = Request::Upload {
        protocol_version: Version,
        source: source.clone(),
        provenance: provenance.clone(),
        batch_id: id.clone(),
        ordered_membership_sha256: membership.clone(),
        batch,
        retry_count: 0,
    };
    let text = serde_json::to_string(&request).map_err(encode)?;
    // The transport also checks the actual authenticated outer envelope.
    if text.len() >= elastic::MAX_ACCOUNT_BYTES {
        return Err("Usage upload request exceeds its envelope limit".into());
    }
    Ok((text, count, membership))
}
#[cfg(any(cargo_ai_cli, test))]
pub(crate) async fn sync() -> Result<Value, String> {
    let started = Instant::now();
    let Some(mut selection) = selected()? else {
        return local_status();
    };
    if local_status()?["enabled"] != true {
        return local_status();
    }
    let maintenance = crate::usage_store::retention::apply(now_ms())?;
    if maintenance["maintenance_pending"] == true {
        return Ok(
            json!({"schema_version":2,"batches":0,"upload_paused":true,"maintenance_pending":true,"local":local_status()?}),
        );
    }
    let remaining = DEADLINE
        .saturating_sub(started.elapsed())
        .min(REQUEST_TIMEOUT);
    if remaining.is_zero() {
        return Err("Usage sync deadline elapsed before policy refresh".into());
    }
    let mut policy_client = transport::V2Transport::new(remaining)?;
    let status = tokio::time::timeout(
        DEADLINE.saturating_sub(started.elapsed()),
        policy_client.call(&Request::Status {
            protocol_version: Version,
        }),
    )
    .await
    .map_err(|_| "Usage sync deadline elapsed during policy refresh")??;
    supported(&status)?;
    if status["account_binding"] != json!(selection.source.account_binding)
        || status["data_generation"].as_u64() != Some(selection.source.data_generation.get())
        || status["lifecycle_epoch"].as_u64() != Some(selection.provenance.lifecycle_epoch.get())
    {
        return Err("Usage cloud account or generation changed; explicitly enable the current source before uploading".into());
    }
    let registered = status["environments"]
        .as_array()
        .is_some_and(|environments| {
            environments.iter().any(|environment| {
                environment["source_home_id"] == json!(selection.source.source_home_id)
                    && environment["consent_interval"].as_u64()
                        == Some(selection.provenance.consent_interval.get())
            })
        });
    let current = selected()?.ok_or("Usage cloud consent changed during policy refresh")?;
    if current.source != selection.source
        || current.provenance != selection.provenance
        || local_status()?["enabled"] != true
    {
        return Err("Usage cloud consent changed during policy refresh".into());
    }
    let mut db = open()?;
    if !registered {
        queue::disable(&mut db, &selection.source)?;
        return Err(
            "Usage cloud environment is no longer upload-enabled; pending local history preserved"
                .into(),
        );
    }
    selection.cloud_retention_days = cloud_days(&status)?;
    db.execute(
        "UPDATE usage_meta SET value=?2 WHERE key=?1",
        params![SELECTED, serde_json::to_string(&selection).map_err(encode)?],
    )
    .map_err(db_error)?;
    let cutoff_ms = cloud_cutoff(&selection, now_ms())?;
    queue::retire_uploads_before(&mut db, &selection.source, cutoff_ms, now_ms())?;
    let Some(lease) =
        queue::acquire_lease(&mut db, &selection.source, now_ms(), queue::MAX_LEASE_MS)?
    else {
        return Ok(
            json!({"schema_version":2,"busy":true,"batches":0,"policy_refreshed":true,"local":local_status()?}),
        );
    };
    let result = sync_leased(&mut db, &lease, cutoff_ms, started).await;
    let _ = queue::release_lease(&mut db, &lease, now_ms());
    result
}
#[cfg(test)]
#[cfg(any(cargo_ai_cli, test))]
fn insufficient_payload_budget(elapsed: Duration) -> bool {
    DEADLINE.saturating_sub(elapsed) < REQUEST_TIMEOUT
}
#[cfg(any(cargo_ai_cli, test))]
async fn sync_leased(
    db: &mut Connection,
    lease: &queue::Lease,
    cutoff_ms: i64,
    started: Instant,
) -> Result<Value, String> {
    let mut batches = 0usize;
    let mut acknowledged = 0usize;
    let mut uploaded = 0usize;
    let mut expired_ack = 0usize;
    let mut canceled = false;
    let mut stopped_error: Option<String> = None;
    loop {
        if scheduler::canceled() {
            canceled = true;
            break;
        }
        if started.elapsed() >= DEADLINE {
            break;
        }
        // Each capture commits at most 100 rows. Recheck the unscanned count
        // because invalid clocks and unverified origins also advance progress.
        let capture_started = Instant::now();
        let capture_budget =
            (DEADLINE.saturating_sub(started.elapsed()) / 3).min(Duration::from_secs(5));
        while capture_started.elapsed() < capture_budget && started.elapsed() < DEADLINE {
            let before = queue::status(db, lease.source())?.unscanned;
            if before == 0 || scheduler::canceled() {
                break;
            }
            queue::capture(db, lease.source(), project)?;
            if queue::status(db, lease.source())?.unscanned >= before {
                break;
            }
            tokio::task::yield_now().await;
        }
        if scheduler::canceled() {
            canceled = true;
            break;
        }
        let remaining = DEADLINE
            .saturating_sub(started.elapsed())
            .min(REQUEST_TIMEOUT);
        if remaining.is_zero() {
            break;
        }
        let Some(batch) =
            queue::prepare_upload_batch_since(db, lease, now_ms(), cutoff_ms, build_upload)?
        else {
            break;
        };
        let request = batch.request_json().parse::<Request>().map_err(encode)?;
        if !matches!(&request, Request::Upload { ordered_membership_sha256, source, provenance, batch_id, .. } if ordered_membership_sha256 == batch.membership() && source == batch.source() && provenance == batch.provenance() && batch_id == batch.id())
        {
            return Err(
                "Saved usage upload changed exact batch authority; pending batch preserved".into(),
            );
        }
        queue::begin_upload_attempt(db, lease, batch.id(), now_ms())?;
        let remaining = DEADLINE
            .saturating_sub(started.elapsed())
            .min(REQUEST_TIMEOUT);
        if remaining.is_zero() {
            break;
        }
        let mut client = transport::V2Transport::new(remaining)?;
        batches += 1;
        let response = tokio::time::timeout(
            DEADLINE.saturating_sub(started.elapsed()),
            client.call_exact(batch.request_json()),
        )
        .await
        .map_err(|_| "Usage sync deadline elapsed; submitted outcomes remain unknown".to_owned())
        .and_then(|result| result);
        let body = match response {
            Ok(body) => body,
            Err(error) => {
                if acknowledged == 0 {
                    return Err(error);
                }
                stopped_error = Some(error);
                break;
            }
        };
        let verified = serde_json::from_value::<elastic::UploadAck>(body)
            .map_err(|_| "Usage upload acknowledgment is invalid; exact pending batch preserved".to_owned())
            .and_then(|ack| ack.verify(&request).map_err(|_| "Usage upload acknowledgment changed exact batch authority; pending batch preserved".to_owned()));
        if let Err(error) = verified {
            if acknowledged == 0 {
                return Err(error);
            }
            stopped_error = Some(error);
            break;
        }
        acknowledged += batch.members().len();
        match queue::acknowledge_uploaded(db, lease, batch.id(), now_ms()) {
            Ok(()) => uploaded += batch.members().len(),
            Err(error) if error.contains("after its local batch expired") => {
                expired_ack += batch.members().len();
                break;
            }
            Err(error) => {
                stopped_error = Some(error);
                break;
            }
        }
        tokio::task::yield_now().await;
    }
    Ok(
        json!({"schema_version":2,"batches":batches,"policy_refreshed":true,"acknowledged_events":acknowledged,"uploaded_events":uploaded,"acknowledged_after_expiry_events":expired_ack,"processing_status":"not_observed","canceled":canceled,"deadline_reached":started.elapsed() >= DEADLINE,"partial":canceled || stopped_error.is_some() || started.elapsed() >= DEADLINE,"stopped_error":stopped_error,"local":local_status()?}),
    )
}
#[cfg(cargo_ai_cli)]
#[cfg(any(cargo_ai_cli, test))]
pub(crate) async fn clear(preview: Option<(CanonicalUuid, Sha256Hash)>) -> Result<Value, String> {
    let selection = selected()?.ok_or("Usage cloud source is missing")?;
    let request = match preview {
        None => Request::ClearPreview {
            protocol_version: Version,
            source: selection.source.clone(),
            lifecycle_epoch: selection.provenance.lifecycle_epoch,
        },
        Some((preview_id, preview_sha256)) => Request::ClearConfirm {
            protocol_version: Version,
            source: selection.source.clone(),
            lifecycle_epoch: selection.provenance.lifecycle_epoch,
            preview_id,
            preview_sha256,
        },
    };
    let confirm = matches!(request, Request::ClearConfirm { .. });
    if confirm {
        queue::begin_upload_clear(&mut open()?, &request)?;
    }
    let body = transport::V2Transport::new(REQUEST_TIMEOUT)?
        .call(&request)
        .await?;
    let returned: Source = serde_json::from_value(body["source"].clone())
        .map_err(|_| "Clear result omitted its exact source")?;
    if returned != selection.source || body["protocol_version"] != 2 {
        return Err("Clear preview or result changed source; local state preserved".into());
    }
    if confirm {
        if body["cleared"] != true
            || body["data_generation"].as_u64()
                != selection.source.data_generation.get().checked_add(1)
        {
            return Err(
                "Clear outcome requires reconciliation; local history remains preserved".into(),
            );
        }
        queue::finish_upload_clear(&mut open()?, &selection.source)?;
    } else if body["lifecycle_epoch"].as_u64() != Some(selection.provenance.lifecycle_epoch.get())
        || serde_json::from_value::<CanonicalUuid>(body["preview_id"].clone()).is_err()
        || serde_json::from_value::<Sha256Hash>(body["preview_sha256"].clone()).is_err()
    {
        return Err("Clear preview is invalid; local state preserved".into());
    }
    Ok(body)
}
#[cfg(cargo_ai_cli)]
#[cfg(any(cargo_ai_cli, test))]
pub(crate) async fn history(
    after: Option<u64>,
    through: Option<u64>,
    limit: u16,
) -> Result<Value, String> {
    if !(1..=100).contains(&limit) || after.is_some() && through.is_none() {
        return Err("History pagination requires the captured through-sequence boundary".into());
    }
    let selection = selected()?.ok_or("Usage cloud source is missing")?;
    let mut body = transport::V2Transport::new(REQUEST_TIMEOUT)?
        .call(&Request::History {
            protocol_version: Version,
            source: selection.source.clone(),
            after_sequence: after,
            limit,
        })
        .await?;
    let source: Source = serde_json::from_value(body["source"].clone())
        .map_err(|_| "Usage cloud history omitted its source")?;
    let high = body["high_watermark"]
        .as_u64()
        .filter(|n| *n <= i64::MAX as u64)
        .ok_or("Usage cloud history omitted its bounded snapshot")?;
    let events = body["events"]
        .as_array()
        .ok_or("Usage cloud history omitted its bounded events")?;
    if source != selection.source
        || body["protocol_version"] != 2
        || events.len() > usize::from(limit)
        || through.is_some_and(|bound| high < bound)
    {
        return Err("Usage cloud history returned mismatched scope or snapshot bounds".into());
    }
    let bound = through.unwrap_or(high);
    let mut previous = after.unwrap_or(0);
    let mut ids = std::collections::BTreeSet::new();
    for event in events {
        let sequence = event["sequence"]
            .as_u64()
            .filter(|n| *n > previous && *n <= high)
            .ok_or("Usage cloud history returned unordered members")?;
        let mut record = event.clone();
        let object = record
            .as_object_mut()
            .ok_or("Usage cloud history event is invalid")?;
        object.remove("sequence");
        let event_id = object
            .remove("event_id")
            .and_then(|v| v.as_str().map(str::to_owned))
            .ok_or("Usage cloud history identity is missing")?;
        let submission: EventSubmission = serde_json::from_value(record)
            .map_err(|_| "Usage cloud history event fields are invalid")?;
        if submission.member().map_err(encode)?.event_id != event_id || !ids.insert(event_id) {
            return Err("Usage cloud history contains conflicting identities".into());
        }
        previous = sequence;
    }
    body["events"]
        .as_array_mut()
        .unwrap()
        .retain(|event| event["sequence"].as_u64().is_some_and(|n| n <= bound));
    let last = body["events"]
        .as_array()
        .unwrap()
        .last()
        .and_then(|event| event["sequence"].as_u64());
    body["next_sequence"] = json!(last.filter(|n| *n < bound));
    body["through_sequence"] = json!(bound);
    Ok(body)
}
