//! Explicit protocol-2 client journeys use disposable Homes and loopback accounts.
#![cfg(cargo_ai_cli)]
use super::*;
use mockito::{Matcher, Server};
use std::{ffi::OsString, path::PathBuf};

struct Home {
    path: PathBuf,
    previous: Vec<(&'static str, Option<OsString>)>,
    endpoint: Option<String>,
}
impl Home {
    fn new(endpoint: String, credentials: bool) -> Self {
        assert_eq!(std::env::var("RUST_TEST_THREADS").as_deref(), Ok("1"));
        let path =
            std::env::temp_dir().join(format!("usage-upload-client-{}", uuid::Uuid::new_v4()));
        let previous = [
            "CARGO_AI_HOME",
            "CARGO_AI_DISABLE_KEYCHAIN",
            "CARGO_AI_USAGE_TRACKING",
        ]
        .into_iter()
        .map(|key| (key, std::env::var_os(key)))
        .collect();
        unsafe {
            std::env::set_var("CARGO_AI_HOME", &path);
            std::env::set_var("CARGO_AI_DISABLE_KEYCHAIN", "1");
            std::env::set_var("CARGO_AI_USAGE_TRACKING", "on");
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
    fn policy(&self) {
        crate::usage_store::retention::configure(crate::usage_store::retention::Policy {
            schema_version: 2,
            days: 7,
            enabled: true,
            local_detail_loss_acknowledged: true,
            unselected_loss_acknowledged: true,
        })
        .unwrap();
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
fn uuid(n: u64) -> CanonicalUuid {
    CanonicalUuid::try_from(format!("00000000-0000-4000-8000-{n:012}")).unwrap()
}
fn matcher(command: &str) -> Matcher {
    Matcher::PartialJson(
        json!({"action":"usage_analytics","usage_analytics":{"command":command,"protocol_version":2}}),
    )
}
fn response(body: Value) -> String {
    json!({"status":"success","type":"usage_analytics_succeeded","usage_analytics":body})
        .to_string()
}
async fn enable_source(server: &mut Server, generation: u64) {
    let status = server.mock("POST", "/account").match_body(matcher("status")).with_body(response(json!({"protocol_version":2,"capability":{"enabled":true,"supported_protocol_versions":[2]},"account_binding":uuid(1),"data_generation":generation,"lifecycle_epoch":0,"consent_interval":null,"enabled":false}))).expect(1).create_async().await;
    let enabled = server.mock("POST", "/account").match_body(matcher("enable")).with_body_from_request(move |request| {
        let body: Value = serde_json::from_slice(request.body().unwrap()).unwrap();
        assert!(body["usage_analytics"].get("source").is_none());
        response(json!({"protocol_version":2,"source":{"account_binding":uuid(1),"source_home_id":body["usage_analytics"]["source_home_id"],"data_generation":generation},"provenance":{"lifecycle_epoch":0,"consent_interval":generation},"enabled":true,"policy":{"retention_days":7}})).into_bytes()
    }).expect(1).create_async().await;
    let result = invoke(&["cloud", "enable"]).await["result"].clone();
    assert_eq!(result["enabled"], true);
    assert_eq!(result["historical_records_selected"], 0);
    status.assert_async().await;
    enabled.assert_async().await;
    status.remove_async().await;
    enabled.remove_async().await;
    let _status = server
        .mock("POST", "/account")
        .match_body(matcher("status"))
        .with_body_from_request(|_| response(current_status(7)).into_bytes())
        .expect_at_least(0)
        .create_async()
        .await;
}
fn current_status(days: u32) -> Value {
    let selection = selected().unwrap().unwrap();
    json!({"protocol_version":2,"capability":{"enabled":true,"supported_protocol_versions":[2]},"account_binding":selection.source.account_binding,"data_generation":selection.source.data_generation,"lifecycle_epoch":selection.provenance.lifecycle_epoch,"environments":[{"source_home_id":selection.source.source_home_id,"consent_interval":selection.provenance.consent_interval}],"policy":{"retention_days":days,"retained_event_target":1_000_000,"max_active_environments":3},"processing_status":"not_observed"})
}
fn append(index: u64, offset_ms: i64, restored: bool) -> String {
    let event_id = uuid(index + 100).as_str().to_owned();
    let timestamp = time::OffsetDateTime::from_unix_timestamp_nanos(
        i128::from(now_ms() - offset_ms) * 1_000_000,
    )
    .unwrap()
    .format(&time::format_description::well_known::Rfc3339)
    .unwrap();
    crate::usage_store::persist(&json!({"schema_version":1,"event_id":event_id,"root_run_id":uuid(10),"agent_run_id":uuid(11),"event_type":"provider_request_completed","timestamp":timestamp,"provider":{"server":"typesafe","profile":"private fixture customer","requested_model":"jev-1.13.0"},"agent":{"project_root":"/private/fixture/project","name":"private fixture agent"},"usage":{"input_tokens":100,"output_tokens":3},"status":"success","restored":restored})).unwrap();
    event_id
}
fn ack(text: &[u8]) -> Value {
    let body: Value = serde_json::from_slice(text).unwrap();
    let request: Request = serde_json::from_value(body["usage_analytics"].clone()).unwrap();
    let Request::Upload {
        source,
        provenance,
        batch_id,
        ordered_membership_sha256,
        batch,
        ..
    } = request
    else {
        panic!("expected V2 upload")
    };
    let events = elastic::decode_batch(&batch).unwrap();
    assert_eq!(events.len(), batch.event_count as usize);
    let members = events
        .iter()
        .map(EventSubmission::member)
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(
        elastic::membership_sha256(&source, &members).unwrap(),
        ordered_membership_sha256
    );
    assert!(!serde_json::to_string(&events)
        .unwrap()
        .contains("private fixture"));
    json!(elastic::UploadAck {
        protocol_version: Version,
        source,
        provenance,
        batch_id,
        ordered_membership_sha256,
        encoded_sha256: batch.encoded_sha256,
        acknowledged_events: batch.event_count,
        durable_acknowledged: true,
        processing_status: elastic::ProcessingStatus::NotObserved
    })
}
async fn invoke(args: &[&str]) -> Value {
    let args = clap::Command::new("test")
        .subcommand(crate::usage_analytics_cli::cloud_command())
        .subcommand(crate::usage_analytics_cli::retention_command())
        .try_get_matches_from(std::iter::once("test").chain(args.iter().copied()))
        .unwrap();
    let value = crate::usage_analytics_cli::machine_run(&args)
        .await
        .unwrap();
    crate::usage_analytics_cli::schema_tests::assert_valid(&value);
    value
}

#[tokio::test]
async fn passive_status_and_disabled_sync_create_no_home_or_network() {
    let home = Home::new("http://127.0.0.1:1/account".into(), false);
    assert_eq!(local_status().unwrap()["schema_version"], 2);
    assert_eq!(sync().await.unwrap()["enabled"], false);
    assert!(!home.path.exists());
}
#[tokio::test]
async fn explicit_cloud_consent_requires_selected_rolling_policy_and_enabled_history() {
    let home = Home::new("http://127.0.0.1:1/account".into(), false);
    assert!(enable(None).await.unwrap_err().contains("rolling policy"));
    home.policy();
    unsafe { std::env::set_var("CARGO_AI_USAGE_TRACKING", "off") };
    assert!(enable(None).await.unwrap_err().contains("local history"));
    assert_eq!(selected().unwrap().is_none(), true);
}
#[tokio::test]
async fn v2_capture_drains_short_chunks_newest_first_and_ack_preserves_raw_history() {
    let mut server = Server::new_async().await;
    let home = Home::new(format!("{}/account", server.url()), true);
    home.policy();
    invoke(&["cloud", "status"]).await;
    enable_source(&mut server, 1).await;
    for n in 0..251 {
        append(n, (n as i64 % 7) * 3_600_000, false);
    }
    append(300, 0, true);
    crate::usage_store::persist(&json!({"event_id":uuid(999),"root_run_id":uuid(10),"event_type":"provider_request_completed","timestamp":"invalid","usage":{"input_tokens":1}})).unwrap();
    let upload = server
        .mock("POST", "/account")
        .match_body(matcher("upload"))
        .with_body_from_request(|request| {
            let body: Value = serde_json::from_slice(request.body().unwrap()).unwrap();
            if let Request::Upload { batch, .. } =
                serde_json::from_value::<Request>(body["usage_analytics"].clone()).unwrap()
            {
                let events = elastic::decode_batch(&batch).unwrap();
                assert_eq!(events.len(), 251);
                let hours = events
                    .iter()
                    .map(|e| {
                        serde_json::from_str::<Value>(&e.original_text).unwrap()["occurred_at_ms"]
                            .as_i64()
                            .unwrap()
                            / 3_600_000
                    })
                    .collect::<Vec<_>>();
                assert!(hours.windows(2).all(|w| w[0] >= w[1]));
            }
            response(ack(request.body().unwrap())).into_bytes()
        })
        .expect(1)
        .create_async()
        .await;
    let forbidden = server
        .mock("POST", "/account")
        .match_body(Matcher::PartialJson(
            json!({"usage_analytics":{"command":"receipt"}}),
        ))
        .expect(0)
        .create_async()
        .await;
    let result = invoke(&["cloud", "sync"]).await;
    assert_eq!(result["schema_version"], 2);
    assert_eq!(result["result"]["acknowledged_events"], 251);
    assert_eq!(result["local"]["uploaded"], 251);
    assert_eq!(result["local"]["origin_unverified"], 1);
    assert_eq!(result["local"]["time_ineligible"], 1);
    assert_eq!(result["local"]["unscanned"], 0);
    let db = open().unwrap();
    assert_eq!(queue::high(&db).unwrap(), 253);
    assert_eq!(
        db.query_row("SELECT COUNT(*) FROM usage_events", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        253
    );
    upload.assert_async().await;
    forbidden.assert_async().await;
}
#[tokio::test]
async fn unknown_outcome_retries_same_frozen_v2_request_and_never_receipt_polls() {
    let mut server = Server::new_async().await;
    let home = Home::new(format!("{}/account", server.url()), true);
    home.policy();
    enable_source(&mut server, 1).await;
    append(1, 0, false);
    let lost = server
        .mock("POST", "/account")
        .match_body(matcher("upload"))
        .with_status(500)
        .with_body("fixture-access private server details")
        .expect(1)
        .create_async()
        .await;
    let error = sync().await.unwrap_err();
    assert!(!error.contains("fixture-access"));
    lost.assert_async().await;
    lost.remove_async().await;
    let db = open().unwrap();
    let frozen: String = db
        .query_row(
            "SELECT request_json FROM usage_cloud_upload_batches",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(local_status().unwrap()["unknown_batches"], 1);
    append(2, 0, false);
    let exact = frozen.clone();
    let recovered = server
        .mock("POST", "/account")
        .match_body(matcher("upload"))
        .with_body_from_request(move |request| {
            let body: Value = serde_json::from_slice(request.body().unwrap()).unwrap();
            if body["usage_analytics"]["batch_id"]
                == serde_json::from_str::<Value>(&exact).unwrap()["batch_id"]
            {
                assert!(std::str::from_utf8(request.body().unwrap())
                    .unwrap()
                    .contains(&exact));
                assert_eq!(
                    open()
                        .unwrap()
                        .query_row(
                            "SELECT attempts FROM usage_cloud_upload_batches WHERE request_json=?1",
                            [&exact],
                            |row| row.get::<_, i64>(0)
                        )
                        .unwrap(),
                    2
                );
            }
            response(ack(request.body().unwrap())).into_bytes()
        })
        .expect(2)
        .create_async()
        .await;
    assert_eq!(sync().await.unwrap()["uploaded_events"], 2);
    assert_eq!(
        db.query_row(
            "SELECT COUNT(*) FROM usage_cloud_upload_batches",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    assert_eq!(queue::high(&db).unwrap(), 2);
    assert_eq!(local_status().unwrap()["unknown_batches"], 0);
    recovered.assert_async().await;
}
#[tokio::test]
async fn invalid_ack_does_not_advance_progress_or_delete_history() {
    let mut server = Server::new_async().await;
    let home = Home::new(format!("{}/account", server.url()), true);
    home.policy();
    enable_source(&mut server, 1).await;
    append(1, 0, false);
    let forged = server
        .mock("POST", "/account")
        .match_body(matcher("upload"))
        .with_body_from_request(|request| {
            let mut ack = ack(request.body().unwrap());
            ack["acknowledged_events"] = json!(2);
            response(ack).into_bytes()
        })
        .expect(1)
        .create_async()
        .await;
    assert!(sync().await.unwrap_err().contains("exact batch"));
    assert_eq!(local_status().unwrap()["uploaded"], 0);
    assert_eq!(local_status().unwrap()["pending"], 1);
    assert_eq!(queue::high(&open().unwrap()).unwrap(), 1);
    forged.assert_async().await;
}
#[tokio::test]
async fn ack_after_independent_expiry_reports_real_ack_without_resurrecting_history() {
    let mut server = Server::new_async().await;
    let home = Home::new(format!("{}/account", server.url()), true);
    home.policy();
    enable_source(&mut server, 1).await;
    append(1, 0, false);
    let acknowledged = server
        .mock("POST", "/account")
        .match_body(matcher("upload"))
        .with_body_from_request(|request| {
            let body = ack(request.body().unwrap());
            assert_eq!(
                crate::usage_store::retention::apply(now_ms() + 31 * 86_400_000).unwrap()
                    ["deleted_records"],
                1
            );
            response(body).into_bytes()
        })
        .expect(1)
        .create_async()
        .await;
    let result = sync().await.unwrap();
    assert_eq!(result["acknowledged_events"], 1);
    assert_eq!(result["uploaded_events"], 0);
    assert_eq!(result["acknowledged_after_expiry_events"], 1);
    assert_eq!(
        local_status().unwrap()["expired_without_acknowledgment_total"],
        1
    );
    assert_eq!(
        open()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM usage_events", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    acknowledged.assert_async().await;
}
#[tokio::test]
async fn clear_unknown_fences_sends_and_new_generation_requires_explicit_history_selection() {
    let mut server = Server::new_async().await;
    let home = Home::new(format!("{}/account", server.url()), true);
    home.policy();
    enable_source(&mut server, 1).await;
    append(1, 0, false);
    let source = selected().unwrap().unwrap().source;
    let confirm = (uuid(20), Sha256Hash::of(b"exact cloud preview"));
    let preview = server.mock("POST", "/account").match_body(matcher("clear_preview")).with_body(response(json!({"protocol_version":2,"source":source,"lifecycle_epoch":0,"preview_id":confirm.0,"preview_sha256":confirm.1}))).expect(1).create_async().await;
    assert_eq!(
        invoke(&["cloud", "clear"]).await["result"]["preview_id"],
        json!(confirm.0)
    );
    preview.assert_async().await;
    preview.remove_async().await;
    let unknown = server
        .mock("POST", "/account")
        .match_body(matcher("clear_confirm"))
        .with_status(500)
        .with_body("unknown")
        .expect(1)
        .create_async()
        .await;
    assert!(clear(Some(confirm.clone())).await.is_err());
    assert_eq!(local_status().unwrap()["clear_outcome_unknown"], true);
    assert_eq!(sync().await.unwrap()["enabled"], false);
    unknown.assert_async().await;
    unknown.remove_async().await;
    // Every status call reopens the persisted Home; no original preview or
    // in-memory transport is needed to recover the exact explicit retry.
    let recovered_status = invoke(&["cloud", "status"]).await;
    assert_eq!(
        recovered_status["result"]["pending_clear_confirmation"],
        json!({"source":source,"lifecycle_epoch":0,"preview_id":confirm.0,"preview_sha256":confirm.1})
    );
    let preserved = queue::pending_upload_clear(&open().unwrap(), &source)
        .unwrap()
        .unwrap();
    let cleared = server.mock("POST", "/account").match_body(matcher("clear_confirm")).with_body_from_request(move |request| {
        let body: Value = serde_json::from_slice(request.body().unwrap()).unwrap();
        assert_eq!(body["usage_analytics"], json!(preserved));
        response(json!({"protocol_version":2,"source":source,"cleared":true,"data_generation":2,"purge_state":"pending_processing"})).into_bytes()
    }).expect(1).create_async().await;
    invoke(&[
        "cloud",
        "clear",
        "--confirm",
        "--preview-id",
        confirm.0.as_str(),
        "--preview-hash",
        confirm.1.as_str(),
    ])
    .await;
    cleared.assert_async().await;
    assert_eq!(local_status().unwrap()["cleared"], true);
    assert_eq!(
        local_status().unwrap()["pending_clear_confirmation"],
        Value::Null
    );
    assert_eq!(queue::high(&open().unwrap()).unwrap(), 1);
    enable_source(&mut server, 2).await;
    assert_eq!(local_status().unwrap()["pending"], 0);
    assert_eq!(local_status().unwrap()["unscanned"], 0);
    let preview = history_selection(None, None).unwrap();
    history_selection(Some(1), preview["preview_sha256"].as_str()).unwrap();
    assert_eq!(local_status().unwrap()["unscanned"], 1);
}
#[tokio::test]
async fn token_refresh_rechecks_consent_before_replaying_upload() {
    let mut server = Server::new_async().await;
    let home = Home::new(format!("{}/account", server.url()), true);
    home.policy();
    enable_source(&mut server, 1).await;
    append(1, 0, false);
    let expired = server
        .mock("POST", "/account")
        .match_body(matcher("upload"))
        .with_body(json!({"status":"failed","type":"access_token_expired"}).to_string())
        .expect(1)
        .create_async()
        .await;
    let refresh = server
        .mock("POST", "/account")
        .match_body(Matcher::PartialJson(
            json!({"action":"status","session_policy":{"allow_refresh":true}}),
        ))
        .with_body_from_request(|_| {
            queue::disable(&mut open().unwrap(), &selected().unwrap().unwrap().source).unwrap();
            json!({"status":"success","session":{"access_token":"fixture-new-access"}})
                .to_string()
                .into_bytes()
        })
        .expect(1)
        .create_async()
        .await;
    assert!(sync().await.unwrap_err().contains("consent changed"));
    expired.assert_async().await;
    refresh.assert_async().await;
    assert_eq!(local_status().unwrap()["pending"], 1);
    assert_eq!(local_status().unwrap()["enabled"], false);
}

#[tokio::test]
async fn machine_v2_schema_covers_remote_policy_history_disable_and_local_retention() {
    let mut server = Server::new_async().await;
    let home = Home::new(format!("{}/account", server.url()), true);
    invoke(&["retention", "status"]).await;
    invoke(&["retention", "preview"]).await;
    invoke(&[
        "retention",
        "configure",
        "--days",
        "7",
        "--enabled",
        "on",
        "--acknowledge-local-detail-loss",
        "--acknowledge-unselected-loss",
    ])
    .await;
    let policy =
        json!({"retention_days":7,"retained_event_target":1_000_000,"max_active_environments":3});
    let remote = server.mock("POST", "/account").match_body(matcher("status")).with_body(response(json!({"protocol_version":2,"capability":{"enabled":true,"supported_protocol_versions":[2]},"account_binding":null,"data_generation":null,"lifecycle_epoch":0,"environments":null,"policy":policy,"processing_status":"not_observed"}))).expect(1).create_async().await;
    invoke(&["cloud", "status", "--remote"]).await;
    remote.assert_async().await;
    remote.remove_async().await;
    enable_source(&mut server, 1).await;
    let empty_sync = invoke(&["cloud", "sync"]).await;
    assert_eq!(empty_sync["effects"]["remote"], "read_only");
    assert_eq!(empty_sync["result"]["policy_refreshed"], true);
    let event_id = append(1, 0, false);
    let preview = invoke(&["cloud", "include-history"]).await;
    invoke(&[
        "cloud",
        "include-history",
        "--confirm",
        "--through",
        "1",
        "--preview-hash",
        preview["result"]["preview_sha256"].as_str().unwrap(),
    ])
    .await;
    let source = selected().unwrap().unwrap().source;
    let db = open().unwrap();
    let raw: String = db
        .query_row("SELECT record_json FROM usage_event_records", [], |r| {
            r.get(0)
        })
        .unwrap();
    let event = EventSubmission::from_allowed_text(
        project(&db, &serde_json::from_str(&raw).unwrap()).unwrap(),
    )
    .unwrap();
    let mut historical = json!(event);
    historical["sequence"] = json!(1);
    historical["event_id"] = json!(event_id);
    let history_source = source.clone();
    let history = server.mock("POST", "/account").match_body(matcher("history")).with_body(response(json!({"protocol_version":2,"source":history_source,"high_watermark":1,"events":[historical]}))).expect(1).create_async().await;
    invoke(&["cloud", "history"]).await;
    history.assert_async().await;
    history.remove_async().await;
    let disabled = server
        .mock("POST", "/account")
        .match_body(matcher("disable"))
        .with_body(response(
            json!({"protocol_version":2,"source":source,"enabled":false,"history_deleted":false}),
        ))
        .expect(1)
        .create_async()
        .await;
    invoke(&["cloud", "disable"]).await;
    disabled.assert_async().await;
    invoke(&["retention", "status"]).await;
    invoke(&["retention", "preview"]).await;
    invoke(&["retention", "configure", "--days", "7", "--enabled", "off"]).await;
    assert!(home.path.is_dir());
}
#[tokio::test]
async fn old_experimental_selection_is_not_v2_cloud_consent_and_status_is_read_only() {
    let mut server = Server::new_async().await;
    let home = Home::new(format!("{}/account", server.url()), true);
    home.policy();
    enable_source(&mut server, 1).await;
    let db = open().unwrap();
    db.execute(
        "UPDATE usage_meta SET key='usage_cloud_selected_v1' WHERE key='usage_cloud_selected_v2'",
        [],
    )
    .unwrap();
    let before = db.total_changes();
    assert_eq!(
        invoke(&["cloud", "status"]).await["result"]["enabled"],
        false
    );
    assert_eq!(invoke(&["cloud", "sync"]).await["result"]["enabled"], false);
    assert_eq!(db.total_changes(), before);
    assert_eq!(
        db.query_row(
            "SELECT COUNT(*) FROM usage_meta WHERE key='usage_cloud_selected_v2'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
}
#[tokio::test]
async fn replacement_home_is_explicit_and_server_source_is_authoritative() {
    let mut server = Server::new_async().await;
    let home = Home::new(format!("{}/account", server.url()), true);
    home.policy();
    let status = server.mock("POST", "/account").match_body(matcher("status")).with_body(response(json!({"protocol_version":2,"capability":{"enabled":true,"supported_protocol_versions":[2]},"account_binding":uuid(1),"data_generation":1,"lifecycle_epoch":0}))).expect(1).create_async().await;
    let replace = server.mock("POST", "/account").match_body(Matcher::PartialJson(json!({"usage_analytics":{"replace_home_id":uuid(44)}}))).with_body_from_request(|request| {
        let body: Value = serde_json::from_slice(request.body().unwrap()).unwrap();
        response(json!({"protocol_version":2,"source":{"account_binding":uuid(1),"source_home_id":body["usage_analytics"]["source_home_id"],"data_generation":1},"provenance":{"lifecycle_epoch":0,"consent_interval":2},"enabled":true,"policy":{"retention_days":7}})).into_bytes()
    }).expect(1).create_async().await;
    invoke(&[
        "cloud",
        "enable",
        "--replace-environment",
        uuid(44).as_str(),
    ])
    .await;
    status.assert_async().await;
    replace.assert_async().await;
}
#[tokio::test]
async fn partial_sync_keeps_confirmed_progress_and_reports_machine_partial_outcome() {
    let mut server = Server::new_async().await;
    let home = Home::new(format!("{}/account", server.url()), true);
    home.policy();
    enable_source(&mut server, 1).await;
    append(1, 0, false);
    let initial = server
        .mock("POST", "/account")
        .match_body(matcher("upload"))
        .with_status(500)
        .with_body("unknown")
        .expect(1)
        .create_async()
        .await;
    assert!(sync().await.is_err());
    initial.assert_async().await;
    initial.remove_async().await;
    let frozen: String = open()
        .unwrap()
        .query_row(
            "SELECT request_json FROM usage_cloud_upload_batches",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let id = serde_json::from_str::<Value>(&frozen).unwrap()["batch_id"].clone();
    append(2, 0, false);
    let second_id = id.clone();
    let failed = server
        .mock("POST", "/account")
        .match_body(matcher("upload"))
        .match_request(move |request| {
            serde_json::from_slice::<Value>(request.body().unwrap()).unwrap()["usage_analytics"]
                ["batch_id"]
                != second_id
        })
        .with_status(500)
        .with_body("unknown second batch")
        .expect(1)
        .create_async()
        .await;
    let acknowledged = server
        .mock("POST", "/account")
        .match_body(Matcher::PartialJson(
            json!({"usage_analytics":{"batch_id":id}}),
        ))
        .with_body_from_request(|request| response(ack(request.body().unwrap())).into_bytes())
        .expect(1)
        .create_async()
        .await;
    let args = clap::Command::new("test")
        .subcommand(crate::usage_analytics_cli::cloud_command())
        .try_get_matches_from(["test", "cloud", "sync"])
        .unwrap();
    let failure = crate::usage_analytics_cli::machine_run(&args)
        .await
        .unwrap_err();
    assert_eq!(failure.code, "operation.partial");
    assert_eq!(failure.data["result"]["acknowledged_events"], 1);
    assert_eq!(failure.data["local"]["uploaded"], 1);
    assert_eq!(failure.data["local"]["pending"], 1);
    assert_eq!(failure.data["postconditions_verified"], false);
    crate::usage_analytics_cli::schema_tests::assert_valid(&failure.data);
    acknowledged.assert_async().await;
    failed.assert_async().await;
}

fn select_local_thirty_days() {
    crate::usage_store::retention::configure(crate::usage_store::retention::Policy {
        schema_version: 2,
        days: 30,
        enabled: true,
        local_detail_loss_acknowledged: true,
        unselected_loss_acknowledged: true,
    })
    .unwrap();
}
#[tokio::test]
async fn independent_cloud_window_excludes_old_local_history_and_refreshes_policy_overrides() {
    let mut server = Server::new_async().await;
    let home = Home::new(format!("{}/account", server.url()), true);
    select_local_thirty_days();
    enable_source(&mut server, 1).await;
    let old = append(1, 8 * 86_400_000, false);
    let recent = append(2, 0, false);
    let recent_copy = recent.clone();
    let upload = server
        .mock("POST", "/account")
        .match_body(matcher("upload"))
        .with_body_from_request(move |request| {
            let body: Value = serde_json::from_slice(request.body().unwrap()).unwrap();
            let Request::Upload { batch, .. } =
                serde_json::from_value::<Request>(body["usage_analytics"].clone()).unwrap()
            else {
                unreachable!()
            };
            let events = elastic::decode_batch(&batch).unwrap();
            assert_eq!(events.len(), 1);
            assert_eq!(events[0].member().unwrap().event_id, recent_copy);
            response(ack(request.body().unwrap())).into_bytes()
        })
        .expect(1)
        .create_async()
        .await;
    assert_eq!(sync().await.unwrap()["uploaded_events"], 1);
    upload.assert_async().await;
    upload.remove_async().await;
    assert_eq!(local_status().unwrap()["pending"], 1);
    assert_eq!(local_status().unwrap()["cloud_window_ineligible"], 1);
    assert_eq!(local_status().unwrap()["cloud_retention_days"], 7);
    let override_status = server
        .mock("POST", "/account")
        .match_body(matcher("status"))
        .with_body_from_request(|_| response(current_status(30)).into_bytes())
        .expect(1)
        .create_async()
        .await;
    let older_upload = server
        .mock("POST", "/account")
        .match_body(matcher("upload"))
        .with_body_from_request(move |request| {
            let body: Value = serde_json::from_slice(request.body().unwrap()).unwrap();
            let Request::Upload { batch, .. } =
                serde_json::from_value::<Request>(body["usage_analytics"].clone()).unwrap()
            else {
                unreachable!()
            };
            assert_eq!(
                elastic::decode_batch(&batch).unwrap()[0]
                    .member()
                    .unwrap()
                    .event_id,
                old
            );
            response(ack(request.body().unwrap())).into_bytes()
        })
        .expect(1)
        .create_async()
        .await;
    assert_eq!(sync().await.unwrap()["uploaded_events"], 1);
    assert_eq!(local_status().unwrap()["cloud_retention_days"], 30);
    assert_eq!(local_status().unwrap()["cloud_window_ineligible"], 0);
    assert_eq!(
        open()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM usage_events", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        2
    );
    override_status.assert_async().await;
    older_upload.assert_async().await;
    assert!(home.path.is_dir());
}
#[tokio::test]
async fn frozen_unknown_batch_crossing_cloud_cutoff_is_retired_without_losing_raw_or_newer_events()
{
    let mut server = Server::new_async().await;
    let _home = Home::new(format!("{}/account", server.url()), true);
    select_local_thirty_days();
    enable_source(&mut server, 1).await;
    append(1, 8 * 86_400_000, false);
    append(2, 0, false);
    let older_policy = server
        .mock("POST", "/account")
        .match_body(matcher("status"))
        .with_body_from_request(|_| response(current_status(30)).into_bytes())
        .expect(1)
        .create_async()
        .await;
    let unknown = server
        .mock("POST", "/account")
        .match_body(matcher("upload"))
        .with_status(500)
        .with_body("unknown outcome")
        .expect(1)
        .create_async()
        .await;
    assert!(sync().await.is_err());
    older_policy.assert_async().await;
    older_policy.remove_async().await;
    unknown.assert_async().await;
    unknown.remove_async().await;
    let old_id: String = open()
        .unwrap()
        .query_row("SELECT batch_id FROM usage_cloud_upload_batches", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(local_status().unwrap()["pending"], 2);
    let old_id_copy = old_id.clone();
    let upload = server
        .mock("POST", "/account")
        .match_body(matcher("upload"))
        .with_body_from_request(move |request| {
            let body: Value = serde_json::from_slice(request.body().unwrap()).unwrap();
            assert_ne!(body["usage_analytics"]["batch_id"], old_id_copy);
            assert_eq!(body["usage_analytics"]["batch"]["event_count"], 1);
            response(ack(request.body().unwrap())).into_bytes()
        })
        .expect(1)
        .create_async()
        .await;
    assert_eq!(sync().await.unwrap()["uploaded_events"], 1);
    assert_eq!(local_status().unwrap()["pending"], 1);
    assert_eq!(local_status().unwrap()["cloud_window_ineligible"], 1);
    let db = open().unwrap();
    assert_eq!(
        db.query_row(
            "SELECT COUNT(*) FROM usage_cloud_upload_batches WHERE batch_id=?1",
            [&old_id],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    assert_eq!(
        db.query_row("SELECT COUNT(*) FROM usage_events", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        2
    );
    upload.assert_async().await;
}
#[test]
fn cloud_policy_is_server_derived_bounded_and_never_silently_defaults() {
    for body in [
        json!({}),
        json!({"policy":{"retention_days":null}}),
        json!({"policy":{"retention_days":0}}),
        json!({"policy":{"retention_days":3651}}),
        json!({"policy":{"retention_days":"7"}}),
    ] {
        assert!(cloud_days(&body).is_err());
    }
    assert_eq!(
        cloud_days(&json!({"policy":{"retention_days":7}})).unwrap(),
        7
    );
}

#[tokio::test]
async fn refreshed_environment_authority_stops_disabled_or_replaced_home_before_upload() {
    let mut server = Server::new_async().await;
    let home = Home::new(format!("{}/account", server.url()), true);
    home.policy();
    enable_source(&mut server, 1).await;
    append(1, 0, false);
    let replaced = server
        .mock("POST", "/account")
        .match_body(matcher("status"))
        .with_body_from_request(|_| {
            let mut status = current_status(7);
            status["environments"] = json!([]);
            response(status).into_bytes()
        })
        .expect(1)
        .create_async()
        .await;
    let forbidden = server
        .mock("POST", "/account")
        .match_body(matcher("upload"))
        .expect(0)
        .create_async()
        .await;
    assert!(sync()
        .await
        .unwrap_err()
        .contains("no longer upload-enabled"));
    assert_eq!(local_status().unwrap()["enabled"], false);
    assert_eq!(
        open()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM usage_events", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
    replaced.assert_async().await;
    forbidden.assert_async().await;
}
