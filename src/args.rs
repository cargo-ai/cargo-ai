//! Top-level CLI parser assembly for `cargo-ai`.
//!
//! This module composes command parsers from `src/args/*` and normalizes both
//! invocation forms: `cargo-ai ...` and `cargo ai ...`.
use clap::{ArgMatches, Command};

mod account;
mod hatch;
mod preflight;
mod profile;
mod shipyard;

fn cli_command(bin_name: &'static str) -> Command {
    Command::new("cargo-ai")
        .bin_name(bin_name)
        .version(env!("CARGO_PKG_VERSION"))
        .subcommand(Command::new("version").about("Print version information"))
        .subcommand(preflight::command())
        .subcommand(hatch::command())
        .subcommand(shipyard::command())
        .subcommand(profile::command())
        .subcommand(account::command())
}

/// Parses CLI arguments into clap matches.
pub fn build_cli() -> ArgMatches {
    // Collect raw process args so we can normalize cargo-subcommand mode.
    let mut args: Vec<String> = std::env::args().collect();

    let mut bin_name = "cargo-ai";
    // Check if running as a cargo subcommand, i.e. cargo ai
    if let Some(first_arg) = args.get(1) {
        if first_arg == "ai" {
            bin_name = "cargo ai";
            args.remove(1);
        }
    }

    cli_command(bin_name).get_matches_from(args)
}

#[cfg(test)]
mod tests {
    use super::cli_command;
    use clap::error::ErrorKind;

    #[test]
    fn account_mail_prefs_defaults_to_get_intent() {
        let matches = cli_command("cargo-ai")
            .try_get_matches_from(["cargo-ai", "account", "mail", "prefs"])
            .expect("prefs command should parse");

        let prefs_matches = matches
            .subcommand_matches("account")
            .and_then(|m| m.subcommand_matches("mail"))
            .and_then(|m| m.subcommand_matches("prefs"))
            .expect("prefs subcommand should be available");

        assert!(!prefs_matches.get_flag("disable_all"));
        assert!(!prefs_matches.get_flag("enable_all"));
    }

    #[test]
    fn account_mail_prefs_disable_parses() {
        let matches = cli_command("cargo-ai")
            .try_get_matches_from(["cargo-ai", "account", "mail", "prefs", "--disable-all"])
            .expect("disable-all form should parse");

        let prefs_matches = matches
            .subcommand_matches("account")
            .and_then(|m| m.subcommand_matches("mail"))
            .and_then(|m| m.subcommand_matches("prefs"))
            .expect("prefs subcommand should be available");

        assert!(prefs_matches.get_flag("disable_all"));
        assert!(!prefs_matches.get_flag("enable_all"));
    }

    #[test]
    fn account_mail_prefs_enable_parses() {
        let matches = cli_command("cargo-ai")
            .try_get_matches_from(["cargo-ai", "account", "mail", "prefs", "--enable-all"])
            .expect("enable-all form should parse");

        let prefs_matches = matches
            .subcommand_matches("account")
            .and_then(|m| m.subcommand_matches("mail"))
            .and_then(|m| m.subcommand_matches("prefs"))
            .expect("prefs subcommand should be available");

        assert!(prefs_matches.get_flag("enable_all"));
        assert!(!prefs_matches.get_flag("disable_all"));
    }

    #[test]
    fn account_mail_prefs_conflicting_flags_are_rejected() {
        let err = cli_command("cargo-ai")
            .try_get_matches_from([
                "cargo-ai",
                "account",
                "mail",
                "prefs",
                "--disable-all",
                "--enable-all",
            ])
            .expect_err("conflicting flags should fail parsing");

        assert_eq!(err.kind(), ErrorKind::ArgumentConflict);
    }

    #[test]
    fn hatch_check_flag_parses() {
        let matches = cli_command("cargo-ai")
            .try_get_matches_from(["cargo-ai", "hatch", "adder_check", "--check"])
            .expect("hatch --check should parse");

        let hatch_matches = matches
            .subcommand_matches("hatch")
            .expect("hatch subcommand should be available");

        assert!(hatch_matches.get_flag("check"));
    }
}
