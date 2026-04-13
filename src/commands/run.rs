//! Runtime behavior for `cargo ai run`.
use clap::ArgMatches;
use std::path::Path;

use super::definition_source::{
    load_definition_contents, resolve_definition_source, AgentDefinitionSource,
};

#[cfg(test)]
fn resolve_run_definition_source_in_dir(
    name_or_path: Option<&str>,
    config_path: Option<&str>,
    current_dir: &Path,
) -> Result<AgentDefinitionSource, String> {
    if let Some(config_path) = config_path {
        return Ok(AgentDefinitionSource::LocalPath(config_path.to_string()));
    }

    let Some(name_or_path) = name_or_path else {
        return Err(
            "Missing agent reference. Use `cargo ai run <name-or-path>` or `cargo ai run --config <path-to-json>`."
                .to_string(),
        );
    };

    super::definition_source::resolve_definition_source_in_dir(name_or_path, current_dir, "run")
}

fn resolve_run_definition_source(sub_m: &ArgMatches) -> Result<AgentDefinitionSource, String> {
    if let Some(config_path) = sub_m.get_one::<String>("config").map(String::as_str) {
        return Ok(AgentDefinitionSource::LocalPath(config_path.to_string()));
    }

    let Some(name_or_path) = sub_m.get_one::<String>("name").map(String::as_str) else {
        return Err(
            "Missing agent reference. Use `cargo ai run <name-or-path>` or `cargo ai run --config <path-to-json>`."
                .to_string(),
        );
    };

    resolve_definition_source(name_or_path, "run", "run")
}

fn load_run_definition_from_source(
    source: &AgentDefinitionSource,
) -> Result<crate::runtime_definition::RuntimeAgentDefinition, String> {
    let contents = load_definition_contents(source)?;
    match source {
        AgentDefinitionSource::LocalPath(path) => {
            crate::runtime_definition::RuntimeAgentDefinition::load_from_path(Path::new(path))
        }
        AgentDefinitionSource::RegistryName(_) => {
            crate::runtime_definition::RuntimeAgentDefinition::from_str(contents.as_str())
        }
    }
}

/// Executes the interpreted runtime flow from a local or registry JSON definition.
pub async fn run(sub_m: &ArgMatches) -> bool {
    let definition_source = match resolve_run_definition_source(sub_m) {
        Ok(source) => source,
        Err(error) => {
            eprintln!("x {error}");
            return false;
        }
    };

    let definition = match load_run_definition_from_source(&definition_source) {
        Ok(definition) => definition,
        Err(error) => {
            eprintln!("x {error}");
            return false;
        }
    };

    super::runtime::run_with_definition(sub_m, &definition).await
}

#[cfg(test)]
mod tests {
    use super::{resolve_run_definition_source_in_dir, AgentDefinitionSource};
    use std::path::Path;

    #[test]
    fn explicit_config_uses_local_path_directly() {
        let resolution = resolve_run_definition_source_in_dir(
            None,
            Some("./adder_test.json"),
            Path::new("/tmp"),
        )
        .expect("resolution should succeed");

        match resolution {
            AgentDefinitionSource::LocalPath(path) => assert_eq!(path, "./adder_test.json"),
            AgentDefinitionSource::RegistryName(_) => panic!("expected local path resolution"),
        }
    }

    #[test]
    fn missing_name_and_config_is_rejected() {
        let error = resolve_run_definition_source_in_dir(None, None, Path::new("/tmp"))
            .expect_err("missing run target should fail");

        assert!(error.contains("Missing agent reference"));
        assert!(error.contains("cargo ai run"));
    }
}
