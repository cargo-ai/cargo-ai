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

    match scaffold_in_place(target_dir, template, vcs_mode) {
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

    scaffold_in_place(target_dir, template, vcs_mode)
}

fn scaffold_in_place(
    target_dir: &Path,
    template: Option<ProjectTemplate>,
    vcs_mode: VcsMode,
) -> Result<ScaffoldReport, String> {
    let metadata_path = target_dir.join(".cargo-ai").join("project.toml");
    let template_output_path =
        template.map(|selected| target_dir.join(selected.output_file_name()));

    ensure_no_conflicts(&metadata_path, template_output_path.as_ref())?;

    if let Some(parent) = metadata_path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            format!(
                "Failed to create metadata directory '{}': {}",
                parent.display(),
                error
            )
        })?;
    }

    fs::write(&metadata_path, render_project_metadata(template, vcs_mode)).map_err(|error| {
        format!(
            "Failed to write metadata file '{}': {}",
            metadata_path.display(),
            error
        )
    })?;

    if let Some(selected) = template {
        let output_path = target_dir.join(selected.output_file_name());
        fs::write(&output_path, selected.output_file_contents()).map_err(|error| {
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
        template_output_path,
        git_setup,
    })
}

fn ensure_no_conflicts(
    metadata_path: &Path,
    template_output_path: Option<&PathBuf>,
) -> Result<(), String> {
    let mut conflicts = Vec::new();

    if metadata_path.exists() {
        conflicts.push(metadata_path.display().to_string());
    }
    if let Some(path) = template_output_path {
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

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn scaffold_init_fails_on_managed_file_conflict() {
        let dir = temp_dir_path("init-conflict");
        let metadata_path = dir.join(".cargo-ai").join("project.toml");
        fs::create_dir_all(
            metadata_path
                .parent()
                .expect("metadata parent should exist"),
        )
        .expect("metadata dir should be created");
        fs::write(&metadata_path, "existing = true").expect("metadata fixture should be written");

        let err = scaffold_init(&dir, Some(ProjectTemplate::Claude), VcsMode::None)
            .expect_err("init should fail");
        assert!(err.contains("managed file"));
        assert!(err.contains("project.toml"));

        let _ = fs::remove_dir_all(dir);
    }
}
