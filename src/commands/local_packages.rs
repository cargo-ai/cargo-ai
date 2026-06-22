//! Local machine package install and lookup support.
use clap::ArgMatches;
use semver::Version;
use serde::{Deserialize, Serialize};
use std::collections::{btree_map::Entry, BTreeMap};
use std::fs;
use std::path::{Path, PathBuf};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;
use uuid::Uuid;

const PACKAGE_MANIFEST_FILE_NAME: &str = "cargo-ai-package.toml";
const INSTALL_MANIFEST_FILE_NAME: &str = "install.toml";

#[cfg(test)]
thread_local! {
    static TEST_PACKAGES_ROOT: std::cell::RefCell<Option<PathBuf>> = std::cell::RefCell::new(None);
}

#[derive(Clone, Debug, Deserialize)]
struct PackageManifestDocument {
    format_version: u32,
    #[serde(default)]
    project_name: Option<String>,
    #[serde(default)]
    project_version: Option<String>,
    profile: String,
    #[serde(default)]
    agent_definitions: Vec<String>,
    #[serde(default)]
    hatched_agents: Vec<String>,
    #[serde(default)]
    tools: Vec<String>,
    #[serde(default)]
    assets: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct InstalledPackageDocument {
    format_version: u32,
    alias: String,
    package_name: String,
    package_version: String,
    profile: String,
    content_sha256: String,
    source: InstalledPackageSourceDocument,
    installed_at: String,
    entrypoints: Vec<InstalledPackageEntrypointDocument>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct InstalledPackageSourceDocument {
    kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    path: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct InstalledPackageEntrypointDocument {
    name: String,
    path: String,
    runnable: bool,
    hatchable: bool,
}

#[derive(Clone, Debug)]
struct PreparedPackage {
    package_root: PathBuf,
    manifest: PackageManifestDocument,
    content_sha256: String,
    source: InstalledPackageSourceDocument,
    temporary_root: Option<PathBuf>,
}

#[derive(Clone, Debug)]
struct InstallRequest {
    source: Option<String>,
    alias: Option<String>,
    profile: String,
    replace: bool,
    downgrade: bool,
}

#[derive(Clone, Debug)]
enum InstallAction {
    New,
    Noop,
    Upgrade,
    Replace,
    Downgrade,
}

#[derive(Clone, Debug)]
pub(crate) struct ResolvedPackageEntrypoint {
    pub(crate) alias: String,
    pub(crate) entrypoint: String,
    pub(crate) definition_path: PathBuf,
    pub(crate) package_root: PathBuf,
    pub(crate) package_name: String,
    pub(crate) package_version: String,
    pub(crate) content_sha256: String,
}

pub async fn run(sub_m: &ArgMatches) -> bool {
    if let Some(list_m) = sub_m.subcommand_matches("list") {
        run_list(list_m)
    } else if let Some(install_m) = sub_m.subcommand_matches("install") {
        run_install(install_m)
    } else if let Some(inspect_m) = sub_m.subcommand_matches("inspect") {
        run_inspect(inspect_m)
    } else if let Some(uninstall_m) = sub_m.subcommand_matches("uninstall") {
        run_uninstall(uninstall_m)
    } else {
        eprintln!(
            "No local packages subcommand found. Try 'cargo ai packages list|install|inspect|uninstall'."
        );
        false
    }
}

pub(crate) fn account_handle_from_list_matches(list_m: &ArgMatches) -> Option<Option<String>> {
    list_m.get_one::<String>("account").map(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

fn run_list(list_m: &ArgMatches) -> bool {
    let display_limit = if list_m.get_flag("all") {
        None
    } else {
        Some(
            list_m
                .get_one::<u32>("limit")
                .copied()
                .unwrap_or(20)
                .try_into()
                .expect("u32 limit should fit in usize"),
        )
    };

    match list_installed_packages() {
        Ok(mut packages) => {
            packages.sort_by(|left, right| left.alias.cmp(&right.alias));
            if let Some(limit) = display_limit {
                packages.truncate(limit);
            }
            if packages.is_empty() {
                println!("No local packages are installed.");
                return true;
            }

            println!("Installed packages:");
            for package in packages {
                println!(
                    "- {}  {} {}  {}",
                    package.alias,
                    package.package_name,
                    package.package_version,
                    entrypoint_summary(&package.entrypoints)
                );
            }
            true
        }
        Err(error) => {
            eprintln!("x {error}");
            false
        }
    }
}

fn run_install(install_m: &ArgMatches) -> bool {
    let request = InstallRequest {
        source: install_m
            .get_one::<String>("source")
            .map(|value| value.to_string()),
        alias: install_m
            .get_one::<String>("alias")
            .map(|value| value.to_string()),
        profile: install_m
            .get_one::<String>("profile")
            .map(|value| value.to_string())
            .unwrap_or_else(|| "default".to_string()),
        replace: install_m.get_flag("replace"),
        downgrade: install_m.get_flag("downgrade"),
    };

    match install_local_package(&request) {
        Ok(InstallAction::Noop) => true,
        Ok(_) => true,
        Err(error) => {
            eprintln!("x {error}");
            false
        }
    }
}

fn run_inspect(inspect_m: &ArgMatches) -> bool {
    let Some(alias) = inspect_m.get_one::<String>("alias").map(String::as_str) else {
        eprintln!("x Missing package alias.");
        return false;
    };

    match load_installed_package(alias) {
        Ok(package) => {
            println!("Package:   {}", package.alias);
            println!("Identity:  {}", package.package_name);
            println!("Version:   {}", package.package_version);
            println!("Profile:   {}", package.profile);
            println!("Hash:      {}", package.content_sha256);
            println!("Installed: {}", package.installed_at);
            match package.source.path.as_deref() {
                Some(path) => println!("Source:    {} ({})", package.source.kind, path),
                None => println!("Source:    {}", package.source.kind),
            }
            println!("Entrypoints:");
            for entrypoint in package.entrypoints {
                let mut capabilities = Vec::new();
                if entrypoint.runnable {
                    capabilities.push("run");
                }
                if entrypoint.hatchable {
                    capabilities.push("hatch");
                }
                println!(
                    "- {}  {}  {}",
                    entrypoint.name,
                    capabilities.join(","),
                    entrypoint.path
                );
            }
            true
        }
        Err(error) => {
            eprintln!("x {error}");
            false
        }
    }
}

fn run_uninstall(uninstall_m: &ArgMatches) -> bool {
    let Some(alias) = uninstall_m.get_one::<String>("alias").map(String::as_str) else {
        eprintln!("x Missing package alias.");
        return false;
    };

    match uninstall_package(alias) {
        Ok(()) => {
            println!("✓ Package `{alias}` uninstalled");
            true
        }
        Err(error) => {
            eprintln!("x {error}");
            false
        }
    }
}

fn install_local_package(request: &InstallRequest) -> Result<InstallAction, String> {
    let prepared = match request.source.as_deref() {
        Some(source) => prepare_explicit_source(source)?,
        None => prepare_current_project(request.profile.as_str())?,
    };
    let package_name = required_package_name(&prepared.manifest)?;
    let package_version = required_package_version(&prepared.manifest)?;
    let alias = request
        .alias
        .as_deref()
        .unwrap_or(package_name.as_str())
        .trim()
        .to_string();
    validate_package_alias(alias.as_str())?;
    let entrypoints = build_entrypoints(&prepared.manifest, &prepared.package_root)?;
    let existing = load_installed_package(alias.as_str()).ok();
    let action = determine_install_action(
        existing.as_ref(),
        package_name.as_str(),
        package_version.as_str(),
        prepared.content_sha256.as_str(),
        request.replace,
        request.downgrade,
    )?;

    if matches!(action, InstallAction::Noop) {
        println!(
            "✓ Package `{}` is already installed at version {}.",
            alias, package_version
        );
        cleanup_prepared_package(&prepared);
        return Ok(action);
    }

    let document = InstalledPackageDocument {
        format_version: 1,
        alias: alias.clone(),
        package_name: package_name.clone(),
        package_version: package_version.clone(),
        profile: prepared.manifest.profile.clone(),
        content_sha256: prepared.content_sha256.clone(),
        source: prepared.source.clone(),
        installed_at: now_rfc3339()?,
        entrypoints,
    };

    write_staged_install(alias.as_str(), &prepared.package_root, &document)?;
    cleanup_prepared_package(&prepared);

    match action {
        InstallAction::New => println!(
            "✓ Package `{}` installed as `{}` at version {}.",
            package_name, alias, package_version
        ),
        InstallAction::Upgrade => println!(
            "✓ Package `{}` upgraded as `{}` to version {}.",
            package_name, alias, package_version
        ),
        InstallAction::Replace => println!(
            "✓ Package alias `{}` replaced with `{}` version {}.",
            alias, package_name, package_version
        ),
        InstallAction::Downgrade => println!(
            "✓ Package `{}` downgraded as `{}` to version {}.",
            package_name, alias, package_version
        ),
        InstallAction::Noop => {}
    }
    Ok(action)
}

pub(crate) fn resolve_entrypoint_reference(
    reference: &str,
    require_hatchable: bool,
) -> Result<Option<ResolvedPackageEntrypoint>, String> {
    let Some((alias, entrypoint)) = reference.split_once("::") else {
        return Ok(None);
    };
    validate_package_alias(alias)?;
    validate_entrypoint_name(entrypoint)?;

    let package = load_installed_package(alias)?;
    let installed_root = installed_package_root(alias);
    let entry = package
        .entrypoints
        .iter()
        .find(|candidate| candidate.name == entrypoint)
        .ok_or_else(|| {
            format!(
                "Package alias `{}` does not export entrypoint `{}`.",
                alias, entrypoint
            )
        })?;
    if !entry.runnable {
        return Err(format!(
            "Package entrypoint `{}` is not runnable.",
            reference
        ));
    }
    if require_hatchable && !entry.hatchable {
        return Err(format!(
            "Package entrypoint `{}` is not hatchable.",
            reference
        ));
    }

    let definition_path = installed_root.join("package").join(entry.path.as_str());
    if !definition_path.is_file() {
        return Err(format!(
            "Installed package entrypoint `{}` points to missing definition '{}'.",
            reference,
            definition_path.display()
        ));
    }

    Ok(Some(ResolvedPackageEntrypoint {
        alias: alias.to_string(),
        entrypoint: entrypoint.to_string(),
        definition_path,
        package_root: installed_root.join("package"),
        package_name: package.package_name,
        package_version: package.package_version,
        content_sha256: package.content_sha256,
    }))
}

#[cfg(feature = "developer-tools")]
fn prepare_current_project(profile: &str) -> Result<PreparedPackage, String> {
    let assembled =
        crate::commands::package::assemble_current_project_package(profile, None, true, false)?;
    let manifest_path = assembled.root_path.join(PACKAGE_MANIFEST_FILE_NAME);
    let manifest = load_package_manifest(manifest_path.as_path())?;
    Ok(PreparedPackage {
        package_root: assembled.root_path,
        manifest,
        content_sha256: crate::commands::account::sha256_hex(assembled.archive_bytes.as_slice()),
        source: InstalledPackageSourceDocument {
            kind: "current_project".to_string(),
            path: std::env::current_dir()
                .ok()
                .map(|path| path.display().to_string()),
        },
        temporary_root: None,
    })
}

#[cfg(not(feature = "developer-tools"))]
fn prepare_current_project(_profile: &str) -> Result<PreparedPackage, String> {
    Err(
        "Installing the current project requires a Cargo AI build with developer-tools enabled. Provide an explicit local package source instead."
            .to_string(),
    )
}

fn prepare_explicit_source(source: &str) -> Result<PreparedPackage, String> {
    let trimmed = source.trim();
    if trimmed.is_empty() {
        return Err("Package source cannot be empty.".to_string());
    }
    let source_path = PathBuf::from(trimmed);
    if !source_path.exists() {
        return Err(format!(
            "Local package source '{}' was not found. Use a local package path, or add --account to install from cargo-ai.org in the hosted install lane.",
            source
        ));
    }

    if source_path.is_dir() {
        return prepare_package_root(
            source_path,
            InstalledPackageSourceDocument {
                kind: "local_root".to_string(),
                path: Some(trimmed.to_string()),
            },
            None,
        );
    }

    if source_path
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| name == PACKAGE_MANIFEST_FILE_NAME)
        .unwrap_or(false)
    {
        let package_root = source_path.parent().ok_or_else(|| {
            format!(
                "Package manifest '{}' has no parent package root.",
                source_path.display()
            )
        })?;
        return prepare_package_root(
            package_root.to_path_buf(),
            InstalledPackageSourceDocument {
                kind: "manifest_path".to_string(),
                path: Some(trimmed.to_string()),
            },
            None,
        );
    }

    prepare_archive_source(source_path.as_path(), trimmed)
}

fn prepare_archive_source(
    source_path: &Path,
    source_display: &str,
) -> Result<PreparedPackage, String> {
    let staging_root = packages_staging_root().join(format!("archive-{}", Uuid::new_v4()));
    fs::create_dir_all(&staging_root).map_err(|error| {
        format!(
            "Failed to create package archive staging directory '{}': {}",
            staging_root.display(),
            error
        )
    })?;
    let bytes = fs::read(source_path).map_err(|error| {
        format!(
            "Failed to read package archive '{}': {}",
            source_path.display(),
            error
        )
    })?;
    if let Err(error) =
        crate::commands::account::extract_package_archive_bytes(bytes.as_slice(), &staging_root)
    {
        let _ = fs::remove_dir_all(&staging_root);
        return Err(error);
    }

    prepare_package_root(
        staging_root.clone(),
        InstalledPackageSourceDocument {
            kind: "local_archive".to_string(),
            path: Some(source_display.to_string()),
        },
        Some(staging_root),
    )
}

fn prepare_package_root(
    package_root: PathBuf,
    source: InstalledPackageSourceDocument,
    temporary_root: Option<PathBuf>,
) -> Result<PreparedPackage, String> {
    let manifest = load_package_manifest(package_root.join(PACKAGE_MANIFEST_FILE_NAME).as_path())?;
    let archive_bytes =
        crate::commands::account::create_package_archive_bytes(package_root.as_path())?;
    let content_sha256 = crate::commands::account::sha256_hex(archive_bytes.as_slice());
    Ok(PreparedPackage {
        package_root,
        manifest,
        content_sha256,
        source,
        temporary_root,
    })
}

fn load_package_manifest(path: &Path) -> Result<PackageManifestDocument, String> {
    let contents = fs::read_to_string(path).map_err(|error| {
        format!(
            "Failed to read package manifest '{}': {}",
            path.display(),
            error
        )
    })?;
    let manifest: PackageManifestDocument = toml::from_str(contents.as_str()).map_err(|error| {
        format!(
            "Failed to parse package manifest '{}': {}",
            path.display(),
            error
        )
    })?;
    if manifest.format_version != 1 {
        return Err(format!(
            "Package manifest '{}' has unsupported format_version {}.",
            path.display(),
            manifest.format_version
        ));
    }
    Ok(manifest)
}

fn required_package_name(manifest: &PackageManifestDocument) -> Result<String, String> {
    manifest
        .project_name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .ok_or_else(|| "Package install requires `cargo-ai-package.toml` project_name.".to_string())
}

fn required_package_version(manifest: &PackageManifestDocument) -> Result<String, String> {
    let version = manifest
        .project_version
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .ok_or_else(|| {
            "Package install requires `cargo-ai-package.toml` project_version.".to_string()
        })?;
    Version::parse(version.as_str()).map_err(|error| {
        format!(
            "Package version '{}' is not valid semver: {}",
            version, error
        )
    })?;
    Ok(version)
}

fn build_entrypoints(
    manifest: &PackageManifestDocument,
    package_root: &Path,
) -> Result<Vec<InstalledPackageEntrypointDocument>, String> {
    let _support_material_count = manifest.tools.len() + manifest.assets.len();
    let mut entrypoints = BTreeMap::new();
    for relative_path in &manifest.agent_definitions {
        upsert_entrypoint(&mut entrypoints, package_root, relative_path, true, false)?;
    }
    for relative_path in &manifest.hatched_agents {
        upsert_entrypoint(&mut entrypoints, package_root, relative_path, true, true)?;
    }
    Ok(entrypoints.into_values().collect())
}

fn upsert_entrypoint(
    entrypoints: &mut BTreeMap<String, InstalledPackageEntrypointDocument>,
    package_root: &Path,
    relative_path: &str,
    runnable: bool,
    hatchable: bool,
) -> Result<(), String> {
    validate_package_relative_json_path(relative_path)?;
    let definition_path = package_root.join(relative_path);
    if !definition_path.is_file() {
        return Err(format!(
            "Package entrypoint definition '{}' was not found.",
            definition_path.display()
        ));
    }
    let name = entrypoint_name_from_path(relative_path)?;
    match entrypoints.entry(name.clone()) {
        Entry::Vacant(slot) => {
            slot.insert(InstalledPackageEntrypointDocument {
                name,
                path: relative_path.replace('\\', "/"),
                runnable,
                hatchable,
            });
        }
        Entry::Occupied(mut slot) => {
            if slot.get().path != relative_path.replace('\\', "/") {
                return Err(format!(
                    "Package entrypoint name '{}' is declared by multiple paths ('{}' and '{}').",
                    name,
                    slot.get().path,
                    relative_path
                ));
            }
            slot.get_mut().runnable |= runnable;
            slot.get_mut().hatchable |= hatchable;
        }
    }
    Ok(())
}

fn entrypoint_name_from_path(relative_path: &str) -> Result<String, String> {
    let name = Path::new(relative_path)
        .file_stem()
        .and_then(|value| value.to_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            format!(
                "Unable to derive package entrypoint name from '{}'.",
                relative_path
            )
        })?
        .to_string();
    validate_entrypoint_name(name.as_str())?;
    Ok(name)
}

fn determine_install_action(
    existing: Option<&InstalledPackageDocument>,
    package_name: &str,
    package_version: &str,
    content_sha256: &str,
    replace: bool,
    downgrade: bool,
) -> Result<InstallAction, String> {
    let Some(existing) = existing else {
        return Ok(InstallAction::New);
    };
    if existing.package_name != package_name {
        if replace {
            return Ok(InstallAction::Replace);
        }
        return Err(format!(
            "Package alias `{}` is already installed for package `{}`. Re-run with --replace to replace it with `{}`.",
            existing.alias, existing.package_name, package_name
        ));
    }

    let existing_version = Version::parse(existing.package_version.as_str()).map_err(|error| {
        format!(
            "Installed package alias `{}` has invalid version '{}': {}",
            existing.alias, existing.package_version, error
        )
    })?;
    let new_version = Version::parse(package_version).map_err(|error| {
        format!(
            "Package version '{}' is not valid semver: {}",
            package_version, error
        )
    })?;

    if new_version == existing_version {
        if existing.content_sha256 == content_sha256 {
            return Ok(InstallAction::Noop);
        }
        if replace {
            return Ok(InstallAction::Replace);
        }
        return Err(format!(
            "Package alias `{}` already has `{}` version {} installed with different content. Re-run with --replace to replace same-version content.",
            existing.alias, package_name, package_version
        ));
    }
    if new_version > existing_version {
        return Ok(InstallAction::Upgrade);
    }
    if downgrade {
        return Ok(InstallAction::Downgrade);
    }
    Err(format!(
        "Package alias `{}` already has `{}` version {} installed. Installing older version {} requires --downgrade.",
        existing.alias, package_name, existing.package_version, package_version
    ))
}

fn write_staged_install(
    alias: &str,
    package_root: &Path,
    document: &InstalledPackageDocument,
) -> Result<(), String> {
    let packages_root = packages_root();
    let staging_root = packages_staging_root().join(format!("{}-{}", alias, Uuid::new_v4()));
    let staged_package_root = staging_root.join("package");
    fs::create_dir_all(&staged_package_root).map_err(|error| {
        format!(
            "Failed to create staged package directory '{}': {}",
            staged_package_root.display(),
            error
        )
    })?;
    copy_directory_recursive(package_root, staged_package_root.as_path())?;
    write_install_manifest(
        staging_root.join(INSTALL_MANIFEST_FILE_NAME).as_path(),
        document,
    )?;

    let install_root = packages_root.join(alias);
    if install_root.exists() {
        fs::remove_dir_all(&install_root).map_err(|error| {
            format!(
                "Failed to replace installed package alias '{}': {}",
                install_root.display(),
                error
            )
        })?;
    }
    fs::create_dir_all(&packages_root).map_err(|error| {
        format!(
            "Failed to create package store '{}': {}",
            packages_root.display(),
            error
        )
    })?;
    fs::rename(&staging_root, &install_root).map_err(|error| {
        format!(
            "Failed to move staged package install from '{}' to '{}': {}",
            staging_root.display(),
            install_root.display(),
            error
        )
    })
}

fn write_install_manifest(path: &Path, document: &InstalledPackageDocument) -> Result<(), String> {
    let rendered = toml::to_string_pretty(document)
        .map_err(|error| format!("Failed to render install metadata TOML: {error}"))?;
    fs::write(path, rendered).map_err(|error| {
        format!(
            "Failed to write install metadata '{}': {}",
            path.display(),
            error
        )
    })
}

fn list_installed_packages() -> Result<Vec<InstalledPackageDocument>, String> {
    let root = packages_root();
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut packages = Vec::new();
    for entry in fs::read_dir(root.as_path()).map_err(|error| {
        format!(
            "Failed to read package store '{}': {}",
            root.display(),
            error
        )
    })? {
        let entry = entry.map_err(|error| {
            format!(
                "Failed to read package store entry under '{}': {}",
                root.display(),
                error
            )
        })?;
        if !entry.path().is_dir() {
            continue;
        }
        let alias = entry.file_name().to_string_lossy().to_string();
        if alias.starts_with('.') {
            continue;
        }
        packages.push(load_installed_package(alias.as_str())?);
    }
    Ok(packages)
}

fn load_installed_package(alias: &str) -> Result<InstalledPackageDocument, String> {
    validate_package_alias(alias)?;
    let metadata_path = installed_package_root(alias).join(INSTALL_MANIFEST_FILE_NAME);
    let contents = fs::read_to_string(&metadata_path).map_err(|error| {
        format!(
            "Failed to read installed package metadata '{}': {}",
            metadata_path.display(),
            error
        )
    })?;
    toml::from_str(contents.as_str()).map_err(|error| {
        format!(
            "Failed to parse installed package metadata '{}': {}",
            metadata_path.display(),
            error
        )
    })
}

fn uninstall_package(alias: &str) -> Result<(), String> {
    validate_package_alias(alias)?;
    let install_root = installed_package_root(alias);
    if !install_root.exists() {
        return Err(format!("Package alias `{alias}` is not installed."));
    }
    fs::remove_dir_all(&install_root).map_err(|error| {
        format!(
            "Failed to uninstall package alias '{}' from '{}': {}",
            alias,
            install_root.display(),
            error
        )
    })
}

fn cleanup_prepared_package(prepared: &PreparedPackage) {
    if let Some(path) = prepared.temporary_root.as_ref() {
        let _ = fs::remove_dir_all(path);
    }
}

fn copy_directory_recursive(source: &Path, dest: &Path) -> Result<(), String> {
    fs::create_dir_all(dest).map_err(|error| {
        format!(
            "Failed to create destination directory '{}': {}",
            dest.display(),
            error
        )
    })?;
    let mut entries = fs::read_dir(source)
        .map_err(|error| format!("Failed to read directory '{}': {}", source.display(), error))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| {
            format!(
                "Failed to read directory entry under '{}': {}",
                source.display(),
                error
            )
        })?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let source_path = entry.path();
        let dest_path = dest.join(entry.file_name());
        if source_path.is_dir() {
            copy_directory_recursive(source_path.as_path(), dest_path.as_path())?;
        } else {
            if let Some(parent) = dest_path.parent() {
                fs::create_dir_all(parent).map_err(|error| {
                    format!(
                        "Failed to create destination directory '{}': {}",
                        parent.display(),
                        error
                    )
                })?;
            }
            fs::copy(source_path.as_path(), dest_path.as_path()).map_err(|error| {
                format!(
                    "Failed to copy '{}' into '{}': {}",
                    source_path.display(),
                    dest_path.display(),
                    error
                )
            })?;
        }
    }
    Ok(())
}

fn validate_package_relative_json_path(relative_path: &str) -> Result<(), String> {
    let path = Path::new(relative_path);
    if path.is_absolute() {
        return Err(format!(
            "Package entrypoint path '{}' must be relative.",
            relative_path
        ));
    }
    if path
        .components()
        .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(format!(
            "Package entrypoint path '{}' must not contain parent traversal.",
            relative_path
        ));
    }
    let is_json = path
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.eq_ignore_ascii_case("json"))
        .unwrap_or(false);
    if !is_json {
        return Err(format!(
            "Package entrypoint path '{}' must point to a JSON file.",
            relative_path
        ));
    }
    Ok(())
}

fn validate_package_alias(alias: &str) -> Result<(), String> {
    validate_identifier(alias, "Package alias")
}

fn validate_entrypoint_name(name: &str) -> Result<(), String> {
    validate_identifier(name, "Package entrypoint")
}

fn validate_identifier(value: &str, label: &str) -> Result<(), String> {
    if value.is_empty()
        || !value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
    {
        return Err(format!(
            "{label} '{}' is invalid. Use only letters, numbers, '-' or '_'.",
            value
        ));
    }
    Ok(())
}

fn entrypoint_summary(entrypoints: &[InstalledPackageEntrypointDocument]) -> String {
    if entrypoints.is_empty() {
        return "0 entrypoints".to_string();
    }
    let run_count = entrypoints.iter().filter(|entry| entry.runnable).count();
    let hatch_count = entrypoints.iter().filter(|entry| entry.hatchable).count();
    format!("{run_count} run / {hatch_count} hatch")
}

fn now_rfc3339() -> Result<String, String> {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .map_err(|error| format!("Failed to format install timestamp: {error}"))
}

fn packages_root() -> PathBuf {
    #[cfg(test)]
    if let Some(root) = TEST_PACKAGES_ROOT.with(|value| value.borrow().clone()) {
        return root;
    }

    crate::config::paths::cargo_ai_root().join("packages")
}

fn packages_staging_root() -> PathBuf {
    packages_root().join(".staging")
}

fn installed_package_root(alias: &str) -> PathBuf {
    packages_root().join(alias)
}

#[cfg(test)]
mod tests {
    use super::{
        build_entrypoints, determine_install_action, install_local_package, load_installed_package,
        resolve_entrypoint_reference, uninstall_package, InstallAction, InstallRequest,
        PackageManifestDocument,
    };
    use std::path::{Path, PathBuf};

    fn manifest(
        agent_definitions: Vec<&str>,
        hatched_agents: Vec<&str>,
    ) -> PackageManifestDocument {
        PackageManifestDocument {
            format_version: 1,
            project_name: Some("demo".to_string()),
            project_version: Some("1.0.0".to_string()),
            profile: "default".to_string(),
            agent_definitions: agent_definitions.into_iter().map(str::to_string).collect(),
            hatched_agents: hatched_agents.into_iter().map(str::to_string).collect(),
            tools: Vec::new(),
            assets: Vec::new(),
        }
    }

    struct PackagesRootGuard {
        path: PathBuf,
    }

    impl PackagesRootGuard {
        fn new(stem: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "cargo-ai-local-package-store-{stem}-{}",
                uuid::Uuid::new_v4()
            ));
            super::TEST_PACKAGES_ROOT.with(|value| {
                value.replace(Some(path.join("packages")));
            });
            Self { path }
        }
    }

    impl Drop for PackagesRootGuard {
        fn drop(&mut self) {
            super::TEST_PACKAGES_ROOT.with(|value| {
                value.replace(None);
            });
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    fn temp_package_root(stem: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "cargo-ai-local-package-root-{stem}-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(root.join("agents")).expect("agents dir should be writable");
        std::fs::write(
            root.join("cargo-ai-package.toml"),
            r#"format_version = 1
project_name = "data_integration"
project_version = "1.0.0"
profile = "default"
agent_definitions = ["agents/lookup_account.json"]
hatched_agents = ["agents/daily_digest.json"]
tools = ["snowflake_query"]
assets = ["schemas/customer.sql"]
"#,
        )
        .expect("package manifest should be writable");
        std::fs::write(root.join("agents/lookup_account.json"), "{}")
            .expect("run definition should be writable");
        std::fs::write(root.join("agents/daily_digest.json"), "{}")
            .expect("hatch definition should be writable");
        root
    }

    fn remove_temp_dir_if_present(path: &Path) {
        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn duplicate_entrypoint_merges_run_and_hatch_capability() {
        let root = std::env::temp_dir().join(format!(
            "cargo-ai-local-package-entrypoints-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(root.join("agents")).expect("temp package dir should exist");
        std::fs::write(root.join("agents/demo.json"), "{}").expect("definition should be written");

        let entrypoints = build_entrypoints(
            &manifest(vec!["agents/demo.json"], vec!["agents/demo.json"]),
            root.as_path(),
        )
        .expect("entrypoints should build");

        assert_eq!(entrypoints.len(), 1);
        assert_eq!(entrypoints[0].name, "demo");
        assert!(entrypoints[0].runnable);
        assert!(entrypoints[0].hatchable);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn version_decision_requires_downgrade_for_older_version() {
        let existing = super::InstalledPackageDocument {
            format_version: 1,
            alias: "demo".to_string(),
            package_name: "demo".to_string(),
            package_version: "2.0.0".to_string(),
            profile: "default".to_string(),
            content_sha256: "aaa".to_string(),
            source: super::InstalledPackageSourceDocument {
                kind: "test".to_string(),
                path: None,
            },
            installed_at: "2026-06-22T00:00:00Z".to_string(),
            entrypoints: Vec::new(),
        };

        let error = determine_install_action(Some(&existing), "demo", "1.0.0", "bbb", false, false)
            .expect_err("downgrade should require flag");

        assert!(error.contains("--downgrade"));
    }

    #[test]
    fn local_root_install_resolves_entrypoints_and_uninstalls_alias() {
        let _store = PackagesRootGuard::new("install-resolve");
        let package_root = temp_package_root("install-resolve");

        let request = InstallRequest {
            source: Some(package_root.to_string_lossy().to_string()),
            alias: Some("data_integration".to_string()),
            profile: "default".to_string(),
            replace: false,
            downgrade: false,
        };
        let action = install_local_package(&request).expect("install should succeed");
        assert!(matches!(action, InstallAction::New));

        let installed = load_installed_package("data_integration").expect("metadata should load");
        assert_eq!(installed.alias, "data_integration");
        assert_eq!(installed.package_name, "data_integration");
        assert_eq!(installed.package_version, "1.0.0");
        assert_eq!(installed.entrypoints.len(), 2);

        let run_entrypoint =
            resolve_entrypoint_reference("data_integration::lookup_account", false)
                .expect("run entrypoint should resolve")
                .expect("run entrypoint should be installed");
        assert!(run_entrypoint
            .definition_path
            .ends_with("package/agents/lookup_account.json"));

        let hatch_entrypoint = resolve_entrypoint_reference("data_integration::daily_digest", true)
            .expect("hatch entrypoint should resolve")
            .expect("hatch entrypoint should be installed");
        assert!(hatch_entrypoint
            .definition_path
            .ends_with("package/agents/daily_digest.json"));

        uninstall_package("data_integration").expect("uninstall should succeed");
        assert!(load_installed_package("data_integration").is_err());

        remove_temp_dir_if_present(package_root.as_path());
    }
}
