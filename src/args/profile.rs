//! CLI parser definition for `cargo ai profile`.
use clap::{Arg, ArgAction, ArgGroup, Command};

/// Builds the `profile` command schema and nested subcommands.
pub fn command() -> Command {
    Command::new("profile")
        .about("Manage connection profiles")
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
                    Arg::new("auth")
                        .long("auth")
                        .help("Optional auth mode (default: none)")
                        .required(false)
                        .value_name("MODE")
                        .value_parser(["none", "api_key", "openai_account"]),
                )
                .arg(
                    Arg::new("url")
                        .long("url")
                        .help("Custom transformer server URL (HTTPS preferred)")
                        .required(false)
                        .value_name("URL"),
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
                        .action(ArgAction::SetTrue),
                ),
        )
        .subcommand(
            Command::new("set")
                .about("Update fields for an existing profile")
                .group(
                    ArgGroup::new("mutations")
                        .args([
                            "server",
                            "model",
                            "auth",
                            "url",
                            "clear_url",
                            "description",
                            "clear_description",
                            "token",
                            "stdin",
                            "env",
                            "clear_token",
                            "default",
                        ])
                        .required(true),
                )
                .group(
                    ArgGroup::new("url_update")
                        .args(["url", "clear_url"])
                        .multiple(false),
                )
                .group(
                    ArgGroup::new("description_update")
                        .args(["description", "clear_description"])
                        .multiple(false),
                )
                .group(
                    ArgGroup::new("token_source")
                        .args(["token", "stdin", "env", "clear_token"])
                        .multiple(false),
                )
                .arg(
                    Arg::new("name")
                        .help("Name of the profile to update")
                        .required(true)
                        .value_name("NAME"),
                )
                .arg(
                    Arg::new("server")
                        .long("server")
                        .short('s')
                        .help("Update server (e.g., openai or ollama)")
                        .required(false)
                        .value_name("SERVER"),
                )
                .arg(
                    Arg::new("model")
                        .long("model")
                        .short('m')
                        .help("Update model identifier (e.g., gpt-5.2, mistral)")
                        .required(false)
                        .value_name("MODEL"),
                )
                .arg(
                    Arg::new("auth")
                        .long("auth")
                        .help("Update auth mode")
                        .required(false)
                        .value_name("MODE")
                        .value_parser(["none", "api_key", "openai_account"]),
                )
                .arg(
                    Arg::new("url")
                        .long("url")
                        .help("Set custom transformer server URL")
                        .required(false)
                        .value_name("URL"),
                )
                .arg(
                    Arg::new("clear_url")
                        .long("clear-url")
                        .help("Remove custom transformer server URL")
                        .required(false)
                        .action(ArgAction::SetTrue),
                )
                .arg(
                    Arg::new("description")
                        .long("description")
                        .short('d')
                        .help("Set profile description")
                        .required(false)
                        .value_name("TEXT"),
                )
                .arg(
                    Arg::new("clear_description")
                        .long("clear-description")
                        .help("Remove profile description")
                        .required(false)
                        .action(ArgAction::SetTrue),
                )
                .arg(
                    Arg::new("token")
                        .long("token")
                        .help("Set API token from literal value")
                        .required(false)
                        .value_name("TOKEN")
                        .num_args(1),
                )
                .arg(
                    Arg::new("stdin")
                        .long("stdin")
                        .help("Set API token from standard input")
                        .required(false)
                        .action(ArgAction::SetTrue),
                )
                .arg(
                    Arg::new("env")
                        .long("env")
                        .help("Set API token from environment variable")
                        .required(false)
                        .value_name("ENV_VAR")
                        .num_args(1),
                )
                .arg(
                    Arg::new("clear_token")
                        .long("clear-token")
                        .help("Clear stored API token for this profile")
                        .required(false)
                        .action(ArgAction::SetTrue),
                )
                .arg(
                    Arg::new("default")
                        .long("default")
                        .help("Set this profile as the default")
                        .required(false)
                        .action(ArgAction::SetTrue),
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
