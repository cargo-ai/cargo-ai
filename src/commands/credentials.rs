//! Credentials command dispatcher.
//!
//! This module keeps routing logic thin and delegates subcommands to focused
//! implementations.
use clap::ArgMatches;

mod store;

/// Routes `cargo ai credentials ...` subcommands to runtime handlers.
pub fn run(sub_m: &ArgMatches) -> bool {
    if let Some(store_m) = sub_m.subcommand_matches("store") {
        store::run(store_m)
    } else {
        eprintln!(
            "No credentials subcommand found. Try 'cargo ai credentials store status' or 'cargo ai credentials store set <file|keychain>'."
        );
        false
    }
}
