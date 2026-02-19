// clap - Command Line Arguement Parsing
use clap::{Arg, ArgGroup, ArgMatches, Command};

pub fn build_cli() -> ArgMatches {
    // Collect the original command-line arguments
    let mut args: Vec<String> = std::env::args().collect();

    let mut bin_name = "cargo-ai";
    // Check if runing as a cargo subcommand, i.e. cargo ai
    if let Some(first_arg) = args.get(1) {
        if first_arg == "ai" {
            bin_name = "cargo ai";
            args.remove(1);
        }
    }

    Command::new("cargo-ai")
        .bin_name(bin_name)
        .version(env!("CARGO_PKG_VERSION"))
        .subcommand(
            Command::new("version")
                .about("Print version information")
        )
        .subcommand(
            Command::new("preflight")
                .about("Internal: test agent config file")
                .hide(true)
                    .arg(
                        Arg::new("profile")
                            .long("profile")
                            .short('P')
                            .help("Use a saved connection profile instead of manual flags")
                            .required(false)
                            .value_name("PROFILE")
                    )
                    .arg(
                        Arg::new("server")
                            .long("server")
                            .short('s')
                            .value_name("CLIENT")
                            .help("Client Type - Ollama or OpenAI")
                    )
                    .arg(
                        Arg::new("model")
                            .long("model")
                            .short('m')
                            .value_name("MODEL")
                            .help("LLM model to use")
                    )
                    .arg(
                        Arg::new("url")
                            .long("url")
                            .help("Custom transformer server URL (HTTPS preferred)")
                            .required(false)
                            .value_name("URL")
                    )
                    .arg(
                        Arg::new("token")
                            .long("token")
                            .value_name("TOKEN")
                            .help("API token"),
                    )
                    .arg(
                        Arg::new("timeout_in_sec")
                            .long("timeout_in_sec")
                            .value_name("SECONDS")
                            .help("Client timeout request")
                            .default_value("60")
                    )
                    .arg(
                        Arg::new("prompt")
                            .long("prompt")
                            .short('p')
                            .help("Prompt to provide to the agent at runtime")
                            .value_name("TEXT")
                            .num_args(1)
                    )
        )
        .subcommand(
            Command::new("hatch")
                .about("Hatch a new AI agent from a JSON config")
                .arg(
                    Arg::new("name")
                        .help("Name of the new agent project")
                        .required(true)
                )
                .arg(
                    Arg::new("config")
                        .long("config")
                        .short('c')
                        .help("Path to the agent configuration (local .json file or remote registry name)")
                        .value_name("FILE")
                        .num_args(1)
                )
        )
        .subcommand(
            Command::new("profile")
                .about("Manage connection profiles in the Cargo-AI config file")
                .subcommand(
                    Command::new("list")
                        .about("List all configured profiles")
                )
                .subcommand(
                    Command::new("show")
                        .about("Show detailed information for a specific profile")
                        .arg(
                            Arg::new("name")
                                .help("Name of the profile to display")
                                .required(true)
                                .value_name("NAME")
                        )
                )
                .subcommand(
                    Command::new("add")
                        .about("Add a new connection profile or overwrite an existing one")
                        .arg(
                            Arg::new("name")
                                .help("Name of the profile to add or update")
                                .required(true)
                                .value_name("NAME")
                        )
                        .arg(
                            Arg::new("server")
                                .long("server")
                                .short('s')
                                .help("LLM server (e.g., openai or ollama)")
                                .required(true)
                                .value_name("SERVER")
                        )
                        .arg(
                            Arg::new("model")
                                .long("model")
                                .short('m')
                                .help("LLM model identifier (e.g., gpt-4o, mistral)")
                                .required(true)
                                .value_name("MODEL")
                        )
                        .arg(
                            Arg::new("url")
                                .long("url")
                                .help("Custom transformer server URL (HTTPS preferred)")
                                .required(false)
                                .value_name("URL")
                        )
                        .arg(
                            Arg::new("token")
                                .long("token")
                                .help("API token for the server")
                                .required(false)
                                .value_name("TOKEN")
                        )
                        .arg(
                            Arg::new("description")
                                .long("description")
                                .short('d')
                                .help("Optional description for the profile")
                                .required(false)
                                .value_name("TEXT")
                        )
                        .arg(
                            Arg::new("default")
                                .long("default")
                                .help("Set this profile as the default")
                                .action(clap::ArgAction::SetTrue)
                        )
                )
                .subcommand(
                    Command::new("remove")
                        .about("Remove an existing connection profile by name")
                        .arg(
                            Arg::new("name")
                                .help("Name of the profile to remove")
                                .required(true)
                                .value_name("NAME")
                        )
                )
        )
        .subcommand(
            Command::new("account")
                .about("Manage account lifecycle")
                .subcommand(
                    Command::new("register")
                        .about("Register a new account by email")
                        .arg(
                            Arg::new("email")
                                .help("Email address to register")
                                .required(true)
                                .value_name("EMAIL")
                        )
                )
                .subcommand(
                    Command::new("confirm")
                        .about("Confirm an account using the temporary code")
                        .arg(
                            Arg::new("code")
                                .help("Temporary confirmation code from email")
                                .required(true)
                                .value_name("CODE")
                        )
                )
                .subcommand(
                    Command::new("status")
                        .about("Show account status")
                )
                .subcommand(
                    Command::new("mail")
                        .about("Send account mail")
                        .subcommand(
                            Command::new("test")
                                .about("Send a test email to your account email")
                                .arg(
                                    Arg::new("subject")
                                        .long("subject")
                                        .help("Optional subject override (quote values with spaces)")
                                        .required(false)
                                        .value_name("TEXT")
                                        .num_args(1)
                                )
                                .arg(
                                    Arg::new("text")
                                        .long("text")
                                        .help("Optional body override (quote values with spaces)")
                                        .required(false)
                                        .value_name("TEXT")
                                        .num_args(1)
                                )
                        )
                )
                .subcommand(
                    Command::new("handle")
                        .about("Get or set account handle")
                        .arg(
                            Arg::new("set")
                                .long("set")
                                .help("Set a new handle (if omitted, returns current handle)")
                                .required(false)
                                .value_name("HANDLE")
                                .num_args(1)
                        )
                )
                .subcommand(
                    Command::new("agents")
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
                                        .num_args(1)
                                )
                                .arg(
                                    Arg::new("include_archived")
                                        .long("include-archived")
                                        .help("Include archived agents (applies to listing your own agents)")
                                        .action(clap::ArgAction::SetTrue)
                                )
                        )
                        .subcommand(
                            // TODO: Add shortcut forms after command surface is stable:
                            // - example: `cargo ai account agents push <file>`
                            Command::new("push")
                                .about("Upload or overwrite an agent definition")
                                .group(
                                    ArgGroup::new("push_input")
                                        .args(["json", "json_file"])
                                        .required(false)
                                )
                                .arg(
                                    Arg::new("name")
                                        .long("name")
                                        .help("Agent name (defaults to JSON file name when --json-file is used)")
                                        .required(false)
                                        .required_unless_present("json_file")
                                        .value_name("NAME")
                                        .num_args(1)
                                )
                                .arg(
                                    Arg::new("path")
                                        .long("path")
                                        .help("Path namespace (defaults to '/')")
                                        .required(false)
                                        .value_name("PATH")
                                        .num_args(1)
                                )
                                .arg(
                                    Arg::new("json")
                                        .long("json")
                                        .help("Agent definition JSON (raw JSON string; highest input precedence)")
                                        .required(false)
                                        .value_name("JSON")
                                        .num_args(1)
                                )
                                .arg(
                                    Arg::new("json_file")
                                        .long("json-file")
                                        .help("Path to agent definition JSON file (used when --json is not provided)")
                                        .required(false)
                                        .value_name("FILE")
                                        .num_args(1)
                                )
                                .after_help(
                                    "Notes:\n  - Required: --name unless --json-file is provided (name inferred from file name).\n  - Input precedence: --json, then --json-file, then auto-discovered ./<name>.json.\n  - Auto-discovery uses exact ./<name>.json only (no wildcard scanning).\n  - If --name looks like a file path, use --json-file <FILE> instead."
                                )
                        )
                        .subcommand(
                            Command::new("pull")
                                .about("Fetch an agent definition")
                                .arg(
                                    Arg::new("name")
                                        .long("name")
                                        .help("Agent name")
                                        .required(true)
                                        .value_name("NAME")
                                        .num_args(1)
                                )
                                .arg(
                                    Arg::new("owner_handle")
                                        .long("owner-handle")
                                        .help("Owner handle to pull from (omit to pull your own)")
                                        .required(false)
                                        .value_name("HANDLE")
                                        .num_args(1)
                                )
                                .arg(
                                    Arg::new("path")
                                        .long("path")
                                        .help("Path namespace (defaults to '/')")
                                        .required(false)
                                        .value_name("PATH")
                                        .num_args(1)
                                )
                                .arg(
                                    Arg::new("json_file")
                                        .long("json-file")
                                        .help("Write pulled definition JSON to this file (defaults to ./<name>.json)")
                                        .required(false)
                                        .value_name("FILE")
                                        .num_args(1)
                                )
                                .arg(
                                    Arg::new("stdout")
                                        .long("stdout")
                                        .help("Print pulled definition_json to stdout (no default file write unless --json-file is also set)")
                                        .required(false)
                                        .action(clap::ArgAction::SetTrue)
                                )
                                .arg(
                                    Arg::new("force")
                                        .long("force")
                                        .help("Overwrite output file if it already exists")
                                        .required(false)
                                        .action(clap::ArgAction::SetTrue)
                                )
                                .after_help(
                                    "Notes:\n  - Required: --name.\n  - Default output: ./<name>.json (when --json-file is omitted and --stdout is not set).\n  - --force applies only when writing to a file."
                                )
                        )
                        .subcommand(
                            Command::new("visibility")
                                .about("Set public visibility for an agent")
                                .group(
                                    ArgGroup::new("visibility_state")
                                        .args(["public", "private"])
                                        .required(true)
                                )
                                .arg(
                                    Arg::new("name")
                                        .long("name")
                                        .help("Agent name")
                                        .required(true)
                                        .value_name("NAME")
                                        .num_args(1)
                                )
                                .arg(
                                    Arg::new("path")
                                        .long("path")
                                        .help("Path namespace (defaults to '/')")
                                        .required(false)
                                        .value_name("PATH")
                                        .num_args(1)
                                )
                                .arg(
                                    Arg::new("public")
                                        .long("public")
                                        .help("Set agent visibility to public")
                                        .required(false)
                                        .conflicts_with("private")
                                        .action(clap::ArgAction::SetTrue)
                                )
                                .arg(
                                    Arg::new("private")
                                        .long("private")
                                        .help("Set agent visibility to private")
                                        .required(false)
                                        .conflicts_with("public")
                                        .action(clap::ArgAction::SetTrue)
                                )
                                .arg(
                                    Arg::new("public_from")
                                        .long("public-from")
                                        .help("RFC 3339 timestamp for when agent becomes public")
                                        .required(false)
                                        .value_name("RFC3339")
                                        .num_args(1)
                                )
                                .arg(
                                    Arg::new("public_until")
                                        .long("public-until")
                                        .help("RFC 3339 timestamp for when agent stops being public")
                                        .required(false)
                                        .value_name("RFC3339")
                                        .num_args(1)
                                )
                        )
                        .subcommand(
                            Command::new("archive")
                                .about("Archive or unarchive an agent")
                                .group(
                                    ArgGroup::new("archive_state")
                                        .args(["archive", "unarchive"])
                                        .required(true)
                                )
                                .arg(
                                    Arg::new("name")
                                        .long("name")
                                        .help("Agent name")
                                        .required(true)
                                        .value_name("NAME")
                                        .num_args(1)
                                )
                                .arg(
                                    Arg::new("path")
                                        .long("path")
                                        .help("Path namespace (defaults to '/')")
                                        .required(false)
                                        .value_name("PATH")
                                        .num_args(1)
                                )
                                .arg(
                                    Arg::new("archive")
                                        .long("archive")
                                        .help("Archive the agent")
                                        .required(false)
                                        .conflicts_with("unarchive")
                                        .action(clap::ArgAction::SetTrue)
                                )
                                .arg(
                                    Arg::new("unarchive")
                                        .long("unarchive")
                                        .help("Unarchive the agent")
                                        .required(false)
                                        .conflicts_with("archive")
                                        .action(clap::ArgAction::SetTrue)
                                )
                        )
                )
        )
        .get_matches_from(args)
}
