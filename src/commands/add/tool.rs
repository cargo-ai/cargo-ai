//! Runtime behavior for `cargo ai add tool`.
use clap::ArgMatches;

/// Executes the `cargo ai add tool` flow from parsed CLI arguments.
pub fn run(sub_m: &ArgMatches) -> bool {
    let Some(tool_name) = sub_m.get_one::<String>("name").map(String::as_str) else {
        eprintln!("x Missing tool name. Use `cargo ai add tool <name>`.");
        return false;
    };

    let current_dir = match std::env::current_dir() {
        Ok(current_dir) => current_dir,
        Err(error) => {
            eprintln!("x Failed to read current directory: {error}");
            return false;
        }
    };
    let Some(project_root) = crate::commands::tools::maybe_find_project_root(&current_dir) else {
        eprintln!(
            "x No Cargo AI project metadata was found from the current directory upward. Run `cargo ai init` here or `cargo ai new <path>` for a new project first."
        );
        return false;
    };

    match crate::commands::tools::scaffold_local_tool(&project_root, tool_name) {
        Ok(()) => {
            println!("✓ Tool scaffolded");
            println!("Tool:   {}", tool_name);
            println!(
                "Source: {}",
                project_root.join("tools").join(tool_name).display()
            );
            println!(
                "Managed: {}",
                crate::commands::tools::project_tools_root(&project_root)
                    .join(tool_name)
                    .display()
            );
            true
        }
        Err(error) => {
            eprintln!("x {error}");
            false
        }
    }
}
