//! CLI parser definition for `cargo ai profile`.
use clap::{Arg, ArgAction, ArgGroup, Command};

fn context_reference(value: &str) -> Result<String, String> {
    if value.is_empty() || value.chars().take(1025).count() > 1024 {
        return Err("A connection reference must contain between 1 and 1024 characters.".into());
    }
    Ok(value.to_owned())
}

/// Builds the `profile` command schema and nested subcommands.
pub fn command() -> Command {
    Command::new("profile")
        .about("Manage connection profiles")
        .subcommand(Command::new("list").about("List all configured profiles"))
        .subcommand(Command::new("refresh-context")
            .about("Explicitly refresh private profile identity and connection context")
            .arg(Arg::new("name").required(true).value_name("NAME")))
        .subcommand(Command::new("validate-context")
            .about("Validate remembered connection context without granting execution permission")
            .arg(Arg::new("name").required(true).value_name("NAME"))
            .arg(Arg::new("profile_uuid").long("profile-uuid").required(true).value_name("UUID").value_parser(context_reference))
            .arg(Arg::new("connection_generation").long("connection-generation").required(true).value_name("GENERATION").value_parser(context_reference))
            .arg(Arg::new("renew").long("renew").action(ArgAction::SetTrue)
                .help("Refresh public verification evidence and proven context metadata; never renew provider credentials")))
        .subcommand(
            Command::new("show")
                .about("Show detailed information for a specific profile")
                .arg(
                    Arg::new("name")
                        .help("Name of the profile to display")
                        .required(true)
                        .value_name("NAME"),
                ),
        )
        .subcommand(
            Command::new("add")
                .about("Add a new connection profile or overwrite an existing one")
                .arg(
                    Arg::new("name")
                        .help("Name of the profile to add or update")
                        .required(true)
                        .value_name("NAME"),
                )
                .arg(
                    Arg::new("server")
                        .long("server")
                        .short('s')
                        .help("LLM server (anthropic, gemini, mistral, ollama, openai, typesafe, or xai)")
                        .required(true)
                        .value_name("SERVER"),
                )
                .arg(
                    Arg::new("model")
                        .long("model")
                        .short('m')
                        .help("LLM model identifier (e.g., gpt-4o, mistral)")
                        .required(true)
                        .value_name("MODEL"),
                )
                .arg(
                    Arg::new("auth")
                        .long("auth")
                        .help("Optional auth mode (default: none)")
                        .required(false)
                        .value_name("MODE")
                        .value_parser(["none", "api_key", "openai_account"]),
                )
                .arg(
                    Arg::new("url")
                        .long("url")
                        .help("Custom transformer server URL (HTTPS preferred)")
                        .required(false)
                        .value_name("URL"),
                )
                .arg(temperature_arg())
                .arg(thinking_arg())
                .arg(thinking_choice_arg())
                .arg(thinking_provider_default_arg())
                .arg(
                    Arg::new("max_output_tokens")
                        .long("max-output-tokens")
                        .help("Maximum provider output tokens for this profile")
                        .required(false)
                        .value_name("TOKENS")
                        .value_parser(clap::value_parser!(u32).range(1..)),
                )
                .arg(
                    Arg::new("description")
                        .long("description")
                        .short('d')
                        .help("Optional description for the profile")
                        .required(false)
                        .value_name("TEXT"),
                )
                .arg(
                    Arg::new("default")
                        .long("default")
                        .help("Set this profile as the default")
                        .action(ArgAction::SetTrue),
                ),
        )
        .subcommand(
            Command::new("set")
                .about("Update fields for an existing profile")
                .group(
                    ArgGroup::new("mutations")
                        .args([
                            "server",
                            "model",
                            "auth",
                            "url",
                            "clear_url",
                            "max_output_tokens",
                            "clear_max_output_tokens",
                            "temperature",
                            "clear_temperature",
                            "thinking",
                            "thinking_provider_default",
                            "clear_thinking",
                            "thinking_choice",
                            "description",
                            "clear_description",
                            "token",
                            "stdin",
                            "env",
                            "clear_token",
                            "default",
                        ])
                        .required(true)
                        .multiple(true),
                )
                .group(
                    ArgGroup::new("url_update")
                        .args(["url", "clear_url"])
                        .multiple(false),
                )
                .group(
                    ArgGroup::new("max_output_tokens_update")
                        .args(["max_output_tokens", "clear_max_output_tokens"])
                        .multiple(false),
                )
                .group(
                    ArgGroup::new("temperature_update")
                        .args(["temperature", "clear_temperature"])
                        .multiple(false),
                )
                .arg(
                    Arg::new("clear_temperature")
                        .long("clear-temperature")
                        .help("Use the provider default temperature")
                        .action(ArgAction::SetTrue),
                )
                .arg(
                    Arg::new("clear_thinking")
                        .long("clear-thinking")
                        .help("Clear this profile's thinking override and restore inheritance")
                        .conflicts_with_all(["thinking", "thinking_choice", "thinking_provider_default"])
                        .action(ArgAction::SetTrue),
                )
                .group(
                    ArgGroup::new("description_update")
                        .args(["description", "clear_description"])
                        .multiple(false),
                )
                .group(
                    ArgGroup::new("token_source")
                        .args(["token", "stdin", "env", "clear_token"])
                        .multiple(false),
                )
                .arg(
                    Arg::new("name")
                        .help("Name of the profile to update")
                        .required(true)
                        .value_name("NAME"),
                )
                .arg(
                    Arg::new("server")
                        .long("server")
                        .short('s')
                        .help("Update server (anthropic, gemini, mistral, ollama, openai, typesafe, or xai)")
                        .required(false)
                        .value_name("SERVER"),
                )
                .arg(
                    Arg::new("model")
                        .long("model")
                        .short('m')
                        .help("Update model identifier (e.g., gpt-5.2, mistral)")
                        .required(false)
                        .value_name("MODEL"),
                )
                .arg(
                    Arg::new("auth")
                        .long("auth")
                        .help("Update auth mode")
                        .required(false)
                        .value_name("MODE")
                        .value_parser(["none", "api_key", "openai_account"]),
                )
                .arg(
                    Arg::new("url")
                        .long("url")
                        .help("Set custom transformer server URL")
                        .required(false)
                        .value_name("URL"),
                )
                .arg(
                    Arg::new("clear_url")
                        .long("clear-url")
                        .help("Remove custom transformer server URL")
                        .required(false)
                        .action(ArgAction::SetTrue),
                )
                .arg(temperature_arg())
                .arg(thinking_arg())
                .arg(thinking_choice_arg())
                .arg(thinking_provider_default_arg())
                .arg(
                    Arg::new("max_output_tokens")
                        .long("max-output-tokens")
                        .help("Set the maximum provider output tokens")
                        .required(false)
                        .value_name("TOKENS")
                        .value_parser(clap::value_parser!(u32).range(1..)),
                )
                .arg(
                    Arg::new("clear_max_output_tokens")
                        .long("clear-max-output-tokens")
                        .help("Remove the profile output-token override")
                        .required(false)
                        .action(ArgAction::SetTrue),
                )
                .arg(
                    Arg::new("description")
                        .long("description")
                        .short('d')
                        .help("Set profile description")
                        .required(false)
                        .value_name("TEXT"),
                )
                .arg(
                    Arg::new("clear_description")
                        .long("clear-description")
                        .help("Remove profile description")
                        .required(false)
                        .action(ArgAction::SetTrue),
                )
                .arg(
                    Arg::new("token")
                        .long("token")
                        .help("Set API token from literal value")
                        .required(false)
                        .value_name("TOKEN")
                        .num_args(1),
                )
                .arg(
                    Arg::new("stdin")
                        .long("stdin")
                        .help("Read API token from non-terminal stdin through EOF (maximum 16384 raw bytes; trailing LF/CRLF allowed)")
                        .long_help("Read an API token from a pipe and close stdin. Maximum 16384 raw bytes including trailing LF/CRLF; surrounding spaces/tabs are trimmed. Empty, multiline, NUL and invalid UTF-8 input is rejected. No prompt. Success requires the requested credential and profile updates to be persisted in the selected home/store.")
                        .required(false)
                        .action(ArgAction::SetTrue),
                )
                .arg(
                    Arg::new("env")
                        .long("env")
                        .help("Set API token from environment variable")
                        .required(false)
                        .value_name("ENV_VAR")
                        .num_args(1),
                )
                .arg(
                    Arg::new("clear_token")
                        .long("clear-token")
                        .help("Clear stored API token for this profile")
                        .required(false)
                        .action(ArgAction::SetTrue),
                )
                .arg(
                    Arg::new("default")
                        .long("default")
                        .help("Set this profile as the default")
                        .required(false)
                        .action(ArgAction::SetTrue),
                ),
        )
        .subcommand(
            Command::new("remove")
                .about("Remove an existing connection profile by name")
                .arg(Arg::new("yes").long("yes").action(ArgAction::SetTrue)
                    .help("Explicitly consent to removal in selected machine output mode"))
                .arg(
                    Arg::new("name")
                        .help("Name of the profile to remove")
                        .required(true)
                        .value_name("NAME"),
                ),
        )
}

fn thinking_arg() -> Arg {
    Arg::new("thinking")
        .long("thinking")
        .value_name("VALUE")
        .help("Store On/Off (case insensitive) or an exact thinking choice; checked when running")
        .conflicts_with_all(["thinking_provider_default", "thinking_choice"])
        .value_parser(|value: &str| {
            if value.trim().is_empty() {
                Err("thinking choice must be nonempty".to_string())
            } else {
                Ok(value.to_string())
            }
        })
}

fn thinking_choice_arg() -> Arg {
    Arg::new("thinking_choice")
        .long("thinking-choice")
        .value_name("VALUE")
        .help("Store an exact named choice, including literal on/off")
        .conflicts_with_all(["thinking", "thinking_provider_default"])
        .value_parser(|value: &str| {
            if value.trim().is_empty() {
                Err("thinking choice must be nonempty".to_string())
            } else {
                Ok(value.to_string())
            }
        })
}

fn thinking_provider_default_arg() -> Arg {
    Arg::new("thinking_provider_default")
        .long("thinking-provider-default")
        .help("Explicitly use provider-default thinking instead of an inherited choice")
        .action(ArgAction::SetTrue)
}

fn temperature_arg() -> Arg {
    Arg::new("temperature")
        .long("temperature")
        .help("Set a finite nonnegative temperature; unset uses the provider default")
        .value_name("NUMBER")
        .value_parser(|value: &str| -> Result<f64, String> {
            let value = value
                .parse::<f64>()
                .map_err(|_| "temperature must be a number".to_string())?;
            if !value.is_finite() || value < 0.0 {
                return Err("temperature must be finite and nonnegative".to_string());
            }
            Ok(value)
        })
}

#[cfg(test)]
mod temperature_tests {
    #[test]
    fn thinking_flags_allow_override_default_and_clear_without_model() {
        for words in [
            vec!["profile", "set", "example", "--thinking", "high"],
            vec!["profile", "set", "example", "--thinking-provider-default"],
            vec!["profile", "set", "example", "--clear-thinking"],
            vec![
                "profile",
                "add",
                "example",
                "--server",
                "ollama",
                "--model",
                "fixture",
                "--thinking",
                "max",
            ],
        ] {
            assert!(super::command().try_get_matches_from(words).is_ok());
        }
        for words in [
            vec!["profile", "set", "example", "--thinking", " "],
            vec![
                "profile",
                "set",
                "example",
                "--thinking",
                "high",
                "--thinking-provider-default",
            ],
            vec![
                "profile",
                "set",
                "example",
                "--thinking",
                "high",
                "--clear-thinking",
            ],
            vec![
                "profile",
                "set",
                "example",
                "--thinking-provider-default",
                "--clear-thinking",
            ],
        ] {
            assert!(super::command().try_get_matches_from(words).is_err());
        }
    }

    #[test]
    fn temperature_arguments_validate_and_clear() {
        for value in ["0", "0.7", "12"] {
            assert!(super::command()
                .try_get_matches_from([
                    "profile",
                    "add",
                    "example",
                    "--server",
                    "openai",
                    "--model",
                    "example",
                    "--temperature",
                    value
                ])
                .is_ok());
            assert!(super::command()
                .try_get_matches_from(["profile", "set", "example", "--temperature", value])
                .is_ok());
        }
        for value in ["-0.1", "NaN", "inf", "invalid"] {
            assert!(super::command()
                .try_get_matches_from(["profile", "set", "example", "--temperature", value])
                .is_err());
        }
        assert!(super::command()
            .try_get_matches_from(["profile", "set", "example", "--clear-temperature"])
            .is_ok());
        assert!(super::command()
            .try_get_matches_from([
                "profile",
                "set",
                "example",
                "--temperature",
                "0",
                "--clear-temperature"
            ])
            .is_err());
    }
}
