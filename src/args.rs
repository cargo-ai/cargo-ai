// clap - Command Line Arguement Parsing
use clap::{Arg, ArgMatches, Command};

pub fn build_cli() -> ArgMatches {
    // Collect the original command-line arguments
    let mut args: Vec<String> = std::env::args().collect();

    let mut bin_name = "cargo-ai";
    // Check if runing as a cargo subcommand, i.e. cargo ai
    if let Some(first_arg) = args.get(1) {
        if first_arg == "ai" {
            bin_name = "cargo ai";
            args.remove(1);
        }
    }

    Command::new("cargo-ai")
        .bin_name(bin_name)
        .version(env!("CARGO_PKG_VERSION"))
        .subcommand(
            Command::new("version")
                .about("Print version information")
        )
        .subcommand(
            Command::new("preflight")
                .about("Internal: test agent config file")
                .hide(true)
                    .arg(
                        Arg::new("profile")
                            .long("profile")
                            .short('P')
                            .help("Use a saved connection profile instead of manual flags")
                            .required(false)
                            .value_name("PROFILE")
                    )
                    .arg(
                        Arg::new("server")
                            .long("server")
                            .short('s')
                            .value_name("CLIENT")
                            .help("Client Type - Ollama or OpenAI")
                    )
                    .arg(
                        Arg::new("model")
                            .long("model")
                            .short('m')
                            .value_name("MODEL")
                            .help("LLM model to use")
                    )
                    .arg(
                        Arg::new("url")
                            .long("url")
                            .help("Custom transformer server URL (HTTPS preferred)")
                            .required(false)
                            .value_name("URL")
                    )
                    .arg(
                        Arg::new("token")
                            .long("token")
                            .value_name("TOKEN")
                            .help("API token"),
                    )
                    .arg(
                        Arg::new("timeout_in_sec")
                            .long("timeout_in_sec")
                            .value_name("SECONDS")
                            .help("Client timeout request")
                            .default_value("60")
                    )
                    .arg(
                        Arg::new("prompt")
                            .long("prompt")
                            .short('p')
                            .help("Prompt to provide to the agent at runtime")
                            .value_name("TEXT")
                            .num_args(1)
                    )
        )
        .subcommand(
            Command::new("hatch")
                .about("Hatch a new AI agent from a JSON config")
                .arg(
                    Arg::new("name")
                        .help("Name of the new agent project")
                        .required(true)
                )
                .arg(
                    Arg::new("config")
                        .long("config")
                        .short('c')
                        .help("Path to the agent configuration (local .json file or remote registry name)")
                        .value_name("FILE")
                        .num_args(1)
                )
        )
        .subcommand(
            Command::new("profile")
                .about("Manage connection profiles in the Cargo-AI config file")
                .subcommand(
                    Command::new("list")
                        .about("List all configured profiles")
                )
                .subcommand(
                    Command::new("show")
                        .about("Show detailed information for a specific profile")
                        .arg(
                            Arg::new("name")
                                .help("Name of the profile to display")
                                .required(true)
                                .value_name("NAME")
                        )
                )
                .subcommand(
                    Command::new("add")
                        .about("Add a new connection profile or overwrite an existing one")
                        .arg(
                            Arg::new("name")
                                .help("Name of the profile to add or update")
                                .required(true)
                                .value_name("NAME")
                        )
                        .arg(
                            Arg::new("server")
                                .long("server")
                                .short('s')
                                .help("LLM server (e.g., openai or ollama)")
                                .required(true)
                                .value_name("SERVER")
                        )
                        .arg(
                            Arg::new("model")
                                .long("model")
                                .short('m')
                                .help("LLM model identifier (e.g., gpt-4o, mistral)")
                                .required(true)
                                .value_name("MODEL")
                        )
                        .arg(
                            Arg::new("url")
                                .long("url")
                                .help("Custom transformer server URL (HTTPS preferred)")
                                .required(false)
                                .value_name("URL")
                        )
                        .arg(
                            Arg::new("token")
                                .long("token")
                                .help("API token for the server")
                                .required(false)
                                .value_name("TOKEN")
                        )
                        .arg(
                            Arg::new("description")
                                .long("description")
                                .short('d')
                                .help("Optional description for the profile")
                                .required(false)
                                .value_name("TEXT")
                        )
                        .arg(
                            Arg::new("default")
                                .long("default")
                                .help("Set this profile as the default")
                                .action(clap::ArgAction::SetTrue)
                        )
                )
                .subcommand(
                    Command::new("remove")
                        .about("Remove an existing connection profile by name")
                        .arg(
                            Arg::new("name")
                                .help("Name of the profile to remove")
                                .required(true)
                                .value_name("NAME")
                        )
                )
        )
        .subcommand(
            Command::new("account")
                .about("Manage account lifecycle")
                .subcommand(
                    Command::new("register")
                        .about("Register a new account by email")
                        .arg(
                            Arg::new("email")
                                .help("Email address to register")
                                .required(true)
                                .value_name("EMAIL")
                        )
                )
                .subcommand(
                    Command::new("confirm")
                        .about("Confirm an account using the temporary code")
                        .arg(
                            Arg::new("code")
                                .help("Temporary confirmation code from email")
                                .required(true)
                                .value_name("CODE")
                        )
                )
                .subcommand(
                    Command::new("status")
                        .about("Show account status")
                )
        )
        .get_matches_from(args)
}
