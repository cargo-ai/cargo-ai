//! Warmed template cache helpers for hatch builds.

use crate::agent_builder::{build, project};
use crate::cargo_ai_metadata;
use std::ffi::OsStr;
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
    pub pruned_parent_count: usize,
}

pub(crate) fn ensure_warmed_template(
    explicit_target_triple: Option<&str>,
) -> Result<WarmedTemplate, String> {
    let key = resolve_template_cache_key(explicit_target_triple)?;
    let path = template_workspace_path(&key);

    if template_workspace_ready(&path) {
        let pruned_parent_count =
            prune_stale_template_cache(&crate::agent_builder::templates_workspace_root(), &key)
                .unwrap_or_else(|error| {
                    eprintln!("⚠️ Failed to prune stale template cache parents: {error}");
                    0
                });
        return Ok(WarmedTemplate {
            key,
            path,
            created: false,
            pruned_parent_count,
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
        project::create_template_project(&path, TEMPLATE_SEED_AGENT_NAME, TEMPLATE_SEED_AGENTCFG)
            .map_err(|error| {
            format!(
                "Failed to create warmed template workspace '{}': {error}",
                path.display()
            )
        })?;
        build::build_workspace(&path, explicit_target_triple).map_err(|error| {
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

    let pruned_parent_count =
        prune_stale_template_cache(&crate::agent_builder::templates_workspace_root(), &key)
            .unwrap_or_else(|error| {
                eprintln!("⚠️ Failed to prune stale template cache parents: {error}");
                0
            });

    Ok(WarmedTemplate {
        key,
        path,
        created: true,
        pruned_parent_count,
    })
}

pub(crate) fn template_workspace_path(key: &TemplateCacheKey) -> PathBuf {
    crate::agent_builder::templates_workspace_root()
        .join(&key.binary_sha256)
        .join(&key.rustc_version)
        .join(&key.target_triple)
}

fn resolve_template_cache_key(
    explicit_target_triple: Option<&str>,
) -> Result<TemplateCacheKey, String> {
    Ok(TemplateCacheKey {
        binary_sha256: cargo_ai_metadata::current_binary_sha256()?,
        rustc_version: normalized_rustc_version()?,
        target_triple: crate::agent_builder::requested_target_triple(explicit_target_triple),
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

fn prune_stale_template_cache(root: &Path, active_key: &TemplateCacheKey) -> Result<usize, String> {
    if !root.exists() {
        return Ok(0);
    }

    let mut removed_parent_count = 0;
    for hash_entry in fs::read_dir(root).map_err(|error| {
        format!(
            "Failed to read template cache root '{}': {error}",
            root.display()
        )
    })? {
        let hash_entry = hash_entry.map_err(|error| {
            format!(
                "Failed to read template cache entry under '{}': {error}",
                root.display()
            )
        })?;
        let hash_path = hash_entry.path();
        if !hash_path.is_dir() {
            continue;
        }

        if hash_entry.file_name() != OsStr::new(&active_key.binary_sha256) {
            fs::remove_dir_all(&hash_path).map_err(|error| {
                format!(
                    "Failed to remove stale template cache parent '{}': {error}",
                    hash_path.display()
                )
            })?;
            removed_parent_count += 1;
            continue;
        }

        removed_parent_count += prune_stale_rustc_parents(&hash_path, active_key)?;
    }

    Ok(removed_parent_count)
}

fn prune_stale_rustc_parents(
    binary_root: &Path,
    active_key: &TemplateCacheKey,
) -> Result<usize, String> {
    let mut removed_parent_count = 0;
    for rustc_entry in fs::read_dir(binary_root).map_err(|error| {
        format!(
            "Failed to read Cargo AI template cache parent '{}': {error}",
            binary_root.display()
        )
    })? {
        let rustc_entry = rustc_entry.map_err(|error| {
            format!(
                "Failed to read rustc template cache entry under '{}': {error}",
                binary_root.display()
            )
        })?;
        let rustc_path = rustc_entry.path();
        if !rustc_path.is_dir() {
            continue;
        }

        if rustc_entry.file_name() != OsStr::new(&active_key.rustc_version) {
            fs::remove_dir_all(&rustc_path).map_err(|error| {
                format!(
                    "Failed to remove stale rustc template cache parent '{}': {error}",
                    rustc_path.display()
                )
            })?;
            removed_parent_count += 1;
        }
    }

    Ok(removed_parent_count)
}

#[cfg(test)]
mod tests {
    use super::{
        normalized_rustc_version_from_output, prune_stale_template_cache, template_workspace_path,
        TemplateCacheKey,
    };
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static TEMP_DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_test_dir() -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after epoch")
            .as_nanos();
        let sequence = TEMP_DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("cargo-ai-template-cache-test-{nanos}-{sequence}"));
        fs::create_dir_all(&path).expect("test directory should be creatable");
        path
    }

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

    #[test]
    fn prune_stale_template_cache_keeps_active_parent_and_sibling_targets() {
        let root = temp_test_dir();
        let active_key = TemplateCacheKey {
            binary_sha256: "sha-current".to_string(),
            rustc_version: "rustc-1.91.1".to_string(),
            target_triple: "aarch64-apple-darwin".to_string(),
        };

        let stale_hash_target = root
            .join("sha-old")
            .join("rustc-1.90.0")
            .join("aarch64-apple-darwin");
        let stale_rustc_target = root
            .join("sha-current")
            .join("rustc-1.90.0")
            .join("aarch64-apple-darwin");
        let active_target = root
            .join("sha-current")
            .join("rustc-1.91.1")
            .join("aarch64-apple-darwin");
        let sibling_target = root
            .join("sha-current")
            .join("rustc-1.91.1")
            .join("x86_64-pc-windows-msvc");

        for path in [
            &stale_hash_target,
            &stale_rustc_target,
            &active_target,
            &sibling_target,
        ] {
            fs::create_dir_all(path).expect("template cache fixture should be creatable");
        }

        let removed = prune_stale_template_cache(&root, &active_key)
            .expect("pruning stale template cache should succeed");

        assert_eq!(removed, 2);
        assert!(!root.join("sha-old").exists());
        assert!(!root.join("sha-current").join("rustc-1.90.0").exists());
        assert!(active_target.exists());
        assert!(sibling_target.exists());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn prune_stale_template_cache_is_noop_when_root_missing() {
        let missing_root = std::env::temp_dir().join("cargo-ai-template-cache-missing-root");
        let active_key = TemplateCacheKey {
            binary_sha256: "sha-current".to_string(),
            rustc_version: "rustc-1.91.1".to_string(),
            target_triple: "aarch64-apple-darwin".to_string(),
        };

        let removed = prune_stale_template_cache(&missing_root, &active_key)
            .expect("missing cache root should be treated as empty");
        assert_eq!(removed, 0);
    }
}
