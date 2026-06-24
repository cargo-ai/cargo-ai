//! Account command dispatcher.
//!
//! This module keeps routing logic thin and delegates each account subcommand
//! to a focused implementation module.
use clap::ArgMatches;

mod agents;
mod confirm;
mod handle;
pub(crate) mod helpers;
mod mail;
mod projects;
mod register;
mod status;

pub(crate) use agents::{run as run_agents, run_account_agent, run_hatch};
pub(crate) use mail::run as run_mail;
pub(crate) use projects::{
    create_package_archive_bytes, directory_size_bytes, extract_package_archive_bytes,
    format_bytes, run as run_packages, sha256_hex,
};

/// Routes `cargo ai account ...` subcommands to their runtime handlers.
pub async fn run(sub_m: &ArgMatches) -> bool {
    if let Some(reg_m) = sub_m.subcommand_matches("register") {
        register::run(reg_m).await
    } else if let Some(conf_m) = sub_m.subcommand_matches("confirm") {
        confirm::run(conf_m).await
    } else if sub_m.subcommand_matches("status").is_some() {
        status::run().await
    } else if let Some(handle_m) = sub_m.subcommand_matches("handle") {
        handle::run(handle_m).await
    } else {
        eprintln!(
            "No account subcommand found. Try 'cargo ai account register <email>', 'cargo ai account confirm <code>', 'cargo ai account status', or 'cargo ai account handle [--set <handle>]'."
        );
        false
    }
}
