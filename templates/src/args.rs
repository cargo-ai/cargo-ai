// clap - Command Line Arguement Parsing
use clap::{Arg, ArgAction, ArgMatches, Command};

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
            Command::new("version").about("Print generated-agent provenance and sync status"),
        )
        .subcommand(
            Command::new("inspect")
                .about("Print generated-agent build provenance and embedded definition")
                .arg(
                    Arg::new("json")
                        .long("json")
                        .help("Emit inspect output as pretty JSON")
                        .action(ArgAction::SetTrue),
                ),
        )
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
            Arg::new("url")
                .long("url")
                .help("Custom transformer server URL (HTTPS preferred)")
                .global(true)
                .value_name("URL")
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
            Arg::new("max_agent_depth")
                .long("max-agent-depth")
                .help("Maximum nested child-agent depth for this invocation tree")
                .value_name("DEPTH")
                .value_parser(clap::value_parser!(u32))
                .global(true)
        )
        .arg(
            Arg::new("input_text")
                .long("input-text")
                .help("Text input to provide to the agent at runtime")
                .value_name("TEXT")
                .action(ArgAction::Append)
                .num_args(1),
        )
        .arg(
            Arg::new("input_url")
                .long("input-url")
                .help("URL input to fetch as text at runtime")
                .value_name("URL")
                .action(ArgAction::Append)
                .num_args(1),
        )
        .arg(
            Arg::new("input_image")
                .long("input-image")
                .help("Local image path to provide to the agent at runtime")
                .value_name("PATH")
                .action(ArgAction::Append)
                .num_args(1),
        )
        .get_matches_from(args)
}
