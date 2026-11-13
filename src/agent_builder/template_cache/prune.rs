use super::{key::TemplateCacheKey, windows_group_key};
use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Component, Path};

pub(super) fn prune_stale_template_cache(
    root: &Path,
    active_key: &TemplateCacheKey,
    windows: bool,
) -> Result<usize, String> {
    if windows {
        return prune_windows_template_cache(root, active_key).map_err(|error| error.to_string());
    }
    prune_legacy_template_cache(root, active_key)
}

fn linked(metadata: &fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        return metadata.file_attributes() & 0x400 != 0;
    }
    #[cfg(not(windows))]
    false
}

fn directory_if_present(path: &Path) -> io::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !linked(&metadata) => Ok(true),
        Ok(_) => Err(io::Error::other(format!(
            "Cache directory is linked or is not a directory: {}",
            path.display()
        ))),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

fn unlinked_tree(path: &Path) -> io::Result<bool> {
    let metadata = fs::symlink_metadata(path)?;
    if linked(&metadata) || (!metadata.is_dir() && !metadata.is_file()) {
        return Ok(false);
    }
    if metadata.is_dir() {
        for entry in fs::read_dir(path)? {
            if !unlinked_tree(&entry?.path())? {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

fn recognized_workspace(path: &Path) -> io::Result<bool> {
    if !unlinked_tree(path)? {
        return Ok(false);
    }
    for required in ["Cargo.toml", "build.rs", ".agentcfg", "src/main.rs"] {
        if !path.join(required).is_file() {
            return Ok(false);
        }
    }
    let manifest = fs::read_to_string(path.join("Cargo.toml"))?;
    Ok(toml::from_str::<toml::Value>(&manifest)
        .ok()
        .is_some_and(|manifest| {
            manifest
                .get("package")
                .and_then(|package| package.get("name"))
                .and_then(toml::Value::as_str)
                == Some("template_seed_agent")
        }))
}

pub(super) fn validate_windows_workspace_path(root: &Path, path: &Path) -> io::Result<()> {
    // Check owned ancestors without inventorying artifacts or requiring a completed build.
    directory_if_present(root)?;
    let relative = path
        .strip_prefix(root)
        .map_err(|_| io::Error::other("Cache workspace escapes its root"))?;
    let components = relative.components().collect::<Vec<_>>();
    if components.len() < 4
        || components.first() != Some(&Component::Normal(OsStr::new("w")))
        || components
            .iter()
            .any(|component| !matches!(component, Component::Normal(_)))
        || !matches!(components.last(), Some(Component::Normal(name)) if *name == OsStr::new("r") || *name == OsStr::new("d"))
    {
        return Err(io::Error::other(
            "Unrecognized Windows cache workspace path",
        ));
    }
    let mut current = root.to_path_buf();
    for component in components {
        current.push(component.as_os_str());
        directory_if_present(&current)?;
    }
    directory_if_present(&path.join("target"))?;
    Ok(())
}

fn recognized_group(path: &Path) -> io::Result<bool> {
    let mut found_profile = false;
    for target in fs::read_dir(path)? {
        let target = target?;
        if !directory_if_present(&target.path()).unwrap_or(false) {
            return Ok(false);
        }
        let mut target_has_profile = false;
        for profile in fs::read_dir(target.path())? {
            let profile = profile?;
            if !matches!(profile.file_name().to_str(), Some("r" | "d"))
                || !directory_if_present(&profile.path()).unwrap_or(false)
                || !recognized_workspace(&profile.path())?
            {
                return Ok(false);
            }
            target_has_profile = true;
            found_profile = true;
        }
        if !target_has_profile {
            return Ok(false);
        }
    }
    Ok(found_profile)
}

fn prune_windows_template_cache(root: &Path, active_key: &TemplateCacheKey) -> io::Result<usize> {
    if !directory_if_present(root)? {
        return Ok(0);
    }
    let namespace = root.join("w");
    if !directory_if_present(&namespace)? {
        return Ok(0);
    }
    let active_group = windows_group_key(active_key);
    let mut removed = 0;
    for entry in fs::read_dir(&namespace)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        // The active group's sibling targets and profiles remain reusable without a tree walk.
        if name == active_group
            || name.len() != 64
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            || !directory_if_present(&entry.path()).unwrap_or(false)
            || !recognized_group(&entry.path())?
        {
            continue;
        }
        fs::remove_dir_all(entry.path())?;
        removed += 1;
    }
    Ok(removed)
}

fn prune_legacy_template_cache(
    root: &Path,
    active_key: &TemplateCacheKey,
) -> Result<usize, String> {
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
        prune_legacy_template_cache, prune_windows_template_cache, validate_windows_workspace_path,
    };
    use crate::agent_builder::build_target::CargoCompileProfile;
    use crate::agent_builder::template_cache::key::TemplateCacheKey;
    use crate::agent_builder::template_cache::{template_workspace_path_at, windows_group_key};
    use std::fs;
    use std::path::{Path, PathBuf};
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
    fn prune_stale_template_cache_keeps_active_parent_and_sibling_targets() {
        let root = temp_test_dir();
        let active_key = TemplateCacheKey {
            binary_sha256: "sha-current".to_string(),
            rustc_version: "rustc-1.91.1".to_string(),
            target_triple: "aarch64-apple-darwin".to_string(),
            compile_profile: CargoCompileProfile::Release,
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

        let active_profile = active_target.join("release");
        let sibling_profile = active_target.join("dev");
        fs::create_dir_all(&active_profile).unwrap();
        fs::create_dir_all(&sibling_profile).unwrap();

        let removed = prune_legacy_template_cache(&root, &active_key)
            .expect("pruning stale template cache should succeed");

        assert_eq!(removed, 2);
        assert!(!root.join("sha-old").exists());
        assert!(!root.join("sha-current").join("rustc-1.90.0").exists());
        assert!(active_target.exists());
        assert!(sibling_target.exists());
        assert!(active_profile.exists());
        assert!(sibling_profile.exists());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn prune_stale_template_cache_is_noop_when_root_missing() {
        let missing_root = std::env::temp_dir().join("cargo-ai-template-cache-missing-root");
        let active_key = TemplateCacheKey {
            binary_sha256: "sha-current".to_string(),
            rustc_version: "rustc-1.91.1".to_string(),
            target_triple: "aarch64-apple-darwin".to_string(),
            compile_profile: CargoCompileProfile::Release,
        };

        let removed = prune_legacy_template_cache(&missing_root, &active_key)
            .expect("missing cache root should be treated as empty");
        assert_eq!(removed, 0);
    }

    fn windows_key() -> TemplateCacheKey {
        TemplateCacheKey {
            binary_sha256: "a".repeat(64),
            rustc_version: "rustc-1.100.0".into(),
            target_triple: "x86_64-pc-windows-msvc".into(),
            compile_profile: CargoCompileProfile::Release,
        }
    }

    fn create_workspace(root: &Path, key: &TemplateCacheKey) -> PathBuf {
        let path = template_workspace_path_at(root, key, true);
        fs::create_dir_all(path.join("src")).unwrap();
        fs::create_dir_all(path.join("target")).unwrap();
        fs::write(
            path.join("Cargo.toml"),
            "[package]\nname = 'template_seed_agent'\n",
        )
        .unwrap();
        for name in ["build.rs", ".agentcfg", "src/main.rs"] {
            fs::write(path.join(name), "cache fixture").unwrap();
        }
        path
    }

    #[test]
    fn windows_pruning_preserves_siblings_old_layout_and_unknown_entries() {
        let root = temp_test_dir();
        let active = windows_key();
        let release = create_workspace(&root, &active);
        let mut sibling = active.clone();
        sibling.compile_profile = CargoCompileProfile::Dev;
        let dev = create_workspace(&root, &sibling);
        sibling.target_triple = "aarch64-pc-windows-msvc".into();
        let other_target = create_workspace(&root, &sibling);
        let mut stale = active.clone();
        stale.binary_sha256 = "b".repeat(64);
        let old_binary = create_workspace(&root, &stale);
        stale = active.clone();
        stale.rustc_version = "rustc-1.99.1".into();
        let old_compiler = create_workspace(&root, &stale);
        let legacy = root.join(&active.binary_sha256).join(&active.rustc_version);
        fs::create_dir_all(&legacy).unwrap();
        fs::write(legacy.join("preserve"), "legacy sentinel").unwrap();
        let unknown = root.join("w").join("c".repeat(64));
        fs::create_dir_all(&unknown).unwrap();
        fs::write(unknown.join("notes"), "unknown sentinel").unwrap();
        fs::write(root.join("w/readme"), "unrelated sentinel").unwrap();
        validate_windows_workspace_path(&root, &release).unwrap();
        assert_eq!(prune_windows_template_cache(&root, &active).unwrap(), 2);
        assert!(release.is_dir());
        assert!(dev.is_dir());
        assert!(other_target.is_dir());
        assert!(!old_binary.exists());
        assert!(!old_compiler.exists());
        assert_eq!(
            fs::read_to_string(legacy.join("preserve")).unwrap(),
            "legacy sentinel"
        );
        assert_eq!(
            fs::read_to_string(unknown.join("notes")).unwrap(),
            "unknown sentinel"
        );
        assert_eq!(
            fs::read_to_string(root.join("w/readme")).unwrap(),
            "unrelated sentinel"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn windows_preparation_allows_incomplete_owned_workspaces_and_rejects_escape() {
        let root = temp_test_dir();
        let path = template_workspace_path_at(&root, &windows_key(), true);
        validate_windows_workspace_path(&root, &path).unwrap();
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join("notes"), "preserve").unwrap();
        validate_windows_workspace_path(&root, &path).unwrap();
        assert!(
            validate_windows_workspace_path(&root, &root.join("w/group/../outside/r")).is_err()
        );
        assert_eq!(fs::read_to_string(path.join("notes")).unwrap(), "preserve");
        fs::remove_dir_all(root).unwrap();
    }

    fn link_directory(source: &Path, target: &Path) {
        #[cfg(unix)]
        std::os::unix::fs::symlink(target, source).unwrap();
        #[cfg(windows)]
        {
            let output = std::process::Command::new("cmd")
                .args(["/C", "mklink", "/J"])
                .arg(source.to_string_lossy().replace('/', "\\"))
                .arg(target.to_string_lossy().replace('/', "\\"))
                .output()
                .unwrap();
            assert!(output.status.success());
        }
    }

    fn unlink_directory(path: &Path) {
        #[cfg(unix)]
        fs::remove_file(path).unwrap();
        #[cfg(windows)]
        fs::remove_dir(path).unwrap();
    }

    #[test]
    fn windows_cache_refuses_linked_namespace_groups_and_workspace_entries() {
        let root = temp_test_dir();
        let outside = temp_test_dir();
        fs::write(outside.join("preserve"), "external sentinel").unwrap();
        let key = windows_key();
        let path = template_workspace_path_at(&root, &key, true);
        let namespace = root.join("w");
        link_directory(&namespace, &outside);
        assert!(validate_windows_workspace_path(&root, &path).is_err());
        assert!(prune_windows_template_cache(&root, &key).is_err());
        unlink_directory(&namespace);
        fs::create_dir(&namespace).unwrap();
        let group = namespace.join(windows_group_key(&key));
        link_directory(&group, &outside);
        assert!(validate_windows_workspace_path(&root, &path).is_err());
        assert_eq!(prune_windows_template_cache(&root, &key).unwrap(), 0);
        unlink_directory(&group);
        let stale_link = namespace.join("f".repeat(64));
        link_directory(&stale_link, &outside);
        assert_eq!(prune_windows_template_cache(&root, &key).unwrap(), 0);
        unlink_directory(&stale_link);
        let workspace = create_workspace(&root, &key);
        let nested_link = workspace.join("target/linked");
        link_directory(&nested_link, &outside);
        validate_windows_workspace_path(&root, &path).unwrap();
        let mut other = key.clone();
        other.rustc_version.push_str("-other");
        assert_eq!(prune_windows_template_cache(&root, &other).unwrap(), 0);
        unlink_directory(&nested_link);
        fs::remove_dir(workspace.join("target")).unwrap();
        link_directory(&workspace.join("target"), &outside);
        assert!(validate_windows_workspace_path(&root, &path).is_err());
        unlink_directory(&workspace.join("target"));
        assert_eq!(
            fs::read_to_string(outside.join("preserve")).unwrap(),
            "external sentinel"
        );
        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(outside).unwrap();
    }
}
