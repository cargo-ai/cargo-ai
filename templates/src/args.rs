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
        .arg(
            Arg::new("server")
                .long("server")
                .short('s')
                .help("Client Type - Ollama or OpenAI")
                .global(true)
        )
        .arg(
            Arg::new("model")
                .long("model")
                .short('m')
                .help("LLM model to use")
                .global(true)
        )
        .arg(
            Arg::new("token")
                .long("token")
                .help("API token")
                .global(true)
        )
        .arg(
            Arg::new("timeout_in_sec")
                .long("timeout_in_sec")
                .help("Client timeout request")
                .default_value("60")
                .global(true)
        )
        .arg(
            Arg::new("prompt")
                .long("prompt")
                .short('p')
                .help("Prompt to provide to the agent at runtime")
                .value_name("TEXT")
                .num_args(1)
        )
        .get_matches_from(args)
}
