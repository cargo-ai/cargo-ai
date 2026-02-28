//! Runtime behavior for `cargo ai hatch`.
use clap::ArgMatches;

/// Executes the `hatch` command flow from parsed CLI arguments.
pub fn run(sub_m: &ArgMatches) {
    let Some(new_project_name) = sub_m.get_one::<String>("name") else {
        eprintln!("❌ Missing project name. Use `cargo ai hatch <name>`.");
        return;
    };
    let check_only = sub_m.get_flag("check");
    let force_overwrite = sub_m.get_flag("force");
    let hatch_mode = if check_only {
        super::hatch_pipeline::HatchMode::Check
    } else {
        super::hatch_pipeline::HatchMode::Build
    };

    if check_only {
        println!("Check new cargo agent: {new_project_name}");
    } else {
        println!("Build new cargo agent: {new_project_name}");
    }

    let file_contents = if let Some(config_path) = sub_m.get_one::<String>("config") {
        match super::hatch_pipeline::read_local_config(config_path) {
            Ok(contents) => contents,
            Err(e) => {
                println!("❌ Failed to read local config file '{}'.", config_path);
                println!("Reason: {e}");
                println!("Hint: Ensure the path is valid and points to a UTF-8 JSON file.");
                return;
            }
        }
    } else {
        println!(
            "🌐 No --config flag detected. Fetching default template '{}' from Cargo-AI registry...",
            new_project_name
        );

        match super::hatch_pipeline::fetch_from_registry(new_project_name) {
            Ok(contents) => contents,
            Err(e) => {
                println!(
                    "❌ Failed to fetch agent configuration for '{}' from Cargo-AI registry.",
                    new_project_name
                );
                println!("Reason: {e}");
                println!("Hint: Ensure the agent name exists in the Cargo-AI registry or provide --config <path-to-json>.");
                return;
            }
        }
    };

    super::hatch_pipeline::run_hatch_pipeline(
        new_project_name,
        file_contents,
        hatch_mode,
        force_overwrite,
    );
}
