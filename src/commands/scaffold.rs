//! Shared scaffolding logic for `cargo ai init` and `cargo ai new`.
use std::fmt;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

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
const EXAMPLE_AGENT_ENUM_BOUNDS_VALID: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/templates/shared/examples/agent-enum-bounds-valid.json"
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
        relative_path: ".cargo-ai/examples/agent-enum-bounds-valid.json",
        contents: EXAMPLE_AGENT_ENUM_BOUNDS_VALID,
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

const GITIGNORE_BEGIN_MARKER: &str = "# BEGIN cargo-ai managed artifacts";
const GITIGNORE_END_MARKER: &str = "# END cargo-ai managed artifacts";
const GITIGNORE_ENTRIES: [&str; 8] = [
    "AGENTS.md",
    "CLAUDE.md",
    ".cargo-ai/guidance/",
    ".cargo-ai/docs/",
    ".cargo-ai/examples/",
    ".cargo-ai/tools/",
    ".cargo-ai/agents/",
    "tools/*/target/",
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ManagedFileStatus {
    Created,
    Updated,
    Unchanged,
    Skipped,
}

impl fmt::Display for ManagedFileStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Created => write!(f, "created"),
            Self::Updated => write!(f, "updated"),
            Self::Unchanged => write!(f, "unchanged"),
            Self::Skipped => write!(f, "skipped"),
        }
    }
}

/// Structured scaffold output for CLI reporting and tests.
#[derive(Debug)]
pub struct ScaffoldReport {
    pub project_root: PathBuf,
    pub metadata_path: PathBuf,
    pub metadata_status: ManagedFileStatus,
    pub gitignore_path: PathBuf,
    pub gitignore_status: ManagedFileStatus,
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
    let gitignore_path = target_dir.join(".gitignore");
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

    let git_setup = setup_git(target_dir, vcs_mode)?;
    let include_git_metadata = vcs_mode == VcsMode::Git && git_setup != GitSetup::Skipped;

    if let Some(parent) = metadata_path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            format!(
                "Failed to create metadata directory '{}': {}",
                parent.display(),
                error
            )
        })?;
    }

    let metadata_status = write_project_metadata(&metadata_path, include_git_metadata)?;
    let gitignore_status = ensure_gitignore(&gitignore_path, include_git_metadata)?;

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

    Ok(ScaffoldReport {
        project_root: target_dir.to_path_buf(),
        metadata_path,
        metadata_status,
        gitignore_path,
        gitignore_status,
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
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|error| {
            if error.kind() == ErrorKind::NotFound {
                format!(
                    "Git initialization could not be completed in '{}'. Install Git or re-run with `--vcs none`.",
                    target_dir.display()
                )
            } else {
                format!(
                    "Git initialization could not be completed in '{}': {}. Install Git or re-run with `--vcs none`.",
                    target_dir.display(),
                    error
                )
            }
        })?;

    if !status.success() {
        return Err(format!(
            "Git initialization failed in '{}'. Install Git or re-run with `--vcs none` if you do not want version control. Exit status: {}.",
            target_dir.display(), status
        ));
    }

    Ok(GitSetup::Initialized)
}

fn write_project_metadata(
    metadata_path: &Path,
    include_git_metadata: bool,
) -> Result<ManagedFileStatus, String> {
    let existing = match fs::read_to_string(metadata_path) {
        Ok(contents) => Some(contents),
        Err(error) if error.kind() == ErrorKind::NotFound => None,
        Err(error) => {
            return Err(format!(
                "Failed to read metadata file '{}': {}",
                metadata_path.display(),
                error
            ))
        }
    };
    let rendered = render_project_metadata(existing.as_deref(), include_git_metadata);

    let status = match existing.as_deref() {
        None => ManagedFileStatus::Created,
        Some(contents) if contents == rendered => ManagedFileStatus::Unchanged,
        Some(_) => ManagedFileStatus::Updated,
    };

    if status != ManagedFileStatus::Unchanged {
        fs::write(metadata_path, rendered).map_err(|error| {
            format!(
                "Failed to write metadata file '{}': {}",
                metadata_path.display(),
                error
            )
        })?;
    }

    Ok(status)
}

fn render_project_metadata(existing: Option<&str>, include_git_metadata: bool) -> String {
    let mut trailing_lines = Vec::new();

    if let Some(existing) = existing {
        for line in existing.lines() {
            let trimmed = line.trim_start();
            if trimmed.starts_with("format_version")
                || trimmed.starts_with("vcs")
                || trimmed.starts_with("tool ")
                || trimmed.starts_with("tool_version")
                || trimmed.starts_with("template")
                || trimmed.starts_with("managed_by")
                || trimmed.starts_with("managed_by_version")
                || trimmed == "# Managed by cargo-ai init/new."
            {
                continue;
            }

            trailing_lines.push(line.to_string());
        }
    }

    while trailing_lines.first().map(|line| line.trim().is_empty()) == Some(true) {
        trailing_lines.remove(0);
    }
    while trailing_lines.last().map(|line| line.trim().is_empty()) == Some(true) {
        trailing_lines.pop();
    }

    let mut lines = vec!["format_version = 1".to_string()];
    if include_git_metadata {
        lines.push("vcs = \"git\"".to_string());
    }
    if !trailing_lines.is_empty() {
        lines.push(String::new());
        lines.extend(trailing_lines);
    }

    format!("{}\n", lines.join("\n"))
}

fn ensure_gitignore(
    gitignore_path: &Path,
    include_gitignore_block: bool,
) -> Result<ManagedFileStatus, String> {
    if !include_gitignore_block {
        return Ok(ManagedFileStatus::Skipped);
    }

    let existing = match fs::read_to_string(gitignore_path) {
        Ok(contents) => Some(contents),
        Err(error) if error.kind() == ErrorKind::NotFound => None,
        Err(error) => {
            return Err(format!(
                "Failed to read ignore file '{}': {}",
                gitignore_path.display(),
                error
            ))
        }
    };

    if existing
        .as_deref()
        .map(gitignore_has_required_entries)
        .unwrap_or(false)
    {
        return Ok(ManagedFileStatus::Unchanged);
    }

    let block = render_gitignore_block();
    let rendered = match existing {
        None => block,
        Some(mut contents) => {
            if !contents.ends_with('\n') {
                contents.push('\n');
            }
            if !contents.trim_end().is_empty() {
                contents.push('\n');
            }
            contents.push_str(&block);
            contents
        }
    };

    let status = if gitignore_path.exists() {
        ManagedFileStatus::Updated
    } else {
        ManagedFileStatus::Created
    };

    fs::write(gitignore_path, rendered).map_err(|error| {
        format!(
            "Failed to write ignore file '{}': {}",
            gitignore_path.display(),
            error
        )
    })?;

    Ok(status)
}

fn render_gitignore_block() -> String {
    let mut lines = vec![GITIGNORE_BEGIN_MARKER.to_string()];
    lines.extend(GITIGNORE_ENTRIES.iter().map(|entry| entry.to_string()));
    lines.push(GITIGNORE_END_MARKER.to_string());
    format!("{}\n", lines.join("\n"))
}

fn gitignore_has_required_entries(contents: &str) -> bool {
    GITIGNORE_ENTRIES
        .iter()
        .all(|entry| contents.lines().any(|line| line.trim() == *entry))
}

#[cfg(test)]
mod tests {
    use super::{scaffold_init, scaffold_new, ManagedFileStatus, ProjectTemplate, VcsMode};
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
        assert_eq!(report.metadata_status, ManagedFileStatus::Created);
        assert!(report.metadata_path.exists());
        assert_eq!(report.gitignore_status, ManagedFileStatus::Skipped);
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
        assert_eq!(metadata_contents, "format_version = 1\n");
        assert!(dir.join(".cargo-ai/examples/agent-minimal.json").exists());
        assert!(dir
            .join(".cargo-ai/examples/agent-enum-bounds-valid.json")
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
    fn scaffold_init_preserves_valid_existing_metadata() {
        let dir = temp_dir_path("init-preserve");
        let metadata_path = dir.join(".cargo-ai").join("project.toml");
        fs::create_dir_all(
            metadata_path
                .parent()
                .expect("metadata parent should exist"),
        )
        .expect("metadata dir should be created");
        fs::write(&metadata_path, "format_version = 1\n")
            .expect("metadata fixture should be written");

        let report = scaffold_init(&dir, None, VcsMode::None).expect("init should succeed");
        assert_eq!(report.metadata_status, ManagedFileStatus::Unchanged);
        assert_eq!(report.gitignore_status, ManagedFileStatus::Skipped);
        assert!(report.template_output_path.is_none());

        let metadata_contents =
            fs::read_to_string(&metadata_path).expect("metadata should be readable");
        assert_eq!(metadata_contents, "format_version = 1\n");

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn scaffold_init_normalizes_existing_metadata_to_phase_one_contract() {
        let dir = temp_dir_path("init-normalize");
        let metadata_path = dir.join(".cargo-ai").join("project.toml");
        fs::create_dir_all(
            metadata_path
                .parent()
                .expect("metadata parent should exist"),
        )
        .expect("metadata dir should be created");
        fs::write(
            &metadata_path,
            "# Managed by cargo-ai init/new.\n\
tool = \"cargo-ai\"\n\
tool_version = \"0.1.0\"\n\
template = \"codex\"\n\
existing = true\n",
        )
        .expect("metadata fixture should be written");

        let report = scaffold_init(&dir, None, VcsMode::None).expect("init should succeed");
        assert_eq!(report.metadata_status, ManagedFileStatus::Updated);

        let metadata_contents =
            fs::read_to_string(&metadata_path).expect("metadata should be readable");
        assert_eq!(metadata_contents, "format_version = 1\n\nexisting = true\n");

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn scaffold_init_writes_claude_template_and_shared_assets() {
        let dir = temp_dir_path("init-claude");
        fs::create_dir_all(&dir).expect("test dir should be created");

        let report = scaffold_init(&dir, Some(ProjectTemplate::Claude), VcsMode::None)
            .expect("init should succeed");
        assert_eq!(report.metadata_status, ManagedFileStatus::Created);
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
        assert_eq!(report.metadata_status, ManagedFileStatus::Updated);
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
        assert_eq!(metadata_contents, "format_version = 1\n\nexisting = true\n");

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

    #[test]
    fn scaffold_init_with_git_writes_vcs_and_gitignore_when_git_boundary_exists() {
        let dir = temp_dir_path("init-git");
        fs::create_dir_all(dir.join(".git")).expect("git dir should be created");

        let report = scaffold_init(&dir, None, VcsMode::Git).expect("init should succeed");
        assert_eq!(report.git_setup, super::GitSetup::AlreadyPresent);
        assert_eq!(report.metadata_status, ManagedFileStatus::Created);
        assert_eq!(report.gitignore_status, ManagedFileStatus::Created);

        let metadata_contents =
            fs::read_to_string(&report.metadata_path).expect("metadata should be readable");
        assert_eq!(metadata_contents, "format_version = 1\nvcs = \"git\"\n");

        let gitignore_contents =
            fs::read_to_string(&report.gitignore_path).expect("gitignore should be readable");
        assert!(gitignore_contents.contains("AGENTS.md"));
        assert!(gitignore_contents.contains(".cargo-ai/guidance/"));
        assert!(gitignore_contents.contains("tools/*/target/"));

        let second = scaffold_init(&dir, None, VcsMode::Git).expect("second init should succeed");
        assert_eq!(second.metadata_status, ManagedFileStatus::Unchanged);
        assert_eq!(second.gitignore_status, ManagedFileStatus::Unchanged);

        let _ = fs::remove_dir_all(dir);
    }
}
