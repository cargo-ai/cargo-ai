//! Runtime behavior for `cargo ai package`.
use clap::ArgMatches;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

const PROJECT_METADATA_RELATIVE_PATH: &str = ".cargo-ai/project.toml";
const PROJECT_TOOLS_RELATIVE_PATH: &str = ".cargo-ai/tools";
const PACKAGE_MANIFEST_FILE_NAME: &str = "cargo-ai-package.toml";

#[derive(Clone, Debug, Default, Deserialize)]
struct ProjectMetadataDocument {
    #[serde(default)]
    project: Option<ProjectIdentityDocument>,
    #[serde(default)]
    build: BTreeMap<String, BuildProfileDocument>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct ProjectIdentityDocument {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    version: Option<String>,
    #[serde(flatten)]
    extra: toml::Table,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
struct BuildProfileDocument {
    #[serde(default)]
    agent_definitions: Vec<String>,
    #[serde(default)]
    hatched_agents: Vec<String>,
    #[serde(default)]
    tools: Vec<String>,
    #[serde(default)]
    assets: Vec<String>,
}

#[derive(Clone, Debug)]
struct PackageOutputRoot {
    path: PathBuf,
    explicit: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct AssembledPackage {
    pub root_path: PathBuf,
    pub manifest_project_name: Option<String>,
    pub manifest_project_version: Option<String>,
    pub manifest_value: serde_json::Value,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
struct PackageManifestDocument {
    format_version: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    project_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    project_version: Option<String>,
    profile: String,
    agent_definitions: Vec<String>,
    hatched_agents: Vec<String>,
    tools: Vec<String>,
    assets: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
struct GeneratedProjectMetadataDocument {
    format_version: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    project: Option<ProjectIdentityDocument>,
    tools: GeneratedProjectToolsPolicyDocument,
    build: BTreeMap<String, BuildProfileDocument>,
}

#[derive(Clone, Debug, Serialize)]
struct GeneratedProjectToolsPolicyDocument {
    allow_global_fallback: bool,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct ToolManifestBinary {
    #[serde(default)]
    default_name: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct ToolManifestSource {
    manifest_path: String,
}

#[derive(Clone, Debug, Deserialize)]
struct ToolManifestDocument {
    tool_id: String,
    #[serde(default)]
    source: Option<ToolManifestSource>,
    #[serde(default)]
    binary: ToolManifestBinary,
}

#[derive(Clone, Debug)]
struct ProjectSourceToolContext {
    source_manifest_relative_path: String,
    source_root_relative_path: String,
    binary_name: String,
}

#[derive(Clone, Debug)]
struct LoadedProjectMetadata {
    project_identity: Option<ProjectIdentityDocument>,
    build_profile: BuildProfileDocument,
}

pub fn run(sub_m: &ArgMatches) -> bool {
    let profile_name = sub_m
        .get_one::<String>("profile")
        .map(String::as_str)
        .unwrap_or("default");
    match assemble_current_project_package(
        profile_name,
        sub_m.get_one::<String>("output_dir").map(String::as_str),
        sub_m.get_flag("force"),
        true,
    ) {
        Ok(assembled_package) => {
            println!("✓ Package assembled");
            if let Some(project_name) = assembled_package.manifest_project_name.as_deref() {
                println!("Project: {}", project_name);
            }
            if let Some(project_version) = assembled_package.manifest_project_version.as_deref() {
                println!("Version: {}", project_version);
            }
            println!("Profile: {}", profile_name);
            println!("Output:  {}", assembled_package.root_path.display());
            true
        }
        Err(error) => {
            eprintln!("x {error}");
            false
        }
    }
}

fn current_project_root() -> Option<PathBuf> {
    std::env::current_dir()
        .ok()
        .and_then(|dir| crate::commands::tools::maybe_find_project_root(dir.as_path()))
}

pub(crate) fn assemble_current_project_package(
    profile_name: &str,
    raw_output_dir: Option<&str>,
    force: bool,
    print_banner: bool,
) -> Result<AssembledPackage, String> {
    let project_root = current_project_root().ok_or_else(|| {
        "No Cargo AI project metadata was found from the current directory upward.".to_string()
    })?;
    let loaded_metadata = load_project_metadata(&project_root, profile_name)?;
    let output_root = resolve_package_output_root(&project_root, profile_name, raw_output_dir)?;

    if print_banner {
        println!("Packaging profile `{profile_name}`...");
        println!("Project: {}", project_root.display());
        println!("Output:  {}", output_root.path.display());
        println!();
    }

    let manifest = assemble_package_root(
        &project_root,
        profile_name,
        loaded_metadata.project_identity.as_ref(),
        &loaded_metadata.build_profile,
        &output_root,
        force,
    )?;
    let manifest_value = serde_json::to_value(&manifest)
        .map_err(|error| format!("Failed to serialize package manifest JSON: {error}"))?;

    Ok(AssembledPackage {
        root_path: output_root.path.clone(),
        manifest_project_name: manifest.project_name.clone(),
        manifest_project_version: manifest.project_version.clone(),
        manifest_value,
    })
}

fn load_project_metadata(
    project_root: &Path,
    profile_name: &str,
) -> Result<LoadedProjectMetadata, String> {
    let metadata_path = project_root.join(PROJECT_METADATA_RELATIVE_PATH);
    let contents = fs::read_to_string(&metadata_path).map_err(|error| {
        format!(
            "Failed to read project metadata '{}': {}",
            metadata_path.display(),
            error
        )
    })?;
    let mut metadata: ProjectMetadataDocument = toml::from_str(&contents).map_err(|error| {
        format!(
            "Failed to parse project metadata '{}': {}",
            metadata_path.display(),
            error
        )
    })?;
    let Some(profile) = metadata.build.get(profile_name).cloned() else {
        let mut available = metadata.build.keys().cloned().collect::<Vec<_>>();
        available.sort();
        let available_suffix = if available.is_empty() {
            " No `[build.<profile>]` sections were found.".to_string()
        } else {
            format!(" Available profiles: {}.", available.join(", "))
        };
        return Err(format!(
            "Build profile '{}' was not found in '{}'.{}",
            profile_name,
            metadata_path.display(),
            available_suffix
        ));
    };

    if profile.agent_definitions.is_empty()
        && profile.hatched_agents.is_empty()
        && profile.tools.is_empty()
        && profile.assets.is_empty()
    {
        return Err(format!(
            "Build profile '{}' in '{}' does not declare any `agent_definitions`, `hatched_agents`, `tools`, or `assets`.",
            profile_name,
            metadata_path.display()
        ));
    }

    Ok(LoadedProjectMetadata {
        project_identity: normalize_project_identity(metadata.project.take()),
        build_profile: profile,
    })
}

fn resolve_package_output_root(
    project_root: &Path,
    profile_name: &str,
    raw_output_dir: Option<&str>,
) -> Result<PackageOutputRoot, String> {
    let Some(raw_output_dir) = raw_output_dir else {
        return Ok(PackageOutputRoot {
            path: project_root
                .join("target")
                .join("cargo-ai")
                .join("package")
                .join(profile_name),
            explicit: false,
        });
    };

    let trimmed = raw_output_dir.trim();
    if trimmed.is_empty() {
        return Err("Output directory cannot be empty. Provide --output-dir <DIR>.".to_string());
    }

    let output_path = PathBuf::from(trimmed);
    let normalized_project_root = normalize_path(project_root);
    let normalized_output_root = normalize_against_current_dir(&output_path)?;
    if normalized_output_root == normalized_project_root {
        return Err(format!(
            "Output directory '{}' resolves to the current Cargo AI project root. Choose a nested package folder or omit --output-dir to use the default target path.",
            output_path.display()
        ));
    }

    Ok(PackageOutputRoot {
        path: output_path,
        explicit: true,
    })
}

fn assemble_package_root(
    project_root: &Path,
    profile_name: &str,
    project_identity: Option<&ProjectIdentityDocument>,
    build_profile: &BuildProfileDocument,
    output_root: &PackageOutputRoot,
    force: bool,
) -> Result<PackageManifestDocument, String> {
    let build_profile = BuildProfileDocument {
        agent_definitions: dedupe_preserve_order(&build_profile.agent_definitions),
        hatched_agents: dedupe_preserve_order(&build_profile.hatched_agents),
        tools: dedupe_preserve_order(&build_profile.tools),
        assets: dedupe_preserve_order(&build_profile.assets),
    };

    prepare_output_root(output_root, force)?;

    for relative_path in dedupe_preserve_order(
        &build_profile
            .agent_definitions
            .iter()
            .cloned()
            .chain(build_profile.hatched_agents.iter().cloned())
            .collect::<Vec<_>>(),
    ) {
        copy_declared_path(
            project_root,
            relative_path.as_str(),
            output_root.path.as_path(),
            true,
        )?;
    }
    for relative_path in &build_profile.assets {
        copy_declared_path(
            project_root,
            relative_path,
            output_root.path.as_path(),
            false,
        )?;
    }

    let mut copied_source_roots = BTreeSet::new();
    for tool_name in &build_profile.tools {
        let context = load_project_source_tool_context(project_root, tool_name)?;
        if copied_source_roots.insert(context.source_root_relative_path.clone()) {
            copy_tool_source_root(
                project_root,
                context.source_root_relative_path.as_str(),
                output_root.path.as_path(),
            )?;
        }
        write_package_tool_manifest(
            output_root.path.as_path(),
            tool_name,
            context.source_manifest_relative_path.as_str(),
            context.binary_name.as_str(),
        )?;
    }

    write_generated_project_metadata(
        output_root.path.as_path(),
        project_identity,
        profile_name,
        &build_profile,
    )?;

    let manifest = PackageManifestDocument {
        format_version: 1,
        project_name: project_identity.and_then(|project| project.name.clone()),
        project_version: project_identity.and_then(|project| project.version.clone()),
        profile: profile_name.to_string(),
        agent_definitions: build_profile.agent_definitions.clone(),
        hatched_agents: build_profile.hatched_agents.clone(),
        tools: build_profile.tools.clone(),
        assets: build_profile.assets.clone(),
    };
    write_package_manifest(output_root.path.as_path(), &manifest)?;

    Ok(manifest)
}

fn prepare_output_root(output_root: &PackageOutputRoot, force: bool) -> Result<(), String> {
    if output_root.path.exists() {
        if output_root.explicit && !force {
            return Err(format!(
                "Output directory '{}' already exists. Re-run with --force to replace it, or omit --output-dir to use the default target package path.",
                output_root.path.display()
            ));
        }

        remove_existing_output_root(output_root.path.as_path())?;
    }

    fs::create_dir_all(&output_root.path).map_err(|error| {
        format!(
            "Failed to create package output directory '{}': {}",
            output_root.path.display(),
            error
        )
    })?;
    Ok(())
}

fn remove_existing_output_root(path: &Path) -> Result<(), String> {
    if path.is_dir() {
        fs::remove_dir_all(path).map_err(|error| {
            format!(
                "Failed to replace existing package output directory '{}': {}",
                path.display(),
                error
            )
        })?;
    } else {
        fs::remove_file(path).map_err(|error| {
            format!(
                "Failed to replace existing package output file '{}': {}",
                path.display(),
                error
            )
        })?;
    }

    Ok(())
}

fn load_project_source_tool_context(
    project_root: &Path,
    tool_name: &str,
) -> Result<ProjectSourceToolContext, String> {
    let tool_manifest_path = crate::commands::tools::project_tools_root(project_root)
        .join(tool_name)
        .join("tool.json");
    if !tool_manifest_path.exists() {
        let machine_manifest_path = crate::commands::tools::machine_tools_root()
            .join(tool_name)
            .join("tool.json");
        if machine_manifest_path.exists() {
            return Err(format!(
                "Tool '{}' is only installed in Cargo AI Home. `cargo ai package` requires project-attached source-backed tools. Attach/install this tool into the current project first.",
                tool_name
            ));
        }
        return Err(format!(
            "Tool '{}' was not found in the current project's managed tool metadata. Add or attach/install it into the project before running `cargo ai package`.",
            tool_name
        ));
    }

    let manifest_contents = fs::read_to_string(&tool_manifest_path).map_err(|error| {
        format!(
            "Failed to read tool manifest '{}': {}",
            tool_manifest_path.display(),
            error
        )
    })?;
    let manifest: ToolManifestDocument =
        serde_json::from_str(&manifest_contents).map_err(|error| {
            format!(
                "Failed to parse tool manifest '{}': {}",
                tool_manifest_path.display(),
                error
            )
        })?;
    if manifest.tool_id != tool_name {
        return Err(format!(
            "Tool manifest '{}' declares tool_id '{}', but the project references '{}'.",
            tool_manifest_path.display(),
            manifest.tool_id,
            tool_name
        ));
    }

    let source = manifest.source.ok_or_else(|| {
        format!(
            "Tool '{}' is not source-backed. `cargo ai package` requires project-attached source-backed tools only.",
            tool_name
        )
    })?;
    validate_project_relative_path(source.manifest_path.as_str(), "Tool source manifest")?;
    if Path::new(source.manifest_path.as_str())
        .file_name()
        .and_then(|value| value.to_str())
        != Some("Cargo.toml")
    {
        return Err(format!(
            "Tool '{}' source manifest '{}' must point to a Cargo.toml file.",
            tool_name, source.manifest_path
        ));
    }

    let source_manifest_path = project_root.join(source.manifest_path.as_str());
    if !source_manifest_path.exists() {
        return Err(format!(
            "Tool '{}' source manifest '{}' was not found.",
            tool_name,
            source_manifest_path.display()
        ));
    }

    let source_root = source_manifest_path.parent().ok_or_else(|| {
        format!(
            "Tool '{}' source manifest '{}' has no parent directory.",
            tool_name,
            source_manifest_path.display()
        )
    })?;
    if source_root == project_root {
        return Err(format!(
            "Tool '{}' source manifest '{}' resolves to the current project root. `cargo ai package` currently requires tool source to live in its own project-relative directory.",
            tool_name,
            source.manifest_path
        ));
    }

    let source_root_relative_path = source_root
        .strip_prefix(project_root)
        .map_err(|_| {
            format!(
                "Tool '{}' source directory '{}' is not project-relative.",
                tool_name,
                source_root.display()
            )
        })?
        .to_string_lossy()
        .to_string();

    Ok(ProjectSourceToolContext {
        source_manifest_relative_path: source.manifest_path,
        source_root_relative_path,
        binary_name: manifest
            .binary
            .default_name
            .unwrap_or_else(|| tool_name.to_string()),
    })
}

fn write_package_tool_manifest(
    package_root: &Path,
    tool_name: &str,
    source_manifest_relative_path: &str,
    binary_name: &str,
) -> Result<(), String> {
    let tool_dir = package_root
        .join(PROJECT_TOOLS_RELATIVE_PATH)
        .join(tool_name);
    fs::create_dir_all(&tool_dir).map_err(|error| {
        format!(
            "Failed to create packaged tool metadata directory '{}': {}",
            tool_dir.display(),
            error
        )
    })?;

    let manifest_path = tool_dir.join("tool.json");
    let manifest = serde_json::json!({
        "schema_version": 1,
        "tool_id": tool_name,
        "source": {
            "manifest_path": source_manifest_relative_path
        },
        "binary": {
            "default_name": binary_name
        },
        "artifacts": {}
    })
    .to_string();
    fs::write(&manifest_path, manifest).map_err(|error| {
        format!(
            "Failed to write packaged tool manifest '{}': {}",
            manifest_path.display(),
            error
        )
    })
}

fn write_generated_project_metadata(
    package_root: &Path,
    project_identity: Option<&ProjectIdentityDocument>,
    profile_name: &str,
    build_profile: &BuildProfileDocument,
) -> Result<(), String> {
    let metadata_path = package_root.join(PROJECT_METADATA_RELATIVE_PATH);
    if let Some(parent) = metadata_path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            format!(
                "Failed to create generated metadata directory '{}': {}",
                parent.display(),
                error
            )
        })?;
    }

    let mut build = BTreeMap::new();
    build.insert(profile_name.to_string(), build_profile.clone());
    let document = GeneratedProjectMetadataDocument {
        format_version: 1,
        project: project_identity.cloned(),
        tools: GeneratedProjectToolsPolicyDocument {
            allow_global_fallback: false,
        },
        build,
    };
    let rendered = toml::to_string_pretty(&document)
        .map_err(|error| format!("Failed to render package project metadata TOML: {error}"))?;
    fs::write(&metadata_path, rendered).map_err(|error| {
        format!(
            "Failed to write generated project metadata '{}': {}",
            metadata_path.display(),
            error
        )
    })
}

fn normalize_project_identity(
    project_identity: Option<ProjectIdentityDocument>,
) -> Option<ProjectIdentityDocument> {
    let mut project_identity = project_identity?;
    project_identity.name = normalize_optional_metadata_text(project_identity.name.take());
    project_identity.version = normalize_optional_metadata_text(project_identity.version.take());

    if project_identity.name.is_none()
        && project_identity.version.is_none()
        && project_identity.extra.is_empty()
    {
        None
    } else {
        Some(project_identity)
    }
}

fn normalize_optional_metadata_text(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn write_package_manifest(
    package_root: &Path,
    manifest: &PackageManifestDocument,
) -> Result<(), String> {
    let manifest_path = package_root.join(PACKAGE_MANIFEST_FILE_NAME);
    let rendered = toml::to_string_pretty(manifest)
        .map_err(|error| format!("Failed to render package manifest TOML: {error}"))?;
    fs::write(&manifest_path, rendered).map_err(|error| {
        format!(
            "Failed to write package manifest '{}': {}",
            manifest_path.display(),
            error
        )
    })
}

fn copy_declared_path(
    project_root: &Path,
    relative_path: &str,
    package_root: &Path,
    require_json_file: bool,
) -> Result<(), String> {
    validate_project_relative_path(
        relative_path,
        if require_json_file { "Agent" } else { "Asset" },
    )?;
    let source_path = project_root.join(relative_path);
    if !source_path.exists() {
        return Err(format!(
            "{} path '{}' was not found in the current project.",
            if require_json_file { "Agent" } else { "Asset" },
            source_path.display()
        ));
    }
    if require_json_file {
        if !source_path.is_file() {
            return Err(format!(
                "Agent path '{}' must point to a JSON file.",
                source_path.display()
            ));
        }
        let is_json = source_path
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.eq_ignore_ascii_case("json"))
            .unwrap_or(false);
        if !is_json {
            return Err(format!(
                "Agent path '{}' must point to a JSON file.",
                source_path.display()
            ));
        }
    }

    let dest_path = package_root.join(relative_path);
    if source_path.is_dir() {
        copy_directory_recursive(source_path.as_path(), dest_path.as_path())
    } else {
        copy_file(source_path.as_path(), dest_path.as_path())
    }
}

fn copy_file(source: &Path, dest: &Path) -> Result<(), String> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            format!(
                "Failed to create destination directory '{}': {}",
                parent.display(),
                error
            )
        })?;
    }
    fs::copy(source, dest).map_err(|error| {
        format!(
            "Failed to copy '{}' into '{}': {}",
            source.display(),
            dest.display(),
            error
        )
    })?;
    Ok(())
}

fn copy_tool_source_root(
    project_root: &Path,
    relative_path: &str,
    package_root: &Path,
) -> Result<(), String> {
    validate_project_relative_path(relative_path, "Tool source root")?;
    let source_path = project_root.join(relative_path);
    if !source_path.exists() {
        return Err(format!(
            "Tool source root '{}' was not found in the current project.",
            source_path.display()
        ));
    }
    if !source_path.is_dir() {
        return Err(format!(
            "Tool source root '{}' must point to a directory.",
            source_path.display()
        ));
    }

    let dest_path = package_root.join(relative_path);
    copy_directory_recursive_skipping_target(source_path.as_path(), dest_path.as_path())
}

fn copy_directory_recursive(source: &Path, dest: &Path) -> Result<(), String> {
    fs::create_dir_all(dest).map_err(|error| {
        format!(
            "Failed to create destination directory '{}': {}",
            dest.display(),
            error
        )
    })?;

    for entry in fs::read_dir(source).map_err(|error| {
        format!(
            "Failed to read directory '{}' while assembling package output: {}",
            source.display(),
            error
        )
    })? {
        let entry = entry.map_err(|error| {
            format!(
                "Failed to read directory entry under '{}': {}",
                source.display(),
                error
            )
        })?;
        let source_path = entry.path();
        let dest_path = dest.join(entry.file_name());
        if source_path.is_dir() {
            copy_directory_recursive(source_path.as_path(), dest_path.as_path())?;
        } else {
            copy_file(source_path.as_path(), dest_path.as_path())?;
        }
    }

    Ok(())
}

fn copy_directory_recursive_skipping_target(source: &Path, dest: &Path) -> Result<(), String> {
    fs::create_dir_all(dest).map_err(|error| {
        format!(
            "Failed to create destination directory '{}': {}",
            dest.display(),
            error
        )
    })?;

    for entry in fs::read_dir(source).map_err(|error| {
        format!(
            "Failed to read directory '{}' while assembling package output: {}",
            source.display(),
            error
        )
    })? {
        let entry = entry.map_err(|error| {
            format!(
                "Failed to read directory entry under '{}': {}",
                source.display(),
                error
            )
        })?;
        let source_path = entry.path();
        let dest_path = dest.join(entry.file_name());
        if source_path.is_dir()
            && entry
                .file_name()
                .to_str()
                .map(|name| name == "target")
                .unwrap_or(false)
        {
            continue;
        }
        if source_path.is_dir() {
            copy_directory_recursive_skipping_target(source_path.as_path(), dest_path.as_path())?;
        } else {
            copy_file(source_path.as_path(), dest_path.as_path())?;
        }
    }

    Ok(())
}

fn dedupe_preserve_order(values: &[String]) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut deduped = Vec::new();

    for value in values {
        if seen.insert(value.clone()) {
            deduped.push(value.clone());
        }
    }

    deduped
}

fn validate_project_relative_path(raw_path: &str, label: &str) -> Result<(), String> {
    if raw_path.trim().is_empty() {
        return Err(format!("{label} path must be a non-empty relative path."));
    }
    let candidate = Path::new(raw_path);
    if candidate.is_absolute() {
        return Err(format!(
            "{label} path must be relative and stay at the current level or below."
        ));
    }
    if candidate
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(format!(
            "{label} path must stay at the current level or below; parent traversal (`..`) is not allowed."
        ));
    }
    Ok(())
}

fn normalize_against_current_dir(path: &Path) -> Result<PathBuf, String> {
    if path.is_absolute() {
        return Ok(normalize_path(path));
    }

    let current_dir = std::env::current_dir().map_err(|error| {
        format!("Failed to resolve the current directory while validating --output-dir: {error}")
    })?;
    Ok(normalize_path(current_dir.join(path)))
}

fn normalize_path(path: impl AsRef<Path>) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.as_ref().components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

#[cfg(test)]
mod tests {
    use super::{
        assemble_package_root, load_project_metadata, resolve_package_output_root,
        PackageManifestDocument,
    };
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(stem: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("cargo-ai-package-test-{stem}-{nanos}"))
    }

    fn write_project_metadata(project_root: &PathBuf, body: &str) {
        let metadata_path = project_root.join(".cargo-ai/project.toml");
        fs::create_dir_all(
            metadata_path
                .parent()
                .expect("project metadata parent should exist"),
        )
        .expect("project metadata parent should be created");
        fs::write(metadata_path, body).expect("project metadata should be written");
    }

    fn write_source_tool_fixture(
        project_root: &PathBuf,
        tool_id: &str,
        manifest_relative_path: &str,
    ) {
        let source_manifest_path = project_root.join(manifest_relative_path);
        let source_root = source_manifest_path
            .parent()
            .expect("manifest parent should exist");
        fs::create_dir_all(source_root.join("src")).expect("tool source tree should be created");
        fs::write(
            &source_manifest_path,
            format!(
                "[package]\nname = \"{}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
                tool_id
            ),
        )
        .expect("source manifest should be written");
        fs::write(
            source_root.join("src/main.rs"),
            "fn main() { println!(\"hello\"); }\n",
        )
        .expect("tool source file should be written");

        let tool_dir = project_root.join(".cargo-ai/tools").join(tool_id);
        fs::create_dir_all(&tool_dir).expect("tool metadata dir should be created");
        fs::write(
            tool_dir.join("tool.json"),
            serde_json::json!({
                "schema_version": 1,
                "tool_id": tool_id,
                "source": {
                    "manifest_path": manifest_relative_path
                },
                "binary": {
                    "default_name": tool_id
                },
                "artifacts": {
                    "aarch64-apple-darwin": {
                        "path": format!("bin/aarch64-apple-darwin/{tool_id}")
                    }
                }
            })
            .to_string(),
        )
        .expect("tool manifest should be written");
    }

    fn write_machine_tool_fixture(root: &PathBuf, tool_id: &str) {
        let tool_dir = root.join("tools").join(tool_id);
        fs::create_dir_all(&tool_dir).expect("machine tool dir should be created");
        fs::write(
            tool_dir.join("tool.json"),
            serde_json::json!({
                "schema_version": 1,
                "tool_id": tool_id,
                "binary": {
                    "default_name": tool_id
                },
                "artifacts": {
                    "aarch64-apple-darwin": {
                        "path": tool_id
                    }
                }
            })
            .to_string(),
        )
        .expect("machine tool manifest should be written");
    }

    #[test]
    fn load_project_metadata_allows_missing_project_identity() {
        let project_root = temp_dir("no-project-identity");
        write_project_metadata(
            &project_root,
            r#"
format_version = 1

[build.default]
agent_definitions = ["agents/demo.json"]
"#,
        );

        let loaded_metadata =
            load_project_metadata(&project_root, "default").expect("profile should load");
        assert!(loaded_metadata.project_identity.is_none());
        assert_eq!(
            loaded_metadata.build_profile.agent_definitions,
            vec!["agents/demo.json".to_string()]
        );

        let _ = fs::remove_dir_all(&project_root);
    }

    #[test]
    fn assembles_source_package_from_selected_build_profile() {
        let project_root = temp_dir("assemble");
        fs::create_dir_all(project_root.join("agents")).expect("agents dir should be created");
        fs::create_dir_all(project_root.join("assets/prompts"))
            .expect("asset dir should be created");
        write_project_metadata(
            &project_root,
            r#"
format_version = 1

[project]
name = "hello_package"
version = "0.1.0"

[tools]
allow_global_fallback = true

[build.default]
agent_definitions = ["agents/definition_only.json"]
hatched_agents = ["agents/hello_runner.json"]
tools = ["hello_tool"]
assets = ["assets/prompts/"]
"#,
        );
        fs::write(
            project_root.join("agents/definition_only.json"),
            "{\"name\":\"definition_only\"}\n",
        )
        .expect("agent definition should be written");
        fs::write(
            project_root.join("agents/hello_runner.json"),
            "{\"name\":\"hello_runner\"}\n",
        )
        .expect("hatched agent definition should be written");
        fs::write(
            project_root.join("assets/prompts/example.txt"),
            "example prompt\n",
        )
        .expect("asset should be written");
        write_source_tool_fixture(&project_root, "hello_tool", "tools/hello_tool/Cargo.toml");

        let loaded_metadata =
            load_project_metadata(&project_root, "default").expect("profile should load");
        let output_root = resolve_package_output_root(&project_root, "default", None)
            .expect("output root should resolve");
        let manifest = assemble_package_root(
            &project_root,
            "default",
            loaded_metadata.project_identity.as_ref(),
            &loaded_metadata.build_profile,
            &output_root,
            false,
        )
        .expect("package should assemble");

        assert_eq!(
            manifest,
            PackageManifestDocument {
                format_version: 1,
                project_name: Some("hello_package".to_string()),
                project_version: Some("0.1.0".to_string()),
                profile: "default".to_string(),
                agent_definitions: vec!["agents/definition_only.json".to_string()],
                hatched_agents: vec!["agents/hello_runner.json".to_string()],
                tools: vec!["hello_tool".to_string()],
                assets: vec!["assets/prompts/".to_string()],
            }
        );

        assert!(output_root
            .path
            .join("agents/definition_only.json")
            .exists());
        assert!(output_root.path.join("agents/hello_runner.json").exists());
        assert!(output_root.path.join("assets/prompts/example.txt").exists());
        assert!(output_root
            .path
            .join("tools/hello_tool/Cargo.toml")
            .exists());
        assert!(output_root
            .path
            .join("tools/hello_tool/src/main.rs")
            .exists());
        assert!(!output_root.path.join("tools/hello_tool/target").exists());
        assert!(output_root
            .path
            .join(".cargo-ai/tools/hello_tool/tool.json")
            .exists());
        assert!(!output_root
            .path
            .join(".cargo-ai/tools/hello_tool/bin")
            .exists());

        let generated_project = fs::read_to_string(output_root.path.join(".cargo-ai/project.toml"))
            .expect("generated project metadata should exist");
        assert!(generated_project.contains("[project]"));
        assert!(generated_project.contains("name = \"hello_package\""));
        assert!(generated_project.contains("version = \"0.1.0\""));
        assert!(generated_project.contains("allow_global_fallback = false"));
        assert!(generated_project.contains("[build.default]"));
        assert!(generated_project.contains("hatched_agents = [\"agents/hello_runner.json\"]"));

        let package_manifest: PackageManifestDocument = toml::from_str(
            &fs::read_to_string(output_root.path.join("cargo-ai-package.toml"))
                .expect("package manifest should exist"),
        )
        .expect("package manifest should parse");
        assert_eq!(package_manifest, manifest);

        let packaged_tool_manifest: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(
                output_root
                    .path
                    .join(".cargo-ai/tools/hello_tool/tool.json"),
            )
            .expect("packaged tool manifest should exist"),
        )
        .expect("tool manifest should parse");
        assert_eq!(
            packaged_tool_manifest
                .get("source")
                .and_then(serde_json::Value::as_object)
                .and_then(|source| source.get("manifest_path"))
                .and_then(serde_json::Value::as_str),
            Some("tools/hello_tool/Cargo.toml")
        );
        assert_eq!(
            packaged_tool_manifest
                .get("artifacts")
                .and_then(serde_json::Value::as_object)
                .map(|artifacts| artifacts.len()),
            Some(0)
        );

        let _ = fs::remove_dir_all(&project_root);
    }

    #[test]
    fn package_fails_when_tool_exists_only_in_cargo_ai_home() {
        let _guard = crate::commands::runtime_actions::TEST_ENV_LOCK
            .lock()
            .expect("environment lock should not be poisoned");
        let original_cargo_ai_home = std::env::var_os("CARGO_AI_HOME");
        let cargo_ai_home = temp_dir("machine-home");
        std::env::set_var("CARGO_AI_HOME", &cargo_ai_home);

        let project_root = temp_dir("machine-only");
        write_project_metadata(
            &project_root,
            r#"
format_version = 1

[build.default]
tools = ["machine_only"]
"#,
        );
        write_machine_tool_fixture(&cargo_ai_home, "machine_only");

        let loaded_metadata =
            load_project_metadata(&project_root, "default").expect("profile should load");
        let output_root = resolve_package_output_root(&project_root, "default", None)
            .expect("output root should resolve");
        let error = assemble_package_root(
            &project_root,
            "default",
            loaded_metadata.project_identity.as_ref(),
            &loaded_metadata.build_profile,
            &output_root,
            false,
        )
        .expect_err("machine-only tool should fail package assembly");
        assert!(error.contains("only installed in Cargo AI Home"));
        assert!(error.contains("requires project-attached source-backed tools"));

        match original_cargo_ai_home {
            Some(value) => std::env::set_var("CARGO_AI_HOME", value),
            None => std::env::remove_var("CARGO_AI_HOME"),
        }
        let _ = fs::remove_dir_all(&project_root);
        let _ = fs::remove_dir_all(&cargo_ai_home);
    }
}
