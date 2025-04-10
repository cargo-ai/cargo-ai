use clap::{Arg, Command, ArgMatches};

pub fn build_cli() -> ArgMatches {
    Command::new("cargo-ai")
        .arg(
            Arg::new("server")
                .long("server")
                .short('s')
                .help("Client Type - Ollama or OpenAI")
                .required(true),
        )
        .arg(
            Arg::new("model")
                .long("model")
                .short('m')
                .help("LLM model to use")
                .required(true),
        )
        .arg(
            Arg::new("token")
                .long("token")
                .help("API token")
        )
        .arg(
            Arg::new("timeout_in_sec")
                .long("timeout_in_sec")
                .help("Client timeout request")
                .default_value("60"),
        )
        .get_matches()
}