use clap::{Arg, ArgAction, Command};
fn json(command: Command) -> Command {
    command.arg(
        Arg::new("json")
            .long("json")
            .action(ArgAction::SetTrue)
            .help("Print the versioned machine-readable contract"),
    )
}
fn selection(command: Command) -> Command {
    command.args(
        [
            "after",
            "before",
            "profile",
            "provider",
            "model",
            "resolved-model",
        ]
        .map(|name| Arg::new(name).long(name).value_name("VALUE")),
    )
}
fn page(command: Command) -> Command {
    selection(json(command))
        .arg(
            Arg::new("limit")
                .long("limit")
                .default_value("100")
                .value_parser(clap::value_parser!(u32).range(1..=1000)),
        )
        .arg(
            Arg::new("cursor")
                .long("cursor")
                .help("Continue a snapshot page using the returned opaque cursor"),
        )
}
pub(super) fn command() -> Command {
    Command::new("usage")
        .subcommand(crate::usage_backup_cli::command())
        .about("Inspect and manage automatic local usage metadata")
        .subcommand_required(true)
        .subcommand(json(
            Command::new("settings")
                .about("Inspect settings or change collection without deleting history")
                .arg(
                    Arg::new("tracking")
                        .long("tracking")
                        .value_parser(["on", "off"]),
                ),
        ))
        .subcommand(page(
            Command::new("runs").about("List root runs and their child relationships"),
        ))
        .subcommand(page(
            Command::new("show")
                .about("Show committed run events")
                .arg(Arg::new("run-id").required(true)),
        ))
        .subcommand(selection(json(Command::new("summary").about(
            "Aggregate known usage facts with coverage and operational statistics",
        ))))
        .subcommand(
            page(Command::new("export").about("Export a page of immutable metadata facts")).arg(
                Arg::new("format")
                    .long("format")
                    .default_value("ndjson")
                    .value_parser(["ndjson"]),
            ),
        )
        .subcommand(json(
            Command::new("delete")
                .about("Preview or explicitly delete local history; remote copies remain")
                .arg(
                    Arg::new("before")
                        .long("before")
                        .conflicts_with("all")
                        .required_unless_present("all"),
                )
                .arg(Arg::new("all").long("all").action(ArgAction::SetTrue))
                .arg(
                    Arg::new("confirm")
                        .long("confirm")
                        .action(ArgAction::SetTrue),
                ),
        ))
}
