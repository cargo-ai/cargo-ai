//! Shared scaffolding logic for `cargo ai init` and `cargo ai new`.
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const CODEX_TEMPLATE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/templates/codex/agent-guidance.md.tmpl"
));
const CLAUDE_TEMPLATE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/templates/claude/assistant-guidance.md.tmpl"
));
const EXAMPLE_AGENT_MINIMAL: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/templates/shared/examples/agent-minimal.json"
));
const EXAMPLE_AGENT_ARRAY_VALID: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/templates/shared/examples/agent-array-valid.json"
));
const EXAMPLE_AGENT_LOGIC_INVALID_VAR: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/templates/shared/examples/invalid/agent-logic-invalid-var.json"
));
const DOC_SCHEMA_QUICK_REFERENCE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/templates/shared/docs/schema-quick-reference.md"
));
const DOC_HATCH_CHECK_LOOP: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/templates/shared/docs/hatch-check-loop.md"
));

#[derive(Clone, Copy)]
struct TemplateArtifact {
    relative_path: &'static str,
    contents: &'static str,
}

const COMMON_TEMPLATE_ARTIFACTS: [TemplateArtifact; 5] = [
    TemplateArtifact {
        relative_path: ".cargo-ai/examples/agent-minimal.json",
        contents: EXAMPLE_AGENT_MINIMAL,
    },
    TemplateArtifact {
        relative_path: ".cargo-ai/examples/agent-array-valid.json",
        contents: EXAMPLE_AGENT_ARRAY_VALID,
    },
    TemplateArtifact {
        relative_path: ".cargo-ai/examples/invalid/agent-logic-invalid-var.json",
        contents: EXAMPLE_AGENT_LOGIC_INVALID_VAR,
    },
    TemplateArtifact {
        relative_path: ".cargo-ai/docs/schema-quick-reference.md",
        contents: DOC_SCHEMA_QUICK_REFERENCE,
    },
    TemplateArtifact {
        relative_path: ".cargo-ai/docs/hatch-check-loop.md",
        contents: DOC_HATCH_CHECK_LOOP,
    },
];

/// Supported template overlays for scaffolding.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProjectTemplate {
    Codex,
    Claude,
}

impl ProjectTemplate {
    /// Parses a template value from CLI argument text.
    pub fn from_cli(value: Option<&str>) -> Result<Option<Self>, String> {
        match value {
            None => Ok(None),
            Some("codex") => Ok(Some(Self::Codex)),
            Some("claude") => Ok(Some(Self::Claude)),
            Some(other) => Err(format!(
                "Unsupported template '{}'. Use `--template codex` or `--template claude`.",
                other
            )),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Claude => "claude",
        }
    }

    fn output_file_name(self) -> &'static str {
        match self {
            Self::Codex => "AGENTS.md",
            Self::Claude => "CLAUDE.md",
        }
    }

    fn output_file_contents(self) -> &'static str {
        match self {
            Self::Codex => CODEX_TEMPLATE,
            Self::Claude => CLAUDE_TEMPLATE,
        }
    }

    fn artifacts(self) -> Vec<TemplateArtifact> {
        let mut artifacts = Vec::with_capacity(1 + COMMON_TEMPLATE_ARTIFACTS.len());
        artifacts.push(TemplateArtifact {
            relative_path: self.output_file_name(),
            contents: self.output_file_contents(),
        });
        artifacts.extend_from_slice(&COMMON_TEMPLATE_ARTIFACTS);
        artifacts
    }
}

/// Supported version-control initialization modes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VcsMode {
    Git,
    None,
}

impl VcsMode {
    /// Parses VCS mode from CLI argument text.
    pub fn from_cli(value: Option<&str>) -> Result<Self, String> {
        match value.unwrap_or("git") {
            "git" => Ok(Self::Git),
            "none" => Ok(Self::None),
            other => Err(format!(
                "Unsupported VCS mode '{}'. Use `--vcs git` or `--vcs none`.",
                other
            )),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Git => "git",
            Self::None => "none",
        }
    }
}

/// Git setup result for scaffold execution reporting.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GitSetup {
    Initialized,
    AlreadyPresent,
    Skipped,
}

impl fmt::Display for GitSetup {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Initialized => write!(f, "initialized"),
            Self::AlreadyPresent => write!(f, "already-present"),
            Self::Skipped => write!(f, "skipped"),
        }
    }
}

/// Structured scaffold output for CLI reporting and tests.
#[derive(Debug)]
pub struct ScaffoldReport {
    pub project_root: PathBuf,
    pub metadata_path: PathBuf,
    pub metadata_written: bool,
    pub template_output_path: Option<PathBuf>,
    pub git_setup: GitSetup,
}

/// Creates a new Cargo-AI project directory and initializes managed files.
pub fn scaffold_new(
    target_dir: &Path,
    template: Option<ProjectTemplate>,
    vcs_mode: VcsMode,
) -> Result<ScaffoldReport, String> {
    if target_dir.exists() {
        return Err(format!(
            "Target path '{}' already exists. Use `cargo ai init <path>` for existing directories.",
            target_dir.display()
        ));
    }

    fs::create_dir_all(target_dir).map_err(|error| {
        format!(
            "Failed to create project directory '{}': {}",
            target_dir.display(),
            error
        )
    })?;

    match scaffold_in_place(target_dir, template, vcs_mode, false) {
        Ok(report) => Ok(report),
        Err(error) => {
            let _ = fs::remove_dir_all(target_dir);
            Err(error)
        }
    }
}

/// Initializes managed files in an existing Cargo-AI project directory.
pub fn scaffold_init(
    target_dir: &Path,
    template: Option<ProjectTemplate>,
    vcs_mode: VcsMode,
) -> Result<ScaffoldReport, String> {
    if !target_dir.exists() {
        return Err(format!(
            "Target path '{}' does not exist. Use `cargo ai new <path>` to create a new directory.",
            target_dir.display()
        ));
    }

    if !target_dir.is_dir() {
        return Err(format!(
            "Target path '{}' is not a directory.",
            target_dir.display()
        ));
    }

    scaffold_in_place(target_dir, template, vcs_mode, true)
}

fn scaffold_in_place(
    target_dir: &Path,
    template: Option<ProjectTemplate>,
    vcs_mode: VcsMode,
    allow_existing_metadata: bool,
) -> Result<ScaffoldReport, String> {
    let metadata_path = target_dir.join(".cargo-ai").join("project.toml");
    let metadata_exists = metadata_path.exists();
    let template_artifacts = template.map(ProjectTemplate::artifacts).unwrap_or_default();
    let template_output_path =
        template.map(|selected| target_dir.join(selected.output_file_name()));

    let mut managed_paths = Vec::new();
    if !metadata_exists || !allow_existing_metadata {
        managed_paths.push(metadata_path.clone());
    }

    for artifact in &template_artifacts {
        managed_paths.push(target_dir.join(artifact.relative_path));
    }

    ensure_no_conflicts(&managed_paths)?;

    let metadata_written = if metadata_exists {
        false
    } else {
        if let Some(parent) = metadata_path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                format!(
                    "Failed to create metadata directory '{}': {}",
                    parent.display(),
                    error
                )
            })?;
        }

        fs::write(&metadata_path, render_project_metadata(template, vcs_mode)).map_err(
            |error| {
                format!(
                    "Failed to write metadata file '{}': {}",
                    metadata_path.display(),
                    error
                )
            },
        )?;
        true
    };

    for artifact in &template_artifacts {
        let output_path = target_dir.join(artifact.relative_path);
        if let Some(parent) = output_path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                format!(
                    "Failed to create template directory '{}': {}",
                    parent.display(),
                    error
                )
            })?;
        }

        fs::write(&output_path, artifact.contents).map_err(|error| {
            format!(
                "Failed to write template output '{}': {}",
                output_path.display(),
                error
            )
        })?;
    }

    let git_setup = setup_git(target_dir, vcs_mode)?;

    Ok(ScaffoldReport {
        project_root: target_dir.to_path_buf(),
        metadata_path,
        metadata_written,
        template_output_path,
        git_setup,
    })
}

fn ensure_no_conflicts(managed_paths: &[PathBuf]) -> Result<(), String> {
    let mut conflicts = Vec::new();

    for path in managed_paths {
        if path.exists() {
            conflicts.push(path.display().to_string());
        }
    }

    if conflicts.is_empty() {
        return Ok(());
    }

    Err(format!(
        "Scaffold conflicts detected. The following managed file(s) already exist: {}. Remove conflicting files or choose a different target path.",
        conflicts.join(", ")
    ))
}

fn setup_git(target_dir: &Path, vcs_mode: VcsMode) -> Result<GitSetup, String> {
    if vcs_mode == VcsMode::None {
        return Ok(GitSetup::Skipped);
    }

    if target_dir.join(".git").exists() {
        return Ok(GitSetup::AlreadyPresent);
    }

    let status = Command::new("git")
        .arg("init")
        .current_dir(target_dir)
        .status()
        .map_err(|error| {
            format!(
                "Failed to run `git init` in '{}': {}",
                target_dir.display(),
                error
            )
        })?;

    if !status.success() {
        return Err(format!(
            "`git init` failed in '{}'. Exit status: {}.",
            target_dir.display(),
            status
        ));
    }

    Ok(GitSetup::Initialized)
}

fn render_project_metadata(template: Option<ProjectTemplate>, vcs_mode: VcsMode) -> String {
    let selected_template = template.map(ProjectTemplate::as_str).unwrap_or("none");
    format!(
        "# Managed by cargo-ai init/new.\n\
format_version = 1\n\
tool = \"cargo-ai\"\n\
tool_version = \"{}\"\n\
template = \"{}\"\n\
vcs = \"{}\"\n",
        env!("CARGO_PKG_VERSION"),
        selected_template,
        vcs_mode.as_str(),
    )
}

#[cfg(test)]
mod tests {
    use super::{scaffold_init, scaffold_new, ProjectTemplate, VcsMode};
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir_path(stem: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("cargo-ai-scaffold-test-{}-{}", stem, nanos))
    }

    #[test]
    fn scaffold_new_fails_if_target_exists() {
        let dir = temp_dir_path("existing");
        fs::create_dir_all(&dir).expect("test dir should be created");

        let err = scaffold_new(&dir, None, VcsMode::None).expect_err("should fail");
        assert!(err.contains("already exists"));

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn scaffold_init_writes_metadata_and_codex_template() {
        let dir = temp_dir_path("init-codex");
        fs::create_dir_all(&dir).expect("test dir should be created");

        let report = scaffold_init(&dir, Some(ProjectTemplate::Codex), VcsMode::None)
            .expect("init should succeed");
        assert!(report.metadata_written);
        assert!(report.metadata_path.exists());
        let template_path = report
            .template_output_path
            .expect("template output should be present");
        assert_eq!(
            template_path.file_name().and_then(|name| name.to_str()),
            Some("AGENTS.md")
        );
        assert!(template_path.exists());

        let metadata_contents =
            fs::read_to_string(&report.metadata_path).expect("metadata should be readable");
        assert!(metadata_contents.contains("template = \"codex\""));
        assert!(metadata_contents.contains("vcs = \"none\""));
        assert!(dir.join(".cargo-ai/examples/agent-minimal.json").exists());
        assert!(dir
            .join(".cargo-ai/examples/agent-array-valid.json")
            .exists());
        assert!(dir
            .join(".cargo-ai/examples/invalid/agent-logic-invalid-var.json")
            .exists());
        assert!(dir
            .join(".cargo-ai/docs/schema-quick-reference.md")
            .exists());
        assert!(dir.join(".cargo-ai/docs/hatch-check-loop.md").exists());

        let guidance =
            fs::read_to_string(template_path).expect("guidance template output should be readable");
        assert!(guidance.contains("Cargo-AI Agent Authoring (Codex)"));
        assert!(guidance.contains("--config <config.json> --check"));

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn scaffold_init_is_idempotent_when_metadata_exists() {
        let dir = temp_dir_path("init-idempotent");
        let metadata_path = dir.join(".cargo-ai").join("project.toml");
        fs::create_dir_all(
            metadata_path
                .parent()
                .expect("metadata parent should exist"),
        )
        .expect("metadata dir should be created");
        fs::write(&metadata_path, "existing = true\n").expect("metadata fixture should be written");

        let report = scaffold_init(&dir, None, VcsMode::None).expect("init should succeed");
        assert!(!report.metadata_written);
        assert!(report.template_output_path.is_none());

        let metadata_contents =
            fs::read_to_string(&metadata_path).expect("metadata should be readable");
        assert_eq!(metadata_contents, "existing = true\n");

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn scaffold_init_writes_claude_template_and_shared_assets() {
        let dir = temp_dir_path("init-claude");
        fs::create_dir_all(&dir).expect("test dir should be created");

        let report = scaffold_init(&dir, Some(ProjectTemplate::Claude), VcsMode::None)
            .expect("init should succeed");
        assert!(report.metadata_written);
        let template_path = report
            .template_output_path
            .expect("template output should be present");
        assert_eq!(
            template_path.file_name().and_then(|name| name.to_str()),
            Some("CLAUDE.md")
        );
        assert!(!dir.join("AGENTS.md").exists());
        assert!(dir.join(".cargo-ai/examples/agent-minimal.json").exists());
        assert!(dir
            .join(".cargo-ai/docs/schema-quick-reference.md")
            .exists());

        let guidance =
            fs::read_to_string(template_path).expect("guidance template output should be readable");
        assert!(guidance.contains("Cargo-AI Agent Authoring (Claude)"));

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn scaffold_init_allows_template_add_when_metadata_exists() {
        let dir = temp_dir_path("init-template-add");
        let metadata_path = dir.join(".cargo-ai").join("project.toml");
        fs::create_dir_all(
            metadata_path
                .parent()
                .expect("metadata parent should exist"),
        )
        .expect("metadata dir should be created");
        fs::write(&metadata_path, "existing = true\n").expect("metadata fixture should be written");

        let report = scaffold_init(&dir, Some(ProjectTemplate::Codex), VcsMode::None)
            .expect("template add should succeed");
        assert!(!report.metadata_written);
        let template_path = report
            .template_output_path
            .expect("template output should be present");
        assert_eq!(
            template_path.file_name().and_then(|name| name.to_str()),
            Some("AGENTS.md")
        );
        assert!(dir.join(".cargo-ai/examples/agent-minimal.json").exists());

        let metadata_contents =
            fs::read_to_string(&metadata_path).expect("metadata should be readable");
        assert_eq!(metadata_contents, "existing = true\n");

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn scaffold_init_fails_when_template_output_conflicts() {
        let dir = temp_dir_path("init-template-conflict");
        fs::create_dir_all(&dir).expect("test dir should be created");
        let conflict_path = dir.join("AGENTS.md");
        fs::write(&conflict_path, "# existing")
            .expect("template conflict fixture should be written");

        let err = scaffold_init(&dir, Some(ProjectTemplate::Codex), VcsMode::None)
            .expect_err("init should fail");
        assert!(err.contains("managed file"));
        assert!(err.contains("AGENTS.md"));

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn scaffold_init_fails_when_companion_asset_conflicts() {
        let dir = temp_dir_path("init-companion-conflict");
        let conflict_path = dir.join(".cargo-ai/examples/agent-minimal.json");
        fs::create_dir_all(
            conflict_path
                .parent()
                .expect("conflict parent should be available"),
        )
        .expect("conflict parent should be created");
        fs::write(&conflict_path, "{}").expect("conflict fixture should be written");

        let err = scaffold_init(&dir, Some(ProjectTemplate::Codex), VcsMode::None)
            .expect_err("init should fail");
        assert!(err.contains("managed file"));
        assert!(err.contains("agent-minimal.json"));

        let _ = fs::remove_dir_all(dir);
    }
}
