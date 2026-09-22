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
