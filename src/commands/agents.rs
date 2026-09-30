//! Runtime behavior for `cargo ai agents`.
use clap::ArgMatches;

/// Routes top-level account-agent management commands.
pub async fn run(sub_m: &ArgMatches) -> bool {
    crate::commands::account::run_agents(sub_m).await
}

pub(crate) async fn machine_run(
    matches: &ArgMatches,
) -> Result<serde_json::Value, crate::commands::machine::Failure> {
    crate::commands::account::machine_agents(matches).await
}
