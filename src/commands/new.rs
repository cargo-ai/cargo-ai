//! Runtime behavior for `cargo ai new`.
use clap::ArgMatches;
use std::path::Path;

fn display_path(path: &Path) -> String {
    if path.is_relative() {
        return path.display().to_string();
    }

    match std::env::current_dir() {
        Ok(current_dir) => match path.strip_prefix(&current_dir) {
            Ok(relative) if relative.as_os_str().is_empty() => ".".to_string(),
            Ok(relative) => format!("./{}", relative.display()),
            Err(_) => path.display().to_string(),
        },
        Err(_) => path.display().to_string(),
    }
}

fn print_success(report: &super::scaffold::ScaffoldReport) {
    println!("✓ Project created");
    println!("Root:      {}", display_path(&report.project_root));
    println!(
        "Metadata:  {} ({})",
        report.metadata_status,
        display_path(&report.metadata_path)
    );
    if report.gitignore_status != super::scaffold::ManagedFileStatus::Skipped {
        println!(
            "Gitignore: {} ({})",
            report.gitignore_status,
            display_path(&report.gitignore_path)
        );
    }
    let vcs_status = match report.git_setup {
        super::scaffold::GitSetup::Skipped => "none".to_string(),
        _ => report.git_setup.to_string(),
    };
    println!("VCS:       {vcs_status}");
}

/// Executes the `new` command flow from parsed CLI arguments.
pub fn run(sub_m: &ArgMatches) -> bool {
    if let Err(error) = run_impl(sub_m) {
        eprintln!("x {error}");
        return false;
    }
    true
}

fn run_impl(sub_m: &ArgMatches) -> Result<(), String> {
    let path = sub_m
        .get_one::<String>("path")
        .ok_or_else(|| "Missing path. Use `cargo ai new <path>`.".to_string())?;

    let vcs_mode = match super::scaffold::VcsMode::from_cli(
        sub_m.get_one::<String>("vcs").map(String::as_str),
    ) {
        Ok(vcs_mode) => vcs_mode,
        Err(error) => return Err(error),
    };

    match super::scaffold::scaffold_new(Path::new(path), vcs_mode) {
        Ok(report) => {
            print_success(&report);
            Ok(())
        }
        Err(error) => Err(error),
    }
}
