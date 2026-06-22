//! Runtime behavior for `cargo ai packages`.
use clap::ArgMatches;

/// Routes package management commands.
pub async fn run(sub_m: &ArgMatches) -> bool {
    if let Some(list_m) = sub_m.subcommand_matches("list") {
        if crate::commands::local_packages::account_handle_from_list_matches(list_m).is_some() {
            return crate::commands::account::run_packages(sub_m).await;
        }
    }

    if sub_m.subcommand_matches("install").is_some()
        || sub_m.subcommand_matches("inspect").is_some()
        || sub_m.subcommand_matches("uninstall").is_some()
        || sub_m.subcommand_matches("list").is_some()
    {
        return crate::commands::local_packages::run(sub_m).await;
    }

    crate::commands::account::run_packages(sub_m).await
}
