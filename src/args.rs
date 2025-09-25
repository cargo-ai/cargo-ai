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
        .subcommand(
            Command::new("preflight")
                .about("Internal: test agent config file")
                .hide(true)
                    .arg(
                        Arg::new("server")
                            .long("server")
                            .short('s')
                            .value_name("CLIENT")
                            .help("Client Type - Ollama or OpenAI"),
                    )
                    .arg(
                        Arg::new("model")
                            .long("model")
                            .short('m')
                            .value_name("MODEL")
                            .help("LLM model to use"),
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
                            .default_value("60"),
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
                        .help("Path to the agent configuration file (JSON format)")
                        .value_name("FILE")
                        .num_args(1)
                )
        )
        .get_matches_from(args)
}
