//! CLI parser definition for `cargo ai hatch`.
use clap::{Arg, ArgAction, ArgGroup, Command};

/// Builds the `hatch` command schema.
pub fn command() -> Command {
    Command::new("hatch")
        .about("Hatch a new AI agent from an agent definition JSON source")
        .group(
            ArgGroup::new("explicit_definition_source")
                .args(["config", "json", "stdin"]),
        )
        .arg(
            Arg::new("name")
                .help(
                    "Agent name (or local .json config path shorthand when no explicit definition source flag is used)",
                )
                .required(true),
        )
        .arg(
            Arg::new("check")
                .long("check")
                .help("Validate scaffold and compile path with `cargo check` (no binary export)")
                .required(false)
                .action(clap::ArgAction::SetTrue),
        )
        .arg(
            Arg::new("config")
                .long("config")
                .short('c')
                .help("Path to agent definition JSON file")
                .value_name("FILE")
                .num_args(1),
        )
        .arg(
            Arg::new("json")
                .long("json")
                .help("Agent definition JSON (raw JSON string)")
                .value_name("JSON")
                .num_args(1),
        )
        .arg(
            Arg::new("stdin")
                .long("stdin")
                .help("Read agent definition JSON from stdin")
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("target")
                .long("target")
                .help("Rust target triple to pass through to cargo build/check")
                .value_name("TRIPLE")
                .num_args(1),
        )
        .arg(
            Arg::new("output_dir")
                .long("output-dir")
                .help(
                    "Destination directory for the exported binary (defaults to current directory)",
                )
                .value_name("DIR")
                .num_args(1),
        )
        .arg(
            Arg::new("force")
                .long("force")
                .short('f')
                .help("Overwrite existing output binary and replace any kept internal workspace")
                .required(false)
                .action(clap::ArgAction::SetTrue),
        )
        .arg(
            Arg::new("keep_project")
                .long("keep-project")
                .help("Preserve the internal hatched project workspace for inspection")
                .required(false)
                .action(clap::ArgAction::SetTrue),
        )
        .arg(
            Arg::new("ignore_tools")
                .long("ignore-tools")
                .help("Skip upfront tool contract checks while hatching or checking")
                .required(false)
                .action(clap::ArgAction::SetTrue),
        )
        .after_help(
            "Definition sources:\n  - NAME by itself: agent name, registry name, or local .json shorthand\n  - --config <FILE>: agent definition JSON file\n  - --json <JSON>: raw agent definition JSON string\n  - --stdin: read agent definition JSON from stdin\n\nWhen --config, --json, or --stdin is used, positional NAME is always the local output/project name.",
        )
}

#[cfg(test)]
mod tests {
    #[test]
    fn help_describes_definition_source_flags() {
        let mut command = super::command();
        let mut help = Vec::new();
        command
            .write_long_help(&mut help)
            .expect("hatch help should render");
        let help = String::from_utf8(help).expect("help should be utf8");

        assert!(help.contains("--config <FILE>"));
        assert!(help.contains("--json <JSON>"));
        assert!(help.contains("--stdin"));
        assert!(help.contains("Definition sources:"));
        assert!(help.contains("When --config, --json, or --stdin is used"));
    }
}
