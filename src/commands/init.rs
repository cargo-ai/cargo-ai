//! Runtime behavior for `cargo ai init`.
use clap::ArgMatches;
use std::path::Path;
use std::process;

fn print_success(report: &super::scaffold::ScaffoldReport) {
    println!(
        "✅ Cargo-AI project initialized at: {}",
        report.project_root.display()
    );
    println!("🧩 Wrote metadata: {}", report.metadata_path.display());
    if let Some(template_path) = &report.template_output_path {
        println!("🧩 Applied template file: {}", template_path.display());
    }
    println!("🌿 VCS setup: {}", report.git_setup);
}

/// Executes the `init` command flow from parsed CLI arguments.
pub fn run(sub_m: &ArgMatches) {
    if let Err(error) = run_impl(sub_m) {
        eprintln!("❌ {}", error);
        process::exit(1);
    }
}

fn run_impl(sub_m: &ArgMatches) -> Result<(), String> {
    let path = sub_m
        .get_one::<String>("path")
        .ok_or_else(|| "Missing path. Use `cargo ai init [path]`.".to_string())?;

    let template = match super::scaffold::ProjectTemplate::from_cli(
        sub_m.get_one::<String>("template").map(String::as_str),
    ) {
        Ok(template) => template,
        Err(error) => return Err(error),
    };

    let vcs_mode = match super::scaffold::VcsMode::from_cli(
        sub_m.get_one::<String>("vcs").map(String::as_str),
    ) {
        Ok(vcs_mode) => vcs_mode,
        Err(error) => return Err(error),
    };

    println!("Initialize Cargo-AI project: {path}");
    match super::scaffold::scaffold_init(Path::new(path), template, vcs_mode) {
        Ok(report) => {
            print_success(&report);
            Ok(())
        }
        Err(error) => Err(error),
    }
}
