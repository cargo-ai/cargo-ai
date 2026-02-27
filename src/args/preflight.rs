use clap::{Arg, Command};

pub fn command() -> Command {
    Command::new("preflight")
        .about("Internal: test agent config file")
        .hide(true)
        .arg(
            Arg::new("profile")
                .long("profile")
                .short('P')
                .help("Use a saved connection profile instead of manual flags")
                .required(false)
                .value_name("PROFILE"),
        )
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
            Arg::new("url")
                .long("url")
                .help("Custom transformer server URL (HTTPS preferred)")
                .required(false)
                .value_name("URL"),
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
        .arg(
            Arg::new("prompt")
                .long("prompt")
                .short('p')
                .help("Prompt to provide to the agent at runtime")
                .value_name("TEXT")
                .num_args(1),
        )
}
