//! CLI parser definition for `cargo ai run`.
use clap::{Arg, ArgAction, ArgGroup, Command};

/// Builds the `run` command schema.
pub fn command() -> Command {
    super::runtime_common::runtime_command("run", "Run an agent JSON definition without hatching")
        .group(
            ArgGroup::new("run_definition_source")
                .args(["name", "config", "json", "stdin"])
                .required(true),
        )
        .arg(
            Arg::new("name")
                .help("Agent name or local .json config path (shorthand)")
                .conflicts_with_all(["config", "json", "stdin"]),
        )
        .arg(
            Arg::new("config")
                .long("config")
                .short('c')
                .help("Path to agent definition JSON file")
                .value_name("FILE")
                .conflicts_with_all(["name", "json", "stdin"])
                .num_args(1),
        )
        .arg(
            Arg::new("json")
                .long("json")
                .help("Agent definition JSON (raw JSON string)")
                .value_name("JSON")
                .conflicts_with_all(["name", "config", "stdin"])
                .num_args(1),
        )
        .arg(
            Arg::new("stdin")
                .long("stdin")
                .help("Read agent definition JSON from stdin")
                .conflicts_with_all(["name", "config", "json"])
                .action(ArgAction::SetTrue),
        )
        .after_help(
            "Definition sources:\n  - NAME or PATH: registry name or local .json shorthand\n  - --config <FILE>: agent definition JSON file\n  - --json <JSON>: raw agent definition JSON string\n  - --stdin: read agent definition JSON from stdin",
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
            .expect("run help should render");
        let help = String::from_utf8(help).expect("help should be utf8");

        assert!(help.contains("--config <FILE>"));
        assert!(help.contains("--json <JSON>"));
        assert!(help.contains("--stdin"));
        assert!(help.contains("[name]"));
        assert!(help.contains("Agent name or local .json config path (shorthand)"));
        assert!(help.contains("Run an agent JSON definition without hatching"));
        assert!(help.contains("Definition sources:"));
    }
}
