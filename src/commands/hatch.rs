//! Runtime behavior for `cargo ai hatch`.
use clap::ArgMatches;

/// Executes the `hatch` command flow from parsed CLI arguments.
pub fn run(sub_m: &ArgMatches) {
    let new_project_name = sub_m
        .get_one::<String>("name")
        .expect("project name is required");

    println!("Build new cargo agent: {new_project_name}");

    // Determine config source: use flag if provided, otherwise default to project name
    let agentcfg: &str = sub_m
        .get_one::<String>("config")
        .map(String::as_str)
        .unwrap_or(new_project_name);

    if sub_m.get_one::<String>("config").is_none() {
        println!("🌐 No --config flag detected. Fetching default template '{agentcfg}' from Cargo-AI registry...");
    }

    let file_contents = match super::hatch_pipeline::config_contents(agentcfg) {
        Ok(contents) => contents,
        Err(e) => {
            println!("❌ Failed to fetch agent configuration for '{agentcfg}'.");
            println!("Reason: {e}");
            println!("Hint: Ensure the agent name exists in the Cargo-AI registry or provide a local .json file.");
            return;
        }
    };

    super::hatch_pipeline::run_hatch_pipeline(new_project_name, file_contents);
}
