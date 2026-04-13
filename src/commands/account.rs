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
pub async fn run(sub_m: &ArgMatches) -> bool {
    if let Some(reg_m) = sub_m.subcommand_matches("register") {
        register::run(reg_m).await
    } else if let Some(conf_m) = sub_m.subcommand_matches("confirm") {
        confirm::run(conf_m).await
    } else if sub_m.subcommand_matches("status").is_some() {
        status::run().await
    } else if let Some(mail_m) = sub_m.subcommand_matches("mail") {
        mail::run(mail_m).await
    } else if let Some(handle_m) = sub_m.subcommand_matches("handle") {
        handle::run(handle_m).await
    } else if let Some(run_m) = sub_m.subcommand_matches("run") {
        agents::run_account_agent(run_m).await
    } else if cfg!(feature = "developer-tools") {
        if let Some(hatch_m) = sub_m.subcommand_matches("hatch") {
            return agents::run_hatch(hatch_m).await;
        }

        if let Some(agents_m) = sub_m.subcommand_matches("agents") {
            return agents::run(agents_m).await;
        }

        eprintln!(
            "No account subcommand found. Try 'cargo ai account register <email>', 'cargo ai account confirm <code>', 'cargo ai account status', 'cargo ai account mail test [--subject <text>] [--text <text>]', 'cargo ai account mail prefs [--disable-all|--enable-all]', 'cargo ai account handle [--set <handle>]', 'cargo ai account run <name>', 'cargo ai account hatch <name>', or 'cargo ai account agents <list|push|pull|run|hatch|visibility|archive>'."
        );
        false
    } else if let Some(agents_m) = sub_m.subcommand_matches("agents") {
        agents::run(agents_m).await
    } else {
        eprintln!(
            "No account subcommand found. Try 'cargo ai account register <email>', 'cargo ai account confirm <code>', 'cargo ai account status', 'cargo ai account mail test [--subject <text>] [--text <text>]', 'cargo ai account mail prefs [--disable-all|--enable-all]', 'cargo ai account handle [--set <handle>]', 'cargo ai account run <name>', or 'cargo ai account agents <list|push|pull|run|visibility|archive>'."
        );
        false
    }
}
