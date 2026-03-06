//! Warmed template cache helpers for hatch builds.

use crate::agent_builder::{build, project};
use crate::cargo_ai_metadata;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const TEMPLATE_SEED_AGENT_NAME: &str = "template_seed_agent";
const TEMPLATE_SEED_AGENTCFG: &str = include_str!("../../templates/.agentcfg");

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TemplateCacheKey {
    pub binary_sha256: String,
    pub rustc_version: String,
    pub target_triple: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WarmedTemplate {
    pub key: TemplateCacheKey,
    pub path: PathBuf,
    pub created: bool,
}

pub(crate) fn ensure_warmed_template() -> Result<WarmedTemplate, String> {
    let key = resolve_template_cache_key()?;
    let path = template_workspace_path(&key);

    if template_workspace_ready(&path) {
        return Ok(WarmedTemplate {
            key,
            path,
            created: false,
        });
    }

    if path.exists() {
        fs::remove_dir_all(&path).map_err(|error| {
            format!(
                "Failed to reset incomplete template workspace '{}': {error}",
                path.display()
            )
        })?;
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            format!(
                "Failed to create template cache directory '{}': {error}",
                parent.display()
            )
        })?;
    }

    let creation_result = (|| -> Result<(), String> {
        project::create_template_project(
            &path,
            TEMPLATE_SEED_AGENT_NAME,
            TEMPLATE_SEED_AGENTCFG,
        )
        .map_err(|error| {
            format!(
                "Failed to create warmed template workspace '{}': {error}",
                path.display()
            )
        })?;
        build::build_workspace(&path).map_err(|error| {
            format!(
                "Failed to warm template workspace '{}': {error}",
                path.display()
            )
        })?;
        Ok(())
    })();

    if let Err(error) = creation_result {
        let _ = fs::remove_dir_all(&path);
        return Err(error);
    }

    if !template_workspace_ready(&path) {
        let _ = fs::remove_dir_all(&path);
        return Err(format!(
            "Warmed template workspace '{}' is incomplete after build.",
            path.display()
        ));
    }

    Ok(WarmedTemplate {
        key,
        path,
        created: true,
    })
}

pub(crate) fn template_workspace_path(key: &TemplateCacheKey) -> PathBuf {
    crate::agent_builder::templates_workspace_root()
        .join(&key.binary_sha256)
        .join(&key.rustc_version)
        .join(&key.target_triple)
}

fn resolve_template_cache_key() -> Result<TemplateCacheKey, String> {
    Ok(TemplateCacheKey {
        binary_sha256: cargo_ai_metadata::current_binary_sha256()?,
        rustc_version: normalized_rustc_version()?,
        target_triple: crate::agent_builder::requested_target_triple(),
    })
}

fn normalized_rustc_version() -> Result<String, String> {
    let output = Command::new("rustc")
        .arg("--version")
        .output()
        .map_err(|error| format!("Failed to execute `rustc --version`: {error}"))?;

    if !output.status.success() {
        return Err(format!(
            "`rustc --version` exited with status {}",
            output.status
        ));
    }

    let stdout = String::from_utf8(output.stdout)
        .map_err(|error| format!("`rustc --version` returned non-UTF-8 output: {error}"))?;

    normalized_rustc_version_from_output(&stdout).ok_or_else(|| {
        format!(
            "Unable to normalize rustc version from output: {}",
            stdout.trim()
        )
    })
}

fn normalized_rustc_version_from_output(output: &str) -> Option<String> {
    let mut parts = output.split_whitespace();
    let first = parts.next()?;
    let second = parts.next()?;
    if first != "rustc" {
        return None;
    }
    Some(format!("rustc-{second}"))
}

fn template_workspace_ready(path: &Path) -> bool {
    path.join("Cargo.toml").is_file()
        && path.join("build.rs").is_file()
        && path.join(".agentcfg").is_file()
        && path.join("src").join("main.rs").is_file()
        && path.join("target").is_dir()
}

#[cfg(test)]
mod tests {
    use super::{normalized_rustc_version_from_output, template_workspace_path, TemplateCacheKey};
    use std::path::PathBuf;

    #[test]
    fn normalizes_rustc_semver_output() {
        let output = "rustc 1.90.0 (1159e78c4 2025-09-14)\n";
        assert_eq!(
            normalized_rustc_version_from_output(output).as_deref(),
            Some("rustc-1.90.0")
        );
    }

    #[test]
    fn rejects_unexpected_rustc_output_shape() {
        assert!(normalized_rustc_version_from_output("cargo 1.90.0").is_none());
        assert!(normalized_rustc_version_from_output("").is_none());
    }

    #[test]
    fn builds_nested_template_cache_path() {
        let key = TemplateCacheKey {
            binary_sha256: "abc123".to_string(),
            rustc_version: "rustc-1.90.0".to_string(),
            target_triple: "aarch64-apple-darwin".to_string(),
        };

        let path = template_workspace_path(&key);
        let suffix = PathBuf::from("abc123")
            .join("rustc-1.90.0")
            .join("aarch64-apple-darwin");

        assert!(path.ends_with(suffix));
    }
}
