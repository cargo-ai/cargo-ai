//! Shared runtime CLI flag builder for interpreted execution commands.
use clap::{Arg, ArgAction, Command};

pub(crate) fn runtime_command(name: &'static str, about: &'static str) -> Command {
    Command::new(name)
        .about(about)
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
                .help("Provider - Anthropic, Ollama, or OpenAI"),
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
            Arg::new("max_output_tokens")
                .long("max-output-tokens")
                .value_name("TOKENS")
                .help("Maximum provider output tokens for this invocation")
                .value_parser(clap::value_parser!(u32).range(1..)),
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
            Arg::new("action_execution")
                .long("action-execution")
                .value_name("MODE")
                .help("Force top-level actions to run sequentially for this invocation tree")
                .value_parser(["sequential"]),
        )
        .arg(
            Arg::new("render_mode")
                .long("render-mode")
                .value_name("MODE")
                .help(
                    "Terminal render mode for action progress (auto, live, append-only) [default: auto]",
                )
                .value_parser(["auto", "live", "append-only"]),
        )
        .arg(
            Arg::new("usage_log")
                .long("usage-log")
                .value_name("PATH")
                .help("Write Cargo AI usage events as newline-delimited JSON (.ndjson/.jsonl)")
                .required(false),
        )
        .arg(
            Arg::new("input_mode")
                .long("input-mode")
                .value_name("MODE")
                .help(
                    "How runtime input flags combine with baked inputs: replace, append, or prepend",
                )
                .value_parser(["replace", "append", "prepend"]),
        )
        .arg(
            Arg::new("run_var")
                .long("run-var")
                .help("Runtime variable assignment declared in runtime_vars (repeatable: name=value)")
                .value_name("NAME=VALUE")
                .action(ArgAction::Append)
                .num_args(1),
        )
        .arg(
            Arg::new("input_override")
                .long("input-override")
                .help("Override a declared named top-level input for this invocation (repeatable: name=value)")
                .value_name("NAME=VALUE")
                .action(ArgAction::Append)
                .num_args(1),
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
        .arg(
            Arg::new("ignore_tools")
                .long("ignore-tools")
                .help("Skip upfront tool contract checks and fail only if a tool step is reached")
                .action(ArgAction::SetTrue),
        )
}

#[cfg(test)]
mod tests {
    #[test]
    fn help_describes_input_file_as_supported_file_path() {
        let mut command = super::runtime_command("runtime-test", "Runtime test command");
        let mut help = Vec::new();
        command
            .write_long_help(&mut help)
            .expect("runtime help should render");
        let help = String::from_utf8(help).expect("help should be utf8");

        assert!(help.contains("Local supported file path to provide to the agent at runtime"));
        assert!(!help.contains("Local PDF file path to provide to the agent at runtime"));
    }

    #[test]
    fn help_describes_input_mode_values() {
        let mut command = super::runtime_command("runtime-test", "Runtime test command");
        let mut help = Vec::new();
        command
            .write_long_help(&mut help)
            .expect("runtime help should render");
        let help = String::from_utf8(help).expect("help should be utf8");

        assert!(help.contains("--input-mode <MODE>"));
        assert!(help.contains(
            "How runtime input flags combine with baked inputs: replace, append, or prepend"
        ));
    }

    #[test]
    fn help_describes_action_execution_override() {
        let mut command = super::runtime_command("runtime-test", "Runtime test command");
        let mut help = Vec::new();
        command
            .write_long_help(&mut help)
            .expect("runtime help should render");
        let help = String::from_utf8(help).expect("help should be utf8");

        assert!(help.contains("--action-execution <MODE>"));
        assert!(
            help.contains("Force top-level actions to run sequentially for this invocation tree")
        );
    }

    #[test]
    fn help_describes_render_mode_override() {
        let mut command = super::runtime_command("runtime-test", "Runtime test command");
        let mut help = Vec::new();
        command
            .write_long_help(&mut help)
            .expect("runtime help should render");
        let help = String::from_utf8(help).expect("help should be utf8");

        assert!(help.contains("--render-mode <MODE>"));
        assert!(help.contains(
            "Terminal render mode for action progress (auto, live, append-only) [default: auto]"
        ));
    }

    #[test]
    fn help_describes_usage_log_path() {
        let mut command = super::runtime_command("runtime-test", "Runtime test command");
        let mut help = Vec::new();
        command
            .write_long_help(&mut help)
            .expect("runtime help should render");
        let help = String::from_utf8(help).expect("help should be utf8");

        assert!(help.contains("--usage-log <PATH>"));
        assert!(help.contains("newline-delimited JSON"));
    }

    #[test]
    fn help_describes_run_var_assignments() {
        let mut command = super::runtime_command("runtime-test", "Runtime test command");
        let mut help = Vec::new();
        command
            .write_long_help(&mut help)
            .expect("runtime help should render");
        let help = String::from_utf8(help).expect("help should be utf8");

        assert!(help.contains("--run-var <NAME=VALUE>"));
        assert!(help.contains(
            "Runtime variable assignment declared in runtime_vars (repeatable: name=value)"
        ));
    }

    #[test]
    fn help_describes_input_override_assignments() {
        let mut command = super::runtime_command("runtime-test", "Runtime test command");
        let mut help = Vec::new();
        command
            .write_long_help(&mut help)
            .expect("runtime help should render");
        let help = String::from_utf8(help).expect("help should be utf8");

        assert!(help.contains("--input-override <NAME=VALUE>"));
        assert!(help.contains("Override a declared named top-level input for this invocation"));
    }
}
