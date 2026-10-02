//! Explicitly enabled logical backup shared by interpreted and standalone runtimes.
#[path = "usage_backup/attribution.rs"]
pub(crate) mod attribution;
#[path = "usage_backup/projection.rs"]
mod projection;
#[path = "usage_backup/queue.rs"]
mod queue;
#[path = "usage_backup/transport.rs"]
mod transport;
use crate::usage_store;
use serde_json::{json, Value};
use std::time::Duration;
pub(crate) use transport::Auth;
#[cfg(test)]
thread_local! { pub(crate) static TEST_ENDPOINT: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) }; }

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Binding {
    pub(crate) account: String,
    pub(crate) generation: u64,
}
impl Binding {
    pub(crate) fn from_result(value: &Value) -> Result<Self, String> {
        let account = value["account_binding"]
            .as_str()
            .ok_or("No cloud backup exists for this account")?;
        if uuid::Uuid::parse_str(account).is_err() {
            return Err("Backup service returned an invalid account binding".into());
        }
        let generation = value["generation"]
            .as_u64()
            .filter(|g| *g > 0 && *g <= i64::MAX as u64)
            .ok_or("Backup service returned an invalid generation")?;
        Ok(Self {
            account: account.into(),
            generation,
        })
    }
    fn from_settings() -> Result<Self, String> {
        let settings = usage_store::settings()?;
        Ok(Self {
            account: settings
                .backup_account_binding
                .ok_or("Enable usage backup explicitly first")?,
            generation: settings
                .backup_generation
                .ok_or("Enable usage backup explicitly first")?,
        })
    }
    fn payload(&self, command: &str) -> Value {
        json!({"command":command,"account_binding":self.account,"generation":self.generation})
    }
    pub(crate) fn check(&self, result: &Value) -> Result<(), String> {
        if Self::from_result(result)? != *self {
            return Err("Signed-in account or backup generation changed; existing queues were not redirected".into());
        }
        Ok(())
    }
}

fn transport(timeout: Duration) -> Result<transport::Transport, String> {
    let auth = crate::usage_backup_host::load_auth()?;
    #[cfg(test)]
    if let Some(endpoint) = TEST_ENDPOINT.with(|value| value.borrow().clone()) {
        return transport::Transport::at(auth, timeout, endpoint);
    }
    transport::Transport::new(auth, timeout)
}
#[cfg(cargo_ai_cli)]
pub(crate) fn local_status() -> Result<Value, String> {
    let mut status = queue::status()?;
    let settings = usage_store::settings()?;
    status["enabled"] = json!(settings.backup_enabled);
    status["account_binding"] = json!(settings.backup_account_binding);
    status["generation"] = json!(settings.backup_generation);
    Ok(status)
}
#[cfg(cargo_ai_cli)]
pub(crate) async fn remote_status() -> Result<Value, String> {
    transport(Duration::from_secs(10))?
        .call(json!({"command":"status"}))
        .await
}
/// Effect facts recorded at the transport and local persistence boundaries.
#[derive(Debug, Default)]
pub(crate) struct OperationEffects {
    pub(crate) remote: &'static str,
    pub(crate) local: &'static str,
    pub(crate) queue: &'static str,
    #[cfg(cargo_ai_cli)]
    pub(crate) settings: &'static str,
    pub(crate) uploaded_records: usize,
    pub(crate) accepted_records: usize,
    pub(crate) batches: usize,
    pub(crate) binding: Option<Binding>,
}
impl OperationEffects {
    fn initial() -> Self {
        Self {
            remote: "unapplied",
            local: "unapplied",
            queue: "unapplied",
            #[cfg(cargo_ai_cli)]
            settings: "unapplied",
            ..Self::default()
        }
    }
    #[cfg(cargo_ai_cli)]
    pub(crate) fn value(&self) -> Value {
        json!({"remote":self.remote,"local":self.local,"queue":self.queue,"settings":self.settings,
            "uploaded_records":self.uploaded_records.to_string(),"remote_accepted_records":self.accepted_records.to_string(),"batches":self.batches,
            "account_binding":self.binding.as_ref().map(|b| &b.account),
            "generation":self.binding.as_ref().map(|b| b.generation),
            "reconciliation":"usage backup status; do not replay an uncertain remote mutation"})
    }
}
#[cfg(cargo_ai_cli)]
#[derive(Debug)]
pub(crate) struct OperationFailure {
    pub(crate) message: String,
    pub(crate) effects: OperationEffects,
}

#[cfg(cargo_ai_cli)]
pub(crate) async fn enable() -> Result<Value, String> {
    enable_outcome().await.map_err(|error| error.message)
}
#[cfg(cargo_ai_cli)]
pub(crate) async fn enable_outcome() -> Result<Value, OperationFailure> {
    let mut effects = OperationEffects::initial();
    enable_impl(&mut effects, crate::usage_backup_host::save_settings)
        .await
        .map_err(|message| OperationFailure { message, effects })
}
#[cfg(all(cargo_ai_cli, test))]
pub(crate) async fn enable_outcome_with_writer(
    persist: impl FnOnce(&crate::usage_store::UsageSettings) -> Result<(), String>,
) -> Result<Value, OperationFailure> {
    let mut effects = OperationEffects::initial();
    enable_impl(&mut effects, persist)
        .await
        .map_err(|message| OperationFailure { message, effects })
}
#[cfg(cargo_ai_cli)]
async fn enable_impl(
    effects: &mut OperationEffects,
    persist: impl FnOnce(&crate::usage_store::UsageSettings) -> Result<(), String>,
) -> Result<Value, String> {
    let _lease = queue::Lease::acquire()?;
    effects.local = "applied";
    let mut client = transport(Duration::from_secs(10))?;
    let status = client.call(json!({"command":"status"})).await?;
    let payload = if status["account_binding"].is_null() {
        json!({"command":"enable"})
    } else {
        Binding::from_result(&status)?.payload("enable")
    };
    effects.remote = "unknown";
    let result = client.call(payload).await?;
    let binding = Binding::from_result(&result)?;
    if result["enabled"] != true {
        return Err("Backup service did not enable consent".into());
    }
    effects.remote = "applied";
    effects.binding = Some(binding.clone());
    let mut settings = usage_store::settings()?;
    let prior = Binding::from_settings().ok();
    let preserve_active = settings.backup_enabled && prior.as_ref() == Some(&binding);
    effects.local = "unknown";
    effects.queue = "unknown";
    if settings.backup_enabled && !preserve_active {
        if let Some(prior) = prior.as_ref() {
            queue::pause(prior)?;
        }
    }
    let boundary = queue::enable(&binding, preserve_active)?;
    effects.queue = "applied";
    settings.backup_enabled = true;
    settings.backup_account_binding = Some(binding.account);
    settings.backup_generation = Some(binding.generation);
    effects.settings = "unknown";
    if let Err(error) = persist(&settings) {
        let _ = queue::pause(&Binding {
            account: settings.backup_account_binding.clone().unwrap(),
            generation: settings.backup_generation.unwrap(),
        });
        return Err(error);
    }
    effects.local = "applied";
    effects.settings = "applied";
    Ok(
        json!({"enabled":true,"account_binding":settings.backup_account_binding,"generation":settings.backup_generation,"future_records_after":boundary.to_string(),"historical_records_selected":0,"note":"Previously queued selections for this same account and generation remain selected. Records collected while upload was disabled require explicit historical selection."}),
    )
}
#[cfg(cargo_ai_cli)]
pub(crate) fn disable() -> Result<Value, String> {
    disable_outcome().map_err(|error| error.message)
}
#[cfg(cargo_ai_cli)]
pub(crate) fn disable_outcome() -> Result<Value, OperationFailure> {
    disable_outcome_with_writer(crate::usage_backup_host::save_settings)
}
#[cfg(cargo_ai_cli)]
pub(crate) fn disable_outcome_with_writer(
    persist: impl FnOnce(&crate::usage_store::UsageSettings) -> Result<(), String>,
) -> Result<Value, OperationFailure> {
    let mut effects = OperationEffects::initial();
    disable_impl(&mut effects, persist).map_err(|message| OperationFailure { message, effects })
}
#[cfg(cargo_ai_cli)]
fn disable_impl(
    effects: &mut OperationEffects,
    persist: impl FnOnce(&crate::usage_store::UsageSettings) -> Result<(), String>,
) -> Result<Value, String> {
    let _lease = queue::Lease::acquire()?;
    effects.local = "applied";
    let mut settings = usage_store::settings()?;
    if let Ok(binding) = Binding::from_settings() {
        effects.queue = "unknown";
        queue::pause(&binding)?;
        effects.queue = "applied";
    }
    settings.backup_enabled = false;
    effects.settings = "unknown";
    persist(&settings)?;
    effects.settings = "applied";
    Ok(
        json!({"enabled":false,"history_deleted":false,"queued_selections_preserved":true,"note":"Already transmitted requests may finish; future upload passes stop."}),
    )
}
#[cfg(cargo_ai_cli)]
pub(crate) fn include_history(
    through: Option<i64>,
    expected: Option<Binding>,
    confirmed: bool,
) -> Result<Value, String> {
    let _lease = queue::Lease::acquire()?;
    let binding = Binding::from_settings()?;
    if !usage_store::settings()?.backup_enabled {
        return Err("Enable backup for the intended account before selecting older history".into());
    }
    if confirmed && expected.as_ref() != Some(&binding) {
        return Err(
            "The selected account or generation changed since preview; preview again".into(),
        );
    }
    if confirmed && through.is_none() {
        return Err("Confirmation requires the preview snapshot using --through".into());
    }
    let high = queue::high(&queue::open()?)?;
    let through = through.unwrap_or(high);
    let count = queue::include_history(&binding, through, confirmed)?;
    Ok(
        json!({"account_binding":binding.account,"generation":binding.generation,"through":through.to_string(),"selected_records":count,"queued":confirmed,"note":"Selection does not upload records; restored facts are never automatically selected."}),
    )
}
async fn drain(max_batches: usize, timeout: Duration) -> Result<Value, String> {
    drain_impl(max_batches, timeout, &mut OperationEffects::initial()).await
}
async fn drain_impl(
    max_batches: usize,
    timeout: Duration,
    effects: &mut OperationEffects,
) -> Result<Value, String> {
    // Disabled backup must not inspect account credentials or create a database.
    if !usage_store::settings()?.backup_enabled {
        return Ok(json!({"enabled":false,"uploaded_records":0}));
    }
    let binding = Binding::from_settings()?;
    let _lease = queue::Lease::acquire()?;
    effects.local = "applied";
    let mut client = transport(timeout)?;
    let status = client.call(json!({"command":"status"})).await?;
    binding.check(&status)?;
    if status["enabled"] != true {
        return Err("Cloud backup consent is disabled; local history remains available".into());
    }
    let mut uploaded = 0;
    let mut batches = 0;
    for _ in 0..max_batches {
        let settings = usage_store::settings()?;
        if !settings.backup_enabled || Binding::from_settings()? != binding {
            break;
        }
        effects.local = "unknown";
        effects.queue = "unknown";
        queue::capture(&binding)?;
        effects.local = "applied";
        effects.queue = "applied";
        let records = queue::batch(&binding)?;
        if records.is_empty() {
            break;
        }
        if records.iter().any(|record| record["schema_version"] == 2)
            && !status["supported_ingest_record_schema_versions"]
                .as_array()
                .is_some_and(|versions| versions.iter().any(|version| version.as_u64() == Some(2)))
        {
            return Err("Backup service cannot ingest record schema 2; update or recover the service before syncing. Pending selections remain".into());
        }
        let expected: std::collections::BTreeSet<String> = records
            .iter()
            .filter_map(|r| r["event_id"].as_str().map(str::to_owned))
            .collect();
        let mut payload = binding.payload("ingest");
        payload["records"] = json!(records);
        effects.remote = "unknown";
        let response = client.call(payload).await?;
        binding.check(&response)?;
        let acknowledged = response["acknowledged_event_ids"]
            .as_array()
            .ok_or("Backup acknowledgment is missing; pending selections remain")?;
        let ids = acknowledged
            .iter()
            .map(|id| {
                id.as_str()
                    .filter(|id| expected.contains(*id))
                    .map(str::to_owned)
                    .ok_or("Backup acknowledgment contains an unexpected identity")
            })
            .collect::<Result<Vec<_>, _>>()?;
        let unique: std::collections::BTreeSet<_> = ids.iter().collect();
        if unique.len() != ids.len() || ids.len() != expected.len() {
            return Err("Backup acknowledgment is incomplete; pending selections remain".into());
        }
        effects.remote = "applied";
        effects.accepted_records += ids.len();
        effects.binding = Some(binding.clone());
        effects.local = "unknown";
        effects.queue = "unknown";
        queue::acknowledge(&binding, &ids)?;
        effects.local = "applied";
        effects.queue = "applied";
        uploaded += ids.len();
        batches += 1;
        effects.uploaded_records = uploaded;
        effects.batches = batches;
    }
    Ok(
        json!({"enabled":true,"uploaded_records":uploaded,"batches":batches,"account_binding":binding.account,"generation":binding.generation,"local":queue::status()?}),
    )
}
#[cfg(cargo_ai_cli)]
pub(crate) async fn sync() -> Result<Value, String> {
    sync_outcome().await.map_err(|error| error.message)
}
#[cfg(cargo_ai_cli)]
pub(crate) async fn sync_outcome() -> Result<Value, OperationFailure> {
    let mut effects = OperationEffects::initial();
    let result = tokio::time::timeout(
        Duration::from_secs(30),
        drain_impl(10, Duration::from_secs(30), &mut effects),
    )
    .await
    .unwrap_or_else(|_| {
        Err("Backup sync reached its 30-second budget; pending selections remain".into())
    });
    result.map_err(|message| OperationFailure { message, effects })
}
pub(crate) async fn opportunistic() {
    if !usage_store::settings().is_ok_and(|s| s.backup_enabled) {
        return;
    }
    let Ok(binding) = Binding::from_settings() else {
        return;
    };
    if !queue::auto_ready(&binding).unwrap_or(false) {
        return;
    }
    match tokio::time::timeout(Duration::from_secs(2), drain(1, Duration::from_secs(2))).await {
        Ok(Ok(_)) => {
            let _ = queue::reset_backoff(&binding);
        }
        _ => {
            let _ = queue::defer(&binding);
            eprintln!("Usage backup deferred; local execution and history remain available. Use `cargo ai usage backup status --json` or explicit `sync` for details.");
        }
    }
}
#[cfg(cargo_ai_cli)]
pub(crate) async fn restore(
    cursor: Option<String>,
    expected: Option<Binding>,
    confirmed: bool,
) -> Result<Value, String> {
    let _lease = if confirmed {
        Some(queue::Lease::acquire()?)
    } else {
        None
    };
    let mut client = transport(Duration::from_secs(10))?;
    let status = client.call(json!({"command":"status"})).await?;
    let binding = Binding::from_result(&status)?;
    if !confirmed {
        return Ok(
            json!({"account_binding":binding.account,"generation":binding.generation,"cloud_records":status["record_count"],"restored":false,"note":"Use --confirm to import one stable snapshot page (up to 100 records). Original event IDs are preserved and restored records are excluded from automatic upload."}),
        );
    }
    if expected.as_ref() != Some(&binding) {
        return Err("The cloud account or generation changed since preview; preview again".into());
    }
    let mut payload = binding.payload("restore");
    payload["limit"] = json!(100);
    let supports_v2 = status["supported_record_schema_versions"]
        .as_array()
        .is_some_and(|versions| versions.iter().any(|version| version.as_u64() == Some(2)));
    if supports_v2 {
        payload["max_record_schema_version"] = json!(2);
    }
    if let Some(cursor) = cursor {
        payload["cursor"] = json!(cursor);
    }
    let page = client.call(payload).await?;
    binding.check(&page)?;
    let records = page["records"]
        .as_array()
        .ok_or("Backup page is missing its records")?;
    if !supports_v2 && records.iter().any(|record| record["schema_version"] != 1) {
        return Err(
            "Backup service returned a record schema it did not advertise; no page was imported"
                .into(),
        );
    }
    let inserted = queue::restore(records, &binding.account)?;
    Ok(
        json!({"account_binding":binding.account,"generation":binding.generation,"page_records":records.len(),"inserted_records":inserted,"next_cursor":page["next_cursor"],"snapshot_complete":page["snapshot_complete"],"note":"Continue --confirm --cursor with next_cursor until snapshot_complete. Local-only labels are preserved for existing facts; newly restored labels may be opaque IDs."}),
    )
}
#[cfg(cargo_ai_cli)]
pub(crate) async fn delete_cloud(
    expected: Option<Binding>,
    confirmed: bool,
) -> Result<Value, String> {
    let _lease = if confirmed {
        Some(queue::Lease::acquire()?)
    } else {
        None
    };
    let mut client = transport(Duration::from_secs(10))?;
    let status = client.call(json!({"command":"status"})).await?;
    let binding = Binding::from_result(&status)?;
    if !confirmed {
        return Ok(
            json!({"account_binding":binding.account,"generation":binding.generation,"selected_records":status["record_count"],"deleted":false,"note":"Confirm to delete cloud usage only, revoke this generation, and disable backup. Local history remains."}),
        );
    }
    if expected.as_ref() != Some(&binding) {
        return Err("The cloud account or generation changed since preview; preview again".into());
    }
    let mut payload = binding.payload("delete");
    payload["confirmed"] = json!(true);
    let result = client.call(payload).await?;
    let current = usage_store::settings()?;
    if current.backup_account_binding.as_deref() == Some(&binding.account) {
        queue::pause(&binding)?;
        let mut updated = current;
        updated.backup_enabled = false;
        crate::usage_backup_host::save_settings(&updated)?;
    }
    Ok(
        json!({"deleted":true,"cloud":result,"local_history_deleted":false,"note":"Old queues cannot repopulate the deleted generation. Re-enable explicitly and preview/select older history to back it up again."}),
    )
}
