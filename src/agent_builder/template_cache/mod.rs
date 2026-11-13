//! Warmed template cache helpers for hatch builds.

mod key;
mod prepare;
mod prune;

use crate::agent_builder::build_target::{BuildTarget, CargoCompileProfile};
use key::{resolve_template_cache_key, TemplateCacheKey};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WarmedTemplate {
    key: TemplateCacheKey,
    pub path: PathBuf,
    pub created: bool,
    pub pruned_parent_count: usize,
}

pub(crate) fn ensure_warmed_template_with_prepare_hook<F>(
    build_target: &BuildTarget,
    profile: CargoCompileProfile,
    on_prepare_start: F,
) -> Result<WarmedTemplate, String>
where
    F: FnOnce(),
{
    ensure_warmed_template_with_prepare_hook_and_deps(
        &crate::agent_builder::templates_workspace_root(),
        cfg!(windows),
        build_target,
        profile,
        on_prepare_start,
        prepare::template_workspace_ready,
        prepare::prepare_warmed_template_workspace,
    )
}

fn ensure_warmed_template_with_prepare_hook_and_deps<F, G, H>(
    root: &Path,
    windows: bool,
    build_target: &BuildTarget,
    profile: CargoCompileProfile,
    on_prepare_start: F,
    template_workspace_ready: G,
    prepare_warmed_template_workspace: H,
) -> Result<WarmedTemplate, String>
where
    F: FnOnce(),
    G: FnOnce(&Path, &BuildTarget, CargoCompileProfile) -> bool,
    H: FnOnce(&Path, &BuildTarget, CargoCompileProfile) -> Result<(), String>,
{
    let key = resolve_template_cache_key(build_target, profile)?;
    let path = template_workspace_path_at(root, &key, windows);
    if windows {
        prune::validate_windows_workspace_path(root, &path)
            .map_err(|error| format!("Unsafe template cache path '{}': {error}", path.display()))?;
    }

    if template_workspace_ready(&path, build_target, profile) {
        return Ok(WarmedTemplate {
            pruned_parent_count: prune_stale_parents(root, &key, windows),
            key,
            path,
            created: false,
        });
    }

    on_prepare_start();
    prepare_warmed_template_workspace(&path, build_target, profile)?;

    Ok(WarmedTemplate {
        pruned_parent_count: prune_stale_parents(root, &key, windows),
        key,
        path,
        created: true,
    })
}

fn windows_group_key(key: &TemplateCacheKey) -> String {
    let mut digest = Sha256::new();
    digest.update(b"cargo-ai-windows-template-group-v1\0");
    for value in [&key.binary_sha256, &key.rustc_version] {
        digest.update((value.len() as u64).to_le_bytes());
        digest.update(value.as_bytes());
    }
    format!("{:x}", digest.finalize())
}

fn template_workspace_path_at(root: &Path, key: &TemplateCacheKey, windows: bool) -> PathBuf {
    if windows {
        return root
            .join("w")
            .join(windows_group_key(key))
            .join(&key.target_triple)
            .join(match key.compile_profile {
                CargoCompileProfile::Release => "r",
                CargoCompileProfile::Dev => "d",
            });
    }
    root.join(&key.binary_sha256)
        .join(&key.rustc_version)
        .join(&key.target_triple)
        .join(key.compile_profile.name())
}

fn prune_stale_parents(root: &Path, key: &TemplateCacheKey, windows: bool) -> usize {
    prune::prune_stale_template_cache(root, key, windows).unwrap_or_else(|error| {
        eprintln!("⚠️ Failed to prune stale template cache parents: {error}");
        0
    })
}

#[cfg(test)]
mod tests {
    use super::{
        ensure_warmed_template_with_prepare_hook_and_deps, template_workspace_path_at,
        windows_group_key, TemplateCacheKey,
    };
    use crate::agent_builder::build_target::{BuildTarget, CargoCompileProfile};
    use std::path::PathBuf;
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };

    #[test]
    fn builds_nested_template_cache_path() {
        let key = TemplateCacheKey {
            binary_sha256: "abc123".to_string(),
            rustc_version: "rustc-1.90.0".to_string(),
            target_triple: "aarch64-apple-darwin".to_string(),
            compile_profile: CargoCompileProfile::Release,
        };

        let root = PathBuf::from("templates");
        let path = template_workspace_path_at(&root, &key, false);
        let suffix = PathBuf::from("abc123")
            .join("rustc-1.90.0")
            .join("aarch64-apple-darwin")
            .join("release");

        assert!(path.ends_with(suffix));
        let dev_key = TemplateCacheKey {
            compile_profile: CargoCompileProfile::Dev,
            ..key
        };
        assert_ne!(path, template_workspace_path_at(&root, &dev_key, false));
        assert!(template_workspace_path_at(&root, &dev_key, false).ends_with("dev"));
    }

    #[test]
    fn windows_paths_separate_complete_identities_and_preserve_sibling_groups() {
        let root = PathBuf::from("templates");
        let key = TemplateCacheKey {
            binary_sha256: "a".repeat(64),
            rustc_version: "rustc-1.100.0".into(),
            target_triple: "x86_64-pc-windows-msvc".into(),
            compile_profile: CargoCompileProfile::Release,
        };
        let group = windows_group_key(&key);
        assert_eq!(group.len(), 64);
        assert!(group
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()));
        let path = template_workspace_path_at(&root, &key, true);
        assert_eq!(
            path,
            root.join("w")
                .join(&group)
                .join(&key.target_triple)
                .join("r")
        );
        let mut changed = key.clone();
        changed.binary_sha256.replace_range(63.., "b");
        assert_ne!(group, windows_group_key(&changed));
        assert_ne!(path, template_workspace_path_at(&root, &changed, true));
        changed = key.clone();
        changed.rustc_version.push_str("-other");
        assert_ne!(group, windows_group_key(&changed));
        assert_ne!(path, template_workspace_path_at(&root, &changed, true));
        changed = key.clone();
        changed.target_triple = "aarch64-pc-windows-msvc".into();
        assert_eq!(group, windows_group_key(&changed));
        assert_ne!(path, template_workspace_path_at(&root, &changed, true));
        changed = key.clone();
        changed.compile_profile = CargoCompileProfile::Dev;
        assert_eq!(group, windows_group_key(&changed));
        assert!(template_workspace_path_at(&root, &changed, true).ends_with("d"));
        assert_ne!(path, template_workspace_path_at(&root, &changed, true));
        let mut left = key.clone();
        left.binary_sha256 = "ab".into();
        left.rustc_version = "c".into();
        let mut right = left.clone();
        right.binary_sha256 = "a".into();
        right.rustc_version = "bc".into();
        assert_ne!(windows_group_key(&left), windows_group_key(&right));
    }

    #[test]
    fn windows_incomplete_workspace_recovers_and_warm_reuse_skips_artifact_inventory() {
        use std::fs;

        let root = std::env::temp_dir().join(format!("cache-recovery-{}", uuid::Uuid::new_v4()));
        let target = BuildTarget::from_cli(Some("x86_64-pc-windows-msvc")).unwrap();
        let profile = CargoCompileProfile::Release;
        let key = super::resolve_template_cache_key(&target, profile).unwrap();
        let path = template_workspace_path_at(&root, &key, true);
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join("interrupted"), "incomplete owned workspace").unwrap();

        let recovered = ensure_warmed_template_with_prepare_hook_and_deps(
            &root,
            true,
            &target,
            profile,
            || {},
            super::prepare::template_workspace_ready,
            |workspace, target, profile| {
                assert!(workspace.join("interrupted").is_file());
                fs::remove_dir_all(workspace).unwrap();
                fs::create_dir_all(workspace.join("src")).unwrap();
                for file in ["Cargo.toml", "build.rs", ".agentcfg", "src/main.rs"] {
                    fs::write(workspace.join(file), "fixture").unwrap();
                }
                let seed = target.compiled_binary_path(workspace, "template_seed_agent", profile);
                fs::create_dir_all(seed.parent().unwrap()).unwrap();
                fs::write(seed, "compiled seed fixture").unwrap();
                Ok(())
            },
        )
        .unwrap();
        assert!(recovered.created);
        assert!(!path.join("interrupted").exists());

        let outside = root.join("unrelated");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("preserve"), "external sentinel").unwrap();
        let link = path.join("target/unrelated");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        #[cfg(windows)]
        {
            let output = std::process::Command::new("cmd")
                .args(["/C", "mklink", "/J"])
                .arg(link.to_string_lossy().replace('/', "\\"))
                .arg(outside.to_string_lossy().replace('/', "\\"))
                .output()
                .unwrap();
            assert!(output.status.success());
        }
        let reused = ensure_warmed_template_with_prepare_hook_and_deps(
            &root,
            true,
            &target,
            profile,
            || panic!("warm workspace must not start preparation"),
            super::prepare::template_workspace_ready,
            |_, _, _| panic!("warm workspace must not be rebuilt"),
        )
        .unwrap();
        assert!(!reused.created);
        assert_eq!(reused.path, path);
        assert_eq!(
            fs::read_to_string(outside.join("preserve")).unwrap(),
            "external sentinel"
        );
        #[cfg(unix)]
        fs::remove_file(link).unwrap();
        #[cfg(windows)]
        fs::remove_dir(link).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn prepare_hook_runs_only_when_template_is_created() {
        let build_target =
            BuildTarget::from_cli(Some("aarch64-apple-darwin")).expect("target should resolve");
        let hook_called = Arc::new(AtomicBool::new(false));
        let prepare_called = Arc::new(AtomicBool::new(false));
        let root =
            std::env::temp_dir().join(format!("cargo-ai-template-hook-{}", uuid::Uuid::new_v4()));

        let created = ensure_warmed_template_with_prepare_hook_and_deps(
            &root,
            false,
            &build_target,
            CargoCompileProfile::Release,
            {
                let hook_called = Arc::clone(&hook_called);
                move || {
                    hook_called.store(true, Ordering::SeqCst);
                }
            },
            |_, _, profile| {
                assert_eq!(profile, CargoCompileProfile::Release);
                false
            },
            {
                let prepare_called = Arc::clone(&prepare_called);
                move |_, _, profile| {
                    assert_eq!(profile, CargoCompileProfile::Release);
                    prepare_called.store(true, Ordering::SeqCst);
                    Ok(())
                }
            },
        )
        .expect("created template should succeed");

        assert!(created.created);
        assert!(hook_called.load(Ordering::SeqCst));
        assert!(prepare_called.load(Ordering::SeqCst));

        hook_called.store(false, Ordering::SeqCst);
        prepare_called.store(false, Ordering::SeqCst);

        let reused = ensure_warmed_template_with_prepare_hook_and_deps(
            &root,
            false,
            &build_target,
            CargoCompileProfile::Release,
            {
                let hook_called = Arc::clone(&hook_called);
                move || {
                    hook_called.store(true, Ordering::SeqCst);
                }
            },
            |_, _, profile| {
                assert_eq!(profile, CargoCompileProfile::Release);
                true
            },
            {
                let prepare_called = Arc::clone(&prepare_called);
                move |_, _, profile| {
                    assert_eq!(profile, CargoCompileProfile::Release);
                    prepare_called.store(true, Ordering::SeqCst);
                    Ok(())
                }
            },
        )
        .expect("reused template should succeed");

        assert!(!reused.created);
        assert!(!hook_called.load(Ordering::SeqCst));
        assert!(!prepare_called.load(Ordering::SeqCst));
    }
}
