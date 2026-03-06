//! Warmed template cache helpers for hatch builds.

mod key;
mod prepare;
mod prune;

use crate::agent_builder::build_target::BuildTarget;
use key::{resolve_template_cache_key, TemplateCacheKey};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WarmedTemplate {
    key: TemplateCacheKey,
    pub path: PathBuf,
    pub created: bool,
    pub pruned_parent_count: usize,
}

pub(crate) fn ensure_warmed_template(build_target: &BuildTarget) -> Result<WarmedTemplate, String> {
    let key = resolve_template_cache_key(build_target)?;
    let path = template_workspace_path(&key);

    if prepare::template_workspace_ready(&path) {
        return Ok(WarmedTemplate {
            pruned_parent_count: prune_stale_parents(&key),
            key,
            path,
            created: false,
        });
    }

    prepare::prepare_warmed_template_workspace(&path, build_target)?;

    Ok(WarmedTemplate {
        pruned_parent_count: prune_stale_parents(&key),
        key,
        path,
        created: true,
    })
}

fn template_workspace_path(key: &TemplateCacheKey) -> PathBuf {
    crate::agent_builder::templates_workspace_root()
        .join(&key.binary_sha256)
        .join(&key.rustc_version)
        .join(&key.target_triple)
}

fn prune_stale_parents(key: &TemplateCacheKey) -> usize {
    prune::prune_stale_template_cache(&crate::agent_builder::templates_workspace_root(), key)
        .unwrap_or_else(|error| {
            eprintln!("⚠️ Failed to prune stale template cache parents: {error}");
            0
        })
}

#[cfg(test)]
mod tests {
    use super::{template_workspace_path, TemplateCacheKey};
    use std::path::PathBuf;

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
