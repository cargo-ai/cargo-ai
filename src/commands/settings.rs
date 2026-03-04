//! Settings command dispatcher.
//!
//! This module keeps routing logic thin and delegates subcommands to focused
//! implementations.
use clap::ArgMatches;

mod secret_store;

/// Routes `cargo ai settings ...` subcommands to runtime handlers.
pub fn run(sub_m: &ArgMatches) -> bool {
    if let Some(secret_store_m) = sub_m.subcommand_matches("secret-store") {
        secret_store::run(secret_store_m)
    } else {
        eprintln!(
            "No settings subcommand found. Try 'cargo ai settings secret-store status' or 'cargo ai settings secret-store set <file|keychain>'."
        );
        false
    }
}
