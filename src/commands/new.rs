//! Runtime behavior for `cargo ai new`.
use clap::ArgMatches;
use std::path::Path;

fn print_success(report: &super::scaffold::ScaffoldReport) {
    println!(
        "✅ Cargo-AI project created at: {}",
        report.project_root.display()
    );
    println!("🧩 Wrote metadata: {}", report.metadata_path.display());
    if let Some(template_path) = &report.template_output_path {
        println!("🧩 Applied template file: {}", template_path.display());
    }
    println!("🌿 VCS setup: {}", report.git_setup);
}

/// Executes the `new` command flow from parsed CLI arguments.
pub fn run(sub_m: &ArgMatches) {
    let Some(path) = sub_m.get_one::<String>("path") else {
        eprintln!("❌ Missing path. Use `cargo ai new <path>`.");
        return;
    };

    let template = match super::scaffold::ProjectTemplate::from_cli(
        sub_m.get_one::<String>("template").map(String::as_str),
    ) {
        Ok(template) => template,
        Err(error) => {
            eprintln!("❌ {}", error);
            return;
        }
    };

    let vcs_mode = match super::scaffold::VcsMode::from_cli(
        sub_m.get_one::<String>("vcs").map(String::as_str),
    ) {
        Ok(vcs_mode) => vcs_mode,
        Err(error) => {
            eprintln!("❌ {}", error);
            return;
        }
    };

    println!("Create Cargo-AI project: {path}");
    match super::scaffold::scaffold_new(Path::new(path), template, vcs_mode) {
        Ok(report) => print_success(&report),
        Err(error) => eprintln!("❌ {}", error),
    }
}
