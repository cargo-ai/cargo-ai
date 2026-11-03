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
const UPLOAD_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
#[cfg(any(cargo_ai_cli, test))]
const BATCH_FILL_LATENCY: Duration = Duration::from_secs(1);
#[cfg(any(cargo_ai_cli, test))]
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Selection {
    pub(crate) source: Source,
    pub(crate) provenance: Provenance,
    pub(crate) cloud_retention_days: u32,
}
#[cfg(cargo_ai_cli)]
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
pub(crate) fn empty_local_status() -> Value {
    json!({"schema_version":2,"enabled":false,"cleared":false,"clear_outcome_unknown":false,"pending_clear_confirmation":null,"source":null,"provenance":null,"pending":0,"quarantined":0,"uploaded":0,"origin_unverified":0,"time_ineligible":0,"cloud_window_ineligible":0,"cloud_retention_days":null,"unscanned":0,"unknown_batches":0,"expired_without_acknowledgment_total":0,"retention_requires_exact_receipt":false,"processing_status":"not_observed","full_local_restoration_from_cloud":false,"sync_job":null,"server_expired_events":0})
}
#[cfg(any(cargo_ai_cli, test))]
pub(crate) fn local_status() -> Result<Value, String> {
    let mut value = empty_local_status();
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
    if let Some(job) = queue::job::latest(&db, &selection.source)? {
        let mut progress = queue::job::progress(&db, &job)?;
        progress["retry_at_ms"] = json!(job.retry_at_ms);
        value["sync_job"] = progress;
    }
    value["server_expired_events"] = json!(queue::job::terminal_count(&db, &selection.source)?);
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
    let mut client = transport::V2Transport::new(REQUEST_TIMEOUT)?;
    let mut status = client
        .call(&Request::Status {
            protocol_version: Version,
        })
        .await?;
    let freshness = client
        .call(&Request::Freshness {
            protocol_version: Version,
        })
        .await?;
    if freshness["protocol_version"] != 2
        || !matches!(
            freshness["refresh_status"].as_str(),
            Some("fresh" | "overdue" | "missing" | "failed")
        )
        || freshness["target_age_ms"] != 86_400_000
        || !(freshness["last_refresh_ms"].is_null()
            || freshness["last_refresh_ms"]
                .as_i64()
                .is_some_and(|value| value >= 0))
    {
        return Err("Usage cloud freshness status is invalid".into());
    }
    status["freshness"] = freshness;
    Ok(status)
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
#[cfg(any(cargo_ai_cli, test))]
pub(crate) fn build_upload(
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
#[cfg(all(test, cargo_ai_cli))]
pub(crate) async fn sync() -> Result<Value, String> {
    sync_with_history(None).await
}
#[cfg(cargo_ai_cli)]
pub(crate) async fn sync_with_history(history_days: Option<u32>) -> Result<Value, String> {
    sync_pass(history_days, true).await
}
#[cfg(any(cargo_ai_cli, test))]
pub(crate) async fn sync_pass(history_days: Option<u32>, explicit: bool) -> Result<Value, String> {
    // Deny unsupported plans before opening history or loading credentials.
    if history_days.is_some_and(|days| days == 0 || days > 30) {
        return Ok(
            json!({"schema_version":2,"outcome":"plan_limit_denied","maximum_history_days":30,"requested_history_days":history_days,"batches":0,"partial":false}),
        );
    }
    let started = Instant::now();
    let Some(selection) = selected()? else {
        return Ok(
            json!({"schema_version":2,"outcome":"failed","reason":"cloud_disabled","batches":0,"partial":false}),
        );
    };
    if local_status()?["enabled"] != true {
        return Ok(
            json!({"schema_version":2,"outcome":"failed","reason":"cloud_disabled","batches":0,"partial":false}),
        );
    }
    let maintenance = crate::usage_store::retention::apply(now_ms())?;
    if maintenance["maintenance_pending"] == true {
        return Ok(
            json!({"schema_version":2,"outcome":"partial","reason":"maintenance_pending","batches":0,"partial":true}),
        );
    }
    let mut db = open()?;
    let Some(mut lease) =
        queue::acquire_lease(&mut db, &selection.source, now_ms(), queue::MAX_LEASE_MS)?
    else {
        return Ok(
            json!({"schema_version":2,"outcome":"already_running","batches":0,"partial":false}),
        );
    };
    let result=async {
        let job=if explicit {queue::job::start(&mut db,&lease,now_ms(),history_days)?} else {queue::job::resume(&mut db,&lease,now_ms())?};
        if job.retry_reason.as_deref()==Some("invalid_request") || job.retry_at_ms.is_some_and(|retry|retry>now_ms()) {
            let quota=job.retry_reason.is_none() || job.retry_reason.as_deref()==Some("quota_paused");
            return Ok(json!({"schema_version":2,"outcome":if quota {"quota_paused"} else {"partial"},"reason":job.retry_reason.as_deref().unwrap_or("quota_paused"),"batches":0,"partial":true,"retry_at_ms":job.retry_at_ms,"reset_at_ms":if quota {job.retry_at_ms} else {None},"job":queue::job::progress(&db,&job)?}));
        }
        let mut client=transport::V2Transport::new(REQUEST_TIMEOUT)?.with_upload_timeout(UPLOAD_REQUEST_TIMEOUT);
        let status=client.call(&Request::Status{protocol_version:Version}).await?;
        supported(&status)?;
        if status["account_binding"]!=json!(selection.source.account_binding)
            || status["data_generation"].as_u64()!=Some(selection.source.data_generation.get())
            || status["lifecycle_epoch"].as_u64()!=Some(selection.provenance.lifecycle_epoch.get()) {
            return Err("Usage cloud authority changed; pending job preserved".into());
        }
        let registered=status["environments"].as_array().is_some_and(|environments|environments.iter().any(|environment|environment["source_home_id"]==json!(selection.source.source_home_id)&&environment["consent_interval"].as_u64()==Some(selection.provenance.consent_interval.get())));
        if !registered {return Err("Usage cloud environment is no longer enabled; pending job preserved".into());}
        sync_leased(&mut db,&mut lease,&job,started,&mut client).await
    }.await;
    let _ = queue::release_lease(&mut db, &lease, now_ms());
    match result {
        Ok(value) => Ok(value),
        Err(error) => Ok(
            json!({"schema_version":2,"outcome":"failed","reason":error,"batches":0,"partial":true}),
        ),
    }
}
#[cfg(any(cargo_ai_cli, test))]
pub(crate) async fn resume_active() {
    let active = selected().ok().flatten().and_then(|selection| {
        open()
            .ok()
            .and_then(|db| queue::job::active(&db, &selection.source).ok().flatten())
    });
    if active.is_some() {
        let _ = sync_pass(None, false).await;
    }
}
#[cfg(any(cargo_ai_cli, test))]
async fn sync_leased(
    db: &mut Connection,
    lease: &mut queue::Lease,
    job: &queue::job::Job,
    started: Instant,
    client: &mut transport::V2Transport,
) -> Result<Value, String> {
    let mut batches = 0usize;
    let mut uploaded = 0usize;
    let mut acknowledged = 0usize;
    let mut outcome = "partial";
    let mut error = None;
    let mut retry_at = None;
    let mut reason = None;
    let mut quota = None;
    macro_rules! local_step {
        ($label:lifetime, $work:expr) => {
            match $work {
                Ok(value) => value,
                Err(failure) => {
                    outcome = if batches > 0 { "partial" } else { "failed" };
                    error = Some(failure);
                    break $label;
                }
            }
        };
    }
    'passes: while started.elapsed() < DEADLINE && !scheduler::canceled() {
        local_step!('passes, queue::renew_lease(db, lease, now_ms()));
        // Short capture transactions fill one bounded candidate pool. A tail or
        // latency trigger is flushable without a minimum compressed batch size.
        let fill_started = Instant::now();
        let mut captured = 0;
        while local_step!('passes, queue::job::captured_pending(db, job))
            < elastic::MAX_BATCH_EVENTS
        {
            let count = local_step!('passes, queue::job::capture(db, lease, job, now_ms()));
            captured += count;
            if count == 0
                || fill_started.elapsed() >= BATCH_FILL_LATENCY
                || started.elapsed() >= DEADLINE
                || scheduler::canceled()
            {
                break;
            }
        }
        if started.elapsed() >= DEADLINE || scheduler::canceled() {
            break;
        }
        let Some(batch) = local_step!('passes, queue::prepare_job_upload(
            db,
            lease,
            now_ms(),
            &job.id,
            build_upload
        )) else {
            let progress = local_step!('passes, queue::job::progress(db, job));
            if progress["pending_events"] == 0 {
                outcome = if progress["expired_events"] == 0 && progress["excluded_events"] == 0 {
                    "fully_synced"
                } else {
                    "partial"
                };
            } else if captured > 0 {
                continue;
            }
            break;
        };
        let request = local_step!('passes, batch.request_json().parse::<Request>().map_err(encode));
        if !matches!(&request,Request::Upload{ordered_membership_sha256,source,provenance,batch_id,..} if ordered_membership_sha256==batch.membership()&&source==batch.source()&&provenance==batch.provenance()&&batch_id==batch.id())
        {
            outcome = "outcome_unknown";
            error = Some("Saved usage batch authority changed; job preserved".into());
            break;
        }
        // The pass budget admits work; it does not shorten an admitted request.
        local_step!('passes,client.check_upload(batch.request_json()));
        let attempt = local_step!('passes, queue::reserve_upload_attempt(
            db,
            lease,
            &job.id,
            &batch,
            now_ms(),
            UPLOAD_REQUEST_TIMEOUT.as_millis() as i64
        ));
        batches += 1;
        let body = match client.upload_exact(batch.request_json()).await {
            Ok(body) => body,
            Err(failure) => {
                outcome = if failure.unknown {
                    "outcome_unknown"
                } else {
                    "partial"
                };
                error = Some(failure.message);
                if failure.invalid_payload {
                    reason = Some("invalid_request");
                } else if failure.retryable || attempt >= 4 {
                    let cooldown = attempt >= 4;
                    reason = Some(if cooldown {
                        "retry_cooldown"
                    } else {
                        "retry_backoff"
                    });
                    let delay = if cooldown {
                        300_000
                    } else {
                        retry_delay_ms(attempt)
                    };
                    retry_at = Some(
                        now_ms()
                            .saturating_add(delay)
                            .max(failure.retry_at_ms.unwrap_or(0)),
                    );
                }
                local_step!('passes, queue::pause_upload(
                    db,
                    lease,
                    &job.id,
                    &batch,
                    retry_at,
                    reason.unwrap_or("outcome_unknown"),
                    outcome,
                    now_ms()
                ));
                if failure.retryable && attempt < 4 {
                    let delay = Duration::from_millis(
                        retry_at.unwrap().saturating_sub(now_ms()).max(0) as u64,
                    );
                    if delay < DEADLINE.saturating_sub(started.elapsed()) && !scheduler::canceled()
                    {
                        let until = tokio::time::Instant::now() + delay;
                        while tokio::time::Instant::now() < until && !scheduler::canceled() {
                            tokio::time::sleep(
                                (until - tokio::time::Instant::now())
                                    .min(Duration::from_millis(25)),
                            )
                            .await;
                        }
                        if scheduler::canceled() {
                            break;
                        }
                        retry_at = None;
                        reason = None;
                        error = None;
                        continue;
                    }
                }
                break;
            }
        };
        if attempt < 4 {
            // A received response ends the in-flight reservation. Its content
            // must still establish an exact acknowledgment or explicit pause.
            local_step!('passes,queue::pause_upload(db,lease,&job.id,&batch,None,"outcome_unknown","outcome_unknown",now_ms()));
        }
        if body["protocol_version"] == 2 && body["outcome"] == "quota_paused" {
            let retry = body["retry_at_ms"]
                .as_i64()
                .filter(|retry| *retry > now_ms() && *retry % 86_400_000 == 0);
            if retry.is_none()
                || body["reset_at_ms"].as_i64() != retry
                || body["daily_bytes_limit"].as_u64().is_none()
                || body["daily_request_limit"].as_u64().is_none()
            {
                outcome = "outcome_unknown";
                error = Some("Invalid quota pause response; exact batch preserved".into());
                break;
            }
            outcome = "quota_paused";
            retry_at = retry;
            reason = Some("quota_paused");
            local_step!('passes, queue::pause_upload(
                db,
                lease,
                &job.id,
                &batch,
                retry_at,
                "quota_paused",
                outcome,
                now_ms()
            ));
            quota = Some(body);
            break;
        }
        if body["protocol_version"] == 2 && body["outcome"] == "expired" {
            let expired = serde_json::from_value::<Vec<String>>(body["expired_event_ids"].clone())
                .map_err(|_| "Invalid expiry membership".to_owned())
                .and_then(|ids| {
                    queue::job::expire_batch_members(db, lease, job, &batch, &ids, now_ms())
                });
            if let Err(failure) = expired {
                outcome = "outcome_unknown";
                error = Some(failure);
                break;
            }
            continue;
        }
        let verified = serde_json::from_value::<elastic::UploadAck>(body)
            .map_err(|_| "Invalid acknowledgment; exact batch preserved".to_owned())
            .and_then(|ack| {
                ack.verify(&request)
                    .map_err(|_| "Acknowledgment authority mismatch; exact batch preserved".into())
            });
        if let Err(failure) = verified {
            outcome = "outcome_unknown";
            error = Some(failure);
            break;
        }
        acknowledged += batch.members().len();
        if let Err(failure) = queue::acknowledge_uploaded(db, lease, batch.id(), now_ms()) {
            outcome = "outcome_unknown";
            error = Some(failure);
            break;
        }
        uploaded += batch.members().len();
        tokio::task::yield_now().await;
    }
    if error.is_some() && retry_at.is_none() && reason.is_none() {
        if let Ok(Some(saved)) = queue::job::active(db, lease.source()) {
            if saved.retry_reason.as_deref() == Some("retry_cooldown") {
                retry_at = saved.retry_at_ms;
                reason = Some("retry_cooldown");
            }
        }
    }
    let observed = queue::job::progress(db, job).ok();
    if error.is_none()
        && outcome == "partial"
        && observed.as_ref().is_some_and(|progress| {
            progress["pending_events"] == 0
                && progress["expired_events"] == 0
                && progress["excluded_events"] == 0
        })
    {
        outcome = "fully_synced";
    }
    let progress = match queue::job::finish_pass(db, job, retry_at, reason, outcome) {
        Ok(progress) => Some(progress),
        Err(failure) => {
            outcome = if batches > 0 { "partial" } else { "failed" };
            error = Some(failure);
            observed
        }
    };
    let mut result = json!({"schema_version":2,"outcome":outcome,"batches":batches,"policy_refreshed":true,"uploaded_events":uploaded,"acknowledged_events":acknowledged,"processing_status":"not_observed","partial":outcome!="fully_synced","canceled":scheduler::canceled(),"deadline_reached":started.elapsed()>=DEADLINE,"stopped_error":error,"reason":reason,"retry_at_ms":retry_at,"reset_at_ms":if outcome=="quota_paused" {retry_at} else {None},"quota":quota,"job":progress});
    if reason.is_none() {
        result.as_object_mut().unwrap().remove("reason");
    }
    Ok(result)
}
#[cfg(any(cargo_ai_cli, test))]
fn retry_delay_ms(attempt: u32) -> i64 {
    let ceiling = 1000u64 << attempt.saturating_sub(1).min(2);
    let random = uuid::Uuid::new_v4().as_u128() as u64;
    (random % (ceiling + 1)) as i64
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
