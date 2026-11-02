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
fn local_event(index: u64, offset_ms: i64, restored: bool) -> Value {
    let event_id = uuid(index + 100).as_str().to_owned();
    let timestamp = time::OffsetDateTime::from_unix_timestamp_nanos(
        i128::from(now_ms() - offset_ms) * 1_000_000,
    )
    .unwrap()
    .format(&time::format_description::well_known::Rfc3339)
    .unwrap();
    json!({"schema_version":1,"event_id":event_id,"root_run_id":uuid(10),"agent_run_id":uuid(11),"event_type":"provider_request_completed","timestamp":timestamp,"provider":{"server":"typesafe","profile":"private fixture customer","requested_model":"jev-1.13.0"},"agent":{"project_root":"/private/fixture/project","name":"private fixture agent"},"usage":{"input_tokens":100,"output_tokens":3},"status":"success","restored":restored})
}
fn append(index: u64, offset_ms: i64, restored: bool) -> String {
    let event = local_event(index, offset_ms, restored);
    crate::usage_store::persist(&event).unwrap();
    event["event_id"].as_str().unwrap().to_owned()
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
async fn denied_history_never_opens_state_or_calls_network() {
    let home = Home::new("http://127.0.0.1:1/account".into(), false);
    let result = sync_with_history(Some(31)).await.unwrap();
    assert_eq!(result["outcome"], "plan_limit_denied");
    assert!(!home.path.exists());
    let value = invoke(&["cloud", "sync", "--include-history", "31d"]).await;
    assert_eq!(value["result"]["outcome"], "plan_limit_denied");
    assert!(!home.path.exists());
}
#[tokio::test]
async fn enable_never_uploads_and_oldest_first_sync_preserves_local_history() {
    let mut server = Server::new_async().await;
    let home = Home::new(format!("{}/account", server.url()), true);
    home.policy();
    append(1, 1000, false);
    enable_source(&mut server, 1).await;
    let newer = append(2, 1000, false);
    let older = append(3, 2000, false);
    let expected = vec![older, newer];
    let upload = server
        .mock("POST", "/account")
        .match_body(matcher("upload"))
        .with_body_from_request(move |request| {
            let body: Value = serde_json::from_slice(request.body().unwrap()).unwrap();
            let Request::Upload { batch, .. } =
                serde_json::from_value::<Request>(body["usage_analytics"].clone()).unwrap()
            else {
                panic!()
            };
            let ids = elastic::decode_batch(&batch)
                .unwrap()
                .into_iter()
                .map(|event| event.member().unwrap().event_id)
                .collect::<Vec<_>>();
            assert_eq!(ids, expected);
            response(ack(request.body().unwrap())).into_bytes()
        })
        .expect(1)
        .create_async()
        .await;
    let result = invoke(&["cloud", "sync"]).await;
    assert_eq!(result["result"]["outcome"], "fully_synced");
    assert_eq!(result["result"]["job"]["target_events"], 2);
    assert_eq!(queue::high(&open().unwrap()).unwrap(), 3);
    assert_eq!(local_status().unwrap()["uploaded"], 2);
    upload.assert_async().await;
}
#[tokio::test]
async fn unknown_response_preserves_target_and_exact_retry_then_defers_new_arrivals() {
    let mut server = Server::new_async().await;
    let home = Home::new(format!("{}/account", server.url()), true);
    home.policy();
    enable_source(&mut server, 1).await;
    append(1, 1000, false);
    let bytes = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let recorded = bytes.clone();
    let unknown = server
        .mock("POST", "/account")
        .match_body(matcher("upload"))
        .with_body_from_request(move |request| {
            *recorded.lock().unwrap() = request.body().unwrap().to_vec();
            response(
                json!({"protocol_version":2,"outcome":"expired","expired_event_ids":[uuid(999)]}),
            )
            .into_bytes()
        })
        .expect(1)
        .create_async()
        .await;
    let first = sync().await.unwrap();
    assert_eq!(first["outcome"], "outcome_unknown");
    unknown.assert_async().await;
    unknown.remove_async().await;
    append(2, 0, false);
    let exact = bytes.clone();
    let retry = server
        .mock("POST", "/account")
        .match_body(matcher("upload"))
        .with_body_from_request(move |request| {
            assert_eq!(request.body().unwrap(), exact.lock().unwrap().as_slice());
            response(ack(request.body().unwrap())).into_bytes()
        })
        .expect(1)
        .create_async()
        .await;
    let second = invoke(&["cloud", "sync"]).await;
    assert_eq!(second["result"]["outcome"], "fully_synced");
    assert_eq!(second["result"]["job"]["target_events"], 1);
    assert_eq!(second["result"]["job"]["job_id"], first["job"]["job_id"]);
    retry.assert_async().await;
    retry.remove_async().await;
    let next = server
        .mock("POST", "/account")
        .match_body(matcher("upload"))
        .with_body_from_request(|request| response(ack(request.body().unwrap())).into_bytes())
        .expect(1)
        .create_async()
        .await;
    let third = invoke(&["cloud", "sync"]).await;
    assert_eq!(third["result"]["job"]["target_events"], 1);
    assert_ne!(third["result"]["job"]["job_id"], first["job"]["job_id"]);
    next.assert_async().await;
}
#[tokio::test]
async fn quota_pause_is_persisted_without_ack_and_runner_resumes_only_existing_jobs() {
    let mut server = Server::new_async().await;
    let home = Home::new(format!("{}/account", server.url()), true);
    home.policy();
    enable_source(&mut server, 1).await;
    append(1, 1000, false);
    resume_active().await;
    assert!(
        queue::job::active(&open().unwrap(), &selected().unwrap().unwrap().source)
            .unwrap()
            .is_none()
    );
    let tomorrow = (now_ms() / 86_400_000 + 1) * 86_400_000;
    let paused=server.mock("POST","/account").match_body(matcher("upload")).with_body(response(json!({"protocol_version":2,"outcome":"quota_paused","retry_at_ms":tomorrow,"reset_at_ms":tomorrow,"daily_bytes_limit":1_000_000_000_u64,"daily_request_limit":1000}))).expect(1).create_async().await;
    let result = sync_with_history(Some(30)).await.unwrap();
    assert_eq!(result["outcome"], "quota_paused");
    assert_eq!(result["job"]["uploaded_events"], 0);
    paused.assert_async().await;
    paused.remove_async().await;
    append(2, 0, false);
    let none = server
        .mock("POST", "/account")
        .expect(0)
        .create_async()
        .await;
    let again = sync().await.unwrap();
    assert_eq!(again["outcome"], "quota_paused");
    assert_eq!(again["job"]["target_events"], 1);
    resume_active().await;
    none.assert_async().await;
    let db = open().unwrap();
    let active = queue::job::active(&db, &selected().unwrap().unwrap().source)
        .unwrap()
        .unwrap();
    assert_eq!(active.retry_at_ms, Some(tomorrow));
    assert_eq!(queue::high(&db).unwrap(), 2);
}
#[tokio::test]
async fn server_expiry_only_removes_listed_member_and_remaining_member_is_uploaded() {
    let mut server = Server::new_async().await;
    let home = Home::new(format!("{}/account", server.url()), true);
    home.policy();
    enable_source(&mut server, 1).await;
    let expired = append(1, 2000, false);
    append(2, 1000, false);
    let expiry = server
        .mock("POST", "/account")
        .match_body(matcher("upload"))
        .with_body(response(
            json!({"protocol_version":2,"outcome":"expired","expired_event_ids":[expired]}),
        ))
        .expect(1)
        .create_async()
        .await;
    // Stop after the first response so that the next explicit pass exercises persistence.
    let lease_source = selected().unwrap().unwrap().source;
    let mut db = open().unwrap();
    let lease = queue::acquire_lease(&mut db, &lease_source, now_ms(), queue::MAX_LEASE_MS)
        .unwrap()
        .unwrap();
    let job = queue::job::start(&mut db, &lease, now_ms(), None).unwrap();
    queue::job::capture(&mut db, &lease, &job, now_ms()).unwrap();
    let batch = queue::prepare_job_upload(&mut db, &lease, now_ms(), &job.id, build_upload)
        .unwrap()
        .unwrap();
    let body = transport::V2Transport::new(REQUEST_TIMEOUT)
        .unwrap()
        .call_exact(batch.request_json())
        .await
        .unwrap();
    let ids: Vec<String> = serde_json::from_value(body["expired_event_ids"].clone()).unwrap();
    queue::job::expire_batch_members(&mut db, &lease, &job, &batch, &ids, now_ms()).unwrap();
    queue::release_lease(&mut db, &lease, now_ms()).unwrap();
    drop(db);
    expiry.assert_async().await;
    expiry.remove_async().await;
    let upload = server
        .mock("POST", "/account")
        .match_body(matcher("upload"))
        .with_body_from_request(|request| response(ack(request.body().unwrap())).into_bytes())
        .expect(1)
        .create_async()
        .await;
    let result = sync().await.unwrap();
    assert_eq!(result["outcome"], "partial");
    assert_eq!(result["job"]["expired_events"], 1);
    assert_eq!(result["job"]["uploaded_events"], 1);
    assert_eq!(queue::high(&open().unwrap()).unwrap(), 2);
    upload.assert_async().await;
}

#[tokio::test]
async fn explicit_remote_status_reports_freshness_without_sync_using_that_endpoint() {
    let mut server = Server::new_async().await;
    let home = Home::new(format!("{}/account", server.url()), true);
    home.policy();
    enable_source(&mut server, 1).await;
    let freshness=server.mock("POST","/account").match_body(matcher("freshness")).with_body(response(json!({"protocol_version":2,"refresh_status":"overdue","last_refresh_ms":now_ms()-90_000_000,"target_age_ms":86_400_000}))).expect(1).create_async().await;
    let status = invoke(&["cloud", "status", "--remote"]).await;
    assert_eq!(
        status["result"]["remote"]["freshness"]["refresh_status"],
        "overdue"
    );
    freshness.assert_async().await;
    freshness.remove_async().await;
    let forbidden = server
        .mock("POST", "/account")
        .match_body(matcher("freshness"))
        .expect(0)
        .create_async()
        .await;
    assert_eq!(
        invoke(&["cloud", "sync"]).await["result"]["outcome"],
        "fully_synced"
    );
    forbidden.assert_async().await;
}

#[tokio::test]
async fn clear_keeps_exact_confirmation_and_local_history_after_lost_response() {
    let mut server = Server::new_async().await;
    let home = Home::new(format!("{}/account", server.url()), true);
    home.policy();
    enable_source(&mut server, 1).await;
    append(1, 1000, false);
    let selection = selected().unwrap().unwrap();
    let preview_id = uuid(99);
    let hash = Sha256Hash::try_from("a".repeat(64)).unwrap();
    let request = Request::ClearConfirm {
        protocol_version: Version,
        source: selection.source.clone(),
        lifecycle_epoch: selection.provenance.lifecycle_epoch,
        preview_id: preview_id.clone(),
        preview_sha256: hash.clone(),
    };
    let unknown = server
        .mock("POST", "/account")
        .match_body(matcher("clear_confirm"))
        .with_body("{}")
        .expect(1)
        .create_async()
        .await;
    assert!(clear(Some((preview_id.clone(), hash.clone())))
        .await
        .is_err());
    unknown.assert_async().await;
    unknown.remove_async().await;
    assert_eq!(
        queue::pending_upload_clear(&open().unwrap(), &selection.source).unwrap(),
        Some(request)
    );
    assert_eq!(local_status().unwrap()["enabled"], false);
    let confirmed=server.mock("POST","/account").match_body(matcher("clear_confirm")).with_body(response(json!({"protocol_version":2,"source":selection.source,"cleared":true,"data_generation":2,"purge_state":"pending_processing"}))).expect(1).create_async().await;
    clear(Some((preview_id, hash))).await.unwrap();
    confirmed.assert_async().await;
    assert_eq!(queue::high(&open().unwrap()).unwrap(), 1);
}

#[tokio::test]
async fn renewed_consent_requires_explicit_sync_and_preserves_ambiguous_old_request() {
    let mut server = Server::new_async().await;
    let home = Home::new(format!("{}/account", server.url()), true);
    home.policy();
    enable_source(&mut server, 1).await;
    append(1, 1000, false);
    let unknown = server
        .mock("POST", "/account")
        .match_body(matcher("upload"))
        .with_body("{}")
        .expect(1)
        .create_async()
        .await;
    let result = sync().await.unwrap();
    assert_eq!(result["outcome"], "outcome_unknown");
    unknown.assert_async().await;
    unknown.remove_async().await;
    let mut db = open().unwrap();
    let old: String = db
        .query_row(
            "SELECT request_json FROM usage_cloud_upload_batches",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let mut selection = selected().unwrap().unwrap();
    queue::disable(&mut db, &selection.source).unwrap();
    append(2, 0, false);
    selection.provenance.consent_interval = Counter::try_from(2).unwrap();
    queue::enable(&mut db, &selection.source, &selection.provenance, now_ms()).unwrap();
    db.execute(
        "UPDATE usage_meta SET value=?2 WHERE key=?1",
        params![SELECTED, serde_json::to_string(&selection).unwrap()],
    )
    .unwrap();
    drop(db);
    let forbidden = server
        .mock("POST", "/account")
        .expect(0)
        .create_async()
        .await;
    resume_active().await;
    forbidden.assert_async().await;
    forbidden.remove_async().await;
    let stored: String = open()
        .unwrap()
        .query_row(
            "SELECT request_json FROM usage_cloud_upload_batches",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(stored, old);
    let upload = server
        .mock("POST", "/account")
        .match_body(matcher("upload"))
        .with_body_from_request(|request| {
            let body: Value = serde_json::from_slice(request.body().unwrap()).unwrap();
            assert_eq!(body["usage_analytics"]["provenance"]["consent_interval"], 2);
            response(ack(request.body().unwrap())).into_bytes()
        })
        .expect(1)
        .create_async()
        .await;
    let resumed = sync().await.unwrap();
    assert_eq!(resumed["outcome"], "fully_synced");
    assert_eq!(resumed["job"]["job_id"], result["job"]["job_id"]);
    assert_eq!(resumed["job"]["target_events"], 1);
    upload.assert_async().await;
    let stored: String = open()
        .unwrap()
        .query_row(
            "SELECT request_json FROM usage_cloud_upload_batches",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(stored, old);
    assert_eq!(local_status().unwrap()["unknown_batches"], 0);
}

#[tokio::test]
async fn cli_operation_scope_resumes_an_explicit_job_without_selecting_new_events() {
    let mut server = Server::new_async().await;
    let home = Home::new(format!("{}/account", server.url()), true);
    home.policy();
    enable_source(&mut server, 1).await;
    append(1, 1000, false);
    let unknown = server
        .mock("POST", "/account")
        .match_body(matcher("upload"))
        .with_body("{}")
        .expect(1)
        .create_async()
        .await;
    assert_eq!(sync().await.unwrap()["outcome"], "outcome_unknown");
    unknown.assert_async().await;
    unknown.remove_async().await;
    let upload = server
        .mock("POST", "/account")
        .match_body(matcher("upload"))
        .with_body_from_request(|request| response(ack(request.body().unwrap())).into_bytes())
        .expect(1)
        .create_async()
        .await;
    append(2, 0, false);
    scheduler::scope(tokio::time::sleep(Duration::from_millis(250))).await;
    upload.assert_async().await;
    let db = open().unwrap();
    assert!(
        queue::job::active(&db, &selected().unwrap().unwrap().source)
            .unwrap()
            .is_none()
    );
    assert_eq!(local_status().unwrap()["uploaded"], 1);
    assert_eq!(queue::high(&db).unwrap(), 2);
}

#[tokio::test]
async fn review_exclusions_survive_restart_and_a_later_clock() {
    let mut server = Server::new_async().await;
    let home = Home::new(format!("{}/account", server.url()), true);
    home.policy();
    enable_source(&mut server, 1).await;
    append(1, 1000, true);
    let malformed = uuid(900).as_str().to_owned();
    crate::usage_store::persist(&json!({"schema_version":1,"event_id":malformed,"root_run_id":uuid(10),"event_type":"provider_request_completed","timestamp":"now","usage":{"input_tokens":1}})).unwrap();
    let formerly_future = append(2, -3_600_000, false);
    assert_eq!(
        open()
            .unwrap()
            .query_row(
                "SELECT cloud_eligible FROM usage_retention_hours WHERE event_id=?1",
                [&formerly_future],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
    let forbidden = server
        .mock("POST", "/account")
        .match_body(matcher("upload"))
        .expect(0)
        .create_async()
        .await;
    let result = sync().await.unwrap();
    assert_eq!(result["outcome"], "partial");
    assert_eq!(result["job"]["excluded_events"], 3);
    assert_eq!(result["job"]["pending_events"], 0);
    let status = local_status().unwrap();
    assert_eq!(status["origin_unverified"], 1);
    assert_eq!(status["time_ineligible"], 2);
    assert_eq!(status["sync_job"]["outcome"], "partial");
    assert_eq!(status["sync_job"]["finished"], true);
    // A later process clock cannot reverse the persisted capture-time decision.
    let mut reopened = open().unwrap();
    let source = selected().unwrap().unwrap().source;
    let later = now_ms() + 2 * 3_600_000;
    let lease = queue::acquire_lease(&mut reopened, &source, later, queue::MAX_LEASE_MS)
        .unwrap()
        .unwrap();
    let next = queue::job::start(&mut reopened, &lease, later, None).unwrap();
    queue::job::capture(&mut reopened, &lease, &next, later).unwrap();
    assert_eq!(
        queue::job::progress(&reopened, &next).unwrap()["excluded_events"],
        3
    );
    assert!(
        queue::prepare_job_upload(&mut reopened, &lease, later, &next.id, build_upload)
            .unwrap()
            .is_none()
    );
    forbidden.assert_async().await;
}

#[tokio::test]
async fn review_automatic_terminal_expiry_remains_visible_and_cannot_be_reselected() {
    let mut server = Server::new_async().await;
    let home = Home::new(format!("{}/account", server.url()), true);
    home.policy();
    enable_source(&mut server, 1).await;
    let expired = append(1, 1000, false);
    let unknown = server
        .mock("POST", "/account")
        .match_body(matcher("upload"))
        .with_body("{}")
        .expect(1)
        .create_async()
        .await;
    let initial = sync().await.unwrap();
    assert_eq!(initial["outcome"], "outcome_unknown");
    unknown.assert_async().await;
    unknown.remove_async().await;
    let expiry = server
        .mock("POST", "/account")
        .match_body(matcher("upload"))
        .with_body(response(
            json!({"protocol_version":2,"outcome":"expired","expired_event_ids":[expired]}),
        ))
        .expect(1)
        .create_async()
        .await;
    scheduler::scope(tokio::time::sleep(Duration::from_millis(250))).await;
    expiry.assert_async().await;
    expiry.remove_async().await;
    let status = local_status().unwrap();
    assert_eq!(status["sync_job"]["job_id"], initial["job"]["job_id"]);
    assert_eq!(status["sync_job"]["finished"], true);
    assert_eq!(status["sync_job"]["outcome"], "partial");
    assert_eq!(status["sync_job"]["expired_events"], 1);
    assert_eq!(status["server_expired_events"], 1);
    assert_eq!(status["pending"], 0);
    assert_eq!(queue::high(&open().unwrap()).unwrap(), 1);
    let forbidden = server
        .mock("POST", "/account")
        .match_body(matcher("upload"))
        .expect(0)
        .create_async()
        .await;
    let next = invoke(&["cloud", "sync"]).await;
    assert_eq!(next["result"]["job"]["target_events"], 0);
    assert_eq!(next["local"]["server_expired_events"], 1);
    forbidden.assert_async().await;
    assert_eq!(
        open()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM usage_event_records", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
    // Clearing a generation does not reset the server's lifetime event identity.
    enable_source(&mut server, 2).await;
    let replay = invoke(&["cloud", "sync", "--include-history", "30d"]).await;
    assert_eq!(replay["result"]["job"]["target_events"], 0);
    assert_eq!(replay["local"]["server_expired_events"], 1);
    forbidden.assert_async().await;
}

#[tokio::test]
async fn review_machine_effects_preserve_ack_before_a_later_capture_fault() {
    let mut server = Server::new_async().await;
    let home = Home::new(format!("{}/account", server.url()), true);
    home.policy();
    enable_source(&mut server, 1).await;
    assert_eq!(
        crate::usage_store::retention::apply(now_ms()).unwrap()["maintenance_pending"],
        false
    );
    let expected = {
        let mut db = open().unwrap();
        let tx = db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .unwrap();
        let mut expected = std::collections::BTreeMap::new();
        for n in 0..4201 {
            let event = local_event(n, 10_000 - n as i64, false);
            assert!(crate::usage_store::insert_event(&tx, &event).unwrap());
            expected.insert(event["event_id"].as_str().unwrap().to_owned(), event);
        }
        tx.commit().unwrap();
        expected
    };
    let check_raw_history = || {
        let db = open().unwrap();
        let mut statement = db
            .prepare("SELECT event_id,record_json FROM usage_event_records")
            .unwrap();
        let records = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(records.len(), expected.len());
        for (id, text) in records {
            assert_eq!(
                serde_json::from_str::<Value>(&text).unwrap(),
                expected[&id],
                "Local raw event changed: {id}"
            );
        }
    };
    check_raw_history();
    let count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let observed = count.clone();
    let upload=server.mock("POST","/account").match_body(matcher("upload")).with_body_from_request(move|request|{
        let body:Value=serde_json::from_slice(request.body().unwrap()).unwrap();
        observed.store(body["usage_analytics"]["batch"]["event_count"].as_u64().unwrap() as usize,std::sync::atomic::Ordering::SeqCst);
        open().unwrap().execute_batch("CREATE TRIGGER fixture_capture_fault BEFORE INSERT ON usage_cloud_queue BEGIN SELECT RAISE(ABORT,'fixture capture failure'); END;").unwrap();
        response(ack(request.body().unwrap())).into_bytes()
    }).expect(1).create_async().await;
    let args = clap::Command::new("test")
        .subcommand(crate::usage_analytics_cli::cloud_command())
        .try_get_matches_from(["test", "cloud", "sync"])
        .unwrap();
    let failure = crate::usage_analytics_cli::machine_run(&args)
        .await
        .unwrap_err();
    let value = failure.data;
    crate::usage_analytics_cli::schema_tests::assert_valid(&value);
    let count = count.load(std::sync::atomic::Ordering::SeqCst);
    assert!((101..=4096).contains(&count));
    assert_eq!(value["result"]["outcome"], "partial");
    assert_eq!(value["result"]["batches"], 1);
    assert_eq!(value["result"]["acknowledged_events"], count);
    assert_eq!(value["result"]["uploaded_events"], count);
    assert_eq!(value["result"]["job"]["pending_events"], 4201 - count);
    assert_ne!(value["effects"]["remote"], "unapplied");
    assert_eq!(value["local"]["uploaded"], count);
    check_raw_history();
    upload.assert_async().await;
}

async fn invoke_stopped_sync() -> Value {
    let args = clap::Command::new("test")
        .subcommand(crate::usage_analytics_cli::cloud_command())
        .try_get_matches_from(["test", "cloud", "sync"])
        .unwrap();
    let value = crate::usage_analytics_cli::machine_run(&args)
        .await
        .unwrap_err()
        .data;
    crate::usage_analytics_cli::schema_tests::assert_valid(&value);
    value
}

#[tokio::test]
async fn transient_upload_budget_and_cooldown_survive_another_invocation() {
    let mut server = Server::new_async().await;
    let home = Home::new(format!("{}/account", server.url()), true);
    home.policy();
    enable_source(&mut server, 1).await;
    append(1, 1000, false);
    let bodies = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let observed = bodies.clone();
    let rejected = server
        .mock("POST", "/account")
        .match_body(matcher("upload"))
        .with_status(503)
        .with_body_from_request(move |request| {
            observed.lock().unwrap().push(
                serde_json::from_slice::<Value>(request.body().unwrap()).unwrap()
                    ["usage_analytics"]
                    .to_string(),
            );
            b"temporarily unavailable".to_vec()
        })
        .expect(4)
        .create_async()
        .await;
    let first = invoke_stopped_sync().await;
    assert_eq!(first["result"]["batches"], 4);
    assert_eq!(first["result"]["reason"], "retry_cooldown");
    assert_eq!(first["result"]["outcome"], "outcome_unknown");
    assert!(first["result"]["reset_at_ms"].is_null());
    let retry = first["result"]["retry_at_ms"].as_i64().unwrap();
    assert!((299_000..=300_000).contains(&(retry - now_ms())));
    let frozen = bodies.lock().unwrap();
    assert_eq!(frozen.len(), 4);
    assert!(frozen.iter().all(|body| body == &frozen[0]));
    drop(frozen);
    rejected.assert_async().await;
    rejected.remove_async().await;
    let forbidden = server
        .mock("POST", "/account")
        .expect(0)
        .create_async()
        .await;
    let again = invoke_stopped_sync().await;
    assert_eq!(again["result"]["reason"], "retry_cooldown");
    assert_eq!(again["result"]["retry_at_ms"], retry);
    assert_eq!(
        again["result"]["job"]["job_id"],
        first["result"]["job"]["job_id"]
    );
    assert_eq!(again["result"]["batches"], 0);
    forbidden.assert_async().await;
    let db = open().unwrap();
    assert_eq!(
        db.query_row("SELECT attempts FROM usage_cloud_upload_batches", [], |r| r
            .get::<_, u32>(0))
            .unwrap(),
        4
    );
}

#[tokio::test]
async fn longer_retry_after_is_persisted_without_waiting_or_daily_reset() {
    let mut server = Server::new_async().await;
    let home = Home::new(format!("{}/account", server.url()), true);
    home.policy();
    enable_source(&mut server, 1).await;
    append(1, 1000, false);
    let retry = now_ms() + 90_000;
    let gate=server.mock("POST","/account").match_body(matcher("upload"))
        .with_header("Retry-After","60")
        .with_body(response(json!({"protocol_version":2,"outcome":"rate_limited","retry_after_ms":30_000,"retry_at_ms":retry})))
        .expect(1).create_async().await;
    let result = invoke_stopped_sync().await;
    assert_eq!(result["result"]["reason"], "retry_backoff");
    assert_eq!(result["result"]["retry_at_ms"], retry);
    assert_eq!(result["result"]["batches"], 1);
    assert!(result["result"]["reset_at_ms"].is_null());
    gate.assert_async().await;
    gate.remove_async().await;
    let forbidden = server
        .mock("POST", "/account")
        .expect(0)
        .create_async()
        .await;
    resume_active().await;
    forbidden.assert_async().await;
}

#[tokio::test]
async fn invalid_unchanged_payload_is_terminal_across_invocations() {
    let mut server = Server::new_async().await;
    let home = Home::new(format!("{}/account", server.url()), true);
    home.policy();
    enable_source(&mut server, 1).await;
    append(1, 1000, false);
    let invalid = server
        .mock("POST", "/account")
        .match_body(matcher("upload"))
        .with_status(400)
        .with_body(
            json!({"status":"failed","type":"usage_analytics_invalid_request","retryable":true})
                .to_string(),
        )
        .expect(1)
        .create_async()
        .await;
    let first = invoke_stopped_sync().await;
    assert_eq!(first["result"]["reason"], "invalid_request");
    assert_eq!(first["result"]["batches"], 1);
    invalid.assert_async().await;
    invalid.remove_async().await;
    let forbidden = server
        .mock("POST", "/account")
        .expect(0)
        .create_async()
        .await;
    let again = invoke_stopped_sync().await;
    assert_eq!(again["result"]["reason"], "invalid_request");
    assert_eq!(again["result"]["batches"], 0);
    forbidden.assert_async().await;
}

#[tokio::test]
async fn expired_token_refresh_consumes_one_of_four_physical_upload_attempts() {
    let mut server = Server::new_async().await;
    let home = Home::new(format!("{}/account", server.url()), true);
    home.policy();
    enable_source(&mut server, 1).await;
    append(1, 1000, false);
    let expired = server
        .mock("POST", "/account")
        .match_body(Matcher::AllOf(vec![
            matcher("upload"),
            Matcher::PartialJson(json!({"credentials":{"access_token":"fixture-access"}})),
        ]))
        .with_body(json!({"status":"failed","type":"access_token_expired"}).to_string())
        .expect(1)
        .create_async()
        .await;
    let refresh = server
        .mock("POST", "/account")
        .match_body(Matcher::PartialJson(json!({"action":"status"})))
        .with_body(
            json!({"status":"success","session":{"access_token":"renewed-fixture-access"}})
                .to_string(),
        )
        .expect(1)
        .create_async()
        .await;
    let unavailable = server
        .mock("POST", "/account")
        .match_body(Matcher::AllOf(vec![
            matcher("upload"),
            Matcher::PartialJson(json!({"credentials":{"access_token":"renewed-fixture-access"}})),
        ]))
        .with_status(503)
        .with_body("unavailable")
        .expect(3)
        .create_async()
        .await;
    let result = invoke_stopped_sync().await;
    assert_eq!(result["result"]["batches"], 4);
    assert_eq!(result["result"]["reason"], "retry_cooldown");
    expired.assert_async().await;
    refresh.assert_async().await;
    unavailable.assert_async().await;
}

#[tokio::test]
async fn admitted_request_keeps_its_deadline_after_pass_budget() {
    let mut server = Server::new_async().await;
    let home = Home::new(format!("{}/account", server.url()), true);
    home.policy();
    enable_source(&mut server, 1).await;
    append(1, 1000, false);
    let mut db = open().unwrap();
    let selection = selected().unwrap().unwrap();
    let mut lease = queue::acquire_lease(&mut db, &selection.source, now_ms(), queue::MAX_LEASE_MS)
        .unwrap()
        .unwrap();
    let job = queue::job::start(&mut db, &lease, now_ms(), None).unwrap();
    let upload = server
        .mock("POST", "/account")
        .match_body(matcher("upload"))
        .with_body_from_request(|request| {
            std::thread::sleep(Duration::from_millis(250));
            response(ack(request.body().unwrap())).into_bytes()
        })
        .expect(1)
        .create_async()
        .await;
    let mut transport = transport::V2Transport::new(REQUEST_TIMEOUT).unwrap();
    let started = Instant::now() - DEADLINE + Duration::from_millis(100);
    let result = sync_leased(&mut db, &mut lease, &job, started, &mut transport)
        .await
        .unwrap();
    assert_eq!(result["uploaded_events"], 1);
    assert_eq!(result["outcome"], "fully_synced");
    assert_eq!(result["deadline_reached"], true);
    upload.assert_async().await;
}

#[tokio::test]
async fn changed_credentials_stop_a_reused_client_before_retry_transmission() {
    let mut server = Server::new_async().await;
    let home = Home::new(format!("{}/account", server.url()), true);
    home.policy();
    enable_source(&mut server, 1).await;
    append(1, 1000, false);
    let first = server
        .mock("POST", "/account")
        .match_body(matcher("upload"))
        .with_status(503)
        .with_body_from_request(|_| {
            crate::credentials::store::store_account_tokens("different-account-token", None)
                .unwrap();
            b"unavailable".to_vec()
        })
        .expect(1)
        .create_async()
        .await;
    let result = sync().await.unwrap();
    assert_eq!(result["batches"], 1);
    assert_eq!(result["uploaded_events"], 0);
    assert!(result["stopped_error"]
        .as_str()
        .unwrap()
        .contains("credentials changed"));
    first.assert_async().await;
    assert_eq!(
        open()
            .unwrap()
            .query_row("SELECT attempts FROM usage_cloud_upload_batches", [], |r| r
                .get::<_, u32>(0))
            .unwrap(),
        1
    );
}
