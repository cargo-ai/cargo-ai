//! Shared scaffolding logic for `cargo ai init` and `cargo ai new`.
use serde::{Deserialize, Serialize};
use std::fmt;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

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
    pub git_setup: GitSetup,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct ProjectMetadataDocument {
    #[serde(default)]
    format_version: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    vcs: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    project: Option<ProjectIdentityDocument>,
    #[serde(default)]
    tools: Option<ProjectToolsPolicyDocument>,
    #[serde(flatten)]
    extra: toml::Table,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct ProjectIdentityDocument {
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<String>,
    #[serde(flatten)]
    extra: toml::Table,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct ProjectToolsPolicyDocument {
    #[serde(default)]
    allow_global_fallback: Option<bool>,
    #[serde(flatten)]
    extra: toml::Table,
}

/// Creates a new Cargo-AI project directory and initializes managed files.
pub fn scaffold_new(target_dir: &Path, vcs_mode: VcsMode) -> Result<ScaffoldReport, String> {
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

    match scaffold_in_place(target_dir, vcs_mode, false) {
        Ok(report) => Ok(report),
        Err(error) => {
            let _ = fs::remove_dir_all(target_dir);
            Err(error)
        }
    }
}

/// Initializes managed files in an existing Cargo-AI project directory.
pub fn scaffold_init(target_dir: &Path, vcs_mode: VcsMode) -> Result<ScaffoldReport, String> {
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

    scaffold_in_place(target_dir, vcs_mode, true)
}

fn scaffold_in_place(
    target_dir: &Path,
    vcs_mode: VcsMode,
    allow_existing_metadata: bool,
) -> Result<ScaffoldReport, String> {
    let metadata_path = target_dir.join(".cargo-ai").join("project.toml");
    let metadata_exists = metadata_path.exists();
    let gitignore_path = target_dir.join(".gitignore");

    let mut managed_paths = Vec::new();
    if !metadata_exists || !allow_existing_metadata {
        managed_paths.push(metadata_path.clone());
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

    let metadata_status = write_project_metadata(
        &metadata_path,
        include_git_metadata,
        default_project_name(target_dir),
    )?;
    let gitignore_status = ensure_gitignore(&gitignore_path, include_git_metadata)?;

    Ok(ScaffoldReport {
        project_root: target_dir.to_path_buf(),
        metadata_path,
        metadata_status,
        gitignore_path,
        gitignore_status,
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
    default_project_name: String,
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
    let rendered = render_project_metadata(
        existing.as_deref(),
        include_git_metadata,
        default_project_name.as_str(),
    );

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

fn render_project_metadata(
    existing: Option<&str>,
    include_git_metadata: bool,
    default_project_name: &str,
) -> String {
    let mut document = existing
        .and_then(|contents| toml::from_str::<ProjectMetadataDocument>(contents).ok())
        .unwrap_or_default();
    document.format_version = 1;
    document.vcs = include_git_metadata.then(|| "git".to_string());
    document.extra.remove("tool");
    document.extra.remove("tool_version");
    document.extra.remove("template");
    document.extra.remove("managed_by");
    document.extra.remove("managed_by_version");

    let mut tools = document.tools.unwrap_or_default();
    if tools.allow_global_fallback.is_none() {
        tools.allow_global_fallback = Some(true);
    }
    document.tools = Some(tools);

    let mut project = document.project.unwrap_or_default();
    if project
        .name
        .as_deref()
        .map(str::trim)
        .unwrap_or_default()
        .is_empty()
    {
        project.name = Some(default_project_name.to_string());
    }
    if project
        .version
        .as_deref()
        .map(str::trim)
        .unwrap_or_default()
        .is_empty()
    {
        project.version = Some("0.1.0".to_string());
    }
    document.project = Some(project);

    let mut rendered =
        toml::to_string_pretty(&document).expect("project metadata should serialize to TOML");
    if !rendered.ends_with('\n') {
        rendered.push('\n');
    }
    rendered
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

fn default_project_name(target_dir: &Path) -> String {
    target_dir
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or("cargo-ai-project")
        .to_string()
}

fn gitignore_has_required_entries(contents: &str) -> bool {
    GITIGNORE_ENTRIES
        .iter()
        .all(|entry| contents.lines().any(|line| line.trim() == *entry))
}

#[cfg(test)]
mod tests {
    use super::{scaffold_init, scaffold_new, ManagedFileStatus, VcsMode};
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};
    use toml::Value;

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

        let err = scaffold_new(&dir, VcsMode::None).expect_err("should fail");
        assert!(err.contains("already exists"));

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn scaffold_init_writes_metadata_only_for_phase_one_bootstrap() {
        let dir = temp_dir_path("init-minimal");
        fs::create_dir_all(&dir).expect("test dir should be created");

        let report = scaffold_init(&dir, VcsMode::None).expect("init should succeed");
        assert_eq!(report.metadata_status, ManagedFileStatus::Created);
        assert!(report.metadata_path.exists());
        assert_eq!(report.gitignore_status, ManagedFileStatus::Skipped);

        let metadata_contents =
            fs::read_to_string(&report.metadata_path).expect("metadata should be readable");
        let parsed: Value = toml::from_str(&metadata_contents).expect("metadata should parse");
        let expected_project_name = super::default_project_name(&dir);
        assert_eq!(
            parsed.get("format_version").and_then(Value::as_integer),
            Some(1)
        );
        assert_eq!(
            parsed
                .get("project")
                .and_then(Value::as_table)
                .and_then(|project| project.get("name"))
                .and_then(Value::as_str),
            Some(expected_project_name.as_str())
        );
        assert_eq!(
            parsed
                .get("project")
                .and_then(Value::as_table)
                .and_then(|project| project.get("version"))
                .and_then(Value::as_str),
            Some("0.1.0")
        );
        assert_eq!(
            parsed
                .get("tools")
                .and_then(Value::as_table)
                .and_then(|tools| tools.get("allow_global_fallback"))
                .and_then(Value::as_bool),
            Some(true)
        );
        assert!(!dir.join("AGENTS.md").exists());
        assert!(!dir.join("CLAUDE.md").exists());
        assert!(!dir.join(".cargo-ai/guidance").exists());
        assert!(!dir.join(".cargo-ai/docs").exists());
        assert!(!dir.join(".cargo-ai/examples").exists());

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn scaffold_init_adds_default_tool_policy_to_existing_minimal_metadata() {
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

        let report = scaffold_init(&dir, VcsMode::None).expect("init should succeed");
        assert_eq!(report.metadata_status, ManagedFileStatus::Updated);
        assert_eq!(report.gitignore_status, ManagedFileStatus::Skipped);

        let metadata_contents =
            fs::read_to_string(&metadata_path).expect("metadata should be readable");
        let parsed: Value = toml::from_str(&metadata_contents).expect("metadata should parse");
        let expected_project_name = super::default_project_name(&dir);
        assert_eq!(
            parsed.get("format_version").and_then(Value::as_integer),
            Some(1)
        );
        assert_eq!(
            parsed
                .get("project")
                .and_then(Value::as_table)
                .and_then(|project| project.get("name"))
                .and_then(Value::as_str),
            Some(expected_project_name.as_str())
        );
        assert_eq!(
            parsed
                .get("project")
                .and_then(Value::as_table)
                .and_then(|project| project.get("version"))
                .and_then(Value::as_str),
            Some("0.1.0")
        );
        assert_eq!(
            parsed
                .get("tools")
                .and_then(Value::as_table)
                .and_then(|tools| tools.get("allow_global_fallback"))
                .and_then(Value::as_bool),
            Some(true)
        );

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

        let report = scaffold_init(&dir, VcsMode::None).expect("init should succeed");
        assert_eq!(report.metadata_status, ManagedFileStatus::Updated);

        let metadata_contents =
            fs::read_to_string(&metadata_path).expect("metadata should be readable");
        let parsed: Value = toml::from_str(&metadata_contents).expect("metadata should parse");
        let expected_project_name = super::default_project_name(&dir);
        assert_eq!(
            parsed.get("format_version").and_then(Value::as_integer),
            Some(1)
        );
        assert_eq!(
            parsed
                .get("project")
                .and_then(Value::as_table)
                .and_then(|project| project.get("name"))
                .and_then(Value::as_str),
            Some(expected_project_name.as_str())
        );
        assert_eq!(
            parsed
                .get("project")
                .and_then(Value::as_table)
                .and_then(|project| project.get("version"))
                .and_then(Value::as_str),
            Some("0.1.0")
        );
        assert_eq!(parsed.get("existing").and_then(Value::as_bool), Some(true));
        assert_eq!(
            parsed
                .get("tools")
                .and_then(Value::as_table)
                .and_then(|tools| tools.get("allow_global_fallback"))
                .and_then(Value::as_bool),
            Some(true)
        );

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn scaffold_init_preserves_explicit_existing_tool_policy() {
        let dir = temp_dir_path("init-preserve-tool-policy");
        let metadata_path = dir.join(".cargo-ai").join("project.toml");
        fs::create_dir_all(
            metadata_path
                .parent()
                .expect("metadata parent should exist"),
        )
        .expect("metadata dir should be created");
        fs::write(
            &metadata_path,
            "format_version = 1\n\n[tools]\nallow_global_fallback = false\n",
        )
        .expect("metadata fixture should be written");

        let report = scaffold_init(&dir, VcsMode::None).expect("init should succeed");
        assert_eq!(report.metadata_status, ManagedFileStatus::Updated);

        let metadata_contents =
            fs::read_to_string(&metadata_path).expect("metadata should be readable");
        let parsed: Value = toml::from_str(&metadata_contents).expect("metadata should parse");
        let expected_project_name = super::default_project_name(&dir);
        assert_eq!(
            parsed
                .get("tools")
                .and_then(Value::as_table)
                .and_then(|tools| tools.get("allow_global_fallback"))
                .and_then(Value::as_bool),
            Some(false)
        );
        assert_eq!(
            parsed
                .get("project")
                .and_then(Value::as_table)
                .and_then(|project| project.get("name"))
                .and_then(Value::as_str),
            Some(expected_project_name.as_str())
        );
        assert_eq!(
            parsed
                .get("project")
                .and_then(Value::as_table)
                .and_then(|project| project.get("version"))
                .and_then(Value::as_str),
            Some("0.1.0")
        );

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn scaffold_init_preserves_existing_build_section() {
        let dir = temp_dir_path("init-preserve-build");
        let metadata_path = dir.join(".cargo-ai").join("project.toml");
        fs::create_dir_all(
            metadata_path
                .parent()
                .expect("metadata parent should exist"),
        )
        .expect("metadata dir should be created");
        fs::write(
            &metadata_path,
            "format_version = 1\n\n[build.default]\nagent_definitions = [\"agents/demo.json\"]\nhatched_agents = [\"agents/cli.json\"]\ntools = [\"hello_tool\"]\nassets = [\"assets/prompts/\"]\n",
        )
        .expect("metadata fixture should be written");

        let report = scaffold_init(&dir, VcsMode::None).expect("init should succeed");
        assert_eq!(report.metadata_status, ManagedFileStatus::Updated);

        let metadata_contents =
            fs::read_to_string(&metadata_path).expect("metadata should be readable");
        let parsed: Value = toml::from_str(&metadata_contents).expect("metadata should parse");
        let expected_project_name = super::default_project_name(&dir);
        assert_eq!(
            parsed
                .get("build")
                .and_then(Value::as_table)
                .and_then(|build| build.get("default"))
                .and_then(Value::as_table)
                .and_then(|profile| profile.get("hatched_agents"))
                .and_then(Value::as_array)
                .and_then(|entries| entries.first())
                .and_then(Value::as_str),
            Some("agents/cli.json")
        );
        assert_eq!(
            parsed
                .get("tools")
                .and_then(Value::as_table)
                .and_then(|tools| tools.get("allow_global_fallback"))
                .and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            parsed
                .get("project")
                .and_then(Value::as_table)
                .and_then(|project| project.get("name"))
                .and_then(Value::as_str),
            Some(expected_project_name.as_str())
        );
        assert_eq!(
            parsed
                .get("project")
                .and_then(Value::as_table)
                .and_then(|project| project.get("version"))
                .and_then(Value::as_str),
            Some("0.1.0")
        );

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn scaffold_init_preserves_existing_project_identity() {
        let dir = temp_dir_path("init-preserve-project");
        let metadata_path = dir.join(".cargo-ai").join("project.toml");
        fs::create_dir_all(
            metadata_path
                .parent()
                .expect("metadata parent should exist"),
        )
        .expect("metadata dir should be created");
        fs::write(
            &metadata_path,
            "format_version = 1\n\n[project]\nname = \"shared_tools\"\nversion = \"1.2.3\"\n",
        )
        .expect("metadata fixture should be written");

        let report = scaffold_init(&dir, VcsMode::None).expect("init should succeed");
        assert_eq!(report.metadata_status, ManagedFileStatus::Updated);

        let metadata_contents =
            fs::read_to_string(&metadata_path).expect("metadata should be readable");
        let parsed: Value = toml::from_str(&metadata_contents).expect("metadata should parse");
        assert_eq!(
            parsed
                .get("project")
                .and_then(Value::as_table)
                .and_then(|project| project.get("name"))
                .and_then(Value::as_str),
            Some("shared_tools")
        );
        assert_eq!(
            parsed
                .get("project")
                .and_then(Value::as_table)
                .and_then(|project| project.get("version"))
                .and_then(Value::as_str),
            Some("1.2.3")
        );
        assert_eq!(
            parsed
                .get("tools")
                .and_then(Value::as_table)
                .and_then(|tools| tools.get("allow_global_fallback"))
                .and_then(Value::as_bool),
            Some(true)
        );

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn scaffold_init_with_git_writes_vcs_and_gitignore_when_git_boundary_exists() {
        let dir = temp_dir_path("init-git");
        fs::create_dir_all(dir.join(".git")).expect("git dir should be created");

        let report = scaffold_init(&dir, VcsMode::Git).expect("init should succeed");
        assert_eq!(report.git_setup, super::GitSetup::AlreadyPresent);
        assert_eq!(report.metadata_status, ManagedFileStatus::Created);
        assert_eq!(report.gitignore_status, ManagedFileStatus::Created);

        let metadata_contents =
            fs::read_to_string(&report.metadata_path).expect("metadata should be readable");
        let parsed: Value = toml::from_str(&metadata_contents).expect("metadata should parse");
        let expected_project_name = super::default_project_name(&dir);
        assert_eq!(
            parsed.get("format_version").and_then(Value::as_integer),
            Some(1)
        );
        assert_eq!(parsed.get("vcs").and_then(Value::as_str), Some("git"));
        assert_eq!(
            parsed
                .get("project")
                .and_then(Value::as_table)
                .and_then(|project| project.get("name"))
                .and_then(Value::as_str),
            Some(expected_project_name.as_str())
        );
        assert_eq!(
            parsed
                .get("project")
                .and_then(Value::as_table)
                .and_then(|project| project.get("version"))
                .and_then(Value::as_str),
            Some("0.1.0")
        );
        assert_eq!(
            parsed
                .get("tools")
                .and_then(Value::as_table)
                .and_then(|tools| tools.get("allow_global_fallback"))
                .and_then(Value::as_bool),
            Some(true)
        );

        let gitignore_contents =
            fs::read_to_string(&report.gitignore_path).expect("gitignore should be readable");
        assert!(gitignore_contents.contains("AGENTS.md"));
        assert!(gitignore_contents.contains(".cargo-ai/guidance/"));
        assert!(gitignore_contents.contains("tools/*/target/"));

        let second = scaffold_init(&dir, VcsMode::Git).expect("second init should succeed");
        assert_eq!(second.metadata_status, ManagedFileStatus::Unchanged);
        assert_eq!(second.gitignore_status, ManagedFileStatus::Unchanged);

        let _ = fs::remove_dir_all(dir);
    }
}
