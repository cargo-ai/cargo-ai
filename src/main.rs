//! Binary entrypoint for `cargo-ai`.
//!
//! This file intentionally stays thin: it parses CLI arguments and dispatches
//! into command modules, while command behavior lives in `src/commands/*`.
mod agent_builder;
mod args;
mod commands;
mod config;
mod infra_api;
mod providers;
#[cfg(feature = "shipyard-ui")]
mod shipyard_ui;
mod ui;
mod update_check;
mod version_baseline;
mod web_resources;

use serde::{Deserialize, Serialize};
use std::process;

include!(concat!(env!("OUT_DIR"), "/agent_model.rs"));

// Initialize Tokio runtime macro
// Executor: Responsible for polling and running to completion
#[tokio::main]
async fn main() {
    let cmd_args = args::build_cli();
    let skip_update_check_for_invocation = cmd_args.get_flag("no_update_check");

    if let Err(error) = version_baseline::persist_current_baseline() {
        eprintln!("⚠️ Failed to persist local version baseline: {error}");
    }

    let command_succeeded = if let Some(sub_m) = cmd_args.subcommand_matches("version") {
        commands::version::run(sub_m).await
    } else {
        if cmd_args.subcommand_name().is_some() {
            update_check::maybe_run_background_check(skip_update_check_for_invocation).await;
        }

        if let Some(sub_m) = cmd_args.subcommand_matches("preflight") {
            commands::preflight::run(sub_m).await
        } else if let Some(sub_m) = cmd_args.subcommand_matches("hatch") {
            commands::hatch::run(sub_m)
        } else if let Some(sub_m) = cmd_args.subcommand_matches("init") {
            commands::init::run(sub_m)
        } else if let Some(sub_m) = cmd_args.subcommand_matches("new") {
            commands::new::run(sub_m)
        } else if let Some(sub_m) = cmd_args.subcommand_matches("shipyard") {
            commands::shipyard::run(sub_m)
        } else if let Some(sub_m) = cmd_args.subcommand_matches("account") {
            commands::account::run(sub_m).await
        } else if let Some(sub_m) = cmd_args.subcommand_matches("profile") {
            commands::profile::run(sub_m)
        } else {
            eprintln!("❌ Provide subcommand.");
            false
        }
    };

    if !command_succeeded {
        process::exit(1);
    }
}
