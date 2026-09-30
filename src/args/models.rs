use clap::{Arg, ArgAction, ArgGroup, Command};

pub fn command() -> Command {
    Command::new("models")
        .about("Discover models for a selected connection without invoking them")
        .subcommand_required(true)
        .subcommand(
            Command::new("list")
                .about("Read a live model catalog; listing does not prove invocation access")
                .long_about("Read a live model catalog without invoking models. OpenAI account discovery uses the current file-backed commercial Codex session and fixed native endpoint, returning picker-visible IDs. Saved profiles follow the current Codex session; separate Cargo AI homes isolate settings and local logout, not Codex identity. Hidden IDs remain usable through manual selection. Listing does not prove invocation access.")
                .arg(
                    Arg::new("profile")
                        .long("profile")
                        .value_name("NAME")
                        .conflicts_with_all(["server", "auth", "url", "stdin"]),
                )
                .arg(
                    Arg::new("server")
                        .long("server")
                        .value_name("PROVIDER")
                        .requires("auth"),
                )
                .arg(
                    Arg::new("auth")
                        .long("auth")
                        .value_parser(["none", "api_key", "openai_account"])
                        .requires("server"),
                )
                .arg(
                    Arg::new("url")
                        .long("url")
                        .value_name("URL")
                        .requires("server"),
                )
                .arg(
                    Arg::new("stdin")
                        .long("stdin")
                        .action(ArgAction::SetTrue)
                        .requires("server")
                        .help(
                            "Read a draft API key from closed, nonterminal stdin (16 KiB maximum)",
                        ),
                )
                .arg(
                    Arg::new("page-limit")
                        .long("page-limit")
                        .default_value("20")
                        .value_parser(clap::value_parser!(u32).range(1..=20)),
                )
                .group(
                    ArgGroup::new("connection")
                        .args(["profile", "server"])
                        .required(true),
                ),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn discovery_parser_requires_one_explicit_connection_without_model() {
        assert!(command()
            .try_get_matches_from(["models", "list", "--profile", "saved"])
            .is_ok());
        assert!(command()
            .try_get_matches_from(["models", "list", "--server", "ollama", "--auth", "none"])
            .is_ok());
        for args in [
            vec!["models", "list"],
            vec!["models", "list", "--server", "openai"],
            vec![
                "models",
                "list",
                "--profile",
                "saved",
                "--server",
                "openai",
                "--auth",
                "api_key",
            ],
            vec!["models", "list", "--profile", "saved", "--model", "ignored"],
            vec!["models", "list", "--profile", "saved", "--cursor", "opaque"],
            vec!["models", "list", "--profile", "saved", "--page-limit", "21"],
        ] {
            assert!(command().try_get_matches_from(args).is_err());
        }
    }
}
