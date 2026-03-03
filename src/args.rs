//! Top-level CLI parser assembly for `cargo-ai`.
//!
//! This module composes command parsers from `src/args/*` and normalizes both
//! invocation forms: `cargo-ai ...` and `cargo ai ...`.
use clap::{Arg, ArgAction, ArgMatches, Command};

mod account;
mod hatch;
mod init;
mod new;
mod preflight;
mod profile;
mod shipyard;
mod version;

fn cli_command(bin_name: &'static str) -> Command {
    Command::new("cargo-ai")
        .bin_name(bin_name)
        .version(env!("CARGO_PKG_VERSION"))
        .arg(
            Arg::new("no_update_check")
                .long("no-update-check")
                .help("Skip update checks for this invocation")
                .global(true)
                .action(ArgAction::SetTrue),
        )
        .subcommand(version::command())
        .subcommand(preflight::command())
        .subcommand(hatch::command())
        .subcommand(init::command())
        .subcommand(new::command())
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
pub(crate) fn test_cli_command(bin_name: &'static str) -> Command {
    cli_command(bin_name)
}

#[cfg(test)]
mod tests {
    use super::cli_command;
    use clap::error::ErrorKind;

    #[test]
    fn version_supports_check_flag() {
        let matches = cli_command("cargo-ai")
            .try_get_matches_from(["cargo-ai", "version", "--check"])
            .expect("version --check should parse");

        let version = matches
            .subcommand_matches("version")
            .expect("version subcommand should be available");
        assert!(version.get_flag("check"));
        assert!(version.get_one::<String>("update_mode").is_none());
    }

    #[test]
    fn version_supports_update_mode_flag() {
        let matches = cli_command("cargo-ai")
            .try_get_matches_from(["cargo-ai", "version", "--update-mode", "off"])
            .expect("version --update-mode should parse");

        let version = matches
            .subcommand_matches("version")
            .expect("version subcommand should be available");
        assert_eq!(
            version.get_one::<String>("update_mode").map(String::as_str),
            Some("off")
        );
        assert!(!version.get_flag("check"));
    }

    #[test]
    fn version_rejects_conflicting_check_and_update_mode() {
        let err = cli_command("cargo-ai")
            .try_get_matches_from(["cargo-ai", "version", "--check", "--update-mode", "check"])
            .expect_err("version flag conflict should fail parsing");

        assert_eq!(err.kind(), ErrorKind::ArgumentConflict);
    }

    #[test]
    fn no_update_check_flag_is_global() {
        let matches = cli_command("cargo-ai")
            .try_get_matches_from(["cargo-ai", "--no-update-check", "version"])
            .expect("global no-update-check should parse");
        assert!(matches.get_flag("no_update_check"));

        let matches = cli_command("cargo-ai")
            .try_get_matches_from(["cargo-ai", "hatch", "demo", "--no-update-check"])
            .expect("global no-update-check after subcommand should parse");
        assert!(matches.get_flag("no_update_check"));
    }

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

    #[test]
    fn hatch_force_flag_parses_long_and_short() {
        let long_matches = cli_command("cargo-ai")
            .try_get_matches_from(["cargo-ai", "hatch", "adder_force_long", "--force"])
            .expect("hatch --force should parse");
        let long_hatch = long_matches
            .subcommand_matches("hatch")
            .expect("hatch subcommand should be available");
        assert!(long_hatch.get_flag("force"));

        let short_matches = cli_command("cargo-ai")
            .try_get_matches_from(["cargo-ai", "hatch", "adder_force_short", "-f"])
            .expect("hatch -f should parse");
        let short_hatch = short_matches
            .subcommand_matches("hatch")
            .expect("hatch subcommand should be available");
        assert!(short_hatch.get_flag("force"));
    }

    #[test]
    fn init_defaults_parse() {
        let matches = cli_command("cargo-ai")
            .try_get_matches_from(["cargo-ai", "init"])
            .expect("init should parse");

        let init = matches
            .subcommand_matches("init")
            .expect("init subcommand should be available");

        assert_eq!(
            init.get_one::<String>("path").map(String::as_str),
            Some(".")
        );
        assert_eq!(
            init.get_one::<String>("vcs").map(String::as_str),
            Some("git")
        );
        assert!(init.get_one::<String>("template").is_none());
    }

    #[test]
    fn new_requires_path_and_parses_template_vcs() {
        let matches = cli_command("cargo-ai")
            .try_get_matches_from([
                "cargo-ai",
                "new",
                "sample-agent",
                "--template",
                "codex",
                "--vcs",
                "none",
            ])
            .expect("new should parse");

        let new = matches
            .subcommand_matches("new")
            .expect("new subcommand should be available");

        assert_eq!(
            new.get_one::<String>("path").map(String::as_str),
            Some("sample-agent")
        );
        assert_eq!(
            new.get_one::<String>("template").map(String::as_str),
            Some("codex")
        );
        assert_eq!(
            new.get_one::<String>("vcs").map(String::as_str),
            Some("none")
        );
    }
}
