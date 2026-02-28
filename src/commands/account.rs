//! Account command dispatcher.
//!
//! This module keeps routing logic thin and delegates each account subcommand
//! to a focused implementation module.
use clap::ArgMatches;

mod agents;
mod confirm;
mod handle;
mod helpers;
mod mail;
mod register;
mod status;

/// Routes `cargo ai account ...` subcommands to their runtime handlers.
pub async fn run(sub_m: &ArgMatches) {
    if let Some(reg_m) = sub_m.subcommand_matches("register") {
        register::run(reg_m).await;
    } else if let Some(conf_m) = sub_m.subcommand_matches("confirm") {
        confirm::run(conf_m).await;
    } else if sub_m.subcommand_matches("status").is_some() {
        status::run().await;
    } else if let Some(mail_m) = sub_m.subcommand_matches("mail") {
        mail::run(mail_m).await;
    } else if let Some(handle_m) = sub_m.subcommand_matches("handle") {
        handle::run(handle_m).await;
    } else if let Some(agents_m) = sub_m.subcommand_matches("agents") {
        agents::run(agents_m).await;
    } else {
        println!(
            "No account subcommand found. Try 'cargo ai account register <email>', 'cargo ai account confirm <code>', 'cargo ai account status', 'cargo ai account mail test [--subject <text>] [--text <text>]', 'cargo ai account mail prefs [--disable-all|--enable-all]', 'cargo ai account handle [--set <handle>]', or 'cargo ai account agents <list|push|pull|hatch|visibility|archive>'."
        );
    }
}
