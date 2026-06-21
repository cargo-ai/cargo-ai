//! Runtime behavior for `cargo ai run`.
use clap::ArgMatches;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

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

fn is_account_run_invocation(sub_m: &ArgMatches) -> bool {
    sub_m.get_flag("from_account")
        || sub_m.get_one::<String>("owner_handle").is_some()
        || sub_m.get_one::<String>("definition_path").is_some()
}

fn load_run_definition_from_source(
    source: &AgentDefinitionSource,
) -> Result<(crate::runtime_definition::RuntimeAgentDefinition, String), String> {
    let contents = match source {
        AgentDefinitionSource::LocalPath(path) => fs::read_to_string(path)
            .map_err(|error| format!("failed to read '{}': {error}", Path::new(path).display()))?,
        AgentDefinitionSource::RegistryName(_)
        | AgentDefinitionSource::InlineJson(_)
        | AgentDefinitionSource::StdinJson(_) => load_definition_contents(source)?,
    };
    let definition =
        crate::runtime_definition::RuntimeAgentDefinition::from_str(contents.as_str())?;
    Ok((definition, contents))
}

fn project_root_for_definition_source(source: &AgentDefinitionSource) -> Option<PathBuf> {
    match source {
        AgentDefinitionSource::LocalPath(path) => {
            crate::commands::tools::maybe_find_project_root(Path::new(path))
        }
        AgentDefinitionSource::RegistryName(_)
        | AgentDefinitionSource::InlineJson(_)
        | AgentDefinitionSource::StdinJson(_) => std::env::current_dir()
            .ok()
            .and_then(|dir| crate::commands::tools::maybe_find_project_root(dir.as_path())),
    }
}

fn usage_agent_info_for_definition_source(
    source: &AgentDefinitionSource,
    definition_json: &str,
    project_root: Option<&Path>,
) -> Value {
    let mut value = json!({
        "generated": false,
    });

    match source {
        AgentDefinitionSource::LocalPath(path) => {
            value["source"] = json!("local_path");
            value["artifact"] = json!(path);
            if let Some(name) = derived_agent_name_from_path(path) {
                value["name"] = json!(name);
            }
        }
        AgentDefinitionSource::RegistryName(name) => {
            value["source"] = json!("registry");
            value["name"] = json!(name);
        }
        AgentDefinitionSource::InlineJson(_) => {
            value["source"] = json!("inline_json");
        }
        AgentDefinitionSource::StdinJson(_) => {
            value["source"] = json!("stdin_json");
        }
    }

    if let Ok(definition_sha256) = definition_sha256_from_json_str(definition_json) {
        value["definition_sha256"] = json!(definition_sha256);
    }

    if let Some(project_root) = project_root {
        value["project_root"] = json!(project_root.display().to_string());
    }

    value
}

fn derived_agent_name_from_path(path: &str) -> Option<String> {
    Path::new(path)
        .file_stem()
        .map(|name| name.to_string_lossy().trim().to_string())
        .filter(|name| !name.is_empty())
}

fn definition_sha256_from_json_str(json_str: &str) -> Result<String, String> {
    let root = serde_json::from_str::<Value>(json_str)
        .map_err(|error| format!("failed to parse agent JSON for usage metadata: {error}"))?;
    let canonical = canonicalize_json_value(&root);
    let serialized = serde_json::to_string(&canonical).map_err(|error| {
        format!("failed to serialize canonical agent JSON for usage metadata: {error}")
    })?;
    Ok(sha256_hex(serialized.as_str()))
}

fn canonicalize_json_value(value: &Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.iter().map(canonicalize_json_value).collect()),
        Value::Object(map) => {
            let mut keys = map.keys().cloned().collect::<Vec<_>>();
            keys.sort();

            let mut canonical = Map::new();
            for key in keys {
                if let Some(entry) = map.get(&key) {
                    canonical.insert(key, canonicalize_json_value(entry));
                }
            }

            Value::Object(canonical)
        }
        _ => value.clone(),
    }
}

fn sha256_hex(contents: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(contents.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Executes the interpreted runtime flow from a local or registry JSON definition.
pub async fn run(sub_m: &ArgMatches) -> bool {
    if is_account_run_invocation(sub_m) {
        return crate::commands::account::run_account_agent(sub_m).await;
    }

    let definition_source = match resolve_run_definition_source(sub_m) {
        Ok(source) => source,
        Err(error) => {
            eprintln!("x {error}");
            return false;
        }
    };

    let (definition, definition_json) = match load_run_definition_from_source(&definition_source) {
        Ok(loaded) => loaded,
        Err(error) => {
            eprintln!("x {error}");
            return false;
        }
    };
    let project_root = project_root_for_definition_source(&definition_source);
    let usage_agent_info = usage_agent_info_for_definition_source(
        &definition_source,
        definition_json.as_str(),
        project_root.as_deref(),
    );

    super::runtime::run_with_definition_in_context_and_usage_agent(
        sub_m,
        &definition,
        project_root,
        Some(usage_agent_info),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::{
        definition_sha256_from_json_str, resolve_run_definition_source_in_dir,
        usage_agent_info_for_definition_source, AgentDefinitionSource,
    };
    use std::path::Path;

    fn minimal_definition_json() -> &'static str {
        r#"{
            "version": "2026-03-11.r1",
            "inputs": [{"type": "text", "text": "Return a tiny answer."}],
            "agent_schema": {
                "type": "object",
                "properties": {
                    "answer": {
                        "type": "string"
                    }
                }
            },
            "actions": []
        }"#
    }

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

    #[test]
    fn usage_agent_info_for_local_path_includes_concrete_definition_identity() {
        let info = usage_agent_info_for_definition_source(
            &AgentDefinitionSource::LocalPath("./child_gemma_branch.json".to_string()),
            minimal_definition_json(),
            Some(Path::new(".")),
        );

        assert_eq!(info["source"], "local_path");
        assert_eq!(info["generated"], false);
        assert_eq!(info["artifact"], "./child_gemma_branch.json");
        assert_eq!(info["name"], "child_gemma_branch");
        assert_eq!(info["project_root"], ".");
        let hash = info["definition_sha256"]
            .as_str()
            .expect("definition hash should be present");
        assert_eq!(hash.len(), 64);
        assert!(hash.chars().all(|ch| ch.is_ascii_hexdigit()));
    }

    #[test]
    fn usage_agent_info_for_registry_name_keeps_registry_identity() {
        let info = usage_agent_info_for_definition_source(
            &AgentDefinitionSource::RegistryName("invoice_reviewer".to_string()),
            minimal_definition_json(),
            None,
        );

        assert_eq!(info["source"], "registry");
        assert_eq!(info["generated"], false);
        assert_eq!(info["name"], "invoice_reviewer");
        assert!(info.get("artifact").is_none());
        assert!(info["definition_sha256"].as_str().is_some());
    }

    #[test]
    fn interpreted_definition_hash_uses_canonical_json_ordering() {
        let left = r#"{"b":2,"a":{"z":3,"y":[{"d":4,"c":5}]}}"#;
        let right = r#"{
            "a": {
                "y": [
                    {
                        "c": 5,
                        "d": 4
                    }
                ],
                "z": 3
            },
            "b": 2
        }"#;

        assert_eq!(
            definition_sha256_from_json_str(left).expect("left hash should compute"),
            definition_sha256_from_json_str(right).expect("right hash should compute")
        );
    }
}
