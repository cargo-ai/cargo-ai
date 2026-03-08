//! This module handles agent scaffolding in the Cargo-AI agent builds.
//!
//! It creates the `.cargo-ai/agents/{agent_name}` folder and populates it
//! with the necessary config and build files.

use crate::schema_version;
use std::fs;
use std::io::Error;
use std::path::Path;

include!(concat!(env!("OUT_DIR"), "/.generated_templates.rs"));

const MAIN_ARGS_CALL: &str = "    let cmd_args = args::build_cli();";
const PROVENANCE_HOOK_SNIPPET: &str = r#"    let cmd_args = args::build_cli();
    if cmd_args.subcommand_matches("version").is_some() {
        print_agent_version_status();
        return;
    }
    if let Some(sub_m) = cmd_args.subcommand_matches("inspect") {
        print_agent_inspect(sub_m.get_flag("json"));
        return;
    }"#;
const GENERATED_AGENT_VERSION_BLOCK_TEMPLATE: &str =
    include_str!("templates/agent_version_block.rs.tmpl");

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AgentSyncState {
    InSync,
    OutOfSync,
    Unknown,
}

#[cfg_attr(not(test), allow(dead_code))]
fn determine_agent_sync_state(
    generated_by_version: &str,
    generated_template_version: &str,
    local_cargo_ai_version: Option<&str>,
    local_template_version: Option<&str>,
) -> AgentSyncState {
    match (local_cargo_ai_version, local_template_version) {
        (Some(local_cargo_ai), Some(local_template)) => {
            if local_cargo_ai == generated_by_version
                && local_template == generated_template_version
            {
                AgentSyncState::InSync
            } else {
                AgentSyncState::OutOfSync
            }
        }
        _ => AgentSyncState::Unknown,
    }
}

fn sync_state_label(state: AgentSyncState) -> &'static str {
    match state {
        AgentSyncState::InSync => "in_sync",
        AgentSyncState::OutOfSync => "out_of_sync",
        AgentSyncState::Unknown => "unknown",
    }
}

fn rust_string_literal(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_string())
}

fn provenance_block(generated_by_version: &str, template_schema_version: &str) -> String {
    GENERATED_AGENT_VERSION_BLOCK_TEMPLATE
        .replace(
            "__GENERATED_BY_CARGO_AI_VERSION__",
            &rust_string_literal(generated_by_version),
        )
        .replace(
            "__GENERATED_WITH_TEMPLATE_SCHEMA_VERSION__",
            &rust_string_literal(template_schema_version),
        )
        .replace(
            "__SYNC_STATUS_IN_SYNC__",
            &rust_string_literal(sync_state_label(AgentSyncState::InSync)),
        )
        .replace(
            "__SYNC_STATUS_OUT_OF_SYNC__",
            &rust_string_literal(sync_state_label(AgentSyncState::OutOfSync)),
        )
        .replace(
            "__SYNC_STATUS_UNKNOWN__",
            &rust_string_literal(sync_state_label(AgentSyncState::Unknown)),
        )
}

fn inject_version_command_hook(main_source: &str) -> String {
    if !main_source.contains(MAIN_ARGS_CALL) {
        return main_source.to_string();
    }

    main_source.replacen(MAIN_ARGS_CALL, PROVENANCE_HOOK_SNIPPET, 1)
}

fn render_workspace_file_contents(
    file_name: &str,
    template_contents: &str,
    agent_name: &str,
    generated_by_version: &str,
    template_schema_version: &str,
) -> String {
    let mut rendered = if should_replace_agent_identity(file_name) {
        template_contents
            .replace("cargo-ai", agent_name)
            .replace("cargo_ai", agent_name)
    } else {
        template_contents.to_string()
    };

    if file_name == "src/main.rs" {
        rendered = inject_version_command_hook(&rendered);
        rendered.push_str(&provenance_block(
            generated_by_version,
            template_schema_version,
        ));
    }

    rendered
}

fn should_replace_agent_identity(file_name: &str) -> bool {
    matches!(file_name, "Cargo.toml" | "src/args.rs")
}

/// Creates a new agent project directory and initializes required files.
pub fn create_new_agent_project(
    template_path: &Path,
    agent_name: &str,
    agentcfg: Result<String, Error>,
) -> Result<(), Error> {
    create_agent_workspace(agent_name)?;
    seed_agent_workspace(template_path, agent_name)?;
    rewrite_agent_generated_files(
        &super::agent_workspace_path(agent_name),
        agent_name,
        agentcfg,
    )?;
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

/// Creates a safe warmed-template workspace from Cargo AI-owned scaffold inputs.
pub(crate) fn create_template_project(
    base_path: &Path,
    template_agent_name: &str,
    template_agentcfg: &str,
) -> Result<(), Error> {
    if !base_path.exists() {
        fs::create_dir_all(base_path)?;
    }

    write_workspace_files(
        base_path,
        template_agent_name,
        Some(template_agentcfg),
        WorkspaceWriteMode::WriteAll,
    )
}

fn seed_agent_workspace(template_path: &Path, agent_name: &str) -> Result<(), Error> {
    copy_directory_recursive(template_path, &super::agent_workspace_path(agent_name))
}

/// Rewrites only the agent-generated scaffold files after template seeding.
fn rewrite_agent_generated_files(
    base_path: &Path,
    agent_name: &str,
    agentcfg: Result<String, Error>,
) -> Result<(), Error> {
    let provided_agentcfg = agentcfg.ok();
    write_workspace_files(
        base_path,
        agent_name,
        provided_agentcfg.as_deref(),
        WorkspaceWriteMode::GeneratedOnly,
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WorkspaceWriteMode {
    WriteAll,
    GeneratedOnly,
}

/// Writes template files (`build.rs`, `.agentcfg`) to the target workspace.
fn write_workspace_files(
    base_path: &Path,
    agent_name: &str,
    provided_agentcfg: Option<&str>,
    mode: WorkspaceWriteMode,
) -> Result<(), Error> {
    let generated_by_version = env!("CARGO_PKG_VERSION");
    let template_schema_version = provided_agentcfg
        .and_then(schema_version::extract_schema_version_from_agentcfg)
        .unwrap_or_else(schema_version::current_schema_version);

    for (file_name, file_contents) in TEMPLATES {
        let file_path = base_path.join(file_name);

        // Create parent directories if needed
        if let Some(parent) = file_path.parent() {
            fs::create_dir_all(parent)?;
        }

        // Handle custom .agentcfg file
        if file_name == ".agentcfg" {
            if let Some(contents) = provided_agentcfg {
                fs::write(file_path, contents)?;
                continue;
            }
        }

        // Skip replacements for loader.rs so config paths remain shared
        if file_name.ends_with("loader.rs") {
            if mode == WorkspaceWriteMode::WriteAll {
                fs::write(file_path, file_contents)?;
            }
            continue;
        }

        let rendered = render_workspace_file_contents(
            file_name,
            file_contents,
            agent_name,
            generated_by_version,
            &template_schema_version,
        );

        let should_write = match mode {
            WorkspaceWriteMode::WriteAll => true,
            WorkspaceWriteMode::GeneratedOnly => rendered != file_contents,
        };

        if should_write {
            fs::write(file_path, rendered)?;
        }
    }
    Ok(())
}

fn copy_directory_recursive(source: &Path, destination: &Path) -> Result<(), Error> {
    fs::create_dir_all(destination)?;

    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        let file_type = entry.file_type()?;

        if file_type.is_dir() {
            fs::create_dir_all(&destination_path)?;
            copy_directory_recursive(&source_path, &destination_path)?;
        } else {
            fs::copy(&source_path, &destination_path)?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        copy_directory_recursive, determine_agent_sync_state, render_workspace_file_contents,
        should_replace_agent_identity, sync_state_label, AgentSyncState,
    };
    use crate::schema_version;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir_path(stem: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("cargo-ai-project-{stem}-{nanos}"))
    }

    #[test]
    fn resolves_template_schema_version_from_agentcfg_version() {
        let version = schema_version::extract_schema_version_from_agentcfg(
            r#"{"version":"2026-03-03.r2","inputs":[{"type":"text","text":"x"}],"agent_schema":{"type":"object","properties":{}},"actions":[]}"#,
        );
        assert_eq!(version.as_deref(), Some("2026-03-03.r2"));
    }

    #[test]
    fn rejects_legacy_semver_agentcfg_version() {
        let version = schema_version::extract_schema_version_from_agentcfg(
            r#"{"version":"0.0.10","inputs":[{"type":"text","text":"x"}],"agent_schema":{"type":"object","properties":{}},"actions":[]}"#,
        );
        assert!(version.is_none());
    }

    #[test]
    fn falls_back_to_current_schema_version_when_agentcfg_version_is_missing() {
        let version = schema_version::extract_schema_version_from_agentcfg(
            r#"{"inputs":[{"type":"text","text":"x"}],"agent_schema":{"type":"object","properties":{}},"actions":[]}"#,
        );
        assert!(version.is_none());
        let fallback = schema_version::current_schema_version();
        assert!(schema_version::is_valid_schema_version(&fallback));
    }

    #[test]
    fn stamps_main_template_with_provenance_metadata() {
        let rendered = render_workspace_file_contents(
            "src/main.rs",
            "fn main() {\n    let cmd_args = args::build_cli();\n}\n",
            "adder_agent",
            "0.0.11",
            "2026-03-03.r1",
        );

        assert!(rendered.contains("print_agent_version_status();"));
        assert!(rendered.contains(r#"subcommand_matches("version")"#));
        assert!(rendered.contains(r#"subcommand_matches("inspect")"#));
        assert!(rendered.contains("print_agent_inspect(sub_m.get_flag(\"json\"));"));
        assert!(rendered.contains("agent_version_status ="));
        assert!(rendered.contains("generated_agent_provenance"));
        assert!(rendered.contains("generated_agent_inspect_value"));
        assert!(rendered.contains("generated_by_cargo_ai_version"));
        assert!(rendered.contains("generated_with_template_schema_version"));
        assert!(rendered.contains("agent_build_id"));
        assert!(rendered.contains("target_triple"));
        assert!(rendered.contains("definition_sha256"));
        assert!(rendered.contains("embedded_definition_json"));
        assert!(rendered.contains("build_timestamp_utc"));
        assert!(rendered.contains("cargo_ai_metadata"));
        assert!(rendered.contains("0.0.11"));
        assert!(rendered.contains("2026-03-03.r1"));
    }

    #[test]
    fn non_main_templates_do_not_get_provenance_block() {
        let rendered = render_workspace_file_contents(
            "src/args.rs",
            "const BIN: &str = \"cargo-ai\";\n",
            "adder_agent",
            "0.0.11",
            "2026-03-03.r1",
        );

        assert!(rendered.contains("adder_agent"));
        assert!(!rendered.contains("generated_agent_provenance"));
    }

    #[test]
    fn replaces_agent_identity_only_in_manifest_and_cli_files() {
        assert!(should_replace_agent_identity("Cargo.toml"));
        assert!(should_replace_agent_identity("src/args.rs"));
        assert!(!should_replace_agent_identity("src/main.rs"));
        assert!(!should_replace_agent_identity("src/credentials/store.rs"));
    }

    #[test]
    fn preserves_shared_cargo_ai_paths_and_urls_in_runtime_files() {
        let rendered = render_workspace_file_contents(
            "src/main.rs",
            r#"const INFRA_BASE_URL: &str = "https://api.cargo-ai.org";
const KEYCHAIN_SERVICE: &str = "cargo-ai";
const PATH: &str = ".cargo/.cargo-ai/credentials.toml";
fn main() {
    let cmd_args = args::build_cli();
}
"#,
            "number_2_email",
            "0.0.11",
            "2026-03-03.r1",
        );

        assert!(rendered.contains("https://api.cargo-ai.org"));
        assert!(rendered.contains(r#"const KEYCHAIN_SERVICE: &str = "cargo-ai";"#));
        assert!(rendered.contains(r#".cargo/.cargo-ai/credentials.toml"#));
        assert!(!rendered.contains("https://api.number_2_email.org"));
        assert!(!rendered.contains(".cargo/.number_2_email/credentials.toml"));
    }

    #[test]
    fn still_rewrites_manifest_package_name() {
        let rendered = render_workspace_file_contents(
            "Cargo.toml",
            "[package]\nname = \"cargo-ai\"\n",
            "number_2_email",
            "0.0.11",
            "2026-03-03.r1",
        );

        assert!(rendered.contains("name = \"number_2_email\""));
    }

    #[test]
    fn sync_state_is_in_sync_when_versions_match() {
        let state = determine_agent_sync_state(
            "0.0.11",
            "2026-03-03.r1",
            Some("0.0.11"),
            Some("2026-03-03.r1"),
        );
        assert_eq!(state, AgentSyncState::InSync);
        assert_eq!(sync_state_label(state), "in_sync");
    }

    #[test]
    fn sync_state_is_out_of_sync_for_exact_mismatch() {
        let state = determine_agent_sync_state(
            "0.0.11",
            "2026-03-03.r1",
            Some("0.0.12"),
            Some("2026-03-03.r1"),
        );
        assert_eq!(state, AgentSyncState::OutOfSync);
        assert_eq!(sync_state_label(state), "out_of_sync");
    }

    #[test]
    fn sync_state_is_unknown_when_local_baseline_missing() {
        let state =
            determine_agent_sync_state("0.0.11", "2026-03-03.r1", None, Some("2026-03-03.r1"));
        assert_eq!(state, AgentSyncState::Unknown);
        assert_eq!(sync_state_label(state), "unknown");
    }

    #[test]
    fn recursive_copy_copies_nested_files() {
        let source = temp_dir_path("copy-src");
        let destination = temp_dir_path("copy-dst");
        fs::create_dir_all(source.join("src")).expect("source tree should be creatable");
        fs::write(source.join("Cargo.toml"), "name = 'seed'").expect("cargo file should write");
        fs::write(source.join("src").join("main.rs"), "fn main() {}")
            .expect("main file should write");

        copy_directory_recursive(&source, &destination).expect("copy should succeed");

        assert_eq!(
            fs::read_to_string(destination.join("Cargo.toml"))
                .ok()
                .as_deref(),
            Some("name = 'seed'")
        );
        assert_eq!(
            fs::read_to_string(destination.join("src").join("main.rs"))
                .ok()
                .as_deref(),
            Some("fn main() {}")
        );

        let _ = fs::remove_dir_all(source);
        let _ = fs::remove_dir_all(destination);
    }
}
