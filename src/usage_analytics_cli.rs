//! Finite CLI adapters for explicit cloud custody and local retention choices.
use crate::usage_analytics::{self as cloud, contract::*};
use clap::{Arg, ArgAction, ArgMatches, Command};
use serde::Serialize;
use serde_json::{json, Value};
fn parse_history_days(text: &str) -> Result<u32, String> {
    text.strip_suffix('d')
        .and_then(|value| value.parse::<u32>().ok())
        .filter(|days| *days > 0)
        .ok_or_else(|| "Use a positive day window, for example 30d".into())
}
fn json_flag(command: Command) -> Command {
    command.arg(Arg::new("json").long("json").action(ArgAction::SetTrue))
}
pub(crate) fn cloud_command() -> Command {
    Command::new("cloud")
        .about("Manage explicitly consented cloud analytics; disabled by default")
        .subcommand_required(true)
        .subcommand(json_flag(
            Command::new("status")
                .arg(Arg::new("remote").long("remote").action(ArgAction::SetTrue)),
        ))
        .subcommand(json_flag(
            Command::new("enable").arg(
                Arg::new("replace-environment")
                    .long("replace-environment")
                    .value_name("HOME_UUID"),
            ),
        ))
        .subcommand(json_flag(Command::new("disable")))
        .subcommand(json_flag(
            Command::new("sync")
                .about("Resume a fixed oldest-first sync job in a bounded pass")
                .arg(
                    Arg::new("include-history")
                        .long("include-history")
                        .value_name("Nd")
                        .value_parser(parse_history_days),
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
                        .action(ArgAction::SetTrue)
                        .required_if_eq("enabled", "on"),
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
        ("cloud", "clear") => leaf.get_flag("confirm"),
        _ => false,
    }
}
fn requires_remote(kind: &str, command: &str, leaf: &ArgMatches) -> bool {
    kind == "cloud"
        && (matches!(command, "enable" | "disable" | "sync" | "clear")
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
        ("cloud", "enable") => {
            cloud::enable(
                leaf.get_one::<String>("replace-environment")
                    .map(|text| CanonicalUuid::try_from(text.clone()).map_err(|e| e.to_string()))
                    .transpose()?,
            )
            .await?
        }
        ("cloud", "disable") => cloud::disable().await?,
        ("cloud", "sync") => {
            cloud::sync_with_history(leaf.get_one::<u32>("include-history").copied()).await?
        }
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
        ("retention", "status") => crate::usage_store::retention::status(cloud::now_ms())?,
        ("retention", "preview") => crate::usage_store::retention::preview(cloud::now_ms())?,
        ("retention", "configure") => {
            crate::usage_store::retention::configure(crate::usage_store::retention::Policy {
                schema_version: 2,
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
        && (result["enabled"] == false
            || result["outcome"] == "already_running"
            || result["outcome"] == "plan_limit_denied");
    let remote_effect = if !remote {
        Effect::Unapplied
    } else if inactive_sync {
        if result["policy_refreshed"] == true {
            Effect::ReadOnly
        } else {
            Effect::Unapplied
        }
    } else if command == "sync" {
        Effect::Requested
    } else if mutates(kind, command, leaf) {
        Effect::Applied
    } else {
        Effect::ReadOnly
    };
    let postconditions_verified =
        result["partial"] != true && (command != "sync" || result["outcome"] == "fully_synced");
    serde_json::to_value(Payload {
        schema_version: 2,
        operation: format!("usage {kind} {command}"),
        local: if result["outcome"] == "plan_limit_denied" {
            cloud::empty_local_status()
        } else {
            cloud::local_status()?
        },
        result,
        effects: Effects {
            local: if mutates(kind, command, leaf) && !local_read_only_sync {
                Effect::Applied
            } else {
                Effect::ReadOnly
            },
            remote: remote_effect,
        },
        postconditions_verified,
    })
    .map_err(|_| "Cannot encode usage result".into())
}
pub(crate) async fn run(args: &ArgMatches) -> bool {
    match execute(args).await {
        Ok(value) => {
            println!("{value}");
            value["result"]["partial"] != true
        }
        Err(error) => {
            println!(
                "{}",
                json!({"schema_version":2,"error":{"kind":"usage_maintenance_failed","message":error},"effects":{"local":"unknown","remote":"unknown"}})
            );
            false
        }
    }
}
pub(crate) async fn machine_run(
    args: &ArgMatches,
) -> Result<Value, crate::commands::machine::Failure> {
    let value = execute(args).await.map_err(|_|{
        let mutation=selection(args).is_ok_and(|(kind,command,leaf)|mutates(kind,command,leaf));
        let remote=selection(args).is_ok_and(|(kind,command,leaf)|requires_remote(kind,command,leaf));
        crate::commands::machine::Failure::new(if mutation{"operation.partial"}else{"usage.operation_failed"},"Usage maintenance did not complete; inspect preserved local state before retrying.").with_data(json!({"partial":mutation,"observed_local":cloud::local_status().ok(),"effects":Effects{local:if mutation{Effect::Unknown}else{Effect::ReadOnly},remote:if remote{Effect::Unknown}else{Effect::Unapplied}}}))
    })?;
    if value["result"]["partial"] == true {
        return Err(crate::commands::machine::Failure::new(
            "operation.partial",
            "Usage sync stopped after partial progress; inspect acknowledged and pending counts.",
        )
        .with_data(value));
    }
    Ok(value)
}
#[cfg(test)]
pub(crate) mod schema_tests;

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
