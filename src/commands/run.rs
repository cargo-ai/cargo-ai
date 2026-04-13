//! Runtime behavior for `cargo ai run`.
use clap::ArgMatches;
use std::path::Path;

use super::definition_source::{
    load_definition_contents, read_definition_json_from_stdin, resolve_definition_source,
    AgentDefinitionSource,
};

#[cfg(test)]
fn resolve_run_definition_source_in_dir(
    name_or_path: Option<&str>,
    config_path: Option<&str>,
    inline_json: Option<&str>,
    stdin_json: Option<&str>,
    current_dir: &Path,
) -> Result<AgentDefinitionSource, String> {
    if let Some(inline_json) = inline_json {
        return Ok(AgentDefinitionSource::InlineJson(inline_json.to_string()));
    }

    if let Some(stdin_json) = stdin_json {
        return Ok(AgentDefinitionSource::StdinJson(stdin_json.to_string()));
    }

    if let Some(config_path) = config_path {
        return Ok(AgentDefinitionSource::LocalPath(config_path.to_string()));
    }

    let Some(name_or_path) = name_or_path else {
        return Err(
            "Missing agent reference. Use `cargo ai run <name-or-path>`, `cargo ai run --config <path-to-json>`, `cargo ai run --json <json>`, or `cargo ai run --stdin`."
                .to_string(),
        );
    };

    super::definition_source::resolve_definition_source_in_dir(name_or_path, current_dir, "run")
}

fn resolve_run_definition_source(sub_m: &ArgMatches) -> Result<AgentDefinitionSource, String> {
    if let Some(inline_json) = sub_m.get_one::<String>("json").map(String::as_str) {
        return Ok(AgentDefinitionSource::InlineJson(inline_json.to_string()));
    }

    if sub_m.get_flag("stdin") {
        return Ok(AgentDefinitionSource::StdinJson(
            read_definition_json_from_stdin()?,
        ));
    }

    if let Some(config_path) = sub_m.get_one::<String>("config").map(String::as_str) {
        return Ok(AgentDefinitionSource::LocalPath(config_path.to_string()));
    }

    let Some(name_or_path) = sub_m.get_one::<String>("name").map(String::as_str) else {
        return Err(
            "Missing agent reference. Use `cargo ai run <name-or-path>`, `cargo ai run --config <path-to-json>`, `cargo ai run --json <json>`, or `cargo ai run --stdin`."
                .to_string(),
        );
    };

    resolve_definition_source(name_or_path, "run", "run")
}

fn load_run_definition_from_source(
    source: &AgentDefinitionSource,
) -> Result<crate::runtime_definition::RuntimeAgentDefinition, String> {
    match source {
        AgentDefinitionSource::LocalPath(path) => {
            crate::runtime_definition::RuntimeAgentDefinition::load_from_path(Path::new(path))
        }
        AgentDefinitionSource::RegistryName(_)
        | AgentDefinitionSource::InlineJson(_)
        | AgentDefinitionSource::StdinJson(_) => {
            let contents = load_definition_contents(source)?;
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
            None,
            None,
            Path::new("/tmp"),
        )
        .expect("resolution should succeed");

        assert_eq!(
            resolution,
            AgentDefinitionSource::LocalPath("./adder_test.json".to_string())
        );
    }

    #[test]
    fn missing_name_and_config_is_rejected() {
        let error = resolve_run_definition_source_in_dir(None, None, None, None, Path::new("/tmp"))
            .expect_err("missing run target should fail");

        assert!(error.contains("Missing agent reference"));
        assert!(error.contains("cargo ai run"));
    }

    #[test]
    fn inline_json_uses_inline_source_directly() {
        let resolution = resolve_run_definition_source_in_dir(
            None,
            None,
            Some(r#"{"version":"2026-03-03.r1"}"#),
            None,
            Path::new("/tmp"),
        )
        .expect("inline json source should succeed");

        assert_eq!(
            resolution,
            AgentDefinitionSource::InlineJson(r#"{"version":"2026-03-03.r1"}"#.to_string())
        );
    }

    #[test]
    fn stdin_json_uses_stdin_source_directly() {
        let resolution = resolve_run_definition_source_in_dir(
            None,
            None,
            None,
            Some(r#"{"version":"2026-03-03.r1"}"#),
            Path::new("/tmp"),
        )
        .expect("stdin json source should succeed");

        assert_eq!(
            resolution,
            AgentDefinitionSource::StdinJson(r#"{"version":"2026-03-03.r1"}"#.to_string())
        );
    }
}
