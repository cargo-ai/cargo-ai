//! CLI parser definitions for `cargo ai packages`.
use clap::{Arg, ArgGroup, Command};

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
        .about("Manage published account packages")
        .subcommand(
            Command::new("list")
                .about("List packages")
                .arg(
                    Arg::new("owner_handle")
                        .long("owner-handle")
                        .help("List public packages for this owner handle (omit to list your packages)")
                        .required(false)
                        .value_name("HANDLE")
                        .num_args(1),
                )
                .arg(
                    Arg::new("include_archived")
                        .long("include-archived")
                        .help("Include archived packages (applies to listing your own packages)")
                        .action(clap::ArgAction::SetTrue),
                )
                .arg(
                    Arg::new("limit")
                        .long("limit")
                        .help("Maximum number of packages to display (default: 20)")
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
                        .action(clap::ArgAction::SetTrue)
                        .conflicts_with("limit"),
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
                        .action(clap::ArgAction::SetTrue),
                )
                .arg(
                    Arg::new("private")
                        .long("private")
                        .help("Set package visibility to private")
                        .required(false)
                        .conflicts_with("public")
                        .action(clap::ArgAction::SetTrue),
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
                        .action(clap::ArgAction::SetTrue),
                )
                .arg(
                    Arg::new("unarchive")
                        .long("unarchive")
                        .help("Unarchive the package")
                        .required(false)
                        .conflicts_with("archive")
                        .action(clap::ArgAction::SetTrue),
                ),
        );

    #[cfg(feature = "developer-tools")]
    let command = command.subcommand(publish_command());

    command
}
