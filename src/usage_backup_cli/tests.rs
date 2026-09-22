use crate::usage_backup::{self, Binding, TEST_ENDPOINT};
use crate::usage_store;
use mockito::{Matcher, Mock, Server};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

struct Home {
    path: PathBuf,
    previous: Vec<(&'static str, Option<OsString>)>,
    endpoint: Option<String>,
}

impl Home {
    fn new(endpoint: String, credentials: bool) -> Self {
        // The repository wrapper serializes all environment-mutating tests.
        assert_eq!(std::env::var("RUST_TEST_THREADS").as_deref(), Ok("1"));
        let path = std::env::temp_dir().join(format!("backup-client-{}", uuid::Uuid::new_v4()));
        let previous = ["CARGO_AI_HOME", "CARGO_AI_DISABLE_KEYCHAIN"]
            .into_iter()
            .map(|key| (key, std::env::var_os(key)))
            .collect();
        unsafe {
            std::env::set_var("CARGO_AI_HOME", &path);
            std::env::set_var("CARGO_AI_DISABLE_KEYCHAIN", "1");
        }
        let endpoint = TEST_ENDPOINT.with(|value| value.replace(Some(endpoint)));
        let home = Self {
            path,
            previous,
            endpoint,
        };
        if credentials {
            std::fs::create_dir_all(&home.path).unwrap();
            std::fs::write(
                home.path.join("config.toml"),
                "profile = []\nsecret_store = \"file\"\n",
            )
            .unwrap();
            crate::credentials::store::store_account_tokens(
                "fixture-access",
                Some("fixture-refresh"),
            )
            .unwrap();
        }
        home
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        TEST_ENDPOINT.with(|value| value.replace(self.endpoint.take()));
        for (key, old) in &self.previous {
            unsafe {
                match old {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
        }
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn binding() -> Binding {
    Binding {
        account: uuid::Uuid::new_v4().to_string(),
        generation: 1,
    }
}

fn state(binding: &Binding) -> Value {
    json!({"account_binding":binding.account,"generation":binding.generation,"enabled":true,"record_count":0})
}

fn response(body: Value) -> String {
    json!({"status":"success","type":"usage_backup_succeeded","usage_backup":body}).to_string()
}

fn command(command: &str) -> Matcher {
    Matcher::PartialJson(json!({"action":"usage_backup","usage_backup":{"command":command}}))
}

async fn reply(server: &mut Server, name: &str, body: Value, count: usize) -> Mock {
    server
        .mock("POST", "/account")
        .match_body(command(name))
        .with_status(200)
        .with_body(response(body))
        .expect(count)
        .create_async()
        .await
}

async fn enable(server: &mut Server, binding: &Binding) -> Value {
    let status = reply(server, "status", state(binding), 1).await;
    let enable = reply(server, "enable", state(binding), 1).await;
    let result = usage_backup::enable().await.unwrap();
    status.assert_async().await;
    enable.assert_async().await;
    status.remove_async().await;
    enable.remove_async().await;
    result
}

fn append() -> Value {
    let event = json!({
        "schema_version":1,"event_id":format!("cai_event_{}",uuid::Uuid::new_v4()),
        "root_run_id":format!("cai_run_{}",uuid::Uuid::new_v4()),
        "agent_run_id":format!("cai_agent_run_{}",uuid::Uuid::new_v4()),
        "event_type":"provider_request_completed","timestamp":"2026-09-22T00:00:00Z",
        "provider":{"server":"typesafe","profile":"private fixture customer","requested_model":"jev-1.13.0"},
        "agent":{"project_root":"/private/fixture/project","name":"private fixture agent"},
        "usage":{"input_tokens":7,"output_tokens":3},"status":"success"
    });
    usage_store::persist(&event).unwrap();
    event
}

fn event_id(event: &Value) -> String {
    event["event_id"].as_str().unwrap().to_owned()
}

fn stored(event_id: &str) -> Option<Value> {
    use rusqlite::OptionalExtension;
    let db = usage_store::open_database(false).unwrap().unwrap();
    let text: Option<String> = db
        .query_row(
            "SELECT record_json FROM usage_events WHERE event_id=?1",
            [event_id],
            |r| r.get(0),
        )
        .optional()
        .unwrap();
    text.map(|text| serde_json::from_str(&text).unwrap())
}

fn projection(event_id: &str) -> Value {
    let db = usage_store::open_database(false).unwrap().unwrap();
    let text: String = db
        .query_row(
            "SELECT record_json FROM usage_backup_projection WHERE event_id=?1",
            [event_id],
            |r| r.get(0),
        )
        .unwrap();
    serde_json::from_str(&text).unwrap()
}

async fn acknowledge(server: &mut Server, binding: &Binding, ids: &[String]) -> Mock {
    let expected = ids.to_vec();
    let mut body = state(binding);
    body["acknowledged_event_ids"] = json!(ids);
    server
        .mock("POST", "/account")
        .match_body(command("ingest"))
        .match_request(move |request| {
            let Ok(bytes) = request.body() else {
                return false;
            };
            let Ok(body) = serde_json::from_slice::<Value>(bytes) else {
                return false;
            };
            let ids: Vec<_> = body["usage_backup"]["records"]
                .as_array()
                .unwrap()
                .iter()
                .map(event_id)
                .collect();
            ids == expected
                && body.get("cargo_ai_metadata").is_none()
                && !body["usage_backup"].to_string().contains("private fixture")
                && !body["usage_backup"].to_string().contains("/private/")
        })
        .with_status(200)
        .with_body(response(body))
        .expect(1)
        .create_async()
        .await
}

#[tokio::test]
async fn future_consent_survives_duplicate_enable_and_pause_without_selecting_old_facts() {
    let mut server = Server::new_async().await;
    let _home = Home::new(format!("{}/account", server.url()), true);
    let binding = binding();
    let historical = append();
    assert_eq!(
        enable(&mut server, &binding).await["future_records_after"],
        "1"
    );
    let before_pause = append();
    assert_eq!(
        enable(&mut server, &binding).await["future_records_after"],
        "1"
    );
    usage_backup::disable().unwrap();
    let paused = append();
    assert_eq!(
        enable(&mut server, &binding).await["future_records_after"],
        "3"
    );
    let after_pause = append();
    let status = reply(&mut server, "status", state(&binding), 2).await;
    let ingest = acknowledge(
        &mut server,
        &binding,
        &[event_id(&before_pause), event_id(&after_pause)],
    )
    .await;
    assert_eq!(usage_backup::sync().await.unwrap()["uploaded_records"], 2);
    assert_eq!(usage_backup::sync().await.unwrap()["uploaded_records"], 0);
    let local = usage_backup::local_status().unwrap();
    assert_eq!(local["acknowledged_records"], 2);
    assert_eq!(local["pending_records"], 0);
    assert_eq!(stored(&event_id(&historical)), Some(historical));
    assert_eq!(stored(&event_id(&paused)), Some(paused));
    ingest.assert_async().await;
    status.assert_async().await;
}

#[tokio::test]
async fn changed_account_blocks_prior_queue_and_cloud_delete_confirmation() {
    let mut server = Server::new_async().await;
    let _home = Home::new(format!("{}/account", server.url()), true);
    let original = binding();
    let changed = binding();
    enable(&mut server, &original).await;
    let fact = append();
    usage_backup::include_history(Some(1), Some(original.clone()), true).unwrap();
    crate::credentials::store::store_account_tokens("fixture-other-account", None).unwrap();
    let status = reply(&mut server, "status", state(&changed), 2).await;
    let ingest = reply(&mut server, "ingest", state(&changed), 0).await;
    let delete = reply(&mut server, "delete", state(&changed), 0).await;
    assert!(usage_backup::sync().await.unwrap_err().contains("account"));
    assert!(usage_backup::delete_cloud(Some(original.clone()), true)
        .await
        .unwrap_err()
        .contains("changed since preview"));
    let local = usage_backup::local_status().unwrap();
    assert_eq!(local["pending_records"], 1);
    assert_eq!(
        local["account_queues"][0]["account_binding"],
        original.account
    );
    assert_eq!(stored(&event_id(&fact)), Some(fact));
    status.assert_async().await;
    ingest.assert_async().await;
    delete.assert_async().await;
}

#[tokio::test]
async fn incomplete_acknowledgment_preserves_the_whole_batch_for_retry() {
    let mut server = Server::new_async().await;
    let _home = Home::new(format!("{}/account", server.url()), true);
    let binding = binding();
    enable(&mut server, &binding).await;
    let first = append();
    let second = append();
    let status = reply(&mut server, "status", state(&binding), 2).await;
    let mut partial = state(&binding);
    partial["acknowledged_event_ids"] = json!([event_id(&first)]);
    let failed = reply(&mut server, "ingest", partial, 1).await;
    assert!(usage_backup::sync()
        .await
        .unwrap_err()
        .contains("incomplete"));
    assert_eq!(usage_backup::local_status().unwrap()["pending_records"], 2);
    failed.assert_async().await;
    failed.remove_async().await;
    let complete = acknowledge(
        &mut server,
        &binding,
        &[event_id(&first), event_id(&second)],
    )
    .await;
    assert_eq!(usage_backup::sync().await.unwrap()["uploaded_records"], 2);
    assert_eq!(usage_backup::local_status().unwrap()["pending_records"], 0);
    complete.assert_async().await;
    status.assert_async().await;
}

#[tokio::test]
async fn restore_conflict_rolls_back_page_and_identical_restore_preserves_labels_without_reupload()
{
    let mut server = Server::new_async().await;
    let _home = Home::new(format!("{}/account", server.url()), true);
    let binding = binding();
    enable(&mut server, &binding).await;
    let local = append();
    let status = reply(&mut server, "status", state(&binding), 5).await;
    let ingest = acknowledge(&mut server, &binding, &[event_id(&local)]).await;
    usage_backup::sync().await.unwrap();
    let original = projection(&event_id(&local));
    let mut remote = original.clone();
    remote["event_id"] = json!(format!("cai_event_{}", uuid::Uuid::new_v4()));
    let mut conflicting = original.clone();
    conflicting["tokens"]["input_tokens"] = json!(999);
    let mut page = state(&binding);
    page["records"] = json!([remote, conflicting]);
    page["snapshot_complete"] = json!(true);
    page["next_cursor"] = Value::Null;
    let conflict = reply(&mut server, "restore", page.clone(), 1).await;
    assert!(usage_backup::restore(None, Some(binding.clone()), true)
        .await
        .unwrap_err()
        .contains("conflicts"));
    assert!(stored(&event_id(&remote)).is_none());
    assert_eq!(stored(&event_id(&local)), Some(local.clone()));
    conflict.assert_async().await;
    conflict.remove_async().await;
    page["records"] = json!([remote, original]);
    let restored = reply(&mut server, "restore", page, 2).await;
    assert_eq!(
        usage_backup::restore(None, Some(binding.clone()), true)
            .await
            .unwrap()["inserted_records"],
        1
    );
    assert_eq!(
        usage_backup::restore(None, Some(binding.clone()), true)
            .await
            .unwrap()["inserted_records"],
        0
    );
    assert_eq!(stored(&event_id(&local)), Some(local));
    assert_eq!(stored(&event_id(&remote)).unwrap()["restored"], true);
    assert_eq!(usage_backup::sync().await.unwrap()["uploaded_records"], 0);
    let selected = usage_backup::include_history(Some(2), Some(binding), true).unwrap();
    assert_eq!(selected["selected_records"], 0);
    assert_eq!(
        usage_backup::local_status().unwrap()["acknowledged_records"],
        1
    );
    ingest.assert_async().await;
    restored.assert_async().await;
    status.assert_async().await;
}

#[tokio::test]
async fn disabled_opportunistic_backup_creates_no_home_and_ignores_unreadable_credentials() {
    let mut server = Server::new_async().await;
    let home = Home::new(format!("{}/account", server.url()), false);
    let requests = server
        .mock("POST", "/account")
        .expect(0)
        .create_async()
        .await;
    usage_backup::opportunistic().await;
    assert!(!home.path.exists());
    std::fs::create_dir_all(home.path.join("credentials.toml")).unwrap();
    usage_backup::opportunistic().await;
    assert_eq!(usage_backup::sync().await.unwrap()["enabled"], false);
    assert!(!home.path.join("usage").exists());
    assert!(home.path.join("credentials.toml").is_dir());
    requests.assert_async().await;
}

#[tokio::test]
async fn uncertain_http_response_keeps_identical_queue_for_deduplicated_later_sync() {
    let mut server = Server::new_async().await;
    let _home = Home::new(format!("{}/account", server.url()), true);
    let binding = binding();
    enable(&mut server, &binding).await;
    let fact = append();
    let status = reply(&mut server, "status", state(&binding), 2).await;
    let cloud = Arc::new(Mutex::new(BTreeMap::<String, Value>::new()));
    let first_cloud = cloud.clone();
    let interrupted = server
        .mock("POST", "/account")
        .match_body(command("ingest"))
        .with_status(500)
        .with_body_from_request(move |request| {
            let body: Value = serde_json::from_slice(request.body().unwrap()).unwrap();
            let mut cloud = first_cloud.lock().unwrap();
            for record in body["usage_backup"]["records"].as_array().unwrap() {
                cloud.insert(event_id(record), record.clone());
            }
            b"{".to_vec()
        })
        .expect(1)
        .create_async()
        .await;
    assert!(usage_backup::sync().await.is_err());
    assert_eq!(usage_backup::local_status().unwrap()["pending_records"], 1);
    interrupted.assert_async().await;
    interrupted.remove_async().await;
    let second_cloud = cloud.clone();
    let response_state = state(&binding);
    let retried = server
        .mock("POST", "/account")
        .match_body(command("ingest"))
        .with_status(200)
        .with_body_from_request(move |request| {
            let body: Value = serde_json::from_slice(request.body().unwrap()).unwrap();
            let cloud = second_cloud.lock().unwrap();
            let records = body["usage_backup"]["records"].as_array().unwrap();
            if records
                .iter()
                .any(|record| cloud.get(&event_id(record)) != Some(record))
            {
                return br#"{"status":"failed","type":"usage_backup_conflict"}"#.to_vec();
            }
            let mut acknowledged = response_state.clone();
            acknowledged["acknowledged_event_ids"] =
                json!(records.iter().map(event_id).collect::<Vec<_>>());
            response(acknowledged).into_bytes()
        })
        .expect(1)
        .create_async()
        .await;
    assert_eq!(usage_backup::sync().await.unwrap()["uploaded_records"], 1);
    assert_eq!(usage_backup::local_status().unwrap()["pending_records"], 0);
    assert_eq!(cloud.lock().unwrap().len(), 1);
    assert_eq!(
        cloud.lock().unwrap().get(&event_id(&fact)),
        Some(&projection(&event_id(&fact)))
    );
    retried.assert_async().await;
    status.assert_async().await;
}

#[tokio::test]
async fn logout_or_credential_change_during_status_stops_ingest() {
    for logout in [true, false] {
        let mut server = Server::new_async().await;
        let home = Home::new(format!("{}/account", server.url()), true);
        let binding = binding();
        enable(&mut server, &binding).await;
        let fact = append();
        let credentials_path = home.path.join("credentials.toml");
        let status_body = response(state(&binding));
        let status = server
            .mock("POST", "/account")
            .match_body(command("status"))
            .with_status(200)
            .with_body_from_request(move |_| {
                // Complete the local account change before returning the cloud
                // status from the session that was loaded at drain startup.
                if logout {
                    std::fs::remove_file(&credentials_path).unwrap();
                } else {
                    std::fs::write(
                        &credentials_path,
                        "[account]\naccess_token = \"fixture-changed-account\"\n",
                    )
                    .unwrap();
                }
                status_body.as_bytes().to_vec()
            })
            .expect(1)
            .create_async()
            .await;
        let ingest = server
            .mock("POST", "/account")
            .match_body(command("ingest"))
            .expect(0)
            .create_async()
            .await;
        let error = usage_backup::sync().await.unwrap_err();
        assert!(error.contains("Sign in") || error.contains("credentials changed"));
        assert_eq!(usage_backup::local_status().unwrap()["pending_records"], 1);
        assert_eq!(stored(&event_id(&fact)), Some(fact));
        status.assert_async().await;
        ingest.assert_async().await;
    }
}

#[tokio::test]
async fn explicit_expiry_refreshes_in_memory_without_changing_stored_credentials() {
    let mut server = Server::new_async().await;
    let home = Home::new(format!("{}/account", server.url()), true);
    let binding = binding();
    enable(&mut server, &binding).await;
    let fact = append();
    let credentials_path = home.path.join("credentials.toml");
    let original_credentials = std::fs::read(&credentials_path).unwrap();
    let status = reply(&mut server, "status", state(&binding), 1).await;
    let expired = server
        .mock("POST", "/account")
        .match_body(Matcher::PartialJson(json!({
            "action":"usage_backup",
            "credentials":{"access_token":"fixture-access"},
            "usage_backup":{"command":"ingest","account_binding":binding.account,"generation":binding.generation}
        })))
        .with_status(200)
        .with_body(r#"{"status":"failed","type":"access_token_expired"}"#)
        .expect(1)
        .create_async()
        .await;
    let refresh = server
        .mock("POST", "/account")
        .match_body(Matcher::Json(json!({
            "action":"status",
            "credentials":{"access_token":"fixture-access","refresh_token":"fixture-refresh"},
            "session_policy":{"allow_refresh":true}
        })))
        .with_status(200)
        .with_body(r#"{"status":"success","session":{"access_token":"fixture-renewed-access"}}"#)
        .expect(1)
        .create_async()
        .await;
    let mut acknowledged = state(&binding);
    acknowledged["acknowledged_event_ids"] = json!([event_id(&fact)]);
    let renewed = server
        .mock("POST", "/account")
        .match_body(Matcher::PartialJson(json!({
            "action":"usage_backup",
            "credentials":{"access_token":"fixture-renewed-access"},
            "usage_backup":{"command":"ingest","account_binding":binding.account,"generation":binding.generation,
                "records":[{"event_id":event_id(&fact)}]}
        })))
        .with_status(200)
        .with_body(response(acknowledged))
        .expect(1)
        .create_async()
        .await;
    assert_eq!(usage_backup::sync().await.unwrap()["uploaded_records"], 1);
    assert_eq!(usage_backup::local_status().unwrap()["pending_records"], 0);
    assert_eq!(
        std::fs::read(credentials_path).unwrap(),
        original_credentials
    );
    status.assert_async().await;
    expired.assert_async().await;
    refresh.assert_async().await;
    renewed.assert_async().await;
}

#[tokio::test]
async fn tracking_changes_preserve_completed_backup_disable_and_pending_facts() {
    let mut server = Server::new_async().await;
    let _home = Home::new(format!("{}/account", server.url()), true);
    let binding = binding();
    enable(&mut server, &binding).await;
    let fact = append();
    let stale = usage_store::settings().unwrap();
    assert!(stale.backup_enabled);
    usage_backup::disable().unwrap();
    crate::config::settings::mutate_usage_tracking(!stale.tracking).unwrap();
    let current = usage_store::settings().unwrap();
    assert!(!current.backup_enabled);
    assert!(!current.tracking);
    assert_eq!(current.backup_account_binding, stale.backup_account_binding);
    assert_eq!(stored(&event_id(&fact)), Some(fact));
    let ingest = server
        .mock("POST", "/account")
        .expect(0)
        .create_async()
        .await;
    usage_backup::opportunistic().await;
    ingest.assert_async().await;
    // A backup-owned write cannot restore the earlier tracking choice either.
    crate::config::settings::mutate_usage_backup_settings(&stale).unwrap();
    assert!(!usage_store::settings().unwrap().tracking);
    usage_backup::disable().unwrap();
}

#[tokio::test]
async fn disable_reports_busy_without_claiming_to_override_an_active_consent_transition() {
    let mut server = Server::new_async().await;
    let _home = Home::new(format!("{}/account", server.url()), true);
    enable(&mut server, &binding()).await;
    let db = usage_store::open_database(true).unwrap().unwrap();
    db.execute("INSERT INTO usage_backup_lease(singleton,token,expires_at) VALUES(1,'fixture-active-transition',?1)",
        [time::OffsetDateTime::now_utc().unix_timestamp()+40]).unwrap();
    assert!(usage_backup::disable()
        .unwrap_err()
        .contains("Another backup operation"));
    assert!(usage_store::settings().unwrap().backup_enabled);
    db.execute(
        "DELETE FROM usage_backup_lease WHERE token='fixture-active-transition'",
        [],
    )
    .unwrap();
    usage_backup::disable().unwrap();
    assert!(!usage_store::settings().unwrap().backup_enabled);
}
