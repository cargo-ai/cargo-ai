//! Shared helpers for resolving agent definitions from local files or the registry.
use std::env;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum AgentDefinitionSource {
    LocalPath(String),
    RegistryName(String),
    InlineJson(String),
    StdinJson(String),
}

pub(crate) fn current_lookup_dir(lookup_context: &str) -> Result<PathBuf, String> {
    env::current_dir().map_err(|error| {
        format!(
            "Unable to resolve the current directory for local {lookup_context} lookup: {error}"
        )
    })
}

pub(crate) fn is_existing_json_file(path: &str) -> bool {
    let candidate = Path::new(path);
    candidate.is_file()
        && candidate
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.eq_ignore_ascii_case("json"))
            .unwrap_or(false)
}

pub(crate) fn looks_like_local_path_input(input: &str) -> bool {
    input.starts_with("./")
        || input.starts_with("../")
        || input.starts_with("~/")
        || input.contains('/')
        || input.contains('\\')
        || input
            .rsplit_once('.')
            .map(|(_, ext)| ext.eq_ignore_ascii_case("json"))
            .unwrap_or(false)
}

fn resolve_same_directory_json_fallback(name: &str, current_dir: &Path) -> Option<String> {
    let candidate = current_dir.join(format!("{name}.json"));
    candidate
        .is_file()
        .then(|| candidate.to_string_lossy().to_string())
}

pub(crate) fn resolve_local_definition_path_in_dir(
    name_or_path: &str,
    current_dir: &Path,
    registry_hint_command: &str,
) -> Result<Option<String>, String> {
    if looks_like_local_path_input(name_or_path) {
        if is_existing_json_file(name_or_path) {
            return Ok(Some(name_or_path.to_string()));
        }

        let path = Path::new(name_or_path);
        if !path.exists() {
            return Err(format!(
                "Local config path '{}' was not found. Use an existing .json file, or use `cargo ai {} <name>` for a registry definition.",
                name_or_path, registry_hint_command
            ));
        }

        if !path.is_file() {
            return Err(format!(
                "Local config path '{}' is not a file. Provide a .json file path, or use `cargo ai {} <name>` for a registry definition.",
                name_or_path, registry_hint_command
            ));
        }

        return Err(format!(
            "Local config path '{}' must point to a .json file.",
            name_or_path
        ));
    }

    Ok(resolve_same_directory_json_fallback(
        name_or_path,
        current_dir,
    ))
}

pub(crate) fn resolve_definition_source_in_dir(
    name_or_path: &str,
    current_dir: &Path,
    registry_hint_command: &str,
) -> Result<AgentDefinitionSource, String> {
    match resolve_local_definition_path_in_dir(name_or_path, current_dir, registry_hint_command)? {
        Some(local_path) => Ok(AgentDefinitionSource::LocalPath(local_path)),
        None => Ok(AgentDefinitionSource::RegistryName(
            name_or_path.to_string(),
        )),
    }
}

pub(crate) fn resolve_definition_source(
    name_or_path: &str,
    lookup_context: &str,
    registry_hint_command: &str,
) -> Result<AgentDefinitionSource, String> {
    let current_dir = current_lookup_dir(lookup_context)?;
    resolve_definition_source_in_dir(name_or_path, current_dir.as_path(), registry_hint_command)
}

fn validate_stdin_definition_contents(contents: String) -> Result<String, String> {
    if contents.trim().is_empty() {
        return Err("No agent definition JSON was received from stdin.".to_string());
    }

    Ok(contents)
}

pub(crate) fn read_definition_json_from_stdin() -> Result<String, String> {
    let mut contents = String::new();
    io::stdin()
        .read_to_string(&mut contents)
        .map_err(|error| format!("Failed to read agent definition JSON from stdin: {error}"))?;
    validate_stdin_definition_contents(contents)
}

pub(crate) fn load_definition_contents(source: &AgentDefinitionSource) -> Result<String, String> {
    match source {
        AgentDefinitionSource::LocalPath(path) => super::hatch_pipeline::read_local_config(path)
            .map_err(|error| {
                format!(
                    "Failed to read local config file '{}'.\nReason: {}\nHint: Ensure the path is valid and points to a UTF-8 JSON file.",
                    path, error
                )
            }),
        AgentDefinitionSource::RegistryName(name) => {
            super::hatch_pipeline::fetch_from_registry(name).map_err(|error| {
                format!(
                    "Failed to fetch agent configuration for '{}' from Cargo-AI registry.\nReason: {}\nHint: Ensure the agent name exists in the Cargo-AI registry or provide a local .json file path.",
                    name, error
                )
            })
        }
        AgentDefinitionSource::InlineJson(json) | AgentDefinitionSource::StdinJson(json) => {
            Ok(json.clone())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        resolve_definition_source_in_dir, resolve_local_definition_path_in_dir,
        validate_stdin_definition_contents, AgentDefinitionSource,
    };
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir_path(stem: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("cargo-ai-definition-source-{stem}-{nanos}"))
    }

    fn remove_temp_dir_if_present(path: &Path) {
        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn local_json_path_resolves_directly() {
        let temp_dir = temp_dir_path("local-json");
        fs::create_dir_all(&temp_dir).expect("temp dir should be writable");
        let local_config = temp_dir.join("adder.test.json");
        fs::write(
            &local_config,
            r#"{"agent_definition_schema_version":"2026-03-03.r1"}"#,
        )
        .expect("local config should be writable");

        let resolved = resolve_local_definition_path_in_dir(
            local_config.to_string_lossy().as_ref(),
            &temp_dir,
            "run",
        )
        .expect("local resolution should succeed");

        assert_eq!(resolved, Some(local_config.to_string_lossy().to_string()));
        remove_temp_dir_if_present(&temp_dir);
    }

    #[test]
    fn bare_name_prefers_same_directory_json_file() {
        let temp_dir = temp_dir_path("same-dir");
        fs::create_dir_all(&temp_dir).expect("temp dir should be writable");
        let local_config = temp_dir.join("adder_test.json");
        fs::write(
            &local_config,
            r#"{"agent_definition_schema_version":"2026-03-03.r1"}"#,
        )
        .expect("local config should be writable");

        let resolved = resolve_definition_source_in_dir("adder_test", &temp_dir, "run")
            .expect("resolution should succeed");

        assert_eq!(
            resolved,
            AgentDefinitionSource::LocalPath(local_config.to_string_lossy().to_string())
        );
        remove_temp_dir_if_present(&temp_dir);
    }

    #[test]
    fn bare_name_falls_back_to_registry_when_local_json_is_absent() {
        let temp_dir = temp_dir_path("registry");
        fs::create_dir_all(&temp_dir).expect("temp dir should be writable");

        let resolved = resolve_definition_source_in_dir("adder_test", &temp_dir, "run")
            .expect("resolution should succeed");

        assert_eq!(
            resolved,
            AgentDefinitionSource::RegistryName("adder_test".to_string())
        );
        remove_temp_dir_if_present(&temp_dir);
    }

    #[test]
    fn missing_json_like_path_fails_fast() {
        let temp_dir = temp_dir_path("missing-json");
        fs::create_dir_all(&temp_dir).expect("temp dir should be writable");

        let error = resolve_definition_source_in_dir("missing_agent_config.json", &temp_dir, "run")
            .expect_err("missing local config should fail");

        assert!(error.contains("Local config path"));
        assert!(error.contains("was not found"));
        remove_temp_dir_if_present(&temp_dir);
    }

    #[test]
    fn stdin_definition_rejects_empty_input() {
        let error = validate_stdin_definition_contents("   \n\t".to_string())
            .expect_err("whitespace-only stdin should be rejected");

        assert_eq!(error, "No agent definition JSON was received from stdin.");
    }

    #[test]
    fn inline_and_stdin_definition_contents_round_trip() {
        assert_eq!(
            super::load_definition_contents(&AgentDefinitionSource::InlineJson(
                "{\"agent_definition_schema_version\":\"2026-03-03.r1\"}".to_string()
            ))
            .expect("inline json should round trip"),
            "{\"agent_definition_schema_version\":\"2026-03-03.r1\"}"
        );
        assert_eq!(
            super::load_definition_contents(&AgentDefinitionSource::StdinJson(
                "{\"agent_definition_schema_version\":\"2026-03-03.r1\"}".to_string()
            ))
            .expect("stdin json should round trip"),
            "{\"agent_definition_schema_version\":\"2026-03-03.r1\"}"
        );
    }
}
