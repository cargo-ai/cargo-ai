//! Runtime behavior for `cargo ai run`.
use clap::ArgMatches;
use std::path::Path;

enum RunDefinitionSource {
    LocalPath(String),
    RegistryName(String),
}

fn resolve_run_definition_source_in_dir(
    name_or_path: Option<&str>,
    config_path: Option<&str>,
    current_dir: &Path,
) -> Result<RunDefinitionSource, String> {
    if let Some(config_path) = config_path {
        return Ok(RunDefinitionSource::LocalPath(config_path.to_string()));
    }

    let Some(name_or_path) = name_or_path else {
        return Err(
            "Missing agent reference. Use `cargo ai run <name-or-path>` or `cargo ai run --config <path-to-json>`."
                .to_string(),
        );
    };

    match super::hatch::resolve_local_config_path_in_dir(name_or_path, current_dir)? {
        Some(local_path) => Ok(RunDefinitionSource::LocalPath(local_path)),
        None => Ok(RunDefinitionSource::RegistryName(name_or_path.to_string())),
    }
}

fn resolve_run_definition_source(sub_m: &ArgMatches) -> Result<RunDefinitionSource, String> {
    let current_dir = std::env::current_dir().map_err(|error| {
        format!("Unable to resolve the current directory for local run lookup: {error}")
    })?;
    resolve_run_definition_source_in_dir(
        sub_m.get_one::<String>("name").map(String::as_str),
        sub_m.get_one::<String>("config").map(String::as_str),
        current_dir.as_path(),
    )
}

fn load_run_definition(
    source: RunDefinitionSource,
) -> Result<crate::runtime_definition::RuntimeAgentDefinition, String> {
    match source {
        RunDefinitionSource::LocalPath(path) => {
            crate::runtime_definition::RuntimeAgentDefinition::load_from_path(Path::new(&path))
        }
        RunDefinitionSource::RegistryName(name) => {
            let contents = super::hatch_pipeline::fetch_from_registry(&name).map_err(|error| {
                format!(
                    "Failed to fetch agent configuration for '{}' from Cargo-AI registry.\nReason: {error}\nHint: Ensure the agent name exists in the Cargo-AI registry or provide --config <path-to-json>.",
                    name
                )
            })?;
            crate::runtime_definition::RuntimeAgentDefinition::from_str(contents.as_str())
        }
    }
}

/// Executes the interpreted runtime flow from a local or registry JSON definition.
pub async fn run(sub_m: &ArgMatches) -> bool {
    let definition = match resolve_run_definition_source(sub_m).and_then(load_run_definition) {
        Ok(definition) => definition,
        Err(error) => {
            eprintln!("x {error}");
            return false;
        }
    };

    super::preflight::run_with_definition(sub_m, &definition).await
}

#[cfg(test)]
mod tests {
    use super::{resolve_run_definition_source_in_dir, RunDefinitionSource};
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir_path(stem: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("cargo-ai-run-test-{stem}-{nanos}"))
    }

    fn remove_temp_dir_if_present(path: &Path) {
        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn explicit_config_uses_local_path_directly() {
        let resolution = resolve_run_definition_source_in_dir(
            None,
            Some("./adder_test.json"),
            Path::new("/tmp"),
        )
        .expect("resolution should succeed");

        match resolution {
            RunDefinitionSource::LocalPath(path) => assert_eq!(path, "./adder_test.json"),
            RunDefinitionSource::RegistryName(_) => panic!("expected local path resolution"),
        }
    }

    #[test]
    fn positional_json_shorthand_uses_local_path() {
        let temp_dir = temp_dir_path("positional-json");
        fs::create_dir_all(&temp_dir).expect("temp dir should be writable");
        let local_config = temp_dir.join("adder.test.json");
        fs::write(&local_config, r#"{"version":"2026-03-03.r1"}"#)
            .expect("local config should be writable");

        let resolution = resolve_run_definition_source_in_dir(
            Some(local_config.to_string_lossy().as_ref()),
            None,
            &temp_dir,
        )
        .expect("resolution should succeed");

        match resolution {
            RunDefinitionSource::LocalPath(path) => {
                assert_eq!(path, local_config.to_string_lossy().to_string())
            }
            RunDefinitionSource::RegistryName(_) => panic!("expected local path resolution"),
        }

        remove_temp_dir_if_present(&temp_dir);
    }

    #[test]
    fn bare_name_prefers_same_directory_json_file() {
        let temp_dir = temp_dir_path("same-dir-fallback");
        fs::create_dir_all(&temp_dir).expect("temp dir should be writable");
        let local_config = temp_dir.join("adder_test.json");
        fs::write(&local_config, r#"{"version":"2026-03-03.r1"}"#)
            .expect("local config should be writable");

        let resolution =
            resolve_run_definition_source_in_dir(Some("adder_test"), None, &temp_dir)
                .expect("resolution should succeed");

        match resolution {
            RunDefinitionSource::LocalPath(path) => {
                assert_eq!(path, local_config.to_string_lossy().to_string())
            }
            RunDefinitionSource::RegistryName(_) => panic!("expected local path resolution"),
        }

        remove_temp_dir_if_present(&temp_dir);
    }

    #[test]
    fn bare_name_falls_back_to_registry_when_local_json_is_absent() {
        let temp_dir = temp_dir_path("registry-fallback");
        fs::create_dir_all(&temp_dir).expect("temp dir should be writable");

        let resolution =
            resolve_run_definition_source_in_dir(Some("adder_test"), None, &temp_dir)
                .expect("resolution should succeed");

        match resolution {
            RunDefinitionSource::RegistryName(name) => assert_eq!(name, "adder_test"),
            RunDefinitionSource::LocalPath(_) => panic!("expected registry resolution"),
        }

        remove_temp_dir_if_present(&temp_dir);
    }

    #[test]
    fn missing_json_shorthand_fails_fast() {
        let temp_dir = temp_dir_path("missing-json");
        fs::create_dir_all(&temp_dir).expect("temp dir should be writable");

        let err = match resolve_run_definition_source_in_dir(
            Some("missing_agent_config.json"),
            None,
            &temp_dir,
        ) {
            Ok(_) => panic!("resolution should fail"),
            Err(err) => err,
        };

        assert!(err.contains("Local config path"));
        assert!(err.contains("was not found"));

        remove_temp_dir_if_present(&temp_dir);
    }
}
