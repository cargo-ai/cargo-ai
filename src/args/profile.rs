use clap::{Arg, Command};

pub fn command() -> Command {
    Command::new("profile")
        .about("Manage connection profiles in the Cargo-AI config file")
        .subcommand(Command::new("list").about("List all configured profiles"))
        .subcommand(
            Command::new("show")
                .about("Show detailed information for a specific profile")
                .arg(
                    Arg::new("name")
                        .help("Name of the profile to display")
                        .required(true)
                        .value_name("NAME"),
                ),
        )
        .subcommand(
            Command::new("add")
                .about("Add a new connection profile or overwrite an existing one")
                .arg(
                    Arg::new("name")
                        .help("Name of the profile to add or update")
                        .required(true)
                        .value_name("NAME"),
                )
                .arg(
                    Arg::new("server")
                        .long("server")
                        .short('s')
                        .help("LLM server (e.g., openai or ollama)")
                        .required(true)
                        .value_name("SERVER"),
                )
                .arg(
                    Arg::new("model")
                        .long("model")
                        .short('m')
                        .help("LLM model identifier (e.g., gpt-4o, mistral)")
                        .required(true)
                        .value_name("MODEL"),
                )
                .arg(
                    Arg::new("url")
                        .long("url")
                        .help("Custom transformer server URL (HTTPS preferred)")
                        .required(false)
                        .value_name("URL"),
                )
                .arg(
                    Arg::new("token")
                        .long("token")
                        .help("API token for the server")
                        .required(false)
                        .value_name("TOKEN"),
                )
                .arg(
                    Arg::new("description")
                        .long("description")
                        .short('d')
                        .help("Optional description for the profile")
                        .required(false)
                        .value_name("TEXT"),
                )
                .arg(
                    Arg::new("default")
                        .long("default")
                        .help("Set this profile as the default")
                        .action(clap::ArgAction::SetTrue),
                ),
        )
        .subcommand(
            Command::new("remove")
                .about("Remove an existing connection profile by name")
                .arg(
                    Arg::new("name")
                        .help("Name of the profile to remove")
                        .required(true)
                        .value_name("NAME"),
                ),
        )
}
