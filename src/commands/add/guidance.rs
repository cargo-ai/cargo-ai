//! Runtime behavior for `cargo ai add guidance`.
use clap::ArgMatches;
use std::fs;
use std::path::{Path, PathBuf};

const CODEX_GUIDANCE_TEMPLATE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/templates/guidance/codex-agents.md.tmpl"
));
const ACTION_RULES_TEMPLATE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/templates/guidance/action-rules.md"
));
const AGENT_DEFINITION_CONTRACT_TEMPLATE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/templates/guidance/agent-definition-contract.md"
));
const AUTHORING_PATTERNS_TEMPLATE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/templates/guidance/authoring-patterns.md"
));
const EXAMPLES_README_TEMPLATE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/templates/guidance/examples/README.md"
));
const START_HERE_TEMPLATE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/templates/guidance/start-here.md"
));
const PATTERN_SELECTION_TEMPLATE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/templates/guidance/pattern-selection.md"
));
const TROUBLESHOOTING_TEMPLATE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/templates/guidance/troubleshooting.md"
));
const EXAMPLE_BASIC_AGENT: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/templates/guidance/examples/basic-agent.json"
));
const EXAMPLE_SCHEMA_FEATURES: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/templates/guidance/examples/schema-features.json"
));
const EXAMPLE_CHILD_AGENT: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/templates/guidance/examples/child-agent.json"
));
const EXAMPLE_STOP_BY_DEFAULT: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/templates/guidance/examples/stop-by-default.json"
));
const EXAMPLE_CONTINUE_ON_FAILURE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/templates/guidance/examples/continue-on-failure.json"
));
const EXAMPLE_CONDITIONAL_WHEN: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/templates/guidance/examples/conditional-when.json"
));

#[derive(Clone, Copy)]
struct GuidanceArtifact {
    relative_path: &'static str,
    contents: &'static str,
}

const CODEX_GUIDANCE_ARTIFACTS: [GuidanceArtifact; 14] = [
    GuidanceArtifact {
        relative_path: "AGENTS.md",
        contents: CODEX_GUIDANCE_TEMPLATE,
    },
    GuidanceArtifact {
        relative_path: ".cargo-ai/guidance/agent-definition-contract.md",
        contents: AGENT_DEFINITION_CONTRACT_TEMPLATE,
    },
    GuidanceArtifact {
        relative_path: ".cargo-ai/guidance/action-rules.md",
        contents: ACTION_RULES_TEMPLATE,
    },
    GuidanceArtifact {
        relative_path: ".cargo-ai/guidance/authoring-patterns.md",
        contents: AUTHORING_PATTERNS_TEMPLATE,
    },
    GuidanceArtifact {
        relative_path: ".cargo-ai/guidance/examples/README.md",
        contents: EXAMPLES_README_TEMPLATE,
    },
    GuidanceArtifact {
        relative_path: ".cargo-ai/guidance/start-here.md",
        contents: START_HERE_TEMPLATE,
    },
    GuidanceArtifact {
        relative_path: ".cargo-ai/guidance/pattern-selection.md",
        contents: PATTERN_SELECTION_TEMPLATE,
    },
    GuidanceArtifact {
        relative_path: ".cargo-ai/guidance/troubleshooting.md",
        contents: TROUBLESHOOTING_TEMPLATE,
    },
    GuidanceArtifact {
        relative_path: ".cargo-ai/guidance/examples/basic-agent.json",
        contents: EXAMPLE_BASIC_AGENT,
    },
    GuidanceArtifact {
        relative_path: ".cargo-ai/guidance/examples/schema-features.json",
        contents: EXAMPLE_SCHEMA_FEATURES,
    },
    GuidanceArtifact {
        relative_path: ".cargo-ai/guidance/examples/child-agent.json",
        contents: EXAMPLE_CHILD_AGENT,
    },
    GuidanceArtifact {
        relative_path: ".cargo-ai/guidance/examples/stop-by-default.json",
        contents: EXAMPLE_STOP_BY_DEFAULT,
    },
    GuidanceArtifact {
        relative_path: ".cargo-ai/guidance/examples/continue-on-failure.json",
        contents: EXAMPLE_CONTINUE_ON_FAILURE,
    },
    GuidanceArtifact {
        relative_path: ".cargo-ai/guidance/examples/conditional-when.json",
        contents: EXAMPLE_CONDITIONAL_WHEN,
    },
];

#[derive(Debug)]
struct GuidanceBundleReport {
    root_output_path: PathBuf,
    guidance_root: PathBuf,
    artifact_paths: Vec<PathBuf>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum GuidanceStyle {
    Codex,
}

impl GuidanceStyle {
    fn from_cli(value: Option<&str>) -> Result<Self, String> {
        match value {
            Some("codex") => Ok(Self::Codex),
            Some(other) => Err(format!(
                "Unsupported guidance style '{}'. Use `--style codex`.",
                other
            )),
            None => Err(
                "Missing guidance style. Use `cargo ai add guidance --style codex`.".to_string(),
            ),
        }
    }

    fn output_file_name(self) -> &'static str {
        match self {
            Self::Codex => "AGENTS.md",
        }
    }

    fn artifacts(self) -> &'static [GuidanceArtifact] {
        match self {
            Self::Codex => &CODEX_GUIDANCE_ARTIFACTS,
        }
    }
}

fn ensure_no_conflicts(paths: &[PathBuf]) -> Result<(), String> {
    let mut conflicts = Vec::new();

    for path in paths {
        if path.exists() {
            conflicts.push(path.display().to_string());
        }
    }

    if conflicts.is_empty() {
        return Ok(());
    }

    Err(format!(
        "Guidance conflicts detected. The following managed file(s) already exist: {}. Remove conflicting files or choose another directory before retrying.",
        conflicts.join(", ")
    ))
}

fn write_guidance_bundle(
    target_dir: &Path,
    style: GuidanceStyle,
) -> Result<GuidanceBundleReport, String> {
    let artifact_paths = style
        .artifacts()
        .iter()
        .map(|artifact| target_dir.join(artifact.relative_path))
        .collect::<Vec<_>>();

    ensure_no_conflicts(&artifact_paths)?;

    for (artifact, output_path) in style.artifacts().iter().zip(artifact_paths.iter()) {
        if let Some(parent) = output_path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                format!(
                    "Failed to create guidance directory '{}': {}",
                    parent.display(),
                    error
                )
            })?;
        }

        fs::write(output_path, artifact.contents).map_err(|error| {
            format!(
                "Failed to write guidance file '{}': {}",
                output_path.display(),
                error
            )
        })?;
    }

    Ok(GuidanceBundleReport {
        root_output_path: target_dir.join(style.output_file_name()),
        guidance_root: target_dir.join(".cargo-ai").join("guidance"),
        artifact_paths,
    })
}

/// Executes the `guidance` subcommand flow from parsed CLI arguments.
pub fn run(sub_m: &ArgMatches) -> bool {
    if let Err(error) = run_impl(sub_m) {
        eprintln!("❌ {}", error);
        return false;
    }

    true
}

fn run_impl(sub_m: &ArgMatches) -> Result<(), String> {
    let style = GuidanceStyle::from_cli(sub_m.get_one::<String>("style").map(String::as_str))?;
    let current_dir = std::env::current_dir()
        .map_err(|error| format!("Failed to resolve current directory: {error}"))?;
    let report = write_guidance_bundle(&current_dir, style)?;

    println!(
        "✅ Wrote guidance file: {}",
        report.root_output_path.display()
    );
    println!(
        "ℹ️ Added offline guidance bundle: {}",
        report.guidance_root.display()
    );
    for artifact_path in report.artifact_paths.iter().skip(1) {
        println!("   - {}", artifact_path.display());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{write_guidance_bundle, GuidanceStyle};
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir_path(stem: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("cargo-ai-add-guidance-test-{}-{}", stem, nanos))
    }

    #[test]
    fn write_guidance_bundle_writes_agents_md_and_offline_assets_for_codex() {
        let dir = temp_dir_path("codex");
        fs::create_dir_all(&dir).expect("test dir should be created");

        let report =
            write_guidance_bundle(&dir, GuidanceStyle::Codex).expect("guidance write should work");
        assert_eq!(
            report
                .root_output_path
                .file_name()
                .and_then(|name| name.to_str()),
            Some("AGENTS.md")
        );
        assert_eq!(report.artifact_paths.len(), 14);
        assert!(dir
            .join(".cargo-ai/guidance/agent-definition-contract.md")
            .exists());
        assert!(dir.join(".cargo-ai/guidance/action-rules.md").exists());
        assert!(dir
            .join(".cargo-ai/guidance/authoring-patterns.md")
            .exists());
        assert!(dir.join(".cargo-ai/guidance/examples/README.md").exists());
        assert!(dir.join(".cargo-ai/guidance/start-here.md").exists());
        assert!(dir.join(".cargo-ai/guidance/pattern-selection.md").exists());
        assert!(dir.join(".cargo-ai/guidance/troubleshooting.md").exists());
        assert!(dir
            .join(".cargo-ai/guidance/examples/basic-agent.json")
            .exists());
        assert!(dir
            .join(".cargo-ai/guidance/examples/schema-features.json")
            .exists());
        assert!(dir
            .join(".cargo-ai/guidance/examples/child-agent.json")
            .exists());
        assert!(dir
            .join(".cargo-ai/guidance/examples/stop-by-default.json")
            .exists());
        assert!(dir
            .join(".cargo-ai/guidance/examples/continue-on-failure.json")
            .exists());
        assert!(dir
            .join(".cargo-ai/guidance/examples/conditional-when.json")
            .exists());

        let guidance = fs::read_to_string(&report.root_output_path)
            .expect("guidance output should be readable");
        assert!(guidance.contains("Cargo AI Agent Authoring (Codex)"));
        assert!(guidance.contains(".cargo-ai/guidance/start-here.md"));
        assert!(guidance.contains(".cargo-ai/guidance/pattern-selection.md"));
        assert!(guidance.contains(".cargo-ai/guidance/agent-definition-contract.md"));
        assert!(guidance.contains(".cargo-ai/guidance/examples/schema-features.json"));
        assert!(guidance.contains("cargo ai hatch <agent-name> --config <config.json> --check"));
        assert!(guidance.contains("portable across macOS, Windows, and Linux"));

        let rules = fs::read_to_string(dir.join(".cargo-ai/guidance/action-rules.md"))
            .expect("action rules should be readable");
        assert!(rules.contains("failure_mode"));
        assert!(rules.contains("status_variable"));
        assert!(rules.contains("when"));
        assert!(rules.contains("agent-definition-contract.md"));

        let start_here = fs::read_to_string(dir.join(".cargo-ai/guidance/start-here.md"))
            .expect("start-here guidance should be readable");
        assert!(start_here.contains("What do you want this agent to do?"));
        assert!(start_here.contains("current machine"));
        assert!(start_here.contains("portable across macOS, Windows, and Linux"));

        let patterns = fs::read_to_string(dir.join(".cargo-ai/guidance/pattern-selection.md"))
            .expect("pattern selection guidance should be readable");
        assert!(patterns.contains("basic-agent.json"));
        assert!(patterns.contains("schema-features.json"));
        assert!(patterns.contains("continue-on-failure.json"));

        let authoring = fs::read_to_string(dir.join(".cargo-ai/guidance/authoring-patterns.md"))
            .expect("authoring patterns guidance should be readable");
        assert!(authoring.contains("version"));
        assert!(authoring.contains("boxed ASCII"));

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn write_guidance_bundle_fails_when_agents_md_exists() {
        let dir = temp_dir_path("conflict");
        fs::create_dir_all(&dir).expect("test dir should be created");
        fs::write(dir.join("AGENTS.md"), "existing guidance\n")
            .expect("existing guidance file should be written");

        let error = write_guidance_bundle(&dir, GuidanceStyle::Codex)
            .expect_err("existing file should fail");
        assert!(error.contains("AGENTS.md"));
        assert!(error.contains("already exist"));

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn write_guidance_bundle_fails_when_companion_asset_exists() {
        let dir = temp_dir_path("companion-conflict");
        let conflict_path = dir.join(".cargo-ai/guidance/action-rules.md");
        fs::create_dir_all(
            conflict_path
                .parent()
                .expect("conflict parent should be available"),
        )
        .expect("conflict parent should be created");
        fs::write(&conflict_path, "existing guidance rules\n")
            .expect("existing companion file should be written");

        let error = write_guidance_bundle(&dir, GuidanceStyle::Codex)
            .expect_err("existing companion file should fail");
        assert!(error.contains(".cargo-ai/guidance/action-rules.md"));
        assert!(error.contains("already exist"));

        let _ = fs::remove_dir_all(dir);
    }
}
