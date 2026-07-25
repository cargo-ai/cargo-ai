//! CLI parser definitions for `cargo ai packages`.
use clap::{Arg, ArgAction, ArgGroup, Command};

#[cfg(feature = "developer-tools")]
fn publish_command() -> Command {
    Command::new("publish")
        .about("Package the current project and publish it as an account package")
        .arg(
            Arg::new("profile")
                .help("Build/package profile to publish (defaults to 'default')")
                .required(false)
                .value_name("PROFILE")
                .num_args(1)
                .index(1),
        )
        .after_help(
            "Notes:\n  - `publish` packages the current Cargo AI project first, then uploads the resulting package archive.\n  - Project identity is taken from `.cargo-ai/project.toml` `[project].name` and `[project].version`.\n  - If PROFILE is omitted, `default` is used.",
        )
}

pub fn command() -> Command {
    let command = Command::new("packages")
        .about("Manage local and hosted packages")
        .subcommand(
            Command::new("list")
                .about("List installed local packages, or hosted packages with --account")
                .arg(
                    Arg::new("account")
                        .long("account")
                        .help("List hosted packages from your account, or from HANDLE when provided")
                        .required(false)
                        .value_name("HANDLE")
                        .num_args(0..=1)
                        .default_missing_value(""),
                )
                .arg(
                    Arg::new("include_archived")
                        .long("include-archived")
                        .help("Include archived hosted packages")
                        .action(ArgAction::SetTrue),
                )
                .arg(
                    Arg::new("limit")
                        .long("limit")
                        .help("Maximum number of packages to display")
                        .required(false)
                        .value_name("N")
                        .num_args(1)
                        .value_parser(clap::value_parser!(u32).range(1..))
                        .conflicts_with("all"),
                )
                .arg(
                    Arg::new("all")
                        .long("all")
                        .help("Display all returned packages")
                        .action(ArgAction::SetTrue)
                        .conflicts_with("limit"),
                ),
        )
        .subcommand(
            Command::new("install")
                .about("Install a local or hosted package into Cargo AI Home")
                .arg(
                    Arg::new("source")
                        .help("Local package root/archive/manifest path, or hosted package name when --account is present")
                        .required(false)
                        .value_name("SOURCE")
                        .num_args(1)
                        .index(1),
                )
                .arg(
                    Arg::new("account")
                        .long("account")
                        .help("Install a hosted package from your account, or from HANDLE when provided")
                        .required(false)
                        .value_name("HANDLE")
                        .num_args(0..=1)
                        .default_missing_value(""),
                )
                .arg(
                    Arg::new("version")
                        .long("version")
                        .help("Exact hosted package version to install (defaults to latest)")
                        .required(false)
                        .value_name("SEMVER")
                        .num_args(1),
                )
                .arg(
                    Arg::new("alias")
                        .long("as")
                        .help("Local package alias (defaults to the package name)")
                        .required(false)
                        .value_name("ALIAS")
                        .num_args(1),
                )
                .arg(
                    Arg::new("profile")
                        .long("profile")
                        .help("Current-project package profile to install when SOURCE is omitted")
                        .required(false)
                        .value_name("PROFILE")
                        .num_args(1),
                )
                .arg(
                    Arg::new("replace")
                        .long("replace")
                        .help("Replace same-version content or a different package identity at the alias")
                        .action(ArgAction::SetTrue),
                )
                .arg(
                    Arg::new("downgrade")
                        .long("downgrade")
                        .help("Allow installing an older version over the same package identity")
                        .action(ArgAction::SetTrue),
                )
                .arg(
                    Arg::new("accept_permissions")
                        .long("accept-permissions")
                        .help("Accept reviewed hosted package permissions for this install")
                        .requires("account")
                        .action(ArgAction::SetTrue),
                ),
        )
        .subcommand(
            Command::new("update")
                .about("Update an installed hosted package alias to the latest eligible version")
                .arg(
                    Arg::new("alias")
                        .help("Installed hosted package alias")
                        .required(true)
                        .value_name("ALIAS")
                        .num_args(1)
                        .index(1),
                )
                .arg(
                    Arg::new("accept_permissions")
                        .long("accept-permissions")
                        .help("Accept reviewed permission expansion for this hosted update")
                        .action(ArgAction::SetTrue),
                ),
        )
        .subcommand(
            Command::new("rollback")
                .about("Switch an installed hosted package alias to an exact earlier version")
                .arg(
                    Arg::new("alias")
                        .help("Installed hosted package alias")
                        .required(true)
                        .value_name("ALIAS")
                        .num_args(1)
                        .index(1),
                )
                .arg(
                    Arg::new("to")
                        .long("to")
                        .help("Exact hosted package version to switch to")
                        .required(true)
                        .value_name("SEMVER")
                        .num_args(1),
                )
                .arg(
                    Arg::new("accept_permissions")
                        .long("accept-permissions")
                        .help("Accept reviewed permission expansion for this hosted rollback")
                        .action(ArgAction::SetTrue),
                ),
        )
        .subcommand(
            Command::new("inspect")
                .about("Inspect an installed local package")
                .arg(
                    Arg::new("alias")
                        .help("Installed package alias")
                        .required(true)
                        .value_name("ALIAS")
                        .num_args(1)
                        .index(1),
                ),
        )
        .subcommand(
            Command::new("uninstall")
                .about("Uninstall a local package alias")
                .arg(
                    Arg::new("alias")
                        .help("Installed package alias")
                        .required(true)
                        .value_name("ALIAS")
                        .num_args(1)
                        .index(1),
                ),
        )
        .subcommand(
            Command::new("pull")
                .about("Fetch a published package")
                .group(
                    ArgGroup::new("pull_name")
                        .args(["name", "name_positional"])
                        .required(true),
                )
                .arg(
                    Arg::new("name")
                        .long("name")
                        .help("Package name (explicit alias for positional NAME)")
                        .required(false)
                        .value_name("NAME")
                        .num_args(1),
                )
                .arg(
                    Arg::new("name_positional")
                        .help("Package name")
                        .required(false)
                        .value_name("NAME")
                        .num_args(1)
                        .index(1)
                        .conflicts_with("name"),
                )
                .arg(
                    Arg::new("owner_handle")
                        .long("owner-handle")
                        .help("Owner handle to pull from (omit to pull your own)")
                        .required(false)
                        .value_name("HANDLE")
                        .num_args(1),
                )
                .arg(
                    Arg::new("version")
                        .long("version")
                        .help("Exact published version to pull (defaults to latest)")
                        .required(false)
                        .value_name("SEMVER")
                        .num_args(1),
                )
                .arg(
                    Arg::new("output_dir")
                        .long("output-dir")
                        .help("Destination directory for the restored project (defaults to ./<name>)")
                        .required(false)
                        .value_name("DIR")
                        .num_args(1),
                )
                .arg(
                    Arg::new("force")
                        .long("force")
                        .help("Overwrite the destination directory if it already exists")
                        .required(false)
                        .action(clap::ArgAction::SetTrue),
                )
                .after_help(
                    "Notes:\n  - Name can be provided as positional NAME or via --name.\n  - If --version is omitted, the latest published package is restored.\n  - Default output: ./<name> (when --output-dir is omitted).\n  - --force applies only when writing to an existing destination directory.",
                ),
        )
        .subcommand(
            Command::new("visibility")
                .about("Set public visibility for a package")
                .group(
                    ArgGroup::new("visibility_state")
                        .args(["public", "private"])
                        .required(true),
                )
                .arg(
                    Arg::new("name")
                        .long("name")
                        .help("Package name")
                        .required(true)
                        .value_name("NAME")
                        .num_args(1),
                )
                .arg(
                    Arg::new("public")
                        .long("public")
                        .help("Set package visibility to public")
                        .required(false)
                        .conflicts_with("private")
                        .action(ArgAction::SetTrue),
                )
                .arg(
                    Arg::new("private")
                        .long("private")
                        .help("Set package visibility to private")
                        .required(false)
                        .conflicts_with("public")
                        .action(ArgAction::SetTrue),
                ),
        )
        .subcommand(
            Command::new("archive")
                .about("Archive or unarchive a package")
                .group(
                    ArgGroup::new("archive_state")
                        .args(["archive", "unarchive"])
                        .required(true),
                )
                .arg(
                    Arg::new("name")
                        .long("name")
                        .help("Package name")
                        .required(true)
                        .value_name("NAME")
                        .num_args(1),
                )
                .arg(
                    Arg::new("archive")
                        .long("archive")
                        .help("Archive the package")
                        .required(false)
                        .conflicts_with("unarchive")
                        .action(ArgAction::SetTrue),
                )
                .arg(
                    Arg::new("unarchive")
                        .long("unarchive")
                        .help("Unarchive the package")
                        .required(false)
                        .conflicts_with("archive")
                        .action(ArgAction::SetTrue),
                ),
        );

    #[cfg(feature = "developer-tools")]
    let command = command.subcommand(publish_command());

    command
}

#[cfg(test)]
mod tests {
    #[test]
    fn list_defaults_to_local_without_account_selector() {
        let matches = super::command()
            .try_get_matches_from(["packages", "list"])
            .expect("packages list should parse");
        let list = matches
            .subcommand_matches("list")
            .expect("list should be available");

        assert!(list.get_one::<String>("account").is_none());
    }

    #[test]
    fn list_account_selector_accepts_optional_handle() {
        let own_matches = super::command()
            .try_get_matches_from(["packages", "list", "--account", "--limit", "20"])
            .expect("packages list --account should parse");
        let own_list = own_matches
            .subcommand_matches("list")
            .expect("list should be available");

        assert_eq!(
            own_list.get_one::<String>("account").map(String::as_str),
            Some("")
        );
        assert_eq!(own_list.get_one::<u32>("limit").copied(), Some(20));

        let handle_matches = super::command()
            .try_get_matches_from(["packages", "list", "--account", "alice"])
            .expect("packages list --account handle should parse");
        let handle_list = handle_matches
            .subcommand_matches("list")
            .expect("list should be available");

        assert_eq!(
            handle_list.get_one::<String>("account").map(String::as_str),
            Some("alice")
        );
    }

    #[test]
    fn install_supports_local_source_profile_alias_and_safety_flags() {
        let matches = super::command()
            .try_get_matches_from([
                "packages",
                "install",
                "./pkg",
                "--profile",
                "release",
                "--as",
                "sales_stable",
                "--replace",
                "--downgrade",
            ])
            .expect("packages install should parse");
        let install = matches
            .subcommand_matches("install")
            .expect("install should be available");

        assert_eq!(
            install.get_one::<String>("source").map(String::as_str),
            Some("./pkg")
        );
        assert_eq!(
            install.get_one::<String>("profile").map(String::as_str),
            Some("release")
        );
        assert_eq!(
            install.get_one::<String>("alias").map(String::as_str),
            Some("sales_stable")
        );
        assert!(install.get_flag("replace"));
        assert!(install.get_flag("downgrade"));
    }

    #[test]
    fn install_supports_hosted_account_version_and_alias() {
        let own_matches = super::command()
            .try_get_matches_from([
                "packages",
                "install",
                "data_integration",
                "--account",
                "--as",
                "data",
            ])
            .expect("hosted self install should parse");
        let own_install = own_matches
            .subcommand_matches("install")
            .expect("install should be available");
        assert_eq!(
            own_install.get_one::<String>("account").map(String::as_str),
            Some("")
        );
        assert_eq!(
            own_install.get_one::<String>("alias").map(String::as_str),
            Some("data")
        );

        let version_matches = super::command()
            .try_get_matches_from([
                "packages",
                "install",
                "data_integration",
                "--account",
                "alice",
                "--version",
                "1.2.3",
                "--as",
                "data",
            ])
            .expect("hosted handle install should parse");
        let version_install = version_matches
            .subcommand_matches("install")
            .expect("install should be available");
        assert_eq!(
            version_install
                .get_one::<String>("account")
                .map(String::as_str),
            Some("alice")
        );
        assert_eq!(
            version_install
                .get_one::<String>("version")
                .map(String::as_str),
            Some("1.2.3")
        );
    }

    #[test]
    fn update_and_rollback_parse_hosted_alias_commands() {
        let update_matches = super::command()
            .try_get_matches_from(["packages", "update", "data", "--accept-permissions"])
            .expect("packages update should parse");
        let update = update_matches
            .subcommand_matches("update")
            .expect("update should be available");
        assert_eq!(
            update.get_one::<String>("alias").map(String::as_str),
            Some("data")
        );
        assert!(update.get_flag("accept_permissions"));

        let rollback_matches = super::command()
            .try_get_matches_from([
                "packages",
                "rollback",
                "data",
                "--to",
                "1.0.0",
                "--accept-permissions",
            ])
            .expect("packages rollback should parse");
        let rollback = rollback_matches
            .subcommand_matches("rollback")
            .expect("rollback should be available");
        assert_eq!(
            rollback.get_one::<String>("alias").map(String::as_str),
            Some("data")
        );
        assert_eq!(
            rollback.get_one::<String>("to").map(String::as_str),
            Some("1.0.0")
        );
        assert!(rollback.get_flag("accept_permissions"));
    }

    #[test]
    fn permission_acceptance_is_hosted_only_for_install() {
        let error = super::command()
            .try_get_matches_from(["packages", "install", "./pkg", "--accept-permissions"])
            .expect_err("local install must not accept hosted permissions");

        assert!(error.to_string().contains("--account"));
    }
}
