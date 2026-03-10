//! CLI parser definition for `cargo ai preflight`.
use clap::{Arg, ArgAction, Command};

/// Builds the `preflight` command schema.
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
            Arg::new("inference_timeout_in_sec")
                .long("inference-timeout-in-sec")
                .alias("timeout_in_sec")
                .value_name("SECONDS")
                .help("Maximum model request time in seconds for this agent (default: 60)")
                .value_parser(clap::value_parser!(u64)),
        )
        .arg(
            Arg::new("max_agent_depth")
                .long("max-agent-depth")
                .value_name("DEPTH")
                .help("Maximum nested child-agent depth for this invocation tree")
                .value_parser(clap::value_parser!(u32)),
        )
        .arg(
            Arg::new("max_runtime_in_sec")
                .long("max-runtime-in-sec")
                .value_name("SECONDS")
                .help(
                    "Maximum runtime in seconds for this agent and any child agents (default: 600)",
                )
                .value_parser(clap::value_parser!(u64).range(1..)),
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
        .arg(
            Arg::new("input_file")
                .long("input-file")
                .help("Local supported file path to provide to the agent at runtime")
                .value_name("PATH")
                .action(ArgAction::Append)
                .num_args(1),
        )
}

#[cfg(test)]
mod tests {
    #[test]
    fn help_describes_input_file_as_supported_file_path() {
        let mut command = super::command();
        let mut help = Vec::new();
        command
            .write_long_help(&mut help)
            .expect("preflight help should render");
        let help = String::from_utf8(help).expect("help should be utf8");

        assert!(help.contains("Local supported file path to provide to the agent at runtime"));
        assert!(!help.contains("Local PDF file path to provide to the agent at runtime"));
    }
}
