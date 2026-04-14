//! Runtime behavior for `cargo ai add`.
use clap::ArgMatches;

mod guidance;
mod tool;

/// Executes the `add` command flow from parsed CLI arguments.
pub fn run(sub_m: &ArgMatches) -> bool {
    if let Some(guidance_m) = sub_m.subcommand_matches("guidance") {
        guidance::run(guidance_m)
    } else if let Some(tool_m) = sub_m.subcommand_matches("tool") {
        tool::run(tool_m)
    } else {
        eprintln!(
            "❌ No add subcommand found. Try `cargo ai add guidance --style codex` or `cargo ai add tool <name>`."
        );
        false
    }
}
