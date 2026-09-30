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

/// Uses the production scaffold and reports reconciled filesystem effects.
pub(crate) fn machine_run(
    sub_m: &ArgMatches,
) -> Result<serde_json::Value, super::machine::Failure> {
    use super::machine::Failure;
    use serde_json::json;
    let path = sub_m
        .get_one::<String>("path")
        .ok_or_else(|| Failure::new("input.invalid", "A project path is required."))?;
    let mode =
        super::scaffold::VcsMode::from_cli(sub_m.get_one::<String>("vcs").map(String::as_str))
            .map_err(|_| Failure::new("input.invalid", "The VCS mode is unsupported."))?;
    let root = Path::new(path);
    if root.exists() {
        return Err(
            Failure::new("input.conflict", "The project target already exists.")
                .with_data(json!({"effects":{"local":"unapplied","remote":"unapplied"}})),
        );
    }
    let report = super::scaffold::scaffold_new(root, mode).map_err(|_| {
        Failure::new("project.create_failed", "Project creation failed; inspect the target before retrying.")
            .with_data(json!({"effects":{"local":"unknown","remote":"unapplied","project_target":if root.exists() {"unknown"} else {"unapplied"},"ancestor_directories":"unknown"},"target_exists":root.exists()}))
    })?;
    let verified = report.metadata_path.is_file()
        && (report.gitignore_status == super::scaffold::ManagedFileStatus::Skipped
            || report.gitignore_path.is_file())
        && (report.git_setup == super::scaffold::GitSetup::Skipped
            || report.project_root.join(".git").exists());
    let data = json!({"project_root":report.project_root,"metadata_path":report.metadata_path,
        "metadata_status":report.metadata_status.to_string(),"gitignore_path":report.gitignore_path,
        "gitignore_status":report.gitignore_status.to_string(),"vcs":report.git_setup.to_string(),
        "effects":{"local":"applied","remote":"unapplied"},"postconditions_verified":verified});
    if !verified {
        return Err(Failure::new(
            "persistence.unverified",
            "Project creation could not be verified.",
        )
        .with_data({
            let mut data = data;
            data["partial"] = json!(true);
            data
        }));
    }
    Ok(data)
}

#[cfg(test)]
mod machine_tests {
    #[test]
    fn machine_contract_new_creates_metadata_and_preserves_existing_target() {
        let root = std::env::temp_dir().join(format!("machine-new-{}", uuid::Uuid::new_v4()));
        let args = crate::args::parse_cli(
            "cargo-ai",
            ["cargo-ai", "new", root.to_str().unwrap(), "--vcs", "none"]
                .into_iter()
                .map(Into::into)
                .collect(),
        )
        .unwrap();
        let args = args.subcommand_matches("new").unwrap();
        let result = super::machine_run(args).unwrap();
        assert_eq!(result["postconditions_verified"], true);
        assert!(root.join(".cargo-ai/project.toml").is_file());
        assert!(!root.join(".git").exists());
        let before = std::fs::read(root.join(".cargo-ai/project.toml")).unwrap();
        let error = super::machine_run(args).unwrap_err();
        assert_eq!(error.code, "input.conflict");
        assert_eq!(error.data["effects"]["local"], "unapplied");
        assert_eq!(
            std::fs::read(root.join(".cargo-ai/project.toml")).unwrap(),
            before
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
