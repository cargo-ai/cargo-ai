//! Finite CLI adapters for explicit cloud custody and local retention choices.
use crate::usage_analytics::{self as cloud, contract::*};
use clap::{Arg, ArgAction, ArgMatches, Command};
use serde::Serialize;
use serde_json::{json, Value};
fn json_flag(command: Command) -> Command {
    command.arg(Arg::new("json").long("json").action(ArgAction::SetTrue))
}
pub(crate) fn cloud_command() -> Command {
    Command::new("cloud")
        .about("Manage independent cloud analytics custody; disabled by default")
        .subcommand_required(true)
        .subcommand(json_flag(
            Command::new("status")
                .arg(Arg::new("remote").long("remote").action(ArgAction::SetTrue)),
        ))
        .subcommand(json_flag(Command::new("enable")))
        .subcommand(json_flag(Command::new("disable")))
        .subcommand(json_flag(
            Command::new("sync").about("Reconcile at most one immutable batch within 15 seconds"),
        ))
        .subcommand(json_flag(
            Command::new("include-history")
                .about("Preview older history; confirmation selects without uploading")
                .arg(
                    Arg::new("through")
                        .long("through")
                        .value_parser(clap::value_parser!(i64).range(0..)),
                )
                .arg(Arg::new("preview-hash").long("preview-hash"))
                .arg(
                    Arg::new("confirm")
                        .long("confirm")
                        .action(ArgAction::SetTrue)
                        .requires_all(["through", "preview-hash"]),
                ),
        ))
        .subcommand(json_flag(
            Command::new("clear")
                .about(
                    "Preview or explicitly clear the exact cloud generation; local history remains",
                )
                .arg(Arg::new("preview-id").long("preview-id"))
                .arg(Arg::new("preview-hash").long("preview-hash"))
                .arg(
                    Arg::new("confirm")
                        .long("confirm")
                        .action(ArgAction::SetTrue)
                        .requires_all(["preview-id", "preview-hash"]),
                ),
        ))
        .subcommand(json_flag(
            Command::new("history")
                .arg(
                    Arg::new("after-sequence")
                        .long("after-sequence")
                        .value_parser(clap::value_parser!(u64).range(1..=i64::MAX as u64))
                        .requires("through-sequence"),
                )
                .arg(
                    Arg::new("through-sequence")
                        .long("through-sequence")
                        .value_parser(clap::value_parser!(u64).range(0..=i64::MAX as u64)),
                )
                .arg(
                    Arg::new("limit")
                        .long("limit")
                        .default_value("100")
                        .value_parser(clap::value_parser!(u16).range(1..=100)),
                ),
        ))
}
pub(crate) fn retention_command() -> Command {
    Command::new("retention")
        .about("Inspect or explicitly consent to permanent expiry of richer local details")
        .subcommand_required(true)
        .subcommand(json_flag(Command::new("status")))
        .subcommand(json_flag(Command::new("preview")))
        .subcommand(json_flag(
            Command::new("configure")
                .arg(
                    Arg::new("days")
                        .long("days")
                        .value_parser(["1", "7", "30"])
                        .required(true),
                )
                .arg(
                    Arg::new("enabled")
                        .long("enabled")
                        .value_parser(["on", "off"])
                        .required(true),
                )
                .arg(
                    Arg::new("acknowledge-local-detail-loss")
                        .long("acknowledge-local-detail-loss")
                        .action(ArgAction::SetTrue)
                        .required_if_eq("enabled", "on"),
                )
                .arg(
                    Arg::new("acknowledge-unselected-loss")
                        .long("acknowledge-unselected-loss")
                        .action(ArgAction::SetTrue),
                ),
        ))
}
#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum Effect {
    Unapplied,
    Applied,
    Unknown,
    ReadOnly,
    Requested,
}
#[derive(Serialize)]
struct Effects {
    local: Effect,
    remote: Effect,
}
#[derive(Serialize)]
struct Payload {
    schema_version: u8,
    operation: String,
    result: Value,
    local: Value,
    effects: Effects,
    postconditions_verified: bool,
}
fn selection(args: &ArgMatches) -> Result<(&str, &str, &ArgMatches), String> {
    let (kind, group) = args
        .subcommand()
        .ok_or("A usage maintenance command is required")?;
    let (command, leaf) = group
        .subcommand()
        .ok_or("A usage maintenance operation is required")?;
    Ok((kind, command, leaf))
}
fn mutates(kind: &str, command: &str, leaf: &ArgMatches) -> bool {
    match (kind, command) {
        ("cloud", "enable" | "disable" | "sync") | ("retention", "configure") => true,
        ("cloud", "clear" | "include-history") => leaf.get_flag("confirm"),
        _ => false,
    }
}
fn requires_remote(kind: &str, command: &str, leaf: &ArgMatches) -> bool {
    kind == "cloud"
        && (matches!(command, "enable" | "disable" | "sync" | "clear" | "history")
            || command == "status" && leaf.get_flag("remote"))
}
async fn execute(args: &ArgMatches) -> Result<Value, String> {
    let (kind, command, leaf) = selection(args)?;
    let remote = requires_remote(kind, command, leaf);
    let result = match (kind, command) {
        ("cloud", "status") if leaf.get_flag("remote") => {
            json!({"local":cloud::local_status()?,"remote":cloud::remote_status().await?})
        }
        ("cloud", "status") => cloud::local_status()?,
        ("cloud", "enable") => cloud::enable().await?,
        ("cloud", "disable") => cloud::disable().await?,
        ("cloud", "sync") => cloud::sync().await?,
        ("cloud", "include-history") => cloud::history_selection(
            leaf.get_one::<i64>("through").copied(),
            if leaf.get_flag("confirm") {
                Some(
                    leaf.get_one::<String>("preview-hash")
                        .ok_or("Preview hash missing")?
                        .as_str(),
                )
            } else {
                None
            },
        )?,
        ("cloud", "clear") => {
            let preview = if leaf.get_flag("confirm") {
                Some((
                    CanonicalUuid::try_from(
                        leaf.get_one::<String>("preview-id")
                            .ok_or("Preview identity missing")?
                            .clone(),
                    )
                    .map_err(|e| e.to_string())?,
                    Sha256Hash::try_from(
                        leaf.get_one::<String>("preview-hash")
                            .ok_or("Preview hash missing")?
                            .clone(),
                    )
                    .map_err(|e| e.to_string())?,
                ))
            } else {
                None
            };
            cloud::clear(preview).await?
        }
        ("cloud", "history") => {
            cloud::history(
                leaf.get_one::<u64>("after-sequence").copied(),
                leaf.get_one::<u64>("through-sequence").copied(),
                *leaf.get_one::<u16>("limit").unwrap(),
            )
            .await?
        }
        ("retention", "status") => crate::usage_store::retention::status(cloud::now_ms())?,
        ("retention", "preview") => crate::usage_store::retention::preview(cloud::now_ms())?,
        ("retention", "configure") => {
            crate::usage_store::retention::configure(crate::usage_store::retention::Policy {
                schema_version: 1,
                days: leaf.get_one::<String>("days").unwrap().parse().unwrap(),
                enabled: leaf.get_one::<String>("enabled").unwrap() == "on",
                local_detail_loss_acknowledged: leaf.get_flag("acknowledge-local-detail-loss"),
                unselected_loss_acknowledged: leaf.get_flag("acknowledge-unselected-loss"),
            })?
        }
        _ => return Err("Unsupported usage maintenance operation".into()),
    };
    let inactive_sync = kind == "cloud"
        && command == "sync"
        && result.get("batches").and_then(Value::as_u64).unwrap_or(0) == 0;
    let local_read_only_sync = kind == "cloud"
        && command == "sync"
        && (result["enabled"] == false || result["busy"] == true);
    let remote_effect = if !remote || inactive_sync {
        Effect::Unapplied
    } else if command == "sync" {
        Effect::Requested
    } else if mutates(kind, command, leaf) {
        Effect::Applied
    } else {
        Effect::ReadOnly
    };
    serde_json::to_value(Payload {
        schema_version: 1,
        operation: format!("usage {kind} {command}"),
        result,
        local: cloud::local_status()?,
        effects: Effects {
            local: if mutates(kind, command, leaf) && !local_read_only_sync {
                Effect::Applied
            } else {
                Effect::ReadOnly
            },
            remote: remote_effect,
        },
        postconditions_verified: true,
    })
    .map_err(|_| "Cannot encode usage result".into())
}
pub(crate) async fn run(args: &ArgMatches) -> bool {
    match execute(args).await {
        Ok(value) => {
            println!("{value}");
            true
        }
        Err(error) => {
            println!(
                "{}",
                json!({"schema_version":1,"error":{"kind":"usage_maintenance_failed","message":error},"effects":{"local":"unknown","remote":"unknown"}})
            );
            false
        }
    }
}
pub(crate) async fn machine_run(
    args: &ArgMatches,
) -> Result<Value, crate::commands::machine::Failure> {
    execute(args).await.map_err(|_|{
        let mutation=selection(args).is_ok_and(|(kind,command,leaf)|mutates(kind,command,leaf));
        let remote=selection(args).is_ok_and(|(kind,command,leaf)|requires_remote(kind,command,leaf));
        crate::commands::machine::Failure::new(if mutation{"operation.partial"}else{"usage.operation_failed"},"Usage maintenance did not complete; inspect preserved local state before retrying.").with_data(json!({"partial":mutation,"observed_local":cloud::local_status().ok(),"effects":Effects{local:if mutation{Effect::Unknown}else{Effect::ReadOnly},remote:if remote{Effect::Unknown}else{Effect::Unapplied}}}))
    })
}
#[cfg(test)]
mod schema_tests;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explicit_loss_and_preview_confirmation_are_required() {
        let command = || {
            Command::new("test")
                .subcommand(cloud_command())
                .subcommand(retention_command())
        };
        assert!(command()
            .try_get_matches_from([
                "test",
                "retention",
                "configure",
                "--days",
                "7",
                "--enabled",
                "on"
            ])
            .is_err());
        assert!(command()
            .try_get_matches_from(["test", "cloud", "clear", "--confirm"])
            .is_err());
        assert!(command()
            .try_get_matches_from(["test", "cloud", "include-history", "--confirm"])
            .is_err());
        assert!(command()
            .try_get_matches_from([
                "test",
                "retention",
                "configure",
                "--days",
                "0",
                "--enabled",
                "off"
            ])
            .is_err());
    }
}

#[cfg(test)]
mod integration_tests {
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
                std::env::temp_dir().join(format!("analytics-client-{}", uuid::Uuid::new_v4()));
            let previous = ["CARGO_AI_HOME", "CARGO_AI_DISABLE_KEYCHAIN"]
                .into_iter()
                .map(|key| (key, std::env::var_os(key)))
                .collect();
            unsafe {
                std::env::set_var("CARGO_AI_HOME", &path);
                std::env::set_var("CARGO_AI_DISABLE_KEYCHAIN", "1");
            }
            let endpoint = cloud::TEST_ENDPOINT.with(|value| value.replace(Some(endpoint)));
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
            cloud::TEST_ENDPOINT.with(|value| value.replace(self.endpoint.take()));
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
    async fn invoke(arguments: &[&str]) -> Value {
        let args = Command::new("test")
            .subcommand(cloud_command())
            .subcommand(retention_command())
            .try_get_matches_from(std::iter::once("test").chain(arguments.iter().copied()))
            .unwrap();
        let value = execute(&args).await.unwrap();
        super::schema_tests::assert_valid(&value);
        value
    }
    fn matcher(command: &str) -> Matcher {
        Matcher::PartialJson(
            json!({"action":"usage_analytics","usage_analytics":{"command":command}}),
        )
    }
    fn response(body: Value) -> String {
        json!({"status":"success","type":"usage_analytics_succeeded","usage_analytics":body})
            .to_string()
    }
    async fn enable(server: &mut Server, binding: &str) {
        let status=server.mock("POST","/account").match_body(matcher("status")).with_body(response(json!({"protocol_version":1,"capability":{"enabled":true,"supported_protocol_versions":[1]},"account_binding":binding,"data_generation":1,"lifecycle_epoch":0,"consent_interval":null}))).expect(1).create_async().await;
        let enabled=server.mock("POST","/account").match_body(matcher("enable")).with_body_from_request(|request|{let body:Value=serde_json::from_slice(request.body().unwrap()).unwrap();response(json!({"source":body["usage_analytics"]["source"],"provenance":{"lifecycle_epoch":0,"consent_interval":1},"enabled":true})).into_bytes()}).expect(1).create_async().await;
        assert_eq!(
            invoke(&["cloud", "enable"]).await["result"]["enabled"],
            true
        );
        status.assert_async().await;
        enabled.assert_async().await;
        status.remove_async().await;
        enabled.remove_async().await;
    }
    fn append() {
        crate::usage_store::persist(&json!({"schema_version":1,"event_id":format!("cai_event_{}",uuid::Uuid::new_v4()),"root_run_id":format!("cai_run_{}",uuid::Uuid::new_v4()),"agent_run_id":format!("cai_agent_run_{}",uuid::Uuid::new_v4()),"event_type":"provider_request_completed","timestamp":"2026-10-27T00:00:00Z","provider":{"server":"typesafe","profile":"private fixture customer","requested_model":"jev-1.13.0"},"agent":{"project_root":"/private/fixture/project","name":"private fixture agent"},"usage":{"input_tokens":u64::MAX,"output_tokens":3},"status":"success"})).unwrap();
    }
    fn retained(request: Request) -> Value {
        let Request::Sync {
            source,
            provenance,
            batch_id,
            ordered_membership_sha256,
            events,
            ..
        } = request
        else {
            panic!("expected exact sync")
        };
        let members = events
            .iter()
            .map(EventSubmission::member)
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        let admission = AdmissionCommitment {
            protocol_version: Version,
            source: source.clone(),
            provenance: provenance.clone(),
            batch_id: batch_id.clone(),
            ordered_membership_sha256: ordered_membership_sha256.clone(),
            manifest_sha256: Sha256Hash::of(b"authenticated fixture manifest"),
        };
        let receipt = Receipt {
            protocol_version: Version,
            source,
            provenance,
            batch_id,
            ordered_membership_sha256,
            manifest_sha256: admission.manifest_sha256.clone(),
            custody: Custody::Retained,
            members,
        };
        json!({"admission":admission,"custody":"retained","receipt":receipt})
    }
    #[tokio::test]
    async fn real_operation_results_match_closed_machine_schema() {
        let mut server = Server::new_async().await;
        let _home = Home::new(format!("{}/account", server.url()), true);
        invoke(&["cloud", "status"]).await;
        invoke(&["cloud", "sync"]).await;
        invoke(&["retention", "status"]).await;
        invoke(&["retention", "preview"]).await;
        invoke(&["retention", "configure", "--days", "7", "--enabled", "off"]).await;
        let unavailable=server.mock("POST","/account").match_body(matcher("status"))
            .with_body(response(json!({"protocol_version":1,"capability":{"enabled":false,"supported_protocol_versions":[1]}})))
            .expect(1).create_async().await;
        invoke(&["cloud", "status", "--remote"]).await;
        unavailable.assert_async().await;
        unavailable.remove_async().await;
        enable(&mut server, "00000000-0000-4000-8000-000000000001").await;
        let source = cloud::selected().unwrap().unwrap().source;
        let mut lease_db = crate::usage_store::open_database(true).unwrap().unwrap();
        let lease = cloud::queue::acquire_lease(&mut lease_db, &source, cloud::now_ms(), 10_000)
            .unwrap()
            .unwrap();
        assert_eq!(invoke(&["cloud", "sync"]).await["result"]["busy"], true);
        cloud::queue::release_lease(&mut lease_db, &lease, cloud::now_ms()).unwrap();
        let remote=server.mock("POST","/account").match_body(matcher("status"))
            .with_body(response(json!({"protocol_version":1,"capability":{"enabled":true,"supported_protocol_versions":[1]},"account_binding":source.account_binding,"data_generation":1,"lifecycle_epoch":0,"consent_interval":1,"enabled":true})))
            .expect(1).create_async().await;
        invoke(&["cloud", "status", "--remote"]).await;
        remote.assert_async().await;
        remote.remove_async().await;
        let selection = invoke(&["cloud", "include-history"]).await;
        let through = selection["result"]["through_sequence"].as_str().unwrap();
        let hash = selection["result"]["preview_sha256"].as_str().unwrap();
        invoke(&[
            "cloud",
            "include-history",
            "--confirm",
            "--through",
            through,
            "--preview-hash",
            hash,
        ])
        .await;
        invoke(&["cloud", "sync"]).await;
        append();
        let sync = server
            .mock("POST", "/account")
            .match_body(matcher("sync"))
            .with_body_from_request(|request| {
                let value: Value = serde_json::from_slice(request.body().unwrap()).unwrap();
                response(retained(
                    Request::parse(&value["usage_analytics"].to_string()).unwrap(),
                ))
                .into_bytes()
            })
            .expect(1)
            .create_async()
            .await;
        invoke(&["cloud", "sync"]).await;
        sync.assert_async().await;
        let history = server
            .mock("POST", "/account")
            .match_body(matcher("history"))
            .with_body(response(
                json!({"protocol_version":1,"source":source,"high_watermark":0,"events":[]}),
            ))
            .expect(1)
            .create_async()
            .await;
        invoke(&["cloud", "history"]).await;
        history.assert_async().await;
        let disabled = server
            .mock("POST", "/account")
            .match_body(matcher("disable"))
            .with_body(response(
                json!({"protocol_version":1,"source":source,"enabled":false}),
            ))
            .expect(1)
            .create_async()
            .await;
        invoke(&["cloud", "disable"]).await;
        disabled.assert_async().await;
        let preview_id = "00000000-0000-4000-8000-000000000020";
        let hash = Sha256Hash::of(b"exact synthetic clear preview");
        let preview=server.mock("POST","/account").match_body(matcher("clear_preview"))
            .with_body(response(json!({"protocol_version":1,"source":source,"preview_id":preview_id,"preview_sha256":hash,"data_generation":1})))
            .expect(1).create_async().await;
        invoke(&["cloud", "clear"]).await;
        preview.assert_async().await;
        let cleared=server.mock("POST","/account").match_body(matcher("clear_confirm"))
            .with_body(response(json!({"protocol_version":1,"source":source,"cleared":true,"data_generation":2,"purge_state":"pending"})))
            .expect(1).create_async().await;
        invoke(&[
            "cloud",
            "clear",
            "--confirm",
            "--preview-id",
            preview_id,
            "--preview-hash",
            hash.as_str(),
        ])
        .await;
        cleared.assert_async().await;
        invoke(&["cloud", "status"]).await;
        invoke(&["retention", "status"]).await;
        invoke(&["retention", "preview"]).await;
        invoke(&[
            "retention",
            "configure",
            "--days",
            "1",
            "--enabled",
            "on",
            "--acknowledge-local-detail-loss",
        ])
        .await;
    }
    #[tokio::test]
    async fn passive_local_status_and_disabled_sync_have_no_home_or_account_effects() {
        let home = Home::new("http://127.0.0.1:1/never".into(), false);
        assert_eq!(cloud::local_status().unwrap()["enabled"], false);
        assert_eq!(cloud::sync().await.unwrap()["enabled"], false);
        assert!(!home.path.exists());
    }
    #[tokio::test]
    async fn authenticated_readiness_is_required_before_home_or_consent_creation() {
        let mut server = Server::new_async().await;
        let home = Home::new(format!("{}/account", server.url()), true);
        let status=server.mock("POST","/account").match_body(matcher("status")).with_body(response(json!({"protocol_version":1,"capability":{"enabled":false,"supported_protocol_versions":[1]}}))).expect(1).create_async().await;
        assert!(cloud::enable().await.is_err());
        assert!(!home.path.join("usage").exists());
        status.assert_async().await;
    }
    #[tokio::test]
    async fn lost_sync_response_recovers_authenticated_exact_receipt_without_reappend() {
        let mut server = Server::new_async().await;
        let _home = Home::new(format!("{}/account", server.url()), true);
        enable(&mut server, "00000000-0000-4000-8000-000000000001").await;
        append();
        let lost = server
            .mock("POST", "/account")
            .match_body(matcher("sync"))
            .with_status(500)
            .with_body("unknown private provider body")
            .expect(1)
            .create_async()
            .await;
        assert!(cloud::sync().await.is_err());
        lost.assert_async().await;
        lost.remove_async().await;
        let db = crate::usage_store::open_database(false).unwrap().unwrap();
        let text: String = db
            .query_row("SELECT request_json FROM usage_cloud_batches", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert!(!text.contains("private fixture"));
        assert!(!text.contains("/private/"));
        let exact = Request::parse(&text).unwrap();
        let result = retained(exact);
        let recovery = server
            .mock("POST", "/account")
            .match_body(matcher("receipt"))
            .with_body(response(result))
            .expect(1)
            .create_async()
            .await;
        let result = invoke(&["cloud", "sync"]).await["result"].clone();
        assert_eq!(result["recovered"], true);
        assert_eq!(cloud::local_status().unwrap()["retained"], 1);
        recovery.assert_async().await;
        assert_eq!(
            db.query_row("SELECT COUNT(*) FROM usage_backup_queue", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            db.query_row(
                "SELECT COUNT(*) FROM usage_cloud_queue WHERE original_text IS NOT NULL",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            1
        );
    }
    #[tokio::test]
    async fn admitted_dispatch_gap_resends_only_on_authoritative_payload_required_with_identical_bytes(
    ) {
        let mut server = Server::new_async().await;
        let _home = Home::new(format!("{}/account", server.url()), true);
        enable(&mut server, "00000000-0000-4000-8000-000000000001").await;
        append();
        let lost = server
            .mock("POST", "/account")
            .match_body(matcher("sync"))
            .with_status(500)
            .with_body("unknown")
            .expect(1)
            .create_async()
            .await;
        assert!(cloud::sync().await.is_err());
        lost.assert_async().await;
        lost.remove_async().await;
        let db = crate::usage_store::open_database(false).unwrap().unwrap();
        let text: String = db
            .query_row("SELECT request_json FROM usage_cloud_batches", [], |row| {
                row.get(0)
            })
            .unwrap();
        let retained = retained(Request::parse(&text).unwrap());
        let mut pending = retained.clone();
        pending.as_object_mut().unwrap().remove("receipt");
        pending["custody"] = json!("pending");
        let poll = server
            .mock("POST", "/account")
            .match_body(matcher("receipt"))
            .with_body(response(pending.clone()))
            .expect(1)
            .create_async()
            .await;
        assert_eq!(
            invoke(&["cloud", "sync"]).await["result"]["custody"],
            "pending"
        );
        poll.assert_async().await;
        poll.remove_async().await;
        assert_eq!(
            db.query_row("SELECT attempts FROM usage_cloud_batches", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
        pending["payload_required"] = json!(true);
        let reopen = server
            .mock("POST", "/account")
            .match_body(matcher("receipt"))
            .with_body(response(pending))
            .expect(1)
            .create_async()
            .await;
        let same = text.clone();
        let appended = server
            .mock("POST", "/account")
            .match_body(matcher("sync"))
            .with_body_from_request(move |request| {
                assert!(std::str::from_utf8(request.body().unwrap())
                    .unwrap()
                    .contains(&same));
                response(retained.clone()).into_bytes()
            })
            .expect(1)
            .create_async()
            .await;
        assert_eq!(cloud::sync().await.unwrap()["retained_events"], 1);
        reopen.assert_async().await;
        appended.assert_async().await;
        assert_eq!(
            db.query_row("SELECT request_json FROM usage_cloud_batches", [], |row| {
                row.get::<_, String>(0)
            })
            .unwrap(),
            text
        );
        assert_eq!(
            db.query_row("SELECT attempts FROM usage_cloud_batches", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            2
        );
    }
    #[tokio::test]
    async fn terminal_identity_conflicts_are_durably_quarantined_without_losing_payload() {
        let mut server = Server::new_async().await;
        let _home = Home::new(format!("{}/account", server.url()), true);
        enable(&mut server, "00000000-0000-4000-8000-000000000001").await;
        append();
        let conflict = server
            .mock("POST", "/account")
            .match_body(matcher("sync"))
            .with_body(json!({"status":"failed","type":"usage_analytics_conflict"}).to_string())
            .expect(1)
            .create_async()
            .await;
        assert!(cloud::sync().await.is_err());
        assert_eq!(cloud::local_status().unwrap()["quarantined"], 1);
        assert_eq!(cloud::local_status().unwrap()["pending"], 1);
        let next = invoke(&["cloud", "sync"]).await["result"].clone();
        assert_eq!(next["batches"], 0);
        assert_eq!(next["blocked"], true);
        conflict.assert_async().await;
    }
    #[tokio::test]
    async fn history_pages_keep_the_captured_high_watermark_and_exact_identity() {
        let mut server = Server::new_async().await;
        let _home = Home::new(format!("{}/account", server.url()), true);
        enable(&mut server, "00000000-0000-4000-8000-000000000001").await;
        let source = cloud::selected().unwrap().unwrap().source;
        let event = |sequence: u64| {
            let id = format!("cai_event_00000000-0000-4000-8000-{sequence:012}");
            let text=json!({"schema_version":1,"normalization_version":1,"event_id":id,"root_run_id":"00000000-0000-4000-8000-000000000010","agent_run_id":"00000000-0000-4000-8000-000000000011","event_type":"provider_request","occurred_at_ms":1,"tokens":{"input_tokens":u64::MAX,"output_tokens":null}}).to_string();
            let mut event =
                serde_json::to_value(EventSubmission::from_allowed_text(text).unwrap()).unwrap();
            event["sequence"] = json!(sequence);
            event["event_id"] = json!(id);
            event
        };
        let first=server.mock("POST","/account").match_body(matcher("history")).with_body(response(json!({"protocol_version":1,"source":source,"high_watermark":2,"events":[event(1)]}))).expect(1).create_async().await;
        let page = invoke(&["cloud", "history", "--limit", "1"]).await["result"].clone();
        assert_eq!(page["next_sequence"], 1);
        assert_eq!(page["through_sequence"], 2);
        first.assert_async().await;
        first.remove_async().await;
        let second=server.mock("POST","/account").match_body(Matcher::PartialJson(json!({"usage_analytics":{"command":"history","after_sequence":1,"through_sequence":2,"limit":1}}))).with_body(response(json!({"protocol_version":1,"source":source,"high_watermark":2,"events":[event(2)]}))).expect(1).create_async().await;
        let page = invoke(&[
            "cloud",
            "history",
            "--after-sequence",
            "1",
            "--through-sequence",
            "2",
            "--limit",
            "1",
        ])
        .await["result"]
            .clone();
        assert!(page["next_sequence"].is_null());
        second.assert_async().await;
        second.remove_async().await;
        let changed = server
            .mock("POST", "/account")
            .match_body(matcher("history"))
            .with_body(response(
                json!({"protocol_version":1,"source":source,"high_watermark":3,"events":[]}),
            ))
            .expect(1)
            .create_async()
            .await;
        assert!(cloud::history(Some(1), Some(2), 1).await.is_err());
        changed.assert_async().await;
        assert!(cloud::history(Some(1), None, 1).await.is_err());
    }
    #[tokio::test]
    async fn lost_clear_response_protects_local_history_until_generation_reconciliation() {
        let mut server = Server::new_async().await;
        let home = Home::new(format!("{}/account", server.url()), true);
        enable(&mut server, "00000000-0000-4000-8000-000000000001").await;
        let original_source = cloud::selected().unwrap().unwrap().source;
        append();
        let sync = server
            .mock("POST", "/account")
            .match_body(matcher("sync"))
            .with_body_from_request(|request| {
                let body: Value = serde_json::from_slice(request.body().unwrap()).unwrap();
                response(retained(
                    Request::parse(&body["usage_analytics"].to_string()).unwrap(),
                ))
                .into_bytes()
            })
            .expect(1)
            .create_async()
            .await;
        invoke(&["cloud", "sync"]).await;
        sync.assert_async().await;
        sync.remove_async().await;
        append();
        let mut db = crate::usage_store::open_database(true).unwrap().unwrap();
        cloud::queue::capture(&mut db, &original_source, cloud::project).unwrap();
        let (retained_id, sequence): (String, i64) = db
            .query_row(
                "SELECT event_id,sequence FROM usage_cloud_queue WHERE retained=1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert!(cloud::queue::retained_event(&db, &retained_id).unwrap());
        crate::usage_store::retention::configure(crate::usage_store::retention::Policy {
            schema_version: 1,
            days: 1,
            enabled: true,
            local_detail_loss_acknowledged: true,
            unselected_loss_acknowledged: true,
        })
        .unwrap();
        let path = home.path.join("usage/usage.sqlite3");
        let lost=server.mock("POST","/account").match_body(matcher("clear_confirm"))
            .with_status(500).with_body_from_request(move |_|{
                let check=rusqlite::Connection::open(&path).unwrap();
                assert!(check.query_row("SELECT enabled=0 AND clear_intent_json IS NOT NULL FROM usage_cloud_state",[],|row|row.get::<_,bool>(0)).unwrap(),"clear intent must commit before network dispatch");
                b"unknown clear outcome".to_vec()
            }).expect(1).create_async().await;
        assert!(cloud::clear(Some((
            CanonicalUuid::try_from("00000000-0000-4000-8000-000000000020".to_owned()).unwrap(),
            Sha256Hash::of(b"exact clear preview")
        )))
        .await
        .is_err());
        lost.assert_async().await;
        lost.remove_async().await;
        assert!(cloud::queue::clear_outcome_unknown(&db, &original_source).unwrap());
        assert!(!cloud::queue::retained_event(&db, &retained_id).unwrap());
        assert!(cloud::queue::protected_event(&db, &retained_id, sequence).unwrap());
        assert_eq!(
            invoke(&["cloud", "status"]).await["result"]["clear_outcome_unknown"],
            true
        );
        let age = cloud::now_ms() + 31 * 86_400_000;
        let retention = crate::usage_store::retention::apply(age).unwrap();
        assert_eq!(retention["deleted_records"], 0);
        assert_eq!(retention["custody_blocked_records"], 2);
        let unchanged=server.mock("POST","/account").match_body(matcher("status"))
            .with_body(response(json!({"protocol_version":1,"capability":{"enabled":true,"supported_protocol_versions":[1]},"account_binding":original_source.account_binding,"data_generation":1,"lifecycle_epoch":0,"consent_interval":1,"enabled":false})))
            .expect(1).create_async().await;
        let forbidden = server
            .mock("POST", "/account")
            .match_body(matcher("enable"))
            .expect(0)
            .create_async()
            .await;
        assert!(cloud::enable().await.is_err());
        unchanged.assert_async().await;
        forbidden.assert_async().await;
        unchanged.remove_async().await;
        forbidden.remove_async().await;
        assert!(!cloud::queue::retained_event(&db, &retained_id).unwrap());
        let advanced=server.mock("POST","/account").match_body(matcher("status"))
            .with_body(response(json!({"protocol_version":1,"capability":{"enabled":true,"supported_protocol_versions":[1]},"account_binding":original_source.account_binding,"data_generation":2,"lifecycle_epoch":0,"consent_interval":1,"enabled":false})))
            .expect(1).create_async().await;
        let renewed=server.mock("POST","/account").match_body(matcher("enable"))
            .with_body_from_request(|request|{let body:Value=serde_json::from_slice(request.body().unwrap()).unwrap();assert_eq!(body["usage_analytics"]["source"]["data_generation"],2);response(json!({"protocol_version":1,"source":body["usage_analytics"]["source"],"provenance":{"lifecycle_epoch":0,"consent_interval":2},"enabled":true})).into_bytes()})
            .expect(1).create_async().await;
        invoke(&["cloud", "enable"]).await;
        advanced.assert_async().await;
        renewed.assert_async().await;
        let old = cloud::queue::status(&db, &original_source).unwrap();
        assert!(old.cleared);
        assert!(!old.clear_outcome_unknown);
        assert!(!cloud::queue::retained_event(&db, &retained_id).unwrap());
        assert_eq!(
            db.query_row(
                "SELECT COUNT(*) FROM usage_cloud_queue WHERE original_text IS NOT NULL",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            2
        );
        assert_eq!(
            db.query_row(
                "SELECT COUNT(*) FROM usage_cloud_batches WHERE receipt_json IS NOT NULL",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            1
        );
        crate::usage_store::retention::configure(crate::usage_store::retention::Policy {
            schema_version: 1,
            days: 1,
            enabled: true,
            local_detail_loss_acknowledged: true,
            unselected_loss_acknowledged: false,
        })
        .unwrap();
        let retention = crate::usage_store::retention::apply(age).unwrap();
        assert_eq!(retention["deleted_records"], 0);
        assert_eq!(retention["unselected_loss_blocked_records"], 1);
        assert_eq!(retention["custody_blocked_records"], 1);
    }
    #[tokio::test]
    async fn exact_clear_marks_only_logical_copy_loss_and_preserves_pending_payloads() {
        let mut server = Server::new_async().await;
        let _home = Home::new(format!("{}/account", server.url()), true);
        enable(&mut server, "00000000-0000-4000-8000-000000000001").await;
        append();
        let selection = cloud::selected().unwrap().unwrap();
        let mut db = crate::usage_store::open_database(true).unwrap().unwrap();
        cloud::queue::capture(&mut db, &selection.source, cloud::project).unwrap();
        let cleared = server
            .mock("POST", "/account")
            .match_body(matcher("clear_confirm"))
            .with_body(response(
                json!({"protocol_version":1,"source":selection.source,"cleared":true,"data_generation":2,"purge_state":"pending"}),
            ))
            .expect(1)
            .create_async()
            .await;
        let result = cloud::clear(Some((
            CanonicalUuid::try_from("00000000-0000-4000-8000-000000000020".to_string()).unwrap(),
            Sha256Hash::of(b"exact approved preview"),
        )))
        .await
        .unwrap();
        assert_eq!(result["purge_state"], "pending");
        let status = cloud::local_status().unwrap();
        assert_eq!(status["cleared"], true);
        assert_eq!(status["pending"], 1);
        assert_eq!(status["enabled"], false);
        cleared.assert_async().await;
    }
}
