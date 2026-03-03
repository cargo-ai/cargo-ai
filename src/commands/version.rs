//! Runtime behavior for `cargo ai version`.
use crate::update_check::{self, UpdateMode, VersionStatus};
use clap::ArgMatches;

fn print_forced_check_status(status: VersionStatus) {
    match status {
        VersionStatus::UpdateAvailable { installed, latest } => {
            println!("Installed version: {installed}");
            println!("Latest crates.io version: {latest}");
            println!("Update available. Run `cargo install cargo-ai --locked` to update.");
        }
        VersionStatus::UpToDate { installed, latest } => {
            println!("Installed version: {installed}");
            println!("Latest crates.io version: {latest}");
            println!("cargo-ai is up to date.");
        }
        VersionStatus::UnknownVersionFormat { installed, latest } => {
            println!("Installed version: {installed}");
            println!("Latest crates.io version: {latest}");
            println!("Version comparison unavailable due to unrecognized version format.");
        }
    }
}

/// Executes `cargo ai version` including optional update-check controls.
pub async fn run(sub_m: &ArgMatches) -> bool {
    println!("{}", env!("CARGO_PKG_VERSION"));

    if let Some(mode) = sub_m.get_one::<String>("update_mode") {
        let parsed_mode = UpdateMode::from_cli_value(mode);
        if let Err(error) = update_check::set_update_mode(parsed_mode) {
            eprintln!("❌ Failed to persist update mode: {error}");
            return false;
        }

        println!("Update check mode set to: {}", parsed_mode.as_str());
        return true;
    }

    if sub_m.get_flag("check") {
        match update_check::force_check_and_persist().await {
            Ok(status) => {
                print_forced_check_status(status);
                true
            }
            Err(error) => {
                eprintln!("❌ Failed to check for updates: {error}");
                false
            }
        }
    } else {
        true
    }
}
