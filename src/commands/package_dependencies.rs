//! Project declarations that bind installed package aliases to hosted identity.

use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
#[cfg(windows)]
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};

const PROJECT_METADATA_RELATIVE_PATH: &str = ".cargo-ai/project.toml";
#[cfg(windows)]
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct PackageDependencyDocument {
    pub(crate) hosted_source_id: String,
    pub(crate) version: String,
}

pub(crate) type PackageDependencies = BTreeMap<String, PackageDependencyDocument>;

#[derive(Debug, Default, Deserialize)]
struct ProjectPackageDependenciesDocument {
    #[serde(default)]
    package_dependencies: PackageDependencies,
}

pub(crate) struct InstalledPackageDependencyIdentity<'a> {
    pub(crate) alias: &'a str,
    pub(crate) source_kind: &'a str,
    pub(crate) hosted_source_id: Option<&'a str>,
    pub(crate) package_version: &'a str,
}

pub(crate) fn find_project_root(start: &Path) -> Result<Option<PathBuf>, String> {
    let start_metadata = fs::metadata(start).map_err(|error| {
        format!(
            "Failed to inspect package dependency search path '{}': {error}",
            start.display()
        )
    })?;
    let mut current = if start_metadata.is_dir() {
        start.to_path_buf()
    } else {
        let Some(parent) = start.parent() else {
            return Ok(None);
        };
        parent.to_path_buf()
    };
    loop {
        let metadata_path = current.join(PROJECT_METADATA_RELATIVE_PATH);
        match fs::symlink_metadata(&metadata_path) {
            Ok(metadata) if metadata_is_link_like(&metadata) => {
                return Err(format!(
                    "Project package dependency metadata '{}' must not be a symbolic link or reparse point.",
                    metadata_path.display()
                ));
            }
            Ok(metadata) if !metadata.is_file() => {
                return Err(format!(
                    "Project package dependency metadata '{}' must be a regular file.",
                    metadata_path.display()
                ));
            }
            Ok(_) => return Ok(Some(current)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!(
                    "Failed to inspect project package dependency metadata '{}': {error}",
                    metadata_path.display()
                ));
            }
        }
        if !current.pop() {
            return Ok(None);
        }
    }
}

#[cfg(windows)]
fn metadata_is_link_like(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
        || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn metadata_is_link_like(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

pub(crate) fn load_project_dependencies(
    project_root: &Path,
) -> Result<PackageDependencies, String> {
    let metadata_path = project_root.join(PROJECT_METADATA_RELATIVE_PATH);
    let contents = fs::read_to_string(&metadata_path).map_err(|error| {
        format!(
            "Failed to read project package dependencies '{}': {}",
            metadata_path.display(),
            error
        )
    })?;
    let metadata: ProjectPackageDependenciesDocument =
        toml::from_str(&contents).map_err(|error| {
            format!(
                "Failed to parse project package dependencies '{}': {}",
                metadata_path.display(),
                error
            )
        })?;
    validate_dependency_declarations(&metadata.package_dependencies)?;
    Ok(metadata.package_dependencies)
}

pub(crate) fn validate_dependency_declarations(
    dependencies: &PackageDependencies,
) -> Result<(), String> {
    for (alias, dependency) in dependencies {
        validate_alias(alias)?;
        if dependency.hosted_source_id.trim().is_empty() {
            return Err(format!(
                "Package dependency `{alias}` must declare a non-empty `hosted_source_id`."
            ));
        }
        if dependency.version.trim().is_empty() {
            return Err(format!(
                "Package dependency `{alias}` must declare a non-empty `version`."
            ));
        }
        VersionReq::parse(dependency.version.trim()).map_err(|error| {
            format!(
                "Package dependency `{alias}` has invalid semver requirement '{}': {}",
                dependency.version, error
            )
        })?;
    }
    Ok(())
}

pub(crate) fn validate_installed_dependency(
    project_root: &Path,
    identity: InstalledPackageDependencyIdentity<'_>,
) -> Result<(), String> {
    let dependencies = load_project_dependencies(project_root)?;
    let dependency = dependencies.get(identity.alias);
    if identity.source_kind != "hosted" {
        return if dependency.is_some() {
            Err(format!(
                "Package dependency `{}` is declared as a hosted dependency, but the installed alias came from `{}`.",
                identity.alias, identity.source_kind
            ))
        } else {
            Ok(())
        };
    }
    let dependency = dependency.ok_or_else(|| {
        format!(
            "Package alias `{}` is used by project '{}' but is not declared under `[package_dependencies.{}]` in `.cargo-ai/project.toml`.",
            identity.alias,
            project_root.display(),
            identity.alias
        )
    })?;
    let installed_source_id = identity.hosted_source_id.ok_or_else(|| {
        format!(
            "Installed package alias `{}` is missing hosted source identity metadata.",
            identity.alias
        )
    })?;
    if installed_source_id != dependency.hosted_source_id.trim() {
        return Err(format!(
            "Package dependency `{}` expects hosted source id `{}`, but the installed alias is pinned to `{}`.",
            identity.alias,
            dependency.hosted_source_id.trim(),
            installed_source_id
        ));
    }
    let installed_version = Version::parse(identity.package_version).map_err(|error| {
        format!(
            "Installed package dependency `{}` has invalid version '{}': {}",
            identity.alias, identity.package_version, error
        )
    })?;
    let version_requirement = VersionReq::parse(dependency.version.trim()).map_err(|error| {
        format!(
            "Package dependency `{}` has invalid semver requirement '{}': {}",
            identity.alias, dependency.version, error
        )
    })?;
    if !version_requirement.matches(&installed_version) {
        return Err(format!(
            "Package dependency `{}` requires version `{}`, but installed version {} does not match.",
            identity.alias, dependency.version, installed_version
        ));
    }
    Ok(())
}

pub(crate) fn is_package_reference(reference: &str) -> bool {
    reference.split_once("::").is_some()
}

fn validate_alias(alias: &str) -> Result<(), String> {
    if !alias
        .chars()
        .next()
        .map(|ch| ch.is_ascii_alphanumeric())
        .unwrap_or(false)
        || !alias
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
    {
        return Err(format!(
            "Package dependency alias '{}' is invalid. Start with a letter or number, then use only letters, numbers, '-' or '_'.",
            alias
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        find_project_root, load_project_dependencies, validate_installed_dependency,
        InstalledPackageDependencyIdentity,
    };
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn project_root(stem: &str, metadata: &str) -> std::path::PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should move forward")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("cargo-ai-dependencies-{stem}-{unique}"));
        fs::create_dir_all(root.join(".cargo-ai")).expect("metadata root should exist");
        fs::write(root.join(".cargo-ai/project.toml"), metadata)
            .expect("metadata should be written");
        root
    }

    fn declared_project() -> std::path::PathBuf {
        project_root(
            "declared",
            r#"
format_version = 1

[package_dependencies.reports]
hosted_source_id = "source-reports"
version = ">=1.2, <2.0"
"#,
        )
    }

    #[test]
    fn loads_and_finds_project_dependencies() {
        let root = declared_project();
        let nested = root.join("agents");
        fs::create_dir_all(&nested).expect("nested directory should exist");
        assert_eq!(
            find_project_root(&nested)
                .expect("project discovery should succeed")
                .as_deref(),
            Some(root.as_path())
        );
        let dependencies =
            load_project_dependencies(&root).expect("dependency declaration should parse");
        assert_eq!(dependencies["reports"].hosted_source_id, "source-reports");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn validates_hosted_source_and_semver_requirement() {
        let root = declared_project();
        validate_installed_dependency(
            &root,
            InstalledPackageDependencyIdentity {
                alias: "reports",
                source_kind: "hosted",
                hosted_source_id: Some("source-reports"),
                package_version: "1.7.3",
            },
        )
        .expect("matching installed identity should validate");

        let source_error = validate_installed_dependency(
            &root,
            InstalledPackageDependencyIdentity {
                alias: "reports",
                source_kind: "hosted",
                hosted_source_id: Some("different-source"),
                package_version: "1.7.3",
            },
        )
        .expect_err("different source should fail closed");
        assert!(source_error.contains("expects hosted source id"));

        let version_error = validate_installed_dependency(
            &root,
            InstalledPackageDependencyIdentity {
                alias: "reports",
                source_kind: "hosted",
                hosted_source_id: Some("source-reports"),
                package_version: "2.0.0",
            },
        )
        .expect_err("out-of-range version should fail closed");
        assert!(version_error.contains("does not match"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn undeclared_alias_fails_closed() {
        let root = declared_project();
        let error = validate_installed_dependency(
            &root,
            InstalledPackageDependencyIdentity {
                alias: "images",
                source_kind: "hosted",
                hosted_source_id: Some("source-images"),
                package_version: "1.0.0",
            },
        )
        .expect_err("undeclared aliases should fail closed");
        assert!(error.contains("[package_dependencies.images]"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn undeclared_local_alias_remains_available_for_development() {
        let root = declared_project();
        validate_installed_dependency(
            &root,
            InstalledPackageDependencyIdentity {
                alias: "local_reports",
                source_kind: "local_root",
                hosted_source_id: None,
                package_version: "1.0.0",
            },
        )
        .expect("an undeclared local alias should remain available");

        let error = validate_installed_dependency(
            &root,
            InstalledPackageDependencyIdentity {
                alias: "reports",
                source_kind: "local_root",
                hosted_source_id: None,
                package_version: "1.0.0",
            },
        )
        .expect_err("a hosted declaration must not bind a local alias");
        assert!(error.contains("installed alias came from `local_root`"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn invalid_project_metadata_marker_fails_closed() {
        let root = project_root("invalid-marker", "format_version = 1\n");
        let marker = root.join(".cargo-ai/project.toml");
        fs::remove_file(&marker).expect("metadata file should be removable");
        fs::create_dir(&marker).expect("directory marker should be created");

        let error = find_project_root(&root)
            .expect_err("a directory metadata marker must not disable dependency binding");
        assert!(error.contains("must be a regular file"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn empty_version_and_option_like_alias_are_rejected() {
        let empty_version = project_root(
            "empty-version",
            r#"[package_dependencies.reports]
hosted_source_id = "source-reports"
version = "   "
"#,
        );
        let error = load_project_dependencies(&empty_version)
            .expect_err("trimmed-empty version must not become a wildcard");
        assert!(error.contains("non-empty `version`"));
        let _ = fs::remove_dir_all(empty_version);

        let option_alias = project_root(
            "option-alias",
            r#"[package_dependencies."-reports"]
hosted_source_id = "source-reports"
version = "^1"
"#,
        );
        let error = load_project_dependencies(&option_alias)
            .expect_err("an option-like alias must be rejected");
        assert!(error.contains("Start with a letter or number"));
        let _ = fs::remove_dir_all(option_alias);
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_project_metadata_marker_fails_closed() {
        use std::os::unix::fs::symlink;

        let root = project_root("symlink-marker", "format_version = 1\n");
        let marker = root.join(".cargo-ai/project.toml");
        let external = root.join("external-project.toml");
        fs::write(&external, "format_version = 1\n").expect("external marker should exist");
        fs::remove_file(&marker).expect("metadata file should be removable");
        symlink(&external, &marker).expect("metadata symlink should be created");

        let error = find_project_root(&root)
            .expect_err("a symlink metadata marker must not disable dependency binding");
        assert!(error.contains("must not be a symbolic link"));
        let _ = fs::remove_dir_all(root);
    }
}
