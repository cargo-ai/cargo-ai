//! Independent, explicitly consented cloud custody for immutable usage history.
#[path = "usage_analytics/contract.rs"]
pub(crate) mod contract;
#[path = "usage_analytics/queue.rs"]
pub(crate) mod queue;
#[path = "usage_analytics/scheduler.rs"]
pub(crate) mod scheduler;
#[cfg(test)]
#[path = "usage_analytics/tests.rs"]
mod tests;
#[path = "usage_analytics/transport.rs"]
mod transport;
#[cfg(test)]
thread_local! { pub(crate) static TEST_ENDPOINT: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) }; }

use contract::*;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::Duration;
const SELECTED: &str = "usage_cloud_selected_v1";
#[cfg(cargo_ai_cli)]
const HOME: &str = "usage_cloud_home_id_v1";
const DEADLINE: Duration = Duration::from_secs(15);
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Selection {
    pub(crate) source: Source,
    pub(crate) provenance: Provenance,
}
pub(crate) fn now_ms() -> i64 {
    time::OffsetDateTime::now_utc()
        .unix_timestamp_nanos()
        .div_euclid(1_000_000)
        .clamp(0, i64::MAX as i128) as i64
}
pub(crate) fn project(db: &Connection, event: &Value) -> Result<String, String> {
    crate::usage_backup::analytics_projection(db, event)
}
fn encode(error: impl std::fmt::Display) -> String {
    error.to_string()
}
fn db_error(_: impl std::fmt::Display) -> String {
    "Usage cloud local persistence failed; pending history preserved".into()
}
fn open() -> Result<Connection, String> {
    crate::usage_store::open_database(true)?.ok_or("Usage history database unavailable".into())
}
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
        serde_json::from_str(&text)
            .map_err(|_| "Saved usage cloud selection is invalid; it was not redirected".into())
    })
    .transpose()
}
pub(crate) fn selected() -> Result<Option<Selection>, String> {
    let Some(db) = crate::usage_store::open_database(false)? else {
        return Ok(None);
    };
    read_selection(&db)
}
pub(crate) fn local_status() -> Result<Value, String> {
    let Some(db) = crate::usage_store::open_database(false)? else {
        return Ok(
            json!({"schema_version":1,"enabled":false,"source":null,"pending":0,"retained":0,"origin_unverified":0,"unscanned":0}),
        );
    };
    let Some(selection) = read_selection(&db)? else {
        return Ok(
            json!({"schema_version":1,"enabled":false,"source":null,"pending":0,"retained":0,"origin_unverified":0,"unscanned":0}),
        );
    };
    let status = queue::status(&db, &selection.source)?;
    Ok(
        json!({"schema_version":1,"enabled":status.enabled,"cleared":status.cleared,"clear_outcome_unknown":status.clear_outcome_unknown,"source":selection.source,"provenance":selection.provenance,"pending":status.pending,"quarantined":status.quarantined,"retained":status.retained,"origin_unverified":status.origin_unverified,"unscanned":status.unscanned,"unknown_batches":status.unknown_batches,"buffered_batches":status.buffered_batches,"terminal_unretained_batches":status.terminal_unretained_batches,"retention_requires_exact_receipt":true,"full_local_restoration_from_cloud":false}),
    )
}
#[cfg(cargo_ai_cli)]
fn supported(body: &Value) -> Result<(), String> {
    if body["protocol_version"] != 1
        || body["capability"]["enabled"] != true
        || !body["capability"]["supported_protocol_versions"]
            .as_array()
            .is_some_and(|versions| versions.contains(&json!(1)))
    {
        return Err(
            "Usage cloud protocol 1 is not available; local consent was not enabled".into(),
        );
    }
    Ok(())
}
#[cfg(cargo_ai_cli)]
pub(crate) async fn remote_status() -> Result<Value, String> {
    transport::Transport::new(Duration::from_secs(10))?
        .call(&Request::Status {
            protocol_version: Version,
        })
        .await
}
#[cfg(cargo_ai_cli)]
pub(crate) async fn enable() -> Result<Value, String> {
    tokio::time::timeout(DEADLINE,async {
        let mut client=transport::Transport::new(Duration::from_secs(10))?;
        let status=client.call(&Request::Status{protocol_version:Version}).await?;supported(&status)?;
        let prior=selected()?;
        let binding=if let Some(binding)=status["account_binding"].as_str(){CanonicalUuid::try_from(binding.to_owned()).map_err(encode)?}else{CanonicalUuid::try_from(uuid::Uuid::new_v4().to_string()).map_err(encode)?};
        if prior.as_ref().is_some_and(|prior|prior.source.account_binding!=binding){return Err("Signed-in account differs; existing usage queues were not redirected".into())}
        let generation=Counter::try_from(match status["data_generation"].as_u64(){Some(value)=>value,None if status["account_binding"].is_null()=>1,None=>return Err("Usage cloud generation is missing".into())}).map_err(encode)?;
        let epoch=LifecycleEpoch::try_from(status["lifecycle_epoch"].as_u64().ok_or("Usage cloud lifecycle epoch is missing")?).map_err(encode)?;
        let mut db=open()?;queue::initialize(&mut db)?;
        if let Some(prior)=prior.as_ref(){
            if generation<prior.source.data_generation{return Err("Usage cloud returned an older generation; local custody remains unchanged".into())}
        }
        // Only explicit clear advances the authenticated binding's generation.
        queue::reconcile_generation(&mut db,&binding,generation)?;
        if let Some(prior)=prior.as_ref(){
            if generation==prior.source.data_generation && queue::clear_outcome_unknown(&db,&prior.source)?{
                return Err("Usage cloud clear outcome is unknown; retry the preserved exact confirmation or reconcile a newer generation".into());
            }
        }
        let home:Option<String>=db.query_row("SELECT value FROM usage_meta WHERE key=?1",[HOME],|row|row.get(0)).optional().map_err(db_error)?;
        let home=CanonicalUuid::try_from(home.unwrap_or_else(||uuid::Uuid::new_v4().to_string())).map_err(encode)?;
        db.execute("INSERT OR IGNORE INTO usage_meta(key,value) VALUES(?1,?2)",params![HOME,home.as_str()]).map_err(db_error)?;
        let source=Source{account_binding:binding,source_home_id:home,data_generation:generation};
        let body=client.call(&Request::Enable{protocol_version:Version,source:source.clone(),lifecycle_epoch:epoch}).await?;
        let returned:Source=serde_json::from_value(body["source"].clone()).map_err(|_|"Usage cloud enable omitted its source")?;
        let provenance:Provenance=serde_json::from_value(body["provenance"].clone()).map_err(|_|"Usage cloud enable omitted its permission")?;
        if returned!=source || provenance.lifecycle_epoch!=epoch || body["enabled"]!=true{return Err("Usage cloud enable returned mismatched authority; reconcile remote consent before retrying".into())}
        if let Some(prior)=prior.as_ref(){
            if prior.source!=source||prior.provenance!=provenance||local_status()?["enabled"]!=true {
                queue::disable(&mut db,&prior.source)?;
            }
        }
        let boundary=queue::enable(&mut db,&source,&provenance,now_ms())?;
        let selection=Selection{source:source.clone(),provenance:provenance.clone()};
        db.execute("INSERT INTO usage_meta(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",params![SELECTED,serde_json::to_string(&selection).map_err(encode)?]).map_err(db_error)?;
        // Only this explicit renewed permission may create replacement admission batches.
        let mut readmitted=0;
        if prior.as_ref().is_some_and(|old|old.source==source && old.provenance!=provenance){
            if let Some(lease)=queue::acquire_lease(&mut db,&source,now_ms(),queue::MAX_LEASE_MS)?{
                let ids={let mut statement=db.prepare("SELECT DISTINCT b.batch_id FROM usage_cloud_batches b JOIN usage_cloud_queue q ON q.binding=b.binding AND q.source_home_id=b.source_home_id AND q.data_generation=b.data_generation AND q.active_batch_id=b.batch_id WHERE b.binding=?1 AND b.source_home_id=?2 AND b.data_generation=?3 AND b.outcome!='retained' AND (b.lifecycle_epoch!=?4 OR b.consent_interval!=?5) ORDER BY b.rowid LIMIT 100").map_err(db_error)?;
                    let rows=statement.query_map(params![source.account_binding.as_str(),source.source_home_id.as_str(),source.data_generation.get() as i64,provenance.lifecycle_epoch.get() as i64,provenance.consent_interval.get() as i64],|row|row.get::<_,String>(0)).map_err(db_error)?.collect::<Result<Vec<_>,_>>().map_err(db_error)?;rows};
                for id in ids {queue::readmit_batch(&mut db,&lease,&CanonicalUuid::try_from(id).map_err(encode)?,now_ms())?;readmitted+=1;}
                queue::release_lease(&mut db,&lease,now_ms())?;
            }
        }
        Ok(json!({"schema_version":1,"enabled":true,"source":source,"provenance":provenance,"future_after_sequence":boundary.to_string(),"historical_records_selected":0,"readmitted_batches":readmitted,"local":local_status()?}))
    }).await.map_err(|_|"Usage cloud enable deadline elapsed; remote outcome may be unknown; inspect status before retrying".to_string())?
}
#[cfg(cargo_ai_cli)]
pub(crate) async fn disable() -> Result<Value, String> {
    let selection = selected()?.ok_or("Usage cloud is not enabled")?;
    let mut db = open()?;
    queue::disable(&mut db, &selection.source)?;
    drop(db);
    let body = transport::Transport::new(Duration::from_secs(10))?
        .call(&Request::Disable {
            protocol_version: Version,
            source: selection.source.clone(),
            provenance: selection.provenance,
        })
        .await?;
    let returned: Source = serde_json::from_value(body["source"].clone())
        .map_err(|_| "Usage cloud disable omitted its source")?;
    if returned != selection.source || body["enabled"] != false {
        return Err("Remote disable requires reconciliation; local sends remain stopped".into());
    }
    Ok(
        json!({"schema_version":1,"enabled":false,"history_deleted":false,"pending_preserved":true,"remote":body,"local":local_status()?}),
    )
}
#[cfg(cargo_ai_cli)]
fn preview_hash(selection: &Selection, through: i64) -> Result<Sha256Hash, String> {
    canonical_hash("cargo-ai-usage-history-selection-v1", &(selection, through)).map_err(encode)
}
#[cfg(cargo_ai_cli)]
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
        json!({"schema_version":1,"source":selection.source,"provenance":selection.provenance,"through_sequence":through.to_string(),"preview_sha256":hash,"selected":confirmation.is_some(),"upload_started":false,"restored_origin_unverified_records_remain_blocked":true}),
    )
}
pub(crate) async fn sync() -> Result<Value, String> {
    tokio::time::timeout(DEADLINE, sync_once())
        .await
        .map_err(|_| {
            "Usage cloud sync deadline elapsed; unknown attempts and exact payloads remain pending"
                .to_string()
        })?
}
async fn sync_once() -> Result<Value, String> {
    let Some(selection) = selected()? else {
        return local_status();
    };
    if local_status()?["enabled"] != true {
        return local_status();
    }
    let mut db = open()?;
    let now = now_ms();
    let Some(lease) = queue::acquire_lease(&mut db, &selection.source, now, queue::MAX_LEASE_MS)?
    else {
        return Ok(json!({"schema_version":1,"busy":true,"local":local_status()?}));
    };
    let result=async {
        queue::capture(&mut db,lease.source(),project)?;
        let Some(batch)=queue::prepare_batch(&mut db,&lease,now_ms())? else{let local=local_status()?;return Ok(json!({"schema_version":1,"batches":0,"blocked":local["pending"].as_i64().unwrap_or(0)>0||local["origin_unverified"].as_i64().unwrap_or(0)>0,"local":local}))};
        let remaining=lease.expires_ms().saturating_sub(now_ms()).min(10_000);
        if remaining<=0{return Err("Usage cloud lease expired before transmission".into());}
        let mut client=transport::Transport::new(Duration::from_millis(remaining as u64))?;
        // Recover an uncertain prior admission before deciding whether to append again.
        let attempts:i64=db.query_row("SELECT attempts FROM usage_cloud_batches WHERE binding=?1 AND source_home_id=?2 AND data_generation=?3 AND batch_id=?4",params![selection.source.account_binding.as_str(),selection.source.source_home_id.as_str(),selection.source.data_generation.get() as i64,batch.id().as_str()],|row|row.get(0)).map_err(db_error)?;
        if attempts>0 {
            let recovered=client.call(&Request::Receipt{protocol_version:Version,source:selection.source.clone(),batch_id:batch.id().clone()}).await?;
            if let Some(admission)=recovered.get("admission").filter(|value|!value.is_null()){
                let admission=AdmissionCommitment::parse(&admission.to_string()).map_err(encode)?;
                queue::record_admission(&mut db,&lease,batch.id(),&admission,now_ms())?;
                if recovered["disposition"]=="terminal_unretained" {queue::record_terminal_unretained(&mut db,&lease,batch.id(),&admission,now_ms())?;}
                if let Some(receipt)=recovered.get("receipt").filter(|value|!value.is_null()){
                    let receipt=Receipt::parse(&receipt.to_string()).map_err(encode)?;
                    if receipt.custody!=Custody::Buffered {queue::acknowledge_retained(&mut db,&lease,batch.id(),&receipt,now_ms())?;return Ok(json!({"schema_version":1,"batches":1,"retained_events":batch.members().len(),"recovered":true,"local":local_status()?}))}
                }
                // Only an authenticated authoritative resend directive reopens
                // the same immutable payload after an admitted dispatch gap.
                if recovered["payload_required"]!=true || recovered["custody"]!="pending" {
                    return Ok(json!({"schema_version":1,"batches":1,"custody":"pending","local":local_status()?}));
                }
            }
            if recovered.get("admission").filter(|value|!value.is_null()).is_none() && recovered["custody"]!="not_found" {return Err("Usage cloud admission remains uncertain; exact batch preserved without resubmission".into());}
        }
        queue::begin_attempt(&mut db,&lease,batch.id(),now_ms())?;
        let body=match client.call_exact(batch.request_json()).await{Ok(body)=>body,Err(error)=>{
            if client.terminal_conflict(){for member in batch.members(){queue::quarantine_event(&mut db,batch.source(),&member.event_id)?;}}
            return Err(error);
        }};
        if !matches!(body["custody"].as_str(),Some("pending"|"buffered"|"retained"|"analytics_ready")){return Err("Usage cloud returned invalid custody; exact batch preserved".into());}
        let admission=AdmissionCommitment::parse(&body["admission"].to_string()).map_err(encode)?;
        admission.verify(batch.source(),batch.provenance(),batch.id(),batch.members()).map_err(encode)?;
        if body["custody"]=="buffered" {queue::record_buffered(&mut db,&lease,batch.id(),&admission,now_ms())?;}else{queue::record_admission(&mut db,&lease,batch.id(),&admission,now_ms())?;}
        if body["disposition"]=="terminal_unretained" {queue::record_terminal_unretained(&mut db,&lease,batch.id(),&admission,now_ms())?;}
        let mut retained=0;
        if let Some(receipt)=body.get("receipt").filter(|value|!value.is_null()){
            let receipt=Receipt::parse(&receipt.to_string()).map_err(encode)?;
            if receipt.custody!=Custody::Buffered{queue::acknowledge_retained(&mut db,&lease,batch.id(),&receipt,now_ms())?;retained=batch.members().len();}
        }
        Ok(json!({"schema_version":1,"batches":1,"ordered_membership_sha256":batch.membership(),"retained_events":retained,"custody":body["custody"],"disposition":body["disposition"],"local":local_status()?}))
    }.await;
    let _ = queue::schedule_next(&mut db, &lease, now_ms(), result.is_ok());
    let _ = queue::release_lease(&mut db, &lease, now_ms());
    result
}
pub(crate) async fn opportunistic() -> Result<Value, String> {
    let Some(selection) = selected()? else {
        return Ok(Value::Null);
    };
    if local_status()?["enabled"] != true {
        return Ok(Value::Null);
    }
    if !queue::due(&mut open()?, &selection.source, now_ms())? {
        return Ok(Value::Null);
    }
    sync().await
}
#[cfg(cargo_ai_cli)]
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
    let mut client = transport::Transport::new(Duration::from_secs(10))?;
    if confirm {
        let mut db = open()?;
        queue::initialize(&mut db)?;
        queue::begin_clear(&mut db, &request)?;
    }
    let body = client.call(&request).await?;
    let returned: Source = serde_json::from_value(body["source"].clone())
        .map_err(|_| "Clear result omitted its exact source")?;
    if returned != selection.source {
        return Err("Clear preview or result changed source; local state preserved".into());
    }
    if confirm {
        let source: Source = serde_json::from_value(body["source"].clone())
            .map_err(|_| "Clear result omitted its exact source")?;
        if source != selection.source
            || body["cleared"] != true
            || body["protocol_version"] != 1
            || body["data_generation"]
                .as_u64()
                .and_then(|generation| Counter::try_from(generation).ok())
                .map(Counter::get)
                != selection.source.data_generation.get().checked_add(1)
        {
            return Err(
                "Clear outcome requires reconciliation; local payloads remain preserved".into(),
            );
        }
        queue::mark_cleared(&mut open()?, &source)?;
    }
    Ok(body)
}
#[cfg(cargo_ai_cli)]
pub(crate) async fn history(
    after: Option<u64>,
    through: Option<u64>,
    limit: u16,
) -> Result<Value, String> {
    if !(1..=100).contains(&limit) || after.is_some() && through.is_none() {
        return Err("History pagination requires the captured through-sequence boundary".into());
    }
    let selection = selected()?.ok_or("Usage cloud source is missing")?;
    let after_sequence = after.map(Counter::try_from).transpose().map_err(encode)?;
    let through_sequence = through
        .map(SequenceBoundary::try_from)
        .transpose()
        .map_err(encode)?;
    let mut body = transport::Transport::new(Duration::from_secs(10))?
        .call(&Request::History {
            protocol_version: Version,
            source: selection.source.clone(),
            after_sequence,
            through_sequence,
            limit,
        })
        .await?;
    let returned: Source = serde_json::from_value(body["source"].clone())
        .map_err(|_| "Usage cloud history omitted its source")?;
    let events = body["events"]
        .as_array()
        .ok_or("Usage cloud history omitted its bounded events")?;
    let high = body["high_watermark"]
        .as_u64()
        .filter(|value| *value <= i64::MAX as u64)
        .ok_or("Usage cloud history omitted its bounded snapshot")?;
    if returned != selection.source
        || events.len() > usize::from(limit)
        || through.is_some_and(|through| through != high)
    {
        return Err("Usage cloud history returned mismatched scope or snapshot bounds".into());
    }
    let mut previous = after.unwrap_or(0);
    let mut ids = std::collections::BTreeSet::new();
    for event in events {
        let sequence = event["sequence"]
            .as_u64()
            .filter(|sequence| *sequence > previous && *sequence <= high)
            .ok_or("Usage cloud history returned unordered snapshot members")?;
        let mut record = event.clone();
        let object = record
            .as_object_mut()
            .ok_or("Usage cloud history event is invalid")?;
        object.remove("sequence");
        let event_id = object
            .remove("event_id")
            .and_then(|value| value.as_str().map(str::to_owned))
            .ok_or("Usage cloud history event identity is missing")?;
        let submission: EventSubmission = serde_json::from_value(record)
            .map_err(|_| "Usage cloud history event fields are invalid")?;
        let member = submission.member().map_err(encode)?;
        if member.event_id != event_id || !ids.insert(event_id) {
            return Err("Usage cloud history contains conflicting event identities".into());
        }
        previous = sequence;
    }
    let next = if !events.is_empty() && previous < high {
        Some(previous)
    } else {
        None
    };
    body["next_sequence"] = json!(next);
    body["through_sequence"] = json!(high);
    Ok(body)
}
