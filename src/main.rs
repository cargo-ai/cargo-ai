//! Binary entrypoint for `cargo-ai`.
//!
//! This file intentionally stays thin: it parses CLI arguments and dispatches
//! into command modules, while command behavior lives in `src/commands/*`.
mod agent_builder;
mod args;
mod commands;
mod config;
mod infra_api;
#[cfg(feature = "shipyard-ui")]
mod shipyard_ui;
mod ui;
mod web_resources;

use serde::{Deserialize, Serialize};

include!(concat!(env!("OUT_DIR"), "/agent_model.rs"));

// Initialize Tokio runtime macro
// Executor: Responsible for polling and running to completion
#[tokio::main]
async fn main() {
    let cmd_args = args::build_cli();

    if let Some(sub_m) = cmd_args.subcommand_matches("preflight") {
        commands::preflight::run(sub_m).await;
    } else if let Some(sub_m) = cmd_args.subcommand_matches("hatch") {
        commands::hatch::run(sub_m);
    } else if let Some(sub_m) = cmd_args.subcommand_matches("init") {
        commands::init::run(sub_m);
    } else if let Some(sub_m) = cmd_args.subcommand_matches("new") {
        commands::new::run(sub_m);
    } else if let Some(sub_m) = cmd_args.subcommand_matches("shipyard") {
        commands::shipyard::run(sub_m);
    } else if let Some(sub_m) = cmd_args.subcommand_matches("account") {
        commands::account::run(sub_m).await;
    } else if let Some(sub_m) = cmd_args.subcommand_matches("profile") {
        commands::profile::run(sub_m);
    } else {
        println!("Provide subcommand.");
    }
}
