//! Top-level CLI parser assembly for `cargo-ai`.
//!
//! This module composes command parsers from `src/args/*` and normalizes both
//! invocation forms: `cargo-ai ...` and `cargo ai ...`.
use clap::{Arg, ArgAction, ArgMatches, Command};

mod account;
mod add;
mod auth;
mod credentials;
#[cfg(feature = "developer-tools")]
mod hatch;
mod init;
mod new;
mod profile;
mod run;
pub(crate) mod runtime_common;
mod version;

#[cfg(test)]
fn developer_tools_enabled() -> bool {
    cfg!(feature = "developer-tools")
}

fn cli_command(bin_name: &'static str) -> Command {
    let command = Command::new("cargo-ai")
        .bin_name(bin_name)
        .version(env!("CARGO_PKG_VERSION"))
        .arg(
            Arg::new("no_update_check")
                .long("no-update-check")
                .help("Skip update checks for this invocation")
                .global(true)
                .action(ArgAction::SetTrue),
        )
        .subcommand(run::command());

    #[cfg(feature = "developer-tools")]
    let command = command.subcommand(hatch::command());

    command
        .subcommand(add::command())
        .subcommand(profile::command())
        .subcommand(auth::command())
        .subcommand(credentials::command())
        .subcommand(account::command())
        .subcommand(version::command())
        .subcommand(new::command())
        .subcommand(init::command())
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
    use super::{cli_command, developer_tools_enabled};
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
    fn subcommand_order_prioritizes_visible_core_workflows() {
        let command = cli_command("cargo-ai");
        let names: Vec<String> = command
            .get_subcommands()
            .map(|subcommand| subcommand.get_name().to_string())
            .collect();

        let index_of = |name: &str| {
            names
                .iter()
                .position(|candidate| candidate == name)
                .expect("expected subcommand to exist")
        };

        if developer_tools_enabled() {
            assert!(index_of("run") < index_of("hatch"));
            assert!(index_of("hatch") < index_of("profile"));
            assert!(index_of("hatch") < index_of("add"));
        }
        assert!(index_of("add") < index_of("profile"));
        assert!(index_of("profile") < index_of("auth"));
        assert!(index_of("auth") < index_of("credentials"));
        assert!(index_of("credentials") < index_of("account"));
        assert!(index_of("account") < index_of("version"));
    }

    #[test]
    fn top_level_help_shows_add_and_hides_scaffold_commands() {
        let mut command = cli_command("cargo-ai");
        let mut help = Vec::new();
        command
            .write_long_help(&mut help)
            .expect("top-level help should render");
        let help = String::from_utf8(help).expect("help should be utf8");

        assert!(help.contains("\n  run"));
        assert!(help.contains("\n  add"));
        assert!(!help.contains("\n  new"));
        assert!(!help.contains("\n  init"));
        if developer_tools_enabled() {
            assert!(help.contains("\n  hatch"));
        } else {
            assert!(!help.contains("\n  hatch"));
        }
    }

    #[test]
    fn preflight_subcommand_is_removed() {
        let err = cli_command("cargo-ai")
            .try_get_matches_from(["cargo-ai", "preflight"])
            .expect_err("preflight should no longer parse");

        assert_eq!(err.kind(), ErrorKind::InvalidSubcommand);
    }

    #[test]
    fn run_requires_name_or_config() {
        let err = cli_command("cargo-ai")
            .try_get_matches_from(["cargo-ai", "run"])
            .expect_err("missing run target should fail parsing");

        assert_eq!(err.kind(), ErrorKind::MissingRequiredArgument);
    }

    #[test]
    fn run_accepts_positional_name() {
        let matches = cli_command("cargo-ai")
            .try_get_matches_from(["cargo-ai", "run", "adder_test"])
            .expect("run bare name should parse");

        let run = matches
            .subcommand_matches("run")
            .expect("run subcommand should be available");
        assert_eq!(
            run.get_one::<String>("name").map(String::as_str),
            Some("adder_test")
        );
        assert!(run.get_one::<String>("config").is_none());
    }

    #[test]
    fn run_rejects_positional_name_with_config_flag() {
        let err = cli_command("cargo-ai")
            .try_get_matches_from([
                "cargo-ai",
                "run",
                "adder_test",
                "--config",
                "./adder_test.json",
            ])
            .expect_err("run should reject redundant positional and config inputs");

        assert_eq!(err.kind(), ErrorKind::ArgumentConflict);
    }

    #[test]
    fn add_guidance_style_parses() {
        let matches = cli_command("cargo-ai")
            .try_get_matches_from(["cargo-ai", "add", "guidance", "--style", "codex"])
            .expect("add guidance --style codex should parse");

        let guidance_matches = matches
            .subcommand_matches("add")
            .and_then(|m| m.subcommand_matches("guidance"))
            .expect("guidance subcommand should be available");

        assert_eq!(
            guidance_matches
                .get_one::<String>("style")
                .map(String::as_str),
            Some("codex")
        );
    }

    #[test]
    fn add_guidance_requires_style() {
        let err = cli_command("cargo-ai")
            .try_get_matches_from(["cargo-ai", "add", "guidance"])
            .expect_err("missing --style should fail parsing");

        assert_eq!(err.kind(), ErrorKind::MissingRequiredArgument);
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
            .try_get_matches_from(["cargo-ai", "run", "demo", "--no-update-check"])
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

    #[cfg(feature = "developer-tools")]
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

    #[cfg(feature = "developer-tools")]
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

    #[cfg(feature = "developer-tools")]
    #[test]
    fn hatch_keep_project_flag_parses() {
        let matches = cli_command("cargo-ai")
            .try_get_matches_from(["cargo-ai", "hatch", "adder_keep", "--keep-project"])
            .expect("hatch --keep-project should parse");

        let hatch_matches = matches
            .subcommand_matches("hatch")
            .expect("hatch subcommand should be available");

        assert!(hatch_matches.get_flag("keep_project"));
    }

    #[cfg(feature = "developer-tools")]
    #[test]
    fn hatch_target_flag_parses() {
        let matches = cli_command("cargo-ai")
            .try_get_matches_from([
                "cargo-ai",
                "hatch",
                "adder_target",
                "--target",
                "x86_64-pc-windows-msvc",
            ])
            .expect("hatch --target should parse");

        let hatch_matches = matches
            .subcommand_matches("hatch")
            .expect("hatch subcommand should be available");

        assert_eq!(
            hatch_matches
                .get_one::<String>("target")
                .map(String::as_str),
            Some("x86_64-pc-windows-msvc")
        );
    }

    #[cfg(feature = "developer-tools")]
    #[test]
    fn hatch_output_dir_flag_parses() {
        let matches = cli_command("cargo-ai")
            .try_get_matches_from([
                "cargo-ai",
                "hatch",
                "adder_target",
                "--output-dir",
                "./dist",
            ])
            .expect("hatch --output-dir should parse");

        let hatch_matches = matches
            .subcommand_matches("hatch")
            .expect("hatch subcommand should be available");

        assert_eq!(
            hatch_matches
                .get_one::<String>("output_dir")
                .map(String::as_str),
            Some("./dist")
        );
    }

    #[cfg(feature = "developer-tools")]
    #[test]
    fn account_agents_hatch_parses_agent_output_dir_check_force_keep_project_and_target_flags() {
        let matches = cli_command("cargo-ai")
            .try_get_matches_from([
                "cargo-ai",
                "account",
                "agents",
                "hatch",
                "weather_local",
                "--check",
                "--definition-path",
                "/team/ops",
                "--agent",
                "weather_remote",
                "--target",
                "x86_64-pc-windows-msvc",
                "--output-dir",
                "./dist",
                "--force",
                "--keep-project",
            ])
            .expect("account agents hatch flags should parse");

        let hatch_matches = matches
            .subcommand_matches("account")
            .and_then(|m| m.subcommand_matches("agents"))
            .and_then(|m| m.subcommand_matches("hatch"))
            .expect("account agents hatch should be available");

        assert_eq!(
            hatch_matches.get_one::<String>("name").map(String::as_str),
            Some("weather_local")
        );
        assert_eq!(
            hatch_matches.get_one::<String>("agent").map(String::as_str),
            Some("weather_remote")
        );
        assert_eq!(
            hatch_matches
                .get_one::<String>("definition_path")
                .map(String::as_str),
            Some("/team/ops")
        );
        assert_eq!(
            hatch_matches
                .get_one::<String>("target")
                .map(String::as_str),
            Some("x86_64-pc-windows-msvc")
        );
        assert_eq!(
            hatch_matches
                .get_one::<String>("output_dir")
                .map(String::as_str),
            Some("./dist")
        );
        assert!(hatch_matches.get_flag("check"));
        assert!(hatch_matches.get_flag("force"));
        assert!(hatch_matches.get_flag("keep_project"));
    }

    #[cfg(feature = "developer-tools")]
    #[test]
    fn account_hatch_alias_parses_agent_output_dir_check_force_keep_project_and_target_flags() {
        let matches = cli_command("cargo-ai")
            .try_get_matches_from([
                "cargo-ai",
                "account",
                "hatch",
                "weather_local",
                "--check",
                "--definition-path",
                "/team/ops",
                "--agent",
                "weather_remote",
                "--target",
                "x86_64-pc-windows-msvc",
                "--output-dir",
                "./dist",
                "--force",
                "--keep-project",
            ])
            .expect("account hatch alias flags should parse");

        let hatch_matches = matches
            .subcommand_matches("account")
            .and_then(|m| m.subcommand_matches("hatch"))
            .expect("account hatch alias should be available");

        assert_eq!(
            hatch_matches.get_one::<String>("name").map(String::as_str),
            Some("weather_local")
        );
        assert_eq!(
            hatch_matches.get_one::<String>("agent").map(String::as_str),
            Some("weather_remote")
        );
        assert_eq!(
            hatch_matches
                .get_one::<String>("definition_path")
                .map(String::as_str),
            Some("/team/ops")
        );
        assert_eq!(
            hatch_matches
                .get_one::<String>("target")
                .map(String::as_str),
            Some("x86_64-pc-windows-msvc")
        );
        assert_eq!(
            hatch_matches
                .get_one::<String>("output_dir")
                .map(String::as_str),
            Some("./dist")
        );
        assert!(hatch_matches.get_flag("check"));
        assert!(hatch_matches.get_flag("force"));
        assert!(hatch_matches.get_flag("keep_project"));
    }

    #[test]
    fn account_agents_run_parses_runtime_flags_and_source_selectors() {
        let matches = cli_command("cargo-ai")
            .try_get_matches_from([
                "cargo-ai",
                "account",
                "agents",
                "run",
                "weather_test",
                "--owner-handle",
                "alice",
                "--definition-path",
                "/team/ops",
                "--profile",
                "my_profile",
                "--input-text",
                "hello",
                "--run-var",
                "month=04",
            ])
            .expect("account agents run should parse");

        let run_matches = matches
            .subcommand_matches("account")
            .and_then(|m| m.subcommand_matches("agents"))
            .and_then(|m| m.subcommand_matches("run"))
            .expect("account agents run should be available");

        assert_eq!(
            run_matches.get_one::<String>("name").map(String::as_str),
            Some("weather_test")
        );
        assert_eq!(
            run_matches
                .get_one::<String>("owner_handle")
                .map(String::as_str),
            Some("alice")
        );
        assert_eq!(
            run_matches
                .get_one::<String>("definition_path")
                .map(String::as_str),
            Some("/team/ops")
        );
        assert_eq!(
            run_matches.get_one::<String>("profile").map(String::as_str),
            Some("my_profile")
        );
        assert_eq!(
            run_matches
                .get_many::<String>("input_text")
                .expect("input text should parse")
                .map(String::as_str)
                .collect::<Vec<_>>(),
            vec!["hello"]
        );
        assert_eq!(
            run_matches
                .get_many::<String>("run_var")
                .expect("run vars should parse")
                .map(String::as_str)
                .collect::<Vec<_>>(),
            vec!["month=04"]
        );
    }

    #[test]
    fn account_run_alias_parses_runtime_flags_and_source_selectors() {
        let matches = cli_command("cargo-ai")
            .try_get_matches_from([
                "cargo-ai",
                "account",
                "run",
                "weather_test",
                "--owner-handle",
                "alice",
                "--definition-path",
                "/team/ops",
                "--server",
                "ollama",
                "--model",
                "mistral",
            ])
            .expect("account run alias should parse");

        let run_matches = matches
            .subcommand_matches("account")
            .and_then(|m| m.subcommand_matches("run"))
            .expect("account run alias should be available");

        assert_eq!(
            run_matches.get_one::<String>("name").map(String::as_str),
            Some("weather_test")
        );
        assert_eq!(
            run_matches
                .get_one::<String>("owner_handle")
                .map(String::as_str),
            Some("alice")
        );
        assert_eq!(
            run_matches
                .get_one::<String>("definition_path")
                .map(String::as_str),
            Some("/team/ops")
        );
        assert_eq!(
            run_matches.get_one::<String>("server").map(String::as_str),
            Some("ollama")
        );
        assert_eq!(
            run_matches.get_one::<String>("model").map(String::as_str),
            Some("mistral")
        );
    }

    #[test]
    fn account_agents_run_rejects_hatch_only_agent_flag() {
        let err = cli_command("cargo-ai")
            .try_get_matches_from([
                "cargo-ai",
                "account",
                "agents",
                "run",
                "weather_test",
                "--agent",
                "weather_remote",
            ])
            .expect_err("--agent should be rejected for account agents run");

        assert_eq!(err.kind(), ErrorKind::UnknownArgument);
    }

    #[test]
    fn credentials_store_set_parses_mode_and_flags() {
        let matches = cli_command("cargo-ai")
            .try_get_matches_from([
                "cargo-ai",
                "credentials",
                "store",
                "set",
                "keychain",
                "--migrate",
                "--yes",
            ])
            .expect("credentials store set should parse");

        let set_matches = matches
            .subcommand_matches("credentials")
            .and_then(|m| m.subcommand_matches("store"))
            .and_then(|m| m.subcommand_matches("set"))
            .expect("set subcommand should be available");

        assert_eq!(
            set_matches.get_one::<String>("mode").map(String::as_str),
            Some("keychain")
        );
        assert!(set_matches.get_flag("migrate"));
        assert!(set_matches.get_flag("yes"));
    }

    #[test]
    fn credentials_store_dry_run_requires_migrate() {
        let err = cli_command("cargo-ai")
            .try_get_matches_from([
                "cargo-ai",
                "credentials",
                "store",
                "set",
                "file",
                "--dry-run",
            ])
            .expect_err("dry-run without migrate should fail parsing");

        assert_eq!(err.kind(), ErrorKind::MissingRequiredArgument);
    }

    #[cfg(feature = "developer-tools")]
    #[test]
    fn account_agents_hatch_supports_short_force_flag() {
        let matches = cli_command("cargo-ai")
            .try_get_matches_from([
                "cargo-ai",
                "account",
                "agents",
                "hatch",
                "weather_test",
                "-f",
            ])
            .expect("account agents hatch -f should parse");

        let hatch_matches = matches
            .subcommand_matches("account")
            .and_then(|m| m.subcommand_matches("agents"))
            .and_then(|m| m.subcommand_matches("hatch"))
            .expect("account agents hatch should be available");

        assert!(hatch_matches.get_flag("force"));
    }

    #[cfg(feature = "developer-tools")]
    #[test]
    fn account_hatch_alias_supports_short_force_flag() {
        let matches = cli_command("cargo-ai")
            .try_get_matches_from(["cargo-ai", "account", "hatch", "weather_test", "-f"])
            .expect("account hatch alias -f should parse");

        let hatch_matches = matches
            .subcommand_matches("account")
            .and_then(|m| m.subcommand_matches("hatch"))
            .expect("account hatch alias should be available");

        assert!(hatch_matches.get_flag("force"));
    }

    #[cfg(feature = "developer-tools")]
    #[test]
    fn account_agents_hatch_rejects_removed_name_flag() {
        let err = cli_command("cargo-ai")
            .try_get_matches_from([
                "cargo-ai",
                "account",
                "agents",
                "hatch",
                "--name",
                "weather_test",
            ])
            .expect_err("--name should be rejected for account agents hatch");

        assert_eq!(err.kind(), ErrorKind::UnknownArgument);
    }

    #[cfg(feature = "developer-tools")]
    #[test]
    fn account_agents_hatch_rejects_local_name_flag() {
        let err = cli_command("cargo-ai")
            .try_get_matches_from([
                "cargo-ai",
                "account",
                "agents",
                "hatch",
                "weather_test",
                "--local-name",
                "weather_test_v2",
            ])
            .expect_err("--local-name should be rejected for account agents hatch");

        assert_eq!(err.kind(), ErrorKind::UnknownArgument);
    }

    #[test]
    fn account_agents_definition_path_parses_across_commands() {
        let pull_matches = cli_command("cargo-ai")
            .try_get_matches_from([
                "cargo-ai",
                "account",
                "agents",
                "pull",
                "weather_test",
                "--definition-path",
                "/team/ops",
                "--stdout",
            ])
            .expect("account agents pull --definition-path should parse");
        let pull = pull_matches
            .subcommand_matches("account")
            .and_then(|m| m.subcommand_matches("agents"))
            .and_then(|m| m.subcommand_matches("pull"))
            .expect("pull subcommand should be available");
        assert_eq!(
            pull.get_one::<String>("definition_path")
                .map(String::as_str),
            Some("/team/ops")
        );

        let run_matches = cli_command("cargo-ai")
            .try_get_matches_from([
                "cargo-ai",
                "account",
                "agents",
                "run",
                "weather_test",
                "--definition-path",
                "/team/ops",
                "--server",
                "ollama",
                "--model",
                "mistral",
            ])
            .expect("account agents run --definition-path should parse");
        let run = run_matches
            .subcommand_matches("account")
            .and_then(|m| m.subcommand_matches("agents"))
            .and_then(|m| m.subcommand_matches("run"))
            .expect("run subcommand should be available");
        assert_eq!(
            run.get_one::<String>("definition_path").map(String::as_str),
            Some("/team/ops")
        );

        #[cfg(feature = "developer-tools")]
        {
            let hatch_matches = cli_command("cargo-ai")
                .try_get_matches_from([
                    "cargo-ai",
                    "account",
                    "agents",
                    "hatch",
                    "weather_test",
                    "--definition-path",
                    "/team/ops",
                ])
                .expect("account agents hatch --definition-path should parse");
            let hatch = hatch_matches
                .subcommand_matches("account")
                .and_then(|m| m.subcommand_matches("agents"))
                .and_then(|m| m.subcommand_matches("hatch"))
                .expect("hatch subcommand should be available");
            assert_eq!(
                hatch
                    .get_one::<String>("definition_path")
                    .map(String::as_str),
                Some("/team/ops")
            );
        }

        let push_matches = cli_command("cargo-ai")
            .try_get_matches_from([
                "cargo-ai",
                "account",
                "agents",
                "push",
                "--name",
                "weather_test",
                "--json",
                "{}",
                "--definition-path",
                "/team/ops",
            ])
            .expect("account agents push --definition-path should parse");
        let push = push_matches
            .subcommand_matches("account")
            .and_then(|m| m.subcommand_matches("agents"))
            .and_then(|m| m.subcommand_matches("push"))
            .expect("push subcommand should be available");
        assert_eq!(
            push.get_one::<String>("definition_path")
                .map(String::as_str),
            Some("/team/ops")
        );

        let visibility_matches = cli_command("cargo-ai")
            .try_get_matches_from([
                "cargo-ai",
                "account",
                "agents",
                "visibility",
                "--name",
                "weather_test",
                "--public",
                "--definition-path",
                "/team/ops",
            ])
            .expect("account agents visibility --definition-path should parse");
        let visibility = visibility_matches
            .subcommand_matches("account")
            .and_then(|m| m.subcommand_matches("agents"))
            .and_then(|m| m.subcommand_matches("visibility"))
            .expect("visibility subcommand should be available");
        assert_eq!(
            visibility
                .get_one::<String>("definition_path")
                .map(String::as_str),
            Some("/team/ops")
        );

        let archive_matches = cli_command("cargo-ai")
            .try_get_matches_from([
                "cargo-ai",
                "account",
                "agents",
                "archive",
                "--name",
                "weather_test",
                "--archive",
                "--definition-path",
                "/team/ops",
            ])
            .expect("account agents archive --definition-path should parse");
        let archive = archive_matches
            .subcommand_matches("account")
            .and_then(|m| m.subcommand_matches("agents"))
            .and_then(|m| m.subcommand_matches("archive"))
            .expect("archive subcommand should be available");
        assert_eq!(
            archive
                .get_one::<String>("definition_path")
                .map(String::as_str),
            Some("/team/ops")
        );
    }

    #[cfg(feature = "developer-tools")]
    #[test]
    fn account_agents_hatch_rejects_old_path_flag() {
        let err = cli_command("cargo-ai")
            .try_get_matches_from([
                "cargo-ai",
                "account",
                "agents",
                "hatch",
                "weather_test",
                "--path",
                "/team/ops",
            ])
            .expect_err("--path should be rejected for account agents hatch");

        assert_eq!(err.kind(), ErrorKind::UnknownArgument);
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
    fn init_experimental_flag_parses() {
        let matches = cli_command("cargo-ai")
            .try_get_matches_from(["cargo-ai", "init", "--experimental"])
            .expect("init --experimental should parse");

        let init = matches
            .subcommand_matches("init")
            .expect("init subcommand should be available");

        assert!(init.get_flag("experimental"));
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

    #[test]
    fn new_experimental_flag_parses() {
        let matches = cli_command("cargo-ai")
            .try_get_matches_from(["cargo-ai", "new", "sample-agent", "--experimental"])
            .expect("new --experimental should parse");

        let new = matches
            .subcommand_matches("new")
            .expect("new subcommand should be available");

        assert!(new.get_flag("experimental"));
    }

    #[test]
    fn auth_login_openai_profile_flags_parse() {
        let matches = cli_command("cargo-ai")
            .try_get_matches_from([
                "cargo-ai",
                "auth",
                "login",
                "openai",
                "--profile",
                "openai-prod",
                "--set-default",
            ])
            .expect("auth login openai flags should parse");

        let login_openai = matches
            .subcommand_matches("auth")
            .and_then(|m| m.subcommand_matches("login"))
            .and_then(|m| m.subcommand_matches("openai"))
            .expect("auth login openai should be available");

        assert_eq!(
            login_openai
                .get_one::<String>("profile")
                .map(String::as_str),
            Some("openai-prod")
        );
        assert!(login_openai.get_flag("set_default"));
    }

    #[test]
    fn auth_logout_global_and_alias_flags_parse() {
        let matches = cli_command("cargo-ai")
            .try_get_matches_from(["cargo-ai", "auth", "logout", "--global", "--yes"])
            .expect("auth logout --global should parse");

        let logout = matches
            .subcommand_matches("auth")
            .and_then(|m| m.subcommand_matches("logout"))
            .expect("auth logout should be available");

        assert!(logout.get_flag("global"));
        assert!(logout.get_flag("yes"));

        let alias_matches = cli_command("cargo-ai")
            .try_get_matches_from(["cargo-ai", "auth", "logout", "--revoke"])
            .expect("auth logout --revoke alias should parse");

        let alias_logout = alias_matches
            .subcommand_matches("auth")
            .and_then(|m| m.subcommand_matches("logout"))
            .expect("auth logout should be available");

        assert!(alias_logout.get_flag("revoke"));
    }

    #[test]
    fn profile_set_token_env_parses() {
        let matches = cli_command("cargo-ai")
            .try_get_matches_from([
                "cargo-ai",
                "profile",
                "set",
                "openai-prod",
                "--env",
                "OPENAI_API_KEY",
            ])
            .expect("profile set --env should parse");

        let profile_set = matches
            .subcommand_matches("profile")
            .and_then(|m| m.subcommand_matches("set"))
            .expect("profile set should be available");

        assert_eq!(
            profile_set.get_one::<String>("name").map(String::as_str),
            Some("openai-prod")
        );
        assert_eq!(
            profile_set.get_one::<String>("env").map(String::as_str),
            Some("OPENAI_API_KEY")
        );
    }

    #[test]
    fn profile_set_rejects_legacy_auth_subcommand() {
        let err = cli_command("cargo-ai")
            .try_get_matches_from([
                "cargo-ai",
                "profile",
                "auth",
                "set",
                "openai-prod",
                "openai_account",
            ])
            .expect_err("profile auth should be removed");

        assert_eq!(err.kind(), ErrorKind::InvalidSubcommand);
    }

    #[test]
    fn profile_set_rejects_legacy_token_subcommand() {
        let err = cli_command("cargo-ai")
            .try_get_matches_from([
                "cargo-ai",
                "profile",
                "token",
                "set",
                "openai-prod",
                "--token",
                "sk-test",
            ])
            .expect_err("profile token should be removed");

        assert_eq!(err.kind(), ErrorKind::InvalidSubcommand);
    }

    #[test]
    fn profile_add_auth_flag_parses() {
        let matches = cli_command("cargo-ai")
            .try_get_matches_from([
                "cargo-ai",
                "profile",
                "add",
                "openai-prod",
                "--server",
                "openai",
                "--model",
                "gpt-5.2",
                "--auth",
                "openai_account",
            ])
            .expect("profile add --auth should parse");

        let profile_add = matches
            .subcommand_matches("profile")
            .and_then(|m| m.subcommand_matches("add"))
            .expect("profile add should be available");

        assert_eq!(
            profile_add.get_one::<String>("name").map(String::as_str),
            Some("openai-prod")
        );
        assert_eq!(
            profile_add.get_one::<String>("auth").map(String::as_str),
            Some("openai_account")
        );
    }

    #[test]
    fn profile_add_rejects_legacy_token_flag() {
        let err = cli_command("cargo-ai")
            .try_get_matches_from([
                "cargo-ai",
                "profile",
                "add",
                "openai-prod",
                "--server",
                "openai",
                "--model",
                "gpt-4o",
                "--token",
                "sk-test",
            ])
            .expect_err("legacy --token flag should be rejected for profile add");

        assert_eq!(err.kind(), ErrorKind::UnknownArgument);
    }

    #[cfg(not(feature = "developer-tools"))]
    #[test]
    fn runtime_only_build_rejects_hatch_subcommand() {
        let err = cli_command("cargo-ai")
            .try_get_matches_from(["cargo-ai", "hatch", "adder_test"])
            .expect_err("runtime-only build should reject hatch");

        assert_eq!(err.kind(), ErrorKind::InvalidSubcommand);
    }

    #[cfg(not(feature = "developer-tools"))]
    #[test]
    fn runtime_only_build_rejects_account_hatch_alias() {
        let err = cli_command("cargo-ai")
            .try_get_matches_from(["cargo-ai", "account", "hatch", "weather_test"])
            .expect_err("runtime-only build should reject account hatch alias");

        assert_eq!(err.kind(), ErrorKind::InvalidSubcommand);
    }

    #[cfg(not(feature = "developer-tools"))]
    #[test]
    fn runtime_only_build_rejects_account_agents_hatch() {
        let err = cli_command("cargo-ai")
            .try_get_matches_from(["cargo-ai", "account", "agents", "hatch", "weather_test"])
            .expect_err("runtime-only build should reject account agents hatch");

        assert_eq!(err.kind(), ErrorKind::InvalidSubcommand);
    }

    #[cfg(not(feature = "developer-tools"))]
    #[test]
    fn runtime_only_build_accepts_account_run_alias() {
        let matches = cli_command("cargo-ai")
            .try_get_matches_from([
                "cargo-ai",
                "account",
                "run",
                "weather_test",
                "--server",
                "ollama",
                "--model",
                "mistral",
            ])
            .expect("runtime-only build should accept account run alias");

        assert!(matches
            .subcommand_matches("account")
            .and_then(|m| m.subcommand_matches("run"))
            .is_some());
    }

    #[cfg(not(feature = "developer-tools"))]
    #[test]
    fn runtime_only_build_accepts_account_agents_run() {
        let matches = cli_command("cargo-ai")
            .try_get_matches_from([
                "cargo-ai",
                "account",
                "agents",
                "run",
                "weather_test",
                "--server",
                "ollama",
                "--model",
                "mistral",
            ])
            .expect("runtime-only build should accept account agents run");

        assert!(matches
            .subcommand_matches("account")
            .and_then(|m| m.subcommand_matches("agents"))
            .and_then(|m| m.subcommand_matches("run"))
            .is_some());
    }
}
