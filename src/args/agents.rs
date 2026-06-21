//! CLI parser definitions for `cargo ai account agents`.
use clap::{Arg, ArgGroup, Command};

pub(crate) fn run_command() -> Command {
    crate::args::runtime_common::runtime_command(
        "run",
        "Run an account agent definition through the interpreted runtime",
    )
    .arg(
        Arg::new("name")
            .help("Account agent name")
            .required(true)
            .value_name("NAME")
            .num_args(1),
    )
    .arg(
        Arg::new("owner_handle")
            .long("owner-handle")
            .help("Owner handle to run from (omit to run your own)")
            .required(false)
            .value_name("HANDLE")
            .num_args(1),
    )
    .arg(
        Arg::new("definition_path")
            .long("definition-path")
            .help("Account-side definition namespace path to read from (defaults to '/'; not a local filesystem path).")
            .required(false)
            .value_name("PATH")
            .num_args(1),
    )
    .after_help(
        "Notes:\n  - NAME is the remote/account agent name.\n  - Use --owner-handle to run a public agent from another handle.\n  - --definition-path selects account-side definition namespace path (defaults to '/'; not local filesystem path).\n  - Interpreted account run accepts the same runtime invocation flags as `cargo ai run`.\n  - Hatch/export flags like --check, --target, --output-dir, --keep-project, --force, and --agent are not supported here.",
    )
}

#[cfg(feature = "developer-tools")]
pub(crate) fn hatch_command() -> Command {
    Command::new("hatch")
        .about("Build an executable from an account agent definition")
        .arg(
            Arg::new("name")
                .help("Local output/workspace name")
                .required(true)
                .value_name("NAME")
                .num_args(1),
        )
        .arg(
            Arg::new("check")
                .long("check")
                .help("Validate scaffold and compile path with `cargo check` (no binary export)")
                .required(false)
                .action(clap::ArgAction::SetTrue),
        )
        .arg(
            Arg::new("owner_handle")
                .long("owner-handle")
                .help("Owner handle to hatch from (omit to hatch your own)")
                .required(false)
                .value_name("HANDLE")
                .num_args(1),
        )
        .arg(
            Arg::new("definition_path")
                .long("definition-path")
                .help("Account-side definition namespace path to read from (defaults to '/'; not a local filesystem path).")
                .required(false)
                .value_name("PATH")
                .num_args(1),
        )
        .arg(
            Arg::new("agent")
                .long("agent")
                .help("Remote/account agent name override (defaults to positional NAME)")
                .required(false)
                .value_name("AGENT")
                .num_args(1),
        )
        .arg(
            Arg::new("target")
                .long("target")
                .help("Rust target triple to pass through to cargo build/check")
                .required(false)
                .value_name("TRIPLE")
                .num_args(1),
        )
        .arg(
            Arg::new("output_dir")
                .long("output-dir")
                .help("Destination directory for the exported binary (defaults to current directory)")
                .required(false)
                .value_name("DIR")
                .num_args(1),
        )
        .arg(
            Arg::new("force")
                .long("force")
                .short('f')
                .help("Overwrite output binary and replace any kept internal workspace")
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
        .after_help(
            "Notes:\n  - NAME is the local output/workspace name and defaults the remote account agent name when --agent is omitted.\n  - --agent overrides only the remote/account agent source identifier.\n  - --definition-path selects account-side definition namespace path (defaults to '/'; not local filesystem path).\n  - Owner defaults to your authenticated account when --owner-handle is omitted.\n  - --check validates scaffold and compile behavior without exporting a binary.\n  - --target passes a Rust target triple through to cargo build/check (install targets/toolchains separately when needed).\n  - --output-dir controls where the exported binary is written; NAME still controls the filename.\n  - --keep-project preserves the internal workspace under ~/.cargo/.cargo-ai/agents/<name>.\n  - Existing output binaries or kept workspaces require --force/-f to replace.",
        )
}

/// Builds the `agents` account subcommand schema.
pub fn command() -> Command {
    let command = Command::new("agents")
        .about("Manage account agents")
        .subcommand(
            Command::new("list")
                .about("List agents")
                .arg(
                    Arg::new("owner_handle")
                        .long("owner-handle")
                        .help("List public agents for this owner handle (omit to list your agents)")
                        .required(false)
                        .value_name("HANDLE")
                        .num_args(1),
                )
                .arg(
                    Arg::new("include_archived")
                        .long("include-archived")
                        .help("Include archived agents (applies to listing your own agents)")
                        .action(clap::ArgAction::SetTrue),
                )
                .arg(
                    Arg::new("limit")
                        .long("limit")
                        .help("Maximum number of agents to display (default: 20)")
                        .required(false)
                        .value_name("N")
                        .num_args(1)
                        .value_parser(clap::value_parser!(u32).range(1..))
                        .conflicts_with("all"),
                )
                .arg(
                    Arg::new("all")
                        .long("all")
                        .help("Display all returned agents")
                        .action(clap::ArgAction::SetTrue)
                        .conflicts_with("limit"),
                ),
        )
        .subcommand(
            Command::new("push")
                .about("Upload or overwrite an agent definition")
                .group(
                    ArgGroup::new("push_input")
                        .args(["json", "json_file", "input_file"])
                        .required(true),
                )
                .arg(
                    Arg::new("name")
                        .long("name")
                        .help("Agent name (defaults to file name when --json-file or positional FILE is used)")
                        .required(false)
                        .required_unless_present_any(["json_file", "input_file"])
                        .value_name("NAME")
                        .num_args(1),
                )
                .arg(
                    Arg::new("definition_path")
                        .long("definition-path")
                        .help("Account-side definition namespace path to read from (defaults to '/'; not a local filesystem path).")
                        .required(false)
                        .value_name("PATH")
                        .num_args(1),
                )
                .arg(
                    Arg::new("json")
                        .long("json")
                        .help("Agent definition JSON (raw JSON string; highest input precedence)")
                        .required(false)
                        .value_name("JSON")
                        .num_args(1),
                )
                .arg(
                    Arg::new("json_file")
                        .long("json-file")
                        .help("Path to agent definition JSON file (used when --json is not provided)")
                        .required(false)
                        .value_name("FILE")
                        .num_args(1),
                )
                .arg(
                    Arg::new("input_file")
                        .help("Shortcut file input (equivalent to --json-file <FILE>)")
                        .required(false)
                        .value_name("FILE")
                        .num_args(1)
                        .index(1)
                        .conflicts_with_all(["json", "json_file"]),
                )
                .after_help(
                    "Notes:\n  - Required input: provide one of --json, --json-file, or positional FILE.\n  - Name is required for --json, and inferred from file name for --json-file/FILE when omitted.\n  - Input precedence: --json, then --json-file, then positional FILE.\n  - If --name looks like a file path, use --json-file <FILE> or positional FILE instead.",
                ),
        )
        .subcommand(
            Command::new("pull")
                .about("Fetch an agent definition")
                .group(
                    ArgGroup::new("pull_name")
                        .args(["name", "name_positional"])
                        .required(true),
                )
                .arg(
                    Arg::new("name")
                        .long("name")
                        .help("Agent name (explicit alias for positional NAME)")
                        .required(false)
                        .value_name("NAME")
                        .num_args(1),
                )
                .arg(
                    Arg::new("name_positional")
                        .help("Agent name")
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
                    Arg::new("definition_path")
                        .long("definition-path")
                        .help("Account-side definition namespace path to read from (defaults to '/'; not a local filesystem path).")
                        .required(false)
                        .value_name("PATH")
                        .num_args(1),
                )
                .arg(
                    Arg::new("json_file")
                        .long("json-file")
                        .help("Write pulled definition JSON to this file (defaults to ./<name>.json)")
                        .required(false)
                        .value_name("FILE")
                        .num_args(1),
                )
                .arg(
                    Arg::new("stdout")
                        .long("stdout")
                        .help("Print pulled definition_json to stdout (no default file write unless --json-file is also set)")
                        .required(false)
                        .action(clap::ArgAction::SetTrue),
                )
                .arg(
                    Arg::new("force")
                        .long("force")
                        .help("Overwrite output file if it already exists")
                        .required(false)
                        .action(clap::ArgAction::SetTrue),
                )
                .after_help(
                    "Notes:\n  - Name can be provided as positional NAME or via --name.\n  - Default output: ./<name>.json (when --json-file is omitted and --stdout is not set).\n  - --force applies only when writing to a file.",
                ),
        )
        .subcommand(run_command())
        .subcommand(
            Command::new("visibility")
                .about("Set public visibility for an agent")
                .group(
                    ArgGroup::new("visibility_state")
                        .args(["public", "private"])
                        .required(true),
                )
                .arg(
                    Arg::new("name")
                        .long("name")
                        .help("Agent name")
                        .required(true)
                        .value_name("NAME")
                        .num_args(1),
                )
                .arg(
                    Arg::new("definition_path")
                        .long("definition-path")
                        .help("Account-side definition namespace path to read from (defaults to '/'; not a local filesystem path).")
                        .required(false)
                        .value_name("PATH")
                        .num_args(1),
                )
                .arg(
                    Arg::new("public")
                        .long("public")
                        .help("Set agent visibility to public")
                        .required(false)
                        .conflicts_with("private")
                        .action(clap::ArgAction::SetTrue),
                )
                .arg(
                    Arg::new("private")
                        .long("private")
                        .help("Set agent visibility to private")
                        .required(false)
                        .conflicts_with("public")
                        .action(clap::ArgAction::SetTrue),
                )
                .arg(
                    Arg::new("public_from")
                        .long("public-from")
                        .help("RFC 3339 timestamp for when agent becomes public")
                        .required(false)
                        .value_name("RFC3339")
                        .num_args(1),
                )
                .arg(
                    Arg::new("public_until")
                        .long("public-until")
                        .help("RFC 3339 timestamp for when agent stops being public")
                        .required(false)
                        .value_name("RFC3339")
                        .num_args(1),
                ),
        )
        .subcommand(
            Command::new("archive")
                .about("Archive or unarchive an agent")
                .group(
                    ArgGroup::new("archive_state")
                        .args(["archive", "unarchive"])
                        .required(true),
                )
                .arg(
                    Arg::new("name")
                        .long("name")
                        .help("Agent name")
                        .required(true)
                        .value_name("NAME")
                        .num_args(1),
                )
                .arg(
                    Arg::new("definition_path")
                        .long("definition-path")
                        .help("Account-side definition namespace path to read from (defaults to '/'; not a local filesystem path).")
                        .required(false)
                        .value_name("PATH")
                        .num_args(1),
                )
                .arg(
                    Arg::new("archive")
                        .long("archive")
                        .help("Archive the agent")
                        .required(false)
                        .conflicts_with("unarchive")
                        .action(clap::ArgAction::SetTrue),
                )
                .arg(
                    Arg::new("unarchive")
                        .long("unarchive")
                        .help("Unarchive the agent")
                        .required(false)
                        .conflicts_with("archive")
                        .action(clap::ArgAction::SetTrue),
                ),
        );

    #[cfg(feature = "developer-tools")]
    let command = command.subcommand(hatch_command());

    command
}
