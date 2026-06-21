//! Runtime behavior for `cargo ai mail`.
use clap::ArgMatches;

/// Routes top-level account-mail commands.
pub async fn run(sub_m: &ArgMatches) -> bool {
    crate::commands::account::run_mail(sub_m).await
}
