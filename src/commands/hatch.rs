//! Runtime behavior for `cargo ai hatch`.
use clap::ArgMatches;
use std::path::{Path, PathBuf};

use super::definition_source::{
    current_lookup_dir, load_definition_contents, looks_like_local_path_input,
    read_definition_json_from_stdin, resolve_definition_source_in_dir, AgentDefinitionSource,
};

struct HatchResolution {
    project_name: String,
    source: AgentDefinitionSource,
}

fn project_root_for_hatch_source(source: &AgentDefinitionSource) -> Option<PathBuf> {
    match source {
        AgentDefinitionSource::LocalPath(path) => {
            crate::commands::tools::maybe_find_project_root(Path::new(path))
        }
        AgentDefinitionSource::RegistryName(_)
        | AgentDefinitionSource::InlineJson(_)
        | AgentDefinitionSource::StdinJson(_) => current_lookup_dir("hatch")
            .ok()
            .and_then(|dir| crate::commands::tools::maybe_find_project_root(dir.as_path())),
    }
}

fn current_hatch_platform_label() -> Option<&'static str> {
    if cfg!(target_os = "macos") {
        Some("macos")
    } else if cfg!(target_os = "linux") {
        Some("linux")
    } else if cfg!(target_os = "windows") {
        Some("windows")
    } else {
        None
    }
}

fn presentation_from_resolution(
    resolution: &HatchResolution,
) -> super::hatch_pipeline::HatchPresentation {
    let source = match &resolution.source {
        AgentDefinitionSource::LocalPath(path) => {
            super::hatch_pipeline::HatchSource::LocalFile { path: path.clone() }
        }
        AgentDefinitionSource::RegistryName(name) => {
            super::hatch_pipeline::HatchSource::Registry { name: name.clone() }
        }
        AgentDefinitionSource::InlineJson(_) => super::hatch_pipeline::HatchSource::InlineJson,
        AgentDefinitionSource::StdinJson(_) => super::hatch_pipeline::HatchSource::StdinJson,
    };

    super::hatch_pipeline::HatchPresentation { source }
}

fn resolve_local_output_dir(sub_m: &ArgMatches) -> Result<Option<PathBuf>, String> {
    super::hatch_pipeline::resolve_output_dir(
        sub_m.get_one::<String>("output_dir").map(String::as_str),
    )
}

fn mode_from_check_flag(check_only: bool) -> super::hatch_pipeline::HatchMode {
    if check_only {
        super::hatch_pipeline::HatchMode::Check
    } else {
        super::hatch_pipeline::HatchMode::Build
    }
}

fn is_supported_project_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
}

fn derive_project_name_from_json_path(path: &str) -> Result<String, String> {
    let stem = Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or_else(|| {
            format!(
                "Unable to derive agent name from config path '{}'. Use `cargo ai hatch <name> --config <path-to-json>`.",
                path
            )
        })?;

    if !is_supported_project_name(stem) {
        return Err(format!(
            "Derived agent name '{}' from '{}' is invalid. Use only letters, numbers, '-' or '_' or run `cargo ai hatch <name> --config <path-to-json>`.",
            stem, path
        ));
    }

    Ok(stem.to_string())
}

fn resolve_hatch_input_in_dir(
    name_or_path: &str,
    config_path: Option<&str>,
    inline_json: Option<&str>,
    stdin_json: Option<&str>,
    current_dir: &Path,
) -> Result<HatchResolution, String> {
    if let Some(source) = config_path
        .map(|path| AgentDefinitionSource::LocalPath(path.to_string()))
        .or_else(|| inline_json.map(|json| AgentDefinitionSource::InlineJson(json.to_string())))
        .or_else(|| stdin_json.map(|json| AgentDefinitionSource::StdinJson(json.to_string())))
    {
        if !is_supported_project_name(name_or_path) {
            return Err(format!(
                "Agent name '{}' is invalid. Use only letters, numbers, '-' or '_'.",
                name_or_path
            ));
        }

        return Ok(HatchResolution {
            project_name: name_or_path.to_string(),
            source,
        });
    }

    let source = resolve_definition_source_in_dir(name_or_path, current_dir, "hatch")?;
    let project_name = match &source {
        AgentDefinitionSource::LocalPath(local_path)
            if looks_like_local_path_input(name_or_path) =>
        {
            derive_project_name_from_json_path(local_path)?
        }
        _ => name_or_path.to_string(),
    };

    Ok(HatchResolution {
        project_name,
        source,
    })
}

fn resolve_hatch_input(
    name_or_path: &str,
    config_path: Option<&str>,
    inline_json: Option<&str>,
    stdin_json: Option<&str>,
) -> Result<HatchResolution, String> {
    let current_dir = current_lookup_dir("hatch")?;
    resolve_hatch_input_in_dir(
        name_or_path,
        config_path,
        inline_json,
        stdin_json,
        &current_dir,
    )
}

fn is_account_hatch_invocation(sub_m: &ArgMatches) -> bool {
    sub_m.get_flag("from_account")
        || sub_m.get_one::<String>("owner_handle").is_some()
        || sub_m.get_one::<String>("definition_path").is_some()
        || sub_m.get_one::<String>("agent").is_some()
}

/// Executes the `hatch` command flow from parsed CLI arguments.
pub async fn run(sub_m: &ArgMatches) -> bool {
    if is_account_hatch_invocation(sub_m) {
        return crate::commands::account::run_hatch(sub_m).await;
    }

    let Some(name_or_path) = sub_m.get_one::<String>("name") else {
        eprintln!("x Missing project name. Use `cargo ai hatch <name>`.");
        return false;
    };
    let check_only = sub_m.get_flag("check");
    let force_overwrite = sub_m.get_flag("force");
    let keep_project = sub_m.get_flag("keep_project");
    let output_dir = match resolve_local_output_dir(sub_m) {
        Ok(output_dir) => output_dir,
        Err(error) => {
            eprintln!("x {}", error);
            return false;
        }
    };
    let build_target = match crate::agent_builder::build_target::BuildTarget::from_cli(
        sub_m.get_one::<String>("target").map(String::as_str),
    ) {
        Ok(build_target) => build_target,
        Err(error) => {
            eprintln!("x {}", error);
            return false;
        }
    };
    let stdin_json = if sub_m.get_flag("stdin") {
        match read_definition_json_from_stdin() {
            Ok(json) => Some(json),
            Err(error) => {
                eprintln!("x {error}");
                return false;
            }
        }
    } else {
        None
    };
    let resolution = match resolve_hatch_input(
        name_or_path,
        sub_m.get_one::<String>("config").map(String::as_str),
        sub_m.get_one::<String>("json").map(String::as_str),
        stdin_json.as_deref(),
    ) {
        Ok(resolution) => resolution,
        Err(error) => {
            eprintln!("x {}", error);
            return false;
        }
    };

    let presentation = presentation_from_resolution(&resolution);
    let new_project_name = resolution.project_name;
    let hatch_mode = mode_from_check_flag(check_only);
    super::hatch_pipeline::print_hatch_start(&new_project_name, hatch_mode);
    super::hatch_pipeline::print_hatch_progress(
        super::hatch_pipeline::HatchProgressStep::PreparingDefinition,
    );

    let file_contents = match load_definition_contents(&resolution.source) {
        Ok(contents) => contents,
        Err(error) => {
            println!("x {error}");
            return false;
        }
    };

    if !sub_m.get_flag("ignore_tools") {
        let definition = match crate::runtime_definition::RuntimeAgentDefinition::from_str(
            file_contents.as_str(),
        ) {
            Ok(definition) => definition,
            Err(error) => {
                println!("x {error}");
                return false;
            }
        };
        let resolver = crate::commands::tools::ToolResolver::new(
            project_root_for_hatch_source(&resolution.source),
            build_target.cache_key_target(),
        );
        match crate::commands::tools::audit_actions_for_tools(
            &definition.actions(),
            &resolver,
            current_hatch_platform_label(),
        ) {
            Ok(_) => {}
            Err(error) => {
                println!("x {error}");
                return false;
            }
        }
    }

    let request = super::hatch_pipeline::HatchRequest::new(
        new_project_name,
        file_contents,
        hatch_mode,
        force_overwrite,
        keep_project,
        build_target,
        output_dir,
        presentation,
    );

    super::hatch_pipeline::run_hatch_pipeline(request)
}

#[cfg(test)]
mod tests {
    use super::{
        mode_from_check_flag, resolve_hatch_input, resolve_hatch_input_in_dir,
        resolve_local_output_dir,
    };
    use crate::commands::definition_source::AgentDefinitionSource;
    use crate::commands::hatch_pipeline::HatchMode;
    use clap::{Arg, ArgMatches, Command};
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn output_dir_matches(value: Option<&str>) -> ArgMatches {
        let mut args = vec!["cargo-ai", "hatch", "adder"];
        if let Some(value) = value {
            args.push("--output-dir");
            args.push(value);
        }

        Command::new("cargo-ai")
            .subcommand(
                Command::new("hatch")
                    .arg(Arg::new("name").required(true))
                    .arg(Arg::new("output_dir").long("output-dir").num_args(1)),
            )
            .try_get_matches_from(args)
            .expect("hatch args should parse")
            .subcommand_matches("hatch")
            .expect("hatch subcommand should be available")
            .clone()
    }

    fn temp_json_path(stem: &str) -> String {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after epoch")
            .as_nanos();
        let file_name = format!("cargo-ai-hatch-test-{}-{}.json", stem, nanos);
        std::env::temp_dir()
            .join(file_name)
            .to_string_lossy()
            .to_string()
    }

    fn temp_dir_path(stem: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("cargo-ai-hatch-test-{stem}-{nanos}"))
    }

    fn remove_temp_dir_if_present(path: &Path) {
        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn positional_json_shorthand_derives_project_name() {
        let config_path = temp_json_path("adder_test");
        fs::write(&config_path, r#"{"version":"2026-03-03.r1"}"#)
            .expect("test file should be writable");

        let resolution =
            resolve_hatch_input(&config_path, None, None, None).expect("resolution should succeed");
        assert!(resolution
            .project_name
            .starts_with("cargo-ai-hatch-test-adder_test-"));
        assert_eq!(
            resolution.source,
            AgentDefinitionSource::LocalPath(config_path.clone())
        );

        let _ = fs::remove_file(config_path);
    }

    #[test]
    fn positional_json_shorthand_rejects_invalid_derived_name() {
        let config_path = temp_json_path("bad.name");
        fs::write(&config_path, r#"{"version":"2026-03-03.r1"}"#)
            .expect("test file should be writable");

        let err = match resolve_hatch_input(&config_path, None, None, None) {
            Ok(_) => panic!("resolution should fail"),
            Err(err) => err,
        };
        assert!(err.contains("Derived agent name"));
        assert!(err.contains("invalid"));

        let _ = fs::remove_file(config_path);
    }

    #[test]
    fn explicit_config_keeps_positional_name() {
        let resolution = resolve_hatch_input("adder_custom", Some("./adder.json"), None, None)
            .expect("resolution should succeed");
        assert_eq!(resolution.project_name, "adder_custom");
        assert_eq!(
            resolution.source,
            AgentDefinitionSource::LocalPath("./adder.json".to_string())
        );
    }

    #[test]
    fn explicit_inline_json_keeps_positional_name() {
        let resolution = resolve_hatch_input(
            "adder_custom",
            None,
            Some(r#"{"version":"2026-03-03.r1"}"#),
            None,
        )
        .expect("inline json resolution should succeed");
        assert_eq!(resolution.project_name, "adder_custom");
        assert_eq!(
            resolution.source,
            AgentDefinitionSource::InlineJson(r#"{"version":"2026-03-03.r1"}"#.to_string())
        );
    }

    #[test]
    fn explicit_stdin_json_keeps_positional_name() {
        let resolution = resolve_hatch_input(
            "adder_custom",
            None,
            None,
            Some(r#"{"version":"2026-03-03.r1"}"#),
        )
        .expect("stdin json resolution should succeed");
        assert_eq!(resolution.project_name, "adder_custom");
        assert_eq!(
            resolution.source,
            AgentDefinitionSource::StdinJson(r#"{"version":"2026-03-03.r1"}"#.to_string())
        );
    }

    #[test]
    fn default_resolution_uses_registry_name() {
        let resolution = resolve_hatch_input("adder_registry", None, None, None)
            .expect("resolution should succeed");
        assert_eq!(resolution.project_name, "adder_registry");
        assert_eq!(
            resolution.source,
            AgentDefinitionSource::RegistryName("adder_registry".to_string())
        );
    }

    #[test]
    fn bare_name_prefers_same_directory_json_file() {
        let temp_dir = temp_dir_path("same-dir-fallback");
        fs::create_dir_all(&temp_dir).expect("temp dir should be writable");
        let local_config = temp_dir.join("adder_test.json");
        fs::write(&local_config, r#"{"version":"2026-03-03.r1"}"#)
            .expect("local config should be writable");

        let resolution = resolve_hatch_input_in_dir("adder_test", None, None, None, &temp_dir)
            .expect("resolution should succeed");
        assert_eq!(resolution.project_name, "adder_test");
        assert_eq!(
            resolution.source,
            AgentDefinitionSource::LocalPath(local_config.to_string_lossy().to_string())
        );

        remove_temp_dir_if_present(&temp_dir);
    }

    #[test]
    fn bare_name_uses_registry_when_same_directory_json_is_absent() {
        let temp_dir = temp_dir_path("registry-fallback");
        fs::create_dir_all(&temp_dir).expect("temp dir should be writable");

        let resolution = resolve_hatch_input_in_dir("adder_test", None, None, None, &temp_dir)
            .expect("resolution should succeed");
        assert_eq!(resolution.project_name, "adder_test");
        assert_eq!(
            resolution.source,
            AgentDefinitionSource::RegistryName("adder_test".to_string())
        );

        remove_temp_dir_if_present(&temp_dir);
    }

    #[test]
    fn local_intent_path_without_json_fails_fast() {
        let err = match resolve_hatch_input("~/Developer/cargo-ai/adder_test", None, None, None) {
            Ok(_) => panic!("resolution should fail"),
            Err(err) => err,
        };
        assert!(err.contains("Local config path"));
        assert!(err.contains("was not found") || err.contains("must point to a .json file"));
    }

    #[test]
    fn missing_json_shorthand_fails_fast() {
        let err = match resolve_hatch_input("missing_agent_config.json", None, None, None) {
            Ok(_) => panic!("resolution should fail"),
            Err(err) => err,
        };
        assert!(err.contains("Local config path"));
        assert!(err.contains("was not found"));
    }

    #[test]
    fn check_flag_maps_to_validation_only_mode() {
        assert_eq!(mode_from_check_flag(true), HatchMode::Check);
        assert_eq!(mode_from_check_flag(false), HatchMode::Build);
    }

    #[test]
    fn output_dir_defaults_to_none() {
        let output_dir = resolve_local_output_dir(&output_dir_matches(None))
            .expect("default output dir should resolve");
        assert!(output_dir.is_none());
    }

    #[test]
    fn output_dir_parses_when_provided() {
        let output_dir = resolve_local_output_dir(&output_dir_matches(Some("./dist")))
            .expect("provided output dir should resolve");
        assert_eq!(output_dir, Some(PathBuf::from("./dist")));
    }
}
