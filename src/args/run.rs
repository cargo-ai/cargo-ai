//! CLI parser definition for `cargo ai run`.
use clap::{Arg, Command};

/// Builds the `run` command schema.
pub fn command() -> Command {
    super::preflight::runtime_command("run", "Run an agent JSON definition without hatching").arg(
        Arg::new("config")
            .long("config")
            .short('c')
            .help("Local path to the agent .json configuration file")
            .value_name("FILE")
            .required(true)
            .num_args(1),
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn help_describes_config_flag() {
        let mut command = super::command();
        let mut help = Vec::new();
        command
            .write_long_help(&mut help)
            .expect("run help should render");
        let help = String::from_utf8(help).expect("help should be utf8");

        assert!(help.contains("--config <FILE>"));
        assert!(help.contains("Run an agent JSON definition without hatching"));
    }
}
