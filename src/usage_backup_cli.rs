//! CLI-only consent and maintenance controls over the shared backup implementation.
use crate::usage_backup;
use clap::{Arg, ArgAction, ArgMatches, Command};
use serde_json::{json, Value};
#[cfg(test)]
mod tests;
fn json_flag(command: Command) -> Command {
    command.arg(
        Arg::new("json")
            .long("json")
            .action(ArgAction::SetTrue)
            .help("Print the versioned JSON contract"),
    )
}
fn confirm(command: Command) -> Command {
    json_flag(
        command
            .arg(
                Arg::new("confirm")
                    .long("confirm")
                    .action(ArgAction::SetTrue)
                    .requires_all(["account-binding", "generation"]),
            )
            .arg(
                Arg::new("account-binding")
                    .long("account-binding")
                    .help("Exact opaque account binding returned by preview"),
            )
            .arg(
                Arg::new("generation")
                    .long("generation")
                    .value_parser(clap::value_parser!(u64).range(1..=i64::MAX as u64))
                    .help("Exact consent generation returned by preview"),
            ),
    )
}
fn expected(args: &ArgMatches) -> Option<usage_backup::Binding> {
    Some(usage_backup::Binding {
        account: args.get_one::<String>("account-binding")?.clone(),
        generation: *args.get_one::<u64>("generation")?,
    })
}
pub(crate) fn command() -> Command {
    Command::new("backup")
        .about("Explicit account backup and restore; disabled by default")
        .subcommand_required(true)
        .subcommand(json_flag(
            Command::new("status")
                .about("Inspect local queue state without network access")
                .arg(
                    Arg::new("remote")
                        .long("remote")
                        .action(ArgAction::SetTrue)
                        .help("Explicitly query the signed-in account"),
                ),
        ))
        .subcommand(json_flag(Command::new("enable").about(
            "Enable backup for future records under the signed-in account",
        )))
        .subcommand(json_flag(Command::new("disable").about(
            "Stop future uploads while preserving history and selections",
        )))
        .subcommand(confirm(
            Command::new("include-history")
                .about("Preview or explicitly select older local history")
                .arg(
                    Arg::new("through")
                        .long("through")
                        .value_parser(clap::value_parser!(i64).range(0..))
                        .required_if_eq("confirm", "true")
                        .help("Snapshot returned by the preview; required with --confirm"),
                ),
        ))
        .subcommand(json_flag(Command::new("sync").about(
            "Upload at most 10 batches in 30 seconds; stop on first failure",
        )))
        .subcommand(confirm(
            Command::new("restore")
                .about("Preview or import one stable cloud snapshot page")
                .arg(Arg::new("cursor").long("cursor").requires("confirm")),
        ))
        .subcommand(confirm(Command::new("delete").about(
            "Preview or delete cloud usage and revoke its upload generation",
        )))
}
pub(crate) async fn run(args: &ArgMatches) -> bool {
    match execute(args).await {
        Ok(result) => {
            println!("{}", json!({"schema_version":1,"backup":result}));
            true
        }
        Err(error) => {
            println!(
                "{}",
                json!({"schema_version":1,"error":{"kind":"usage_backup_failed","message":error}})
            );
            false
        }
    }
}
async fn execute(args: &ArgMatches) -> Result<Value, String> {
    let (command, args) = args.subcommand().ok_or("A backup command is required")?;
    match command {
        "status" => {
            if args.get_flag("remote") {
                Ok(
                    json!({"local":usage_backup::local_status()?,"remote":usage_backup::remote_status().await?}),
                )
            } else {
                usage_backup::local_status()
            }
        }
        "enable" => usage_backup::enable().await,
        "disable" => usage_backup::disable(),
        "include-history" => usage_backup::include_history(
            args.get_one::<i64>("through").copied(),
            expected(args),
            args.get_flag("confirm"),
        ),
        "sync" => usage_backup::sync().await,
        "restore" => {
            usage_backup::restore(
                args.get_one::<String>("cursor").cloned(),
                expected(args),
                args.get_flag("confirm"),
            )
            .await
        }
        "delete" => usage_backup::delete_cloud(expected(args), args.get_flag("confirm")).await,
        _ => Err("Unknown backup command".into()),
    }
}

/// New selection wraps existing payloads and retains remote/local failure facts.
pub(crate) async fn machine_run(
    args: &ArgMatches,
) -> Result<Value, crate::commands::machine::Failure> {
    use crate::commands::machine::Failure;
    let (command, _leaf) = args
        .subcommand()
        .ok_or_else(|| Failure::new("input.invalid", "A backup command is required."))?;
    if !matches!(
        command,
        "status" | "enable" | "disable" | "sync" | "include-history"
    ) {
        return Err(Failure::new(
            "contract.unsupported",
            "This backup command has no selected finite contract.",
        ));
    }
    let mutation = matches!(command, "enable" | "disable" | "sync" | "include-history");
    let result = match command {
        "enable" => usage_backup::enable_outcome().await.map_err(|error| {
            Failure::new(if error.effects.remote == "applied" {"operation.partial"} else {"backup.enable_failed"},
                "Backup enable did not complete; reconcile consent and local state before retrying.")
                .with_data(json!({"partial":error.effects.remote == "applied" || error.effects.uploaded_records > 0,"effects":error.effects.value()}))
        }),
        "disable" => usage_backup::disable_outcome().map_err(|error| {
            Failure::new(if error.effects.queue == "applied" {"operation.partial"} else {"backup.disable_failed"},
                "Backup disable did not complete; inspect consent and queue state before retrying.")
                .with_data(json!({"partial":error.effects.queue == "applied","effects":error.effects.value()}))
        }),
        "sync" => usage_backup::sync_outcome().await.map_err(|error| {
            Failure::new(if error.effects.remote == "applied" || error.effects.uploaded_records > 0 {"operation.partial"} else {"backup.sync_failed"},
                "Backup sync did not complete; reconcile remote acknowledgments and local queue before retrying.")
                .with_data(json!({"partial":error.effects.remote == "applied" || error.effects.uploaded_records > 0,"effects":error.effects.value()}))
        }),
        _ => execute(args).await.map_err(|_| Failure::new("backup.operation_failed", "The backup operation could not be completed.")
            .with_data(json!({"effects":{"local":if mutation {"unknown"} else {"unapplied"},"remote":"unapplied"}}))),
    }?;
    let local = usage_backup::local_status().map_err(|_| Failure::new("persistence.unverified", "The local backup state could not be verified.")
        .with_data(json!({"partial":mutation,"backup":result,"effects":{"local":if mutation {"unknown"} else {"unapplied"},"remote":if command == "enable" || command == "sync" && result["uploaded_records"].as_u64().unwrap_or(0) > 0 {"applied"} else {"unapplied"}}})))?;
    if command == "enable" && local["enabled"] != true
        || command == "disable" && local["enabled"] != false
    {
        return Err(Failure::new("persistence.unverified", "The saved backup consent differs from the requested state.")
            .with_data(json!({"partial":true,"backup":result,"local":local,"effects":{"local":"unknown","remote":if command == "enable" {"applied"} else {"unapplied"}}})));
    }
    Ok(
        json!({"backup":result,"local":local,"effects":{"local":if mutation && !(command == "sync" && result["enabled"] == false) {"applied"} else {"unapplied"},
        "remote":if command == "enable" || command == "sync" && result["uploaded_records"].as_u64().unwrap_or(0) > 0 {"applied"} else {"unapplied"}},"postconditions_verified":true}),
    )
}
