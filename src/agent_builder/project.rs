//! This module handles agent scaffolding in the Cargo-AI agent builds.
//!
//! It creates the `.cargo-ai/agents/{agent_name}` folder and populates it
//! with the necessary config and build files.

use serde_json::Value;
use std::fs;
use std::io::Error;

include!(concat!(env!("OUT_DIR"), "/.generated_templates.rs"));

fn resolve_template_schema_version_from_agentcfg(agentcfg_contents: &str) -> String {
    match serde_json::from_str::<Value>(agentcfg_contents)
        .ok()
        .and_then(|json| {
            json.get("version")
                .and_then(|value| value.as_str())
                .map(|value| value.trim().to_string())
        })
        .filter(|value| !value.is_empty())
    {
        Some(version) => version,
        None => env!("CARGO_PKG_VERSION").to_string(),
    }
}

fn provenance_block(generated_by_version: &str, template_schema_version: &str) -> String {
    format!(
        "\n#[allow(dead_code)]\n\
const GENERATED_BY_CARGO_AI_VERSION: &str = {generated_by_version:?};\n\
#[allow(dead_code)]\n\
const GENERATED_WITH_TEMPLATE_SCHEMA_VERSION: &str = {template_schema_version:?};\n\
\n\
#[allow(dead_code)]\n\
pub fn generated_agent_provenance() -> [(&'static str, &'static str); 2] {{\n\
    [\n\
        (\"generated_by_cargo_ai_version\", GENERATED_BY_CARGO_AI_VERSION),\n\
        (\"generated_with_template_schema_version\", GENERATED_WITH_TEMPLATE_SCHEMA_VERSION),\n\
    ]\n\
}}\n"
    )
}

fn render_workspace_file_contents(
    file_name: &str,
    template_contents: &str,
    agent_name: &str,
    generated_by_version: &str,
    template_schema_version: &str,
) -> String {
    let mut rendered = template_contents
        .replace("cargo-ai", agent_name)
        .replace("cargo_ai", agent_name);

    if file_name == "src/main.rs" {
        rendered.push_str(&provenance_block(
            generated_by_version,
            template_schema_version,
        ));
    }

    rendered
}

/// Creates a new agent project directory and initializes required files.
pub fn create_new_agent_project(
    agent_name: &str,
    agentcfg: Result<String, Error>,
) -> Result<(), Error> {
    create_agent_workspace(agent_name)?;
    load_agent_workspace(agent_name, agentcfg)?;
    Ok(())
}

/// Creates the agent-specific directory under `.cargo-ai/agents/{agent_name}`.
fn create_agent_workspace(agent_name: &str) -> Result<(), Error> {
    let agent_workspace_directory = super::agent_workspace_path(agent_name);
    if !agent_workspace_directory.exists() {
        fs::create_dir_all(agent_workspace_directory)?;
    }
    Ok(())
}

/// Writes template files (`build.rs`, `.agentcfg`) to the agent workspace.
fn load_agent_workspace(agent_name: &str, agentcfg: Result<String, Error>) -> Result<(), Error> {
    let base_path = super::agent_workspace_path(agent_name);
    let provided_agentcfg = agentcfg.ok();
    let generated_by_version = env!("CARGO_PKG_VERSION");
    let template_schema_version = provided_agentcfg
        .as_deref()
        .map(resolve_template_schema_version_from_agentcfg)
        .unwrap_or_else(|| generated_by_version.to_string());

    for (file_name, file_contents) in TEMPLATES {
        let file_path = base_path.join(file_name);

        // Create parent directories if needed
        if let Some(parent) = file_path.parent() {
            fs::create_dir_all(parent)?;
        }

        // Handle custom .agentcfg file
        if file_name == ".agentcfg" {
            if let Some(contents) = provided_agentcfg.as_ref() {
                fs::write(file_path, contents)?;
                continue;
            }
        }

        // Skip replacements for loader.rs so config paths remain shared
        if file_name.ends_with("loader.rs") {
            fs::write(file_path, file_contents)?;
            continue;
        }

        fs::write(
            file_path,
            render_workspace_file_contents(
                file_name,
                file_contents,
                agent_name,
                generated_by_version,
                &template_schema_version,
            ),
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{render_workspace_file_contents, resolve_template_schema_version_from_agentcfg};

    #[test]
    fn resolves_template_schema_version_from_agentcfg_version() {
        let version = resolve_template_schema_version_from_agentcfg(
            r#"{"version":"0.0.10","prompt":"x","agent_schema":{"type":"object","properties":{}},"resource_urls":[],"actions":[]}"#,
        );
        assert_eq!(version, "0.0.10");
    }

    #[test]
    fn falls_back_to_package_version_when_agentcfg_version_is_missing() {
        let version = resolve_template_schema_version_from_agentcfg(
            r#"{"prompt":"x","agent_schema":{"type":"object","properties":{}},"resource_urls":[],"actions":[]}"#,
        );
        assert_eq!(version, env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn stamps_main_template_with_provenance_metadata() {
        let rendered = render_workspace_file_contents(
            "src/main.rs",
            "fn main() {}\n",
            "adder_agent",
            "0.0.11",
            "0.0.10",
        );

        assert!(rendered.contains("generated_agent_provenance"));
        assert!(rendered.contains("generated_by_cargo_ai_version"));
        assert!(rendered.contains("generated_with_template_schema_version"));
        assert!(rendered.contains("0.0.11"));
        assert!(rendered.contains("0.0.10"));
    }

    #[test]
    fn non_main_templates_do_not_get_provenance_block() {
        let rendered = render_workspace_file_contents(
            "src/args.rs",
            "const BIN: &str = \"cargo-ai\";\n",
            "adder_agent",
            "0.0.11",
            "0.0.10",
        );

        assert!(rendered.contains("adder_agent"));
        assert!(!rendered.contains("generated_agent_provenance"));
    }
}
