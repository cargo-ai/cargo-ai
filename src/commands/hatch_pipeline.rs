//! Shared hatch pipeline helpers.
//!
//! This module centralizes scaffold/build/export/cleanup execution and config
//! source resolution for hatch-style flows.
use std::fs;
use std::io::{Error, ErrorKind};

/// Execution mode for hatch pipeline.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum HatchMode {
    Build,
    Check,
}

/// Runs the hatch execution pipeline for a single agent definition.
pub(crate) fn run_hatch_pipeline(
    new_project_name: &str,
    file_contents: String,
    mode: HatchMode,
    force_overwrite: bool,
    keep_project: bool,
) -> bool {
    run_hatch_pipeline_with_lock(
        new_project_name,
        file_contents,
        mode,
        force_overwrite,
        keep_project,
        crate::agent_builder::lock::try_acquire_agent_lock,
    )
}

fn run_hatch_pipeline_with_lock<F>(
    new_project_name: &str,
    file_contents: String,
    mode: HatchMode,
    force_overwrite: bool,
    keep_project: bool,
    acquire_lock: F,
) -> bool
where
    F: FnOnce(&str) -> std::io::Result<crate::agent_builder::lock::AgentLockGuard>,
{
    let _agent_lock = match acquire_lock(new_project_name) {
        Ok(lock) => lock,
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
            println!(
                "❌ Agent '{}' is already running a hatch/check operation in another process.",
                new_project_name
            );
            return false;
        }
        Err(error) => {
            println!(
                "❌ Failed to acquire lock for agent '{}' ({}): {}",
                new_project_name,
                crate::agent_builder::agent_workspace_path(new_project_name).display(),
                error
            );
            return false;
        }
    };

    println!(
        "🔒 Acquired workspace lock: {}",
        _agent_lock.path().display()
    );

    if let Err(message) = prepare_workspace_for_hatch(
        new_project_name,
        force_overwrite,
        |agent_name| crate::agent_builder::agent_workspace_path(agent_name).exists(),
        crate::agent_builder::cleanup::delete_agent_workspace,
    ) {
        println!("❌ {message}");
        return false;
    }

    let warmed_template = match crate::agent_builder::template_cache::ensure_warmed_template() {
        Ok(template) => {
            if template.created {
                println!("🧱 Created warmed template: {}", template.path.display());
            } else {
                println!("🧱 Reusing warmed template: {}", template.path.display());
            }
            template
        }
        Err(error) => {
            println!("❌ Failed to prepare warmed template: {error}");
            return false;
        }
    };

    match crate::agent_builder::project::create_new_agent_project(
        &warmed_template.path,
        new_project_name,
        Ok(file_contents),
    ) {
        Ok(_) => println!("✅ Project created successfully."),
        Err(e) => {
            println!("❌ Failed to create project: {e}");
            finalize_workspace(new_project_name, keep_project);
            return false;
        }
    }

    match mode {
        HatchMode::Build => {
            match crate::agent_builder::build::build_agent_project(new_project_name) {
                Ok(_) => println!("✅ Project built successfully."),
                Err(e) => {
                    println!("❌ Build failed: {e}");
                    finalize_workspace(new_project_name, keep_project);
                    return false;
                }
            }

            match crate::agent_builder::export::export_binary(new_project_name, force_overwrite) {
                Ok(_) => println!("✅ Project binary exported successfully."),
                Err(e) => {
                    println!("❌ Export failed: {e}");
                    finalize_workspace(new_project_name, keep_project);
                    return false;
                }
            }
        }
        HatchMode::Check => {
            match crate::agent_builder::build::check_agent_project(new_project_name) {
                Ok(_) => println!("✅ Project checked successfully."),
                Err(e) => {
                    println!("❌ Check failed: {e}");
                    finalize_workspace(new_project_name, keep_project);
                    return false;
                }
            }
        }
    }

    finalize_workspace(new_project_name, keep_project);
    true
}

fn cleanup_workspace(new_project_name: &str) {
    match crate::agent_builder::cleanup::delete_agent_workspace(new_project_name) {
        Ok(_) => println!("🧼 Agent workspace removed."),
        Err(e) => println!("⚠️ Failed to clean up workspace: {e}"),
    }
}

fn finalize_workspace(new_project_name: &str, keep_project: bool) {
    if keep_project {
        let workspace_path = crate::agent_builder::agent_workspace_path(new_project_name);
        if workspace_path.exists() {
            println!("ℹ️ Preserved agent workspace: {}", workspace_path.display());
        }
        return;
    }

    cleanup_workspace(new_project_name);
}

fn prepare_workspace_for_hatch<FExists, FDelete>(
    new_project_name: &str,
    force_overwrite: bool,
    workspace_exists: FExists,
    delete_workspace: FDelete,
) -> Result<(), String>
where
    FExists: FnOnce(&str) -> bool,
    FDelete: FnOnce(&str) -> std::io::Result<()>,
{
    let workspace_path = crate::agent_builder::agent_workspace_path(new_project_name);
    if !workspace_exists(new_project_name) {
        return Ok(());
    }

    if !force_overwrite {
        return Err(format!(
            "Agent project already exists:\n{}\n\nRe-run with --force to replace it, or choose a different local agent name.",
            workspace_path.display()
        ));
    }

    delete_workspace(new_project_name).map_err(|error| {
        format!(
            "Failed to replace existing agent workspace '{}': {error}",
            workspace_path.display()
        )
    })?;
    println!(
        "ℹ️ Existing internal agent workspace removed: {}",
        workspace_path.display()
    );
    Ok(())
}

/// Reads a local agent config file as UTF-8 text.
pub(crate) fn read_local_config(path: &str) -> Result<String, std::io::Error> {
    fs::read_to_string(path)
}

/// Fetches a named agent config template from the Cargo-AI public registry.
pub(crate) fn fetch_from_registry(name: &str) -> Result<String, Error> {
    let url = "https://api.cargo-ai.org/public";
    let client = reqwest::blocking::Client::new();

    let body = serde_json::json!({ "request": name });

    let resp = client
        .post(url)
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .map_err(|e| Error::new(ErrorKind::Other, format!("network error: {e}")))?;

    if !resp.status().is_success() {
        return Err(Error::new(
            ErrorKind::Other,
            format!("HTTP {} for {url}", resp.status()),
        ));
    }

    let text = resp
        .text()
        .map_err(|e| Error::new(ErrorKind::Other, e.to_string()))?;

    // If the registry returns a JSON object with an `error` field,
    // surface it cleanly instead of passing through opaque text.
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) {
        if let Some(err_msg) = value.get("error").and_then(|v| v.as_str()) {
            return Err(Error::new(ErrorKind::Other, err_msg.to_string()));
        }

        // If registry wraps payload in `{ "response": "<json string>" }`, unwrap it.
        if let Some(response) = value.get("response").and_then(|v| v.as_str()) {
            return Ok(response.to_string());
        }
    }

    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::{prepare_workspace_for_hatch, run_hatch_pipeline_with_lock, HatchMode};
    use std::cell::Cell;
    use std::io;

    #[test]
    fn lock_conflict_fails_fast_before_project_mutation() {
        let result = run_hatch_pipeline_with_lock(
            "agent_lock_conflict_test",
            r#"{"version":"2026-03-03.r1"}"#.to_string(),
            HatchMode::Check,
            false,
            false,
            |_| {
                Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "lock already exists",
                ))
            },
        );

        assert!(!result);
    }

    #[test]
    fn existing_workspace_requires_force() {
        let err = prepare_workspace_for_hatch("weather_agent", false, |_| true, |_| Ok(()))
            .expect_err("existing workspace without force should fail");
        assert!(err.contains("Agent project already exists"));
        assert!(err.contains("--force"));
    }

    #[test]
    fn force_replaces_existing_workspace_before_build() {
        let deleted = Cell::new(false);
        prepare_workspace_for_hatch(
            "weather_agent",
            true,
            |_| true,
            |_| {
                deleted.set(true);
                Ok(())
            },
        )
        .expect("force replacement should succeed");

        assert!(deleted.get());
    }
}
