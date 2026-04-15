//! Binary entrypoint for `cargo-ai`.
//!
//! This file intentionally stays thin: it parses CLI arguments and dispatches
//! into command modules, while command behavior lives in `src/commands/*`.
mod agent_builder;
mod args;
mod cargo_ai_metadata;
mod commands;
mod config;
mod credentials;
mod infra_api;
mod providers;
mod runtime_definition;
mod schema_version;
mod ui;
mod update_check;
mod web_resources;

use serde::{Deserialize, Serialize};
use std::path::Path;
use std::process;

include!(concat!(env!("OUT_DIR"), "/agent_model.rs"));

// Initialize Tokio runtime macro
// Executor: Responsible for polling and running to completion
#[tokio::main]
async fn main() {
    let cargo_ai_home = config::paths::cargo_ai_root();
    let cargo_ai_home_preexisting = cargo_ai_home.exists();
    let cmd_args = args::build_cli();
    let skip_update_check_for_invocation = cmd_args.get_flag("no_update_check");

    match credentials::migration::run_phase1_migration() {
        Ok(outcome) if outcome.changed() => {
            println!(
                "✅ Migrated legacy credentials: {} profile token(s), account tokens migrated: {}.",
                outcome.migrated_profile_tokens,
                if outcome.migrated_account_tokens {
                    "yes"
                } else {
                    "no"
                }
            );
        }
        Ok(_) => {}
        Err(error) => {
            eprintln!("⚠️ Failed to migrate legacy credentials: {error}");
        }
    }

    // Metadata only powers local drift checks; project-local commands should not
    // look failed when a sandbox blocks best-effort user-home writes.
    let _ = cargo_ai_metadata::persist_current_metadata();

    if let Some(notice) = cargo_ai_home_initialization_notice(
        cargo_ai_home_preexisting,
        cargo_ai_home.exists(),
        &cargo_ai_home,
    ) {
        eprintln!("{notice}");
    }

    let command_succeeded = if let Some(sub_m) = cmd_args.subcommand_matches("version") {
        commands::version::run(sub_m).await
    } else {
        if cmd_args.subcommand_name().is_some() {
            update_check::maybe_run_background_check(skip_update_check_for_invocation).await;
        }

        if let Some(sub_m) = cmd_args.subcommand_matches("run") {
            commands::run::run(sub_m).await
        } else if let Some(command_succeeded) = try_run_hatch(&cmd_args) {
            command_succeeded
        } else if let Some(sub_m) = cmd_args.subcommand_matches("add") {
            commands::add::run(sub_m)
        } else if let Some(sub_m) = cmd_args.subcommand_matches("tools") {
            commands::tools::run(sub_m)
        } else if let Some(sub_m) = cmd_args.subcommand_matches("init") {
            commands::init::run(sub_m)
        } else if let Some(sub_m) = cmd_args.subcommand_matches("new") {
            commands::new::run(sub_m)
        } else if let Some(sub_m) = cmd_args.subcommand_matches("account") {
            commands::account::run(sub_m).await
        } else if let Some(sub_m) = cmd_args.subcommand_matches("auth") {
            commands::auth::run(sub_m).await
        } else if let Some(sub_m) = cmd_args.subcommand_matches("profile") {
            commands::profile::run(sub_m)
        } else if let Some(sub_m) = cmd_args.subcommand_matches("credentials") {
            commands::credentials::run(sub_m)
        } else {
            eprintln!("❌ Provide subcommand.");
            false
        }
    };

    if !command_succeeded {
        process::exit(1);
    }
}

#[cfg(feature = "developer-tools")]
fn try_run_hatch(cmd_args: &clap::ArgMatches) -> Option<bool> {
    cmd_args
        .subcommand_matches("hatch")
        .map(crate::commands::hatch::run)
}

#[cfg(not(feature = "developer-tools"))]
fn try_run_hatch(_cmd_args: &clap::ArgMatches) -> Option<bool> {
    None
}

fn cargo_ai_home_initialization_notice(
    home_was_preexisting: bool,
    home_exists_now: bool,
    home: &Path,
) -> Option<String> {
    if home_was_preexisting || !home_exists_now {
        return None;
    }

    Some(format!(
        "ℹ️ Initialized Cargo AI Home at {}. Set CARGO_AI_HOME to override this location.",
        home.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::cargo_ai_home_initialization_notice;
    use std::path::Path;

    #[test]
    fn prints_notice_when_home_is_created_during_startup() {
        let notice =
            cargo_ai_home_initialization_notice(false, true, Path::new("/tmp/cargo-ai-home"));

        assert_eq!(
            notice,
            Some(
                "ℹ️ Initialized Cargo AI Home at /tmp/cargo-ai-home. Set CARGO_AI_HOME to override this location."
                    .to_string()
            )
        );
    }

    #[test]
    fn skips_notice_when_home_already_existed() {
        let notice =
            cargo_ai_home_initialization_notice(true, true, Path::new("/tmp/cargo-ai-home"));

        assert_eq!(notice, None);
    }

    #[test]
    fn skips_notice_when_home_still_does_not_exist() {
        let notice =
            cargo_ai_home_initialization_notice(false, false, Path::new("/tmp/cargo-ai-home"));

        assert_eq!(notice, None);
    }
}
