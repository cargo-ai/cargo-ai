//! Runtime behavior for `cargo ai hatch`.
use clap::ArgMatches;
use std::path::{Path, PathBuf};

enum HatchConfigSource {
    LocalPath {
        path: String,
        from_positional_shorthand: bool,
    },
    RegistryName(String),
}

struct HatchResolution {
    project_name: String,
    config_source: HatchConfigSource,
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

fn is_existing_json_file(path: &str) -> bool {
    let p = Path::new(path);
    p.is_file()
        && p.extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.eq_ignore_ascii_case("json"))
            .unwrap_or(false)
}

fn looks_like_local_path_input(input: &str) -> bool {
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

fn resolve_hatch_input(
    name_or_path: &str,
    config_path: Option<&str>,
) -> Result<HatchResolution, String> {
    if let Some(config_path) = config_path {
        if !is_supported_project_name(name_or_path) {
            return Err(format!(
                "Agent name '{}' is invalid. Use only letters, numbers, '-' or '_'.",
                name_or_path
            ));
        }

        return Ok(HatchResolution {
            project_name: name_or_path.to_string(),
            config_source: HatchConfigSource::LocalPath {
                path: config_path.to_string(),
                from_positional_shorthand: false,
            },
        });
    }

    if looks_like_local_path_input(name_or_path) {
        if is_existing_json_file(name_or_path) {
            let derived_project_name = derive_project_name_from_json_path(name_or_path)?;
            return Ok(HatchResolution {
                project_name: derived_project_name,
                config_source: HatchConfigSource::LocalPath {
                    path: name_or_path.to_string(),
                    from_positional_shorthand: true,
                },
            });
        }

        let path = Path::new(name_or_path);
        if !path.exists() {
            return Err(format!(
                "Local config path '{}' was not found. Use an existing .json file, or use `cargo ai hatch <name>` for a registry template.",
                name_or_path
            ));
        }

        if !path.is_file() {
            return Err(format!(
                "Local config path '{}' is not a file. Provide a .json file path, or use `cargo ai hatch <name>` for a registry template.",
                name_or_path
            ));
        }

        return Err(format!(
            "Local config path '{}' must point to a .json file. Use `cargo ai hatch <name> --config <path-to-json>` for explicit naming.",
            name_or_path
        ));
    }

    Ok(HatchResolution {
        project_name: name_or_path.to_string(),
        config_source: HatchConfigSource::RegistryName(name_or_path.to_string()),
    })
}

/// Executes the `hatch` command flow from parsed CLI arguments.
pub fn run(sub_m: &ArgMatches) -> bool {
    let Some(name_or_path) = sub_m.get_one::<String>("name") else {
        eprintln!("❌ Missing project name. Use `cargo ai hatch <name>`.");
        return false;
    };
    let check_only = sub_m.get_flag("check");
    let force_overwrite = sub_m.get_flag("force");
    let keep_project = sub_m.get_flag("keep_project");
    let output_dir = match resolve_local_output_dir(sub_m) {
        Ok(output_dir) => output_dir,
        Err(error) => {
            eprintln!("❌ {}", error);
            return false;
        }
    };
    let build_target = match crate::agent_builder::build_target::BuildTarget::from_cli(
        sub_m.get_one::<String>("target").map(String::as_str),
    ) {
        Ok(build_target) => build_target,
        Err(error) => {
            eprintln!("❌ {}", error);
            return false;
        }
    };
    let resolution = match resolve_hatch_input(
        name_or_path,
        sub_m.get_one::<String>("config").map(String::as_str),
    ) {
        Ok(resolution) => resolution,
        Err(error) => {
            eprintln!("❌ {}", error);
            return false;
        }
    };

    let new_project_name = resolution.project_name;
    let hatch_mode = mode_from_check_flag(check_only);

    if check_only {
        println!("Check new cargo agent: {new_project_name}");
    } else {
        println!("Build new cargo agent: {new_project_name}");
    }

    let file_contents = match resolution.config_source {
        HatchConfigSource::LocalPath {
            path,
            from_positional_shorthand,
        } => {
            if from_positional_shorthand {
                println!(
                    "📄 Detected local JSON config '{}'; derived agent name '{}'.",
                    path, new_project_name
                );
            }

            match super::hatch_pipeline::read_local_config(&path) {
                Ok(contents) => contents,
                Err(e) => {
                    println!("❌ Failed to read local config file '{}'.", path);
                    println!("Reason: {e}");
                    println!("Hint: Ensure the path is valid and points to a UTF-8 JSON file.");
                    return false;
                }
            }
        }
        HatchConfigSource::RegistryName(registry_name) => {
            println!(
                "🌐 No --config flag detected. Fetching default template '{}' from Cargo-AI registry...",
                registry_name
            );

            match super::hatch_pipeline::fetch_from_registry(&registry_name) {
                Ok(contents) => contents,
                Err(e) => {
                    println!(
                        "❌ Failed to fetch agent configuration for '{}' from Cargo-AI registry.",
                        registry_name
                    );
                    println!("Reason: {e}");
                    println!("Hint: Ensure the agent name exists in the Cargo-AI registry or provide --config <path-to-json>.");
                    return false;
                }
            }
        }
    };

    let request = super::hatch_pipeline::HatchRequest::new(
        new_project_name,
        file_contents,
        hatch_mode,
        force_overwrite,
        keep_project,
        build_target,
        output_dir,
    );

    super::hatch_pipeline::run_hatch_pipeline(request)
}

#[cfg(test)]
mod tests {
    use super::{mode_from_check_flag, resolve_hatch_input, resolve_local_output_dir, HatchConfigSource};
    use clap::{Arg, ArgMatches, Command};
    use crate::commands::hatch_pipeline::HatchMode;
    use std::fs;
    use std::path::PathBuf;
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

    #[test]
    fn positional_json_shorthand_derives_project_name() {
        let config_path = temp_json_path("adder_test");
        fs::write(&config_path, r#"{"version":"2026-03-03.r1"}"#)
            .expect("test file should be writable");

        let resolution =
            resolve_hatch_input(&config_path, None).expect("resolution should succeed");
        assert!(resolution
            .project_name
            .starts_with("cargo-ai-hatch-test-adder_test-"));
        match resolution.config_source {
            HatchConfigSource::LocalPath {
                path,
                from_positional_shorthand,
            } => {
                assert_eq!(path, config_path);
                assert!(from_positional_shorthand);
            }
            HatchConfigSource::RegistryName(_) => panic!("expected local path resolution"),
        }

        let _ = fs::remove_file(config_path);
    }

    #[test]
    fn positional_json_shorthand_rejects_invalid_derived_name() {
        let config_path = temp_json_path("bad.name");
        fs::write(&config_path, r#"{"version":"2026-03-03.r1"}"#)
            .expect("test file should be writable");

        let err = match resolve_hatch_input(&config_path, None) {
            Ok(_) => panic!("resolution should fail"),
            Err(err) => err,
        };
        assert!(err.contains("Derived agent name"));
        assert!(err.contains("invalid"));

        let _ = fs::remove_file(config_path);
    }

    #[test]
    fn explicit_config_keeps_positional_name() {
        let resolution = resolve_hatch_input("adder_custom", Some("./adder.json"))
            .expect("resolution should succeed");
        assert_eq!(resolution.project_name, "adder_custom");
        match resolution.config_source {
            HatchConfigSource::LocalPath {
                path,
                from_positional_shorthand,
            } => {
                assert_eq!(path, "./adder.json");
                assert!(!from_positional_shorthand);
            }
            HatchConfigSource::RegistryName(_) => panic!("expected local path resolution"),
        }
    }

    #[test]
    fn default_resolution_uses_registry_name() {
        let resolution =
            resolve_hatch_input("adder_registry", None).expect("resolution should succeed");
        assert_eq!(resolution.project_name, "adder_registry");
        match resolution.config_source {
            HatchConfigSource::RegistryName(name) => assert_eq!(name, "adder_registry"),
            HatchConfigSource::LocalPath { .. } => panic!("expected registry resolution"),
        }
    }

    #[test]
    fn local_intent_path_without_json_fails_fast() {
        let err = match resolve_hatch_input("~/Developer/cargo-ai/adder_test", None) {
            Ok(_) => panic!("resolution should fail"),
            Err(err) => err,
        };
        assert!(err.contains("Local config path"));
        assert!(err.contains("was not found") || err.contains("must point to a .json file"));
    }

    #[test]
    fn missing_json_shorthand_fails_fast() {
        let err = match resolve_hatch_input("missing_agent_config.json", None) {
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
