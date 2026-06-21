//! Runtime behavior for `cargo ai packages`.
use clap::ArgMatches;

/// Routes top-level account-package management commands.
pub async fn run(sub_m: &ArgMatches) -> bool {
    crate::commands::account::run_packages(sub_m).await
}
