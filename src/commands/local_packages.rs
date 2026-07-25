//! Local machine package install and lookup support.
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine as _;
use clap::ArgMatches;
use semver::Version;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{btree_map::Entry, BTreeMap};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;
use uuid::Uuid;

const PACKAGE_MANIFEST_FILE_NAME: &str = "cargo-ai-package.toml";
const INSTALL_MANIFEST_FILE_NAME: &str = "install.toml";
const INSTALLED_PACKAGE_DIR_NAME: &str = "package";
const INSTALLED_PACKAGE_DATA_DIR_NAME: &str = "data";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StagedInstallFailurePoint {
    AfterBackup,
    AfterDataTransfer,
    FinalAliasReplacement,
    RestoreData,
    RestoreAlias,
}

#[cfg(test)]
thread_local! {
    static TEST_PACKAGES_ROOT: std::cell::RefCell<Option<PathBuf>> = std::cell::RefCell::new(None);
    static TEST_STAGED_INSTALL_FAILURES: std::cell::RefCell<Vec<StagedInstallFailurePoint>> =
        const { std::cell::RefCell::new(Vec::new()) };
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
    #[serde(default)]
    permissions: PackagePermissionProfileDocument,
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
    #[serde(default)]
    permissions: PackagePermissionProfileDocument,
    entrypoints: Vec<InstalledPackageEntrypointDocument>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct InstalledPackageSourceDocument {
    kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    account_selector: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    requested_owner_handle: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    hosted_source_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    hosted_version_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    owner_account_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    owner_handle: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct InstalledPackageEntrypointDocument {
    name: String,
    path: String,
    runnable: bool,
    hatchable: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct PackagePermissionProfileDocument {
    pub(crate) package_payload: String,
    pub(crate) package_data: String,
    pub(crate) project_workspace: String,
    pub(crate) subprocess: String,
}

impl Default for PackagePermissionProfileDocument {
    fn default() -> Self {
        Self {
            package_payload: "read".to_string(),
            package_data: "read_write".to_string(),
            project_workspace: "explicit_grant_required".to_string(),
            subprocess: "blocked_without_explicit_grant".to_string(),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct InstalledPackageRuntimeContext {
    pub(crate) alias: String,
    pub(crate) source_kind: String,
    pub(crate) package_data_root: PathBuf,
    pub(crate) permissions: PackagePermissionProfileDocument,
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
    pub(crate) source_kind: String,
    pub(crate) package_data_root: PathBuf,
    pub(crate) permissions: PackagePermissionProfileDocument,
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

pub(crate) fn account_handle_from_install_matches(
    install_m: &ArgMatches,
) -> Option<Option<String>> {
    install_m.get_one::<String>("account").map(|value| {
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
                let provenance = match package.source.kind.as_str() {
                    "hosted" => package
                        .source
                        .owner_handle
                        .as_deref()
                        .map(|handle| format!("hosted {handle}/{}", package.package_name))
                        .unwrap_or_else(|| "hosted".to_string()),
                    _ => package.source.kind.clone(),
                };
                println!(
                    "- {}  {} {}  {}  {}",
                    package.alias,
                    package.package_name,
                    package.package_version,
                    entrypoint_summary(&package.entrypoints),
                    provenance
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

pub(crate) async fn run_hosted_install(install_m: &ArgMatches) -> bool {
    let Some(package_name) = install_m.get_one::<String>("source").map(String::as_str) else {
        eprintln!("x Hosted install requires a package name before --account.");
        return false;
    };
    let package_name = package_name.trim();
    if package_name.is_empty() {
        eprintln!("x Hosted install requires a non-empty package name.");
        return false;
    }
    let Some(alias) = install_m.get_one::<String>("alias").map(String::as_str) else {
        eprintln!(
            "x Hosted install requires --as <alias> so the local runtime reference is explicit."
        );
        return false;
    };
    let alias = alias.trim();
    if let Err(error) = validate_package_alias(alias) {
        eprintln!("x {error}");
        return false;
    }

    let owner_handle = account_handle_from_install_matches(install_m).flatten();
    let version = install_m
        .get_one::<String>("version")
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    if let Some(version) = version.as_deref() {
        if let Err(error) = Version::parse(version) {
            eprintln!("x Hosted package version '{version}' is not valid semver: {error}");
            return false;
        }
    }

    match install_hosted_package(
        package_name,
        owner_handle.as_deref(),
        version.as_deref(),
        alias,
        install_m.get_flag("replace"),
        install_m.get_flag("downgrade"),
    )
    .await
    {
        Ok(InstallAction::Noop) => true,
        Ok(_) => true,
        Err(error) => {
            eprintln!("x {error}");
            false
        }
    }
}

pub(crate) async fn run_hosted_update(update_m: &ArgMatches) -> bool {
    let Some(alias) = update_m.get_one::<String>("alias").map(String::as_str) else {
        eprintln!("x Missing package alias.");
        return false;
    };

    match update_hosted_package(alias).await {
        Ok(InstallAction::Noop) => true,
        Ok(_) => true,
        Err(error) => {
            eprintln!("x {error}");
            false
        }
    }
}

pub(crate) async fn run_hosted_rollback(rollback_m: &ArgMatches) -> bool {
    let Some(alias) = rollback_m.get_one::<String>("alias").map(String::as_str) else {
        eprintln!("x Missing package alias.");
        return false;
    };
    let Some(version) = rollback_m.get_one::<String>("to").map(String::as_str) else {
        eprintln!("x Missing rollback target. Use --to <version>.");
        return false;
    };
    let version = version.trim();
    if let Err(error) = Version::parse(version) {
        eprintln!("x Hosted package rollback version '{version}' is not valid semver: {error}");
        return false;
    }

    match rollback_hosted_package(alias, version).await {
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
            if package.source.kind == "hosted" {
                println!("Hosted:");
                println!(
                    "  Source ID:  {}",
                    package
                        .source
                        .hosted_source_id
                        .as_deref()
                        .unwrap_or("unknown")
                );
                println!(
                    "  Version ID: {}",
                    package
                        .source
                        .hosted_version_id
                        .as_deref()
                        .unwrap_or("unknown")
                );
                if let Some(owner_handle) = package.source.owner_handle.as_deref() {
                    println!("  Owner:      {}", owner_handle);
                }
                if let Some(owner_account_id) = package.source.owner_account_id.as_deref() {
                    println!("  Account ID: {}", owner_account_id);
                }
            }
            println!("Permissions:");
            for line in permission_profile_lines(&package.permissions) {
                println!("  {line}");
            }
            println!(
                "Data root: {}",
                installed_package_data_root(package.alias.as_str()).display()
            );
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
    let materialized = match materialize_prepared_package(
        &prepared,
        request.alias.as_deref(),
        request.replace,
        request.downgrade,
    ) {
        Ok(materialized) => materialized,
        Err(error) => {
            cleanup_prepared_package(&prepared);
            return Err(error);
        }
    };
    cleanup_prepared_package(&prepared);

    if matches!(materialized.action, InstallAction::Noop) {
        println!(
            "✓ Package `{}` is already installed at version {}.",
            materialized.alias, materialized.package_version
        );
        return Ok(materialized.action);
    }

    match materialized.action {
        InstallAction::New => println!(
            "✓ Package `{}` installed as `{}` at version {}.",
            materialized.package_name, materialized.alias, materialized.package_version
        ),
        InstallAction::Upgrade => println!(
            "✓ Package `{}` upgraded as `{}` to version {}.",
            materialized.package_name, materialized.alias, materialized.package_version
        ),
        InstallAction::Replace => println!(
            "✓ Package alias `{}` replaced with `{}` version {}.",
            materialized.alias, materialized.package_name, materialized.package_version
        ),
        InstallAction::Downgrade => println!(
            "✓ Package `{}` downgraded as `{}` to version {}.",
            materialized.package_name, materialized.alias, materialized.package_version
        ),
        InstallAction::Noop => {}
    }
    Ok(materialized.action)
}

#[derive(Clone, Debug)]
struct MaterializedPackageInstall {
    action: InstallAction,
    alias: String,
    package_name: String,
    package_version: String,
    permissions: PackagePermissionProfileDocument,
}

fn materialize_prepared_package(
    prepared: &PreparedPackage,
    alias_override: Option<&str>,
    replace: bool,
    downgrade: bool,
) -> Result<MaterializedPackageInstall, String> {
    let package_name = required_package_name(&prepared.manifest)?;
    let package_version = required_package_version(&prepared.manifest)?;
    validate_permission_profile(&prepared.manifest.permissions)?;
    let alias = alias_override
        .unwrap_or(package_name.as_str())
        .trim()
        .to_string();
    validate_package_alias(alias.as_str())?;
    let entrypoints = build_entrypoints(&prepared.manifest, &prepared.package_root)?;
    let existing = load_installed_package(alias.as_str()).ok();
    ensure_source_identity_replacement_is_explicit(
        existing.as_ref(),
        &prepared.source,
        package_name.as_str(),
        replace,
    )?;
    if prepared.source.kind == "hosted"
        || existing
            .as_ref()
            .map(|package| package.source.kind == "hosted")
            .unwrap_or(false)
    {
        ensure_permission_expansion_is_explicit(
            existing.as_ref(),
            &prepared.manifest.permissions,
            package_name.as_str(),
            package_version.as_str(),
        )?;
    }
    let action = determine_install_action(
        existing.as_ref(),
        package_name.as_str(),
        package_version.as_str(),
        prepared.content_sha256.as_str(),
        replace,
        downgrade,
    )?;

    if !matches!(action, InstallAction::Noop) {
        let document = InstalledPackageDocument {
            format_version: 1,
            alias: alias.clone(),
            package_name: package_name.clone(),
            package_version: package_version.clone(),
            profile: prepared.manifest.profile.clone(),
            content_sha256: prepared.content_sha256.clone(),
            source: prepared.source.clone(),
            installed_at: now_rfc3339()?,
            permissions: prepared.manifest.permissions.clone(),
            entrypoints,
        };

        write_staged_install(alias.as_str(), &prepared.package_root, &document)?;
    }

    Ok(MaterializedPackageInstall {
        action,
        alias,
        package_name,
        package_version,
        permissions: prepared.manifest.permissions.clone(),
    })
}

async fn install_hosted_package(
    package_name: &str,
    owner_handle: Option<&str>,
    version: Option<&str>,
    alias: &str,
    replace: bool,
    downgrade: bool,
) -> Result<InstallAction, String> {
    let response = pull_hosted_package(package_name, owner_handle, version).await?;
    let prepared = prepare_hosted_response(&response, owner_handle)?;
    let materialized =
        match materialize_prepared_package(&prepared, Some(alias), replace, downgrade) {
            Ok(materialized) => materialized,
            Err(error) => {
                cleanup_prepared_package(&prepared);
                return Err(error);
            }
        };
    cleanup_prepared_package(&prepared);

    if matches!(materialized.action, InstallAction::Noop) {
        println!(
            "✓ Hosted package `{}` is already installed as `{}` at version {}.",
            materialized.package_name, materialized.alias, materialized.package_version
        );
        return Ok(materialized.action);
    }

    print_permission_summary(&materialized.permissions);
    println!(
        "✓ Hosted package `{}` installed as `{}` at version {}.",
        materialized.package_name, materialized.alias, materialized.package_version
    );
    Ok(materialized.action)
}

async fn update_hosted_package(alias: &str) -> Result<InstallAction, String> {
    validate_package_alias(alias)?;
    let existing = load_installed_package(alias)?;
    ensure_installed_source_is_hosted(&existing)?;
    let installed_version = Version::parse(existing.package_version.as_str()).map_err(|error| {
        format!(
            "Installed package alias `{}` has invalid version '{}': {}",
            existing.alias, existing.package_version, error
        )
    })?;

    let response = pull_hosted_package(
        existing.package_name.as_str(),
        hosted_owner_handle_for_refresh(&existing).as_deref(),
        None,
    )
    .await?;
    let prepared = prepare_hosted_response(
        &response,
        hosted_owner_handle_for_refresh(&existing).as_deref(),
    )?;
    ensure_hosted_source_matches_existing(&existing, &prepared.source)?;
    let resolved_version = required_package_version(&prepared.manifest)?;
    let resolved = Version::parse(resolved_version.as_str()).map_err(|error| {
        format!(
            "Hosted package version '{}' is not valid semver: {}",
            resolved_version, error
        )
    })?;

    if resolved <= installed_version {
        cleanup_prepared_package(&prepared);
        println!(
            "✓ Hosted package `{}` is already up to date at version {}.",
            existing.alias, existing.package_version
        );
        return Ok(InstallAction::Noop);
    }

    let materialized = match materialize_prepared_package(&prepared, Some(alias), false, false) {
        Ok(materialized) => materialized,
        Err(error) => {
            cleanup_prepared_package(&prepared);
            return Err(error);
        }
    };
    cleanup_prepared_package(&prepared);
    print_permission_summary(&materialized.permissions);
    println!(
        "✓ Hosted package `{}` updated to version {}.",
        materialized.alias, materialized.package_version
    );
    Ok(materialized.action)
}

async fn rollback_hosted_package(
    alias: &str,
    target_version: &str,
) -> Result<InstallAction, String> {
    validate_package_alias(alias)?;
    let existing = load_installed_package(alias)?;
    ensure_installed_source_is_hosted(&existing)?;
    let installed_version = Version::parse(existing.package_version.as_str()).map_err(|error| {
        format!(
            "Installed package alias `{}` has invalid version '{}': {}",
            existing.alias, existing.package_version, error
        )
    })?;
    let requested_version = Version::parse(target_version).map_err(|error| {
        format!("Hosted package rollback version '{target_version}' is not valid semver: {error}")
    })?;
    if requested_version == installed_version {
        println!(
            "✓ Hosted package `{}` is already installed at version {}.",
            alias, target_version
        );
        return Ok(InstallAction::Noop);
    }
    if requested_version > installed_version {
        return Err(format!(
            "Rollback target {} is newer than installed version {}. Use `cargo ai packages update {}` to move forward.",
            requested_version, installed_version, alias
        ));
    }

    let response = pull_hosted_package(
        existing.package_name.as_str(),
        hosted_owner_handle_for_refresh(&existing).as_deref(),
        Some(target_version),
    )
    .await?;
    let prepared = prepare_hosted_response(
        &response,
        hosted_owner_handle_for_refresh(&existing).as_deref(),
    )?;
    ensure_hosted_source_matches_existing(&existing, &prepared.source)?;
    let materialized = match materialize_prepared_package(&prepared, Some(alias), false, true) {
        Ok(materialized) => materialized,
        Err(error) => {
            cleanup_prepared_package(&prepared);
            return Err(error);
        }
    };
    cleanup_prepared_package(&prepared);
    print_permission_summary(&materialized.permissions);
    println!(
        "✓ Hosted package `{}` rolled back to version {}.",
        materialized.alias, materialized.package_version
    );
    Ok(materialized.action)
}

async fn pull_hosted_package(
    package_name: &str,
    owner_handle: Option<&str>,
    version: Option<&str>,
) -> Result<Value, String> {
    use crate::commands::account::helpers::{
        load_account_auth, persist_refreshed_access_token, refresh_access_token_for_retry,
        RefreshAccessError, INFRA_BASE_URL,
    };

    let auth = load_account_auth()
        .map_err(|message| crate::ui::account_status::normalize_leading_glyph(message.as_str()))?;
    let access_token_owned = auth.access_token;
    let refresh_token = auth.refresh_token;

    let mut response = crate::infra_api::account::projects::pull_project(
        INFRA_BASE_URL,
        access_token_owned.as_str(),
        package_name,
        owner_handle,
        version,
    )
    .await
    .map_err(|error| format!("Request failed: {error:?}"))?;

    if response
        .get("type")
        .and_then(Value::as_str)
        .map(|kind| kind == "access_token_expired")
        .unwrap_or(false)
    {
        match refresh_access_token_for_retry(access_token_owned.as_str(), refresh_token.as_deref())
            .await
        {
            Err(RefreshAccessError::MissingRefreshToken) => {
                return Err(
                    "Access token expired, and no refresh token exists in credential store. Run `cargo ai account status` or re-confirm account."
                        .to_string(),
                );
            }
            Err(RefreshAccessError::RequestFailed(error)) => {
                return Err(format!("Request failed while refreshing session: {error}"));
            }
            Err(RefreshAccessError::MissingRefreshedToken(refresh_response)) => {
                return Err(backend_response_message(
                    &refresh_response,
                    "Session refresh did not return a new access token.",
                ));
            }
            Ok((retry_access_token, refreshed_expires_in)) => {
                if let Some(rt) = refresh_token.as_deref() {
                    persist_refreshed_access_token(
                        retry_access_token.as_str(),
                        rt,
                        refreshed_expires_in,
                    );
                }
                response = crate::infra_api::account::projects::pull_project(
                    INFRA_BASE_URL,
                    retry_access_token.as_str(),
                    package_name,
                    owner_handle,
                    version,
                )
                .await
                .map_err(|error| format!("Request failed after session refresh: {error:?}"))?;
            }
        }
    }

    if !is_hosted_pull_success(&response) {
        return Err(backend_response_message(
            &response,
            "Hosted package pull did not succeed.",
        ));
    }

    Ok(response)
}

fn prepare_hosted_response(
    response: &Value,
    requested_owner_handle: Option<&str>,
) -> Result<PreparedPackage, String> {
    let archive_base64 = response
        .get("package_archive_base64")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            "Hosted pull response did not include `package_archive_base64`.".to_string()
        })?;
    let package_sha256 = response
        .get("package_sha256")
        .and_then(Value::as_str)
        .ok_or_else(|| "Hosted pull response did not include `package_sha256`.".to_string())?;
    let package_size_bytes = response
        .get("package_size_bytes")
        .and_then(Value::as_i64)
        .ok_or_else(|| "Hosted pull response did not include `package_size_bytes`.".to_string())?;
    let hosted_source_id = required_response_string(response, "hosted_source_id")?;
    let hosted_version_id = required_response_string(response, "hosted_version_id")?;

    let archive_bytes = BASE64_STANDARD
        .decode(archive_base64.as_bytes())
        .map_err(|error| format!("Failed to decode hosted package archive: {error}"))?;
    let decoded_size_bytes = i64::try_from(archive_bytes.len()).map_err(|_| {
        "Decoded hosted package archive exceeded supported size limits.".to_string()
    })?;
    if decoded_size_bytes != package_size_bytes {
        return Err(format!(
            "Hosted package archive size mismatch. Expected {} bytes, got {} bytes after decoding.",
            package_size_bytes, decoded_size_bytes
        ));
    }

    let decoded_sha256 = crate::commands::account::sha256_hex(archive_bytes.as_slice());
    if decoded_sha256 != package_sha256 {
        return Err(format!(
            "Hosted package archive checksum mismatch. Expected {}, got {}.",
            package_sha256, decoded_sha256
        ));
    }

    let staging_root = packages_staging_root().join(format!("hosted-{}", Uuid::new_v4()));
    fs::create_dir_all(&staging_root).map_err(|error| {
        format!(
            "Failed to create hosted package staging directory '{}': {}",
            staging_root.display(),
            error
        )
    })?;
    if let Err(error) = crate::commands::account::extract_package_archive_bytes(
        archive_bytes.as_slice(),
        &staging_root,
    ) {
        let _ = fs::remove_dir_all(&staging_root);
        return Err(error);
    }

    let mut prepared = prepare_package_root(
        staging_root.clone(),
        InstalledPackageSourceDocument {
            kind: "hosted".to_string(),
            path: None,
            account_selector: Some(if requested_owner_handle.is_some() {
                "handle".to_string()
            } else {
                "self".to_string()
            }),
            requested_owner_handle: requested_owner_handle.map(str::to_string),
            hosted_source_id: Some(hosted_source_id),
            hosted_version_id: Some(hosted_version_id),
            owner_account_id: optional_response_string(response, "owner_account_id"),
            owner_handle: optional_response_string(response, "owner_handle"),
        },
        Some(staging_root),
    )?;
    prepared.content_sha256 = decoded_sha256;
    validate_hosted_response_matches_manifest(response, &prepared.manifest)?;
    Ok(prepared)
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
        package_root: installed_root.join(INSTALLED_PACKAGE_DIR_NAME),
        package_name: package.package_name,
        package_version: package.package_version,
        content_sha256: package.content_sha256,
        source_kind: package.source.kind,
        package_data_root: installed_root.join(INSTALLED_PACKAGE_DATA_DIR_NAME),
        permissions: package.permissions,
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
            account_selector: None,
            requested_owner_handle: None,
            hosted_source_id: None,
            hosted_version_id: None,
            owner_account_id: None,
            owner_handle: None,
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
                account_selector: None,
                requested_owner_handle: None,
                hosted_source_id: None,
                hosted_version_id: None,
                owner_account_id: None,
                owner_handle: None,
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
                account_selector: None,
                requested_owner_handle: None,
                hosted_source_id: None,
                hosted_version_id: None,
                owner_account_id: None,
                owner_handle: None,
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
            account_selector: None,
            requested_owner_handle: None,
            hosted_source_id: None,
            hosted_version_id: None,
            owner_account_id: None,
            owner_handle: None,
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

fn ensure_source_identity_replacement_is_explicit(
    existing: Option<&InstalledPackageDocument>,
    new_source: &InstalledPackageSourceDocument,
    package_name: &str,
    replace: bool,
) -> Result<(), String> {
    let Some(existing) = existing else {
        return Ok(());
    };
    if existing.source.kind != "hosted" && new_source.kind != "hosted" {
        return Ok(());
    }
    let existing_identity = existing
        .source
        .hosted_source_id
        .as_deref()
        .or(Some(existing.source.kind.as_str()));
    let new_identity = new_source
        .hosted_source_id
        .as_deref()
        .or(Some(new_source.kind.as_str()));
    if existing_identity == new_identity {
        return Ok(());
    }
    if replace {
        return Ok(());
    }
    Err(format!(
        "Package alias `{}` is already installed for a different source identity. Re-run with --replace to replace it with `{}`.",
        existing.alias, package_name
    ))
}

fn ensure_hosted_source_matches_existing(
    existing: &InstalledPackageDocument,
    new_source: &InstalledPackageSourceDocument,
) -> Result<(), String> {
    let existing_source_id = existing.source.hosted_source_id.as_deref().ok_or_else(|| {
        format!(
            "Installed package alias `{}` is missing hosted_source_id metadata.",
            existing.alias
        )
    })?;
    let new_source_id = new_source.hosted_source_id.as_deref().ok_or_else(|| {
        "Hosted pull response did not include hosted_source_id metadata.".to_string()
    })?;
    if existing_source_id != new_source_id {
        return Err(format!(
            "Hosted package resolution returned source id `{}` but installed alias `{}` is pinned to `{}`.",
            new_source_id, existing.alias, existing_source_id
        ));
    }
    Ok(())
}

fn ensure_installed_source_is_hosted(package: &InstalledPackageDocument) -> Result<(), String> {
    if package.source.kind != "hosted" {
        return Err(format!(
            "Package alias `{}` was installed from `{}`. update/rollback only apply to hosted package aliases.",
            package.alias, package.source.kind
        ));
    }
    if package.source.hosted_source_id.is_none() {
        return Err(format!(
            "Package alias `{}` is hosted but missing hosted_source_id metadata.",
            package.alias
        ));
    }
    Ok(())
}

fn hosted_owner_handle_for_refresh(package: &InstalledPackageDocument) -> Option<String> {
    match package.source.account_selector.as_deref() {
        Some("handle") => package
            .source
            .requested_owner_handle
            .clone()
            .or_else(|| package.source.owner_handle.clone()),
        _ => None,
    }
}

fn ensure_permission_expansion_is_explicit(
    existing: Option<&InstalledPackageDocument>,
    new_permissions: &PackagePermissionProfileDocument,
    package_name: &str,
    package_version: &str,
) -> Result<(), String> {
    let Some(existing) = existing else {
        return Ok(());
    };
    if !permission_profile_expands(&existing.permissions, new_permissions) {
        return Ok(());
    }
    Err(format!(
        "Package `{}` version {} requests broader runtime permissions than alias `{}` currently has. Hosted permission expansion is blocked in noninteractive install/update/rollback; review the package and reinstall with an explicit replacement flow when broader grants are supported.",
        package_name, package_version, existing.alias
    ))
}

fn permission_profile_expands(
    current: &PackagePermissionProfileDocument,
    candidate: &PackagePermissionProfileDocument,
) -> bool {
    permission_level(candidate.package_payload.as_str())
        > permission_level(current.package_payload.as_str())
        || permission_level(candidate.package_data.as_str())
            > permission_level(current.package_data.as_str())
        || permission_level(candidate.project_workspace.as_str())
            > permission_level(current.project_workspace.as_str())
        || permission_level(candidate.subprocess.as_str())
            > permission_level(current.subprocess.as_str())
}

fn permission_level(value: &str) -> u8 {
    match value {
        "none" => 0,
        "blocked" | "blocked_without_explicit_grant" => 1,
        "explicit_grant_required" => 2,
        "read" => 3,
        "read_write" => 4,
        "allowed" => 5,
        _ => 6,
    }
}

fn validate_permission_profile(
    permissions: &PackagePermissionProfileDocument,
) -> Result<(), String> {
    validate_permission_value(
        permissions.package_payload.as_str(),
        "permissions.package_payload",
        &["read"],
    )?;
    validate_permission_value(
        permissions.package_data.as_str(),
        "permissions.package_data",
        &["read_write"],
    )?;
    validate_permission_value(
        permissions.project_workspace.as_str(),
        "permissions.project_workspace",
        &["none", "explicit_grant_required", "read", "read_write"],
    )?;
    validate_permission_value(
        permissions.subprocess.as_str(),
        "permissions.subprocess",
        &["blocked_without_explicit_grant", "allowed"],
    )
}

fn validate_permission_value(value: &str, label: &str, allowed: &[&str]) -> Result<(), String> {
    if allowed.iter().any(|candidate| *candidate == value) {
        return Ok(());
    }
    Err(format!(
        "Unsupported package permission `{label} = {}`. Expected one of: {}.",
        value,
        allowed.join(", ")
    ))
}

pub(crate) fn hosted_package_allows_subprocess(context: &InstalledPackageRuntimeContext) -> bool {
    context.source_kind != "hosted" || context.permissions.subprocess == "allowed"
}

pub(crate) fn runtime_context_for_package_root(
    package_root: &Path,
) -> Option<InstalledPackageRuntimeContext> {
    if package_root.file_name().and_then(|name| name.to_str()) != Some(INSTALLED_PACKAGE_DIR_NAME) {
        return None;
    }
    let install_root = package_root.parent()?;
    let alias = install_root.file_name()?.to_string_lossy().to_string();
    let package = load_installed_package(alias.as_str()).ok()?;
    Some(InstalledPackageRuntimeContext {
        alias: package.alias,
        source_kind: package.source.kind,
        package_data_root: install_root.join(INSTALLED_PACKAGE_DATA_DIR_NAME),
        permissions: package.permissions,
    })
}

pub(crate) fn resolve_package_data_path(
    context: &InstalledPackageRuntimeContext,
    relative_path: &Path,
) -> Result<PathBuf, String> {
    if relative_path.as_os_str().is_empty() || relative_path.is_absolute() {
        return Err(format!(
            "Package `{}` data path must be a non-empty relative path.",
            context.alias
        ));
    }
    if relative_path
        .components()
        .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(format!(
            "Package `{}` data path must not use parent traversal (`..`).",
            context.alias
        ));
    }

    let data_root_exists = match fs::symlink_metadata(&context.package_data_root) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(format!(
                "Package `{}` data root '{}' must be a real directory and not a symbolic link.",
                context.alias,
                context.package_data_root.display()
            ));
        }
        Ok(_) => true,
        Err(error) if error.kind() == ErrorKind::NotFound => false,
        Err(error) => {
            return Err(format!(
                "Failed to inspect package `{}` data root '{}': {}",
                context.alias,
                context.package_data_root.display(),
                error
            ));
        }
    };

    let mut resolved = context.package_data_root.clone();
    let mut inspect_existing_components = data_root_exists;
    for component in relative_path.components() {
        match component {
            std::path::Component::CurDir => continue,
            std::path::Component::Normal(segment) => resolved.push(segment),
            std::path::Component::ParentDir => unreachable!("parent traversal rejected above"),
            std::path::Component::RootDir | std::path::Component::Prefix(_) => {
                return Err(format!(
                    "Package `{}` data path must be a non-empty relative path.",
                    context.alias
                ));
            }
        }
        if !inspect_existing_components {
            continue;
        }
        match fs::symlink_metadata(&resolved) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(format!(
                    "Package `{}` data path '{}' must not traverse symbolic link '{}'.",
                    context.alias,
                    relative_path.display(),
                    resolved.display()
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {
                inspect_existing_components = false;
            }
            Err(error) => {
                return Err(format!(
                    "Failed to inspect package `{}` data path component '{}': {}",
                    context.alias,
                    resolved.display(),
                    error
                ));
            }
        }
    }
    Ok(resolved)
}

fn permission_profile_lines(permissions: &PackagePermissionProfileDocument) -> Vec<String> {
    vec![
        format!("package payload: {}", permissions.package_payload),
        format!("package data:    {}", permissions.package_data),
        format!("project writes:  {}", permissions.project_workspace),
        format!("subprocess:      {}", permissions.subprocess),
    ]
}

fn print_permission_summary(permissions: &PackagePermissionProfileDocument) {
    println!("Permissions:");
    for line in permission_profile_lines(permissions) {
        println!("  {line}");
    }
}

fn is_hosted_pull_success(response: &Value) -> bool {
    response
        .get("type")
        .and_then(Value::as_str)
        .map(|kind| kind == "account_projects_pull_succeeded")
        .unwrap_or(false)
}

fn backend_response_message(response: &Value, fallback: &str) -> String {
    response
        .get("message")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            response
                .get("error")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .or_else(|| {
            response
                .get("type")
                .and_then(Value::as_str)
                .map(|kind| format!("{fallback} Backend response type: {kind}."))
        })
        .unwrap_or_else(|| fallback.to_string())
}

fn required_response_string(response: &Value, field: &str) -> Result<String, String> {
    response
        .get(field)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .ok_or_else(|| format!("Hosted pull response did not include `{field}`."))
}

fn optional_response_string(response: &Value, field: &str) -> Option<String> {
    response
        .get(field)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn validate_hosted_response_matches_manifest(
    response: &Value,
    manifest: &PackageManifestDocument,
) -> Result<(), String> {
    let response_project = required_response_string(response, "project")?;
    let response_version = required_response_string(response, "project_version")?;
    let manifest_project = required_package_name(manifest)?;
    let manifest_version = required_package_version(manifest)?;
    if response_project != manifest_project {
        return Err(format!(
            "Hosted package response project `{}` did not match package manifest project `{}`.",
            response_project, manifest_project
        ));
    }
    if response_version != manifest_version {
        return Err(format!(
            "Hosted package response version `{}` did not match package manifest version `{}`.",
            response_version, manifest_version
        ));
    }
    Ok(())
}

fn write_staged_install(
    alias: &str,
    package_root: &Path,
    document: &InstalledPackageDocument,
) -> Result<(), String> {
    let packages_root = packages_root();
    let transaction_id = Uuid::new_v4();
    let staging_root =
        packages_staging_root().join(format!("{alias}-{transaction_id}-replacement"));
    let backup_root = packages_staging_root().join(format!("{alias}-{transaction_id}-backup"));
    let staged_package_root = staging_root.join(INSTALLED_PACKAGE_DIR_NAME);
    let prepare_result = (|| {
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
        )
    })();
    if let Err(error) = prepare_result {
        let _ = fs::remove_dir_all(&staging_root);
        return Err(error);
    }

    let staged_data_root = staging_root.join(INSTALLED_PACKAGE_DATA_DIR_NAME);
    let install_root = packages_root.join(alias);
    let install_metadata = match fs::symlink_metadata(&install_root) {
        Ok(metadata) => Some(metadata),
        Err(error) if error.kind() == ErrorKind::NotFound => None,
        Err(error) => {
            let _ = fs::remove_dir_all(&staging_root);
            return Err(format!(
                "Failed to inspect installed package alias '{}': {}",
                install_root.display(),
                error
            ));
        }
    };

    let Some(install_metadata) = install_metadata else {
        fs::create_dir_all(&staged_data_root).map_err(|error| {
            let _ = fs::remove_dir_all(&staging_root);
            format!(
                "Failed to create staged package data directory '{}': {}",
                staged_data_root.display(),
                error
            )
        })?;
        if let Err(error) =
            maybe_fail_staged_install(StagedInstallFailurePoint::FinalAliasReplacement)
        {
            let _ = fs::remove_dir_all(&staging_root);
            return Err(error);
        }
        return fs::rename(&staging_root, &install_root).map_err(|error| {
            let _ = fs::remove_dir_all(&staging_root);
            format!(
                "Failed to move staged package install from '{}' to '{}': {}",
                staging_root.display(),
                install_root.display(),
                error
            )
        });
    };

    if install_metadata.file_type().is_symlink() || !install_metadata.is_dir() {
        let _ = fs::remove_dir_all(&staging_root);
        return Err(format!(
            "Installed package alias '{}' at '{}' must be a real directory and not a symbolic link.",
            alias,
            install_root.display()
        ));
    }

    let current_data_root = install_root.join(INSTALLED_PACKAGE_DATA_DIR_NAME);
    let preserve_existing_data = match fs::symlink_metadata(&current_data_root) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            let _ = fs::remove_dir_all(&staging_root);
            return Err(format!(
                "Installed package alias '{}' has an unsafe data root at '{}'; expected a real directory and not a symbolic link.",
                alias,
                current_data_root.display()
            ));
        }
        Ok(_) => true,
        Err(error) if error.kind() == ErrorKind::NotFound => false,
        Err(error) => {
            let _ = fs::remove_dir_all(&staging_root);
            return Err(format!(
                "Failed to inspect package data directory '{}' while replacing alias '{}': {}",
                current_data_root.display(),
                alias,
                error
            ));
        }
    };

    fs::rename(&install_root, &backup_root).map_err(|error| {
        let _ = fs::remove_dir_all(&staging_root);
        format!(
            "Failed to create recoverable backup '{}' for package alias '{}': {}",
            backup_root.display(),
            alias,
            error
        )
    })?;

    if let Err(error) = maybe_fail_staged_install(StagedInstallFailurePoint::AfterBackup) {
        return Err(staged_install_failure_with_recovery(
            alias,
            error,
            &install_root,
            &backup_root,
            &staging_root,
            false,
        ));
    }

    let preserved_data_moved = if preserve_existing_data {
        let backup_data_root = backup_root.join(INSTALLED_PACKAGE_DATA_DIR_NAME);
        if let Err(error) = fs::rename(&backup_data_root, &staged_data_root) {
            return Err(staged_install_failure_with_recovery(
                alias,
                format!(
                    "Failed to preserve package data directory '{}' while replacing alias '{}': {}",
                    backup_data_root.display(),
                    alias,
                    error
                ),
                &install_root,
                &backup_root,
                &staging_root,
                false,
            ));
        }
        true
    } else {
        if let Err(error) = fs::create_dir_all(&staged_data_root) {
            return Err(staged_install_failure_with_recovery(
                alias,
                format!(
                    "Failed to create staged package data directory '{}': {}",
                    staged_data_root.display(),
                    error
                ),
                &install_root,
                &backup_root,
                &staging_root,
                false,
            ));
        }
        false
    };

    if let Err(error) = maybe_fail_staged_install(StagedInstallFailurePoint::AfterDataTransfer) {
        return Err(staged_install_failure_with_recovery(
            alias,
            error,
            &install_root,
            &backup_root,
            &staging_root,
            preserved_data_moved,
        ));
    }

    let replacement_result = maybe_fail_staged_install(
        StagedInstallFailurePoint::FinalAliasReplacement,
    )
    .and_then(|_| {
        fs::rename(&staging_root, &install_root).map_err(|error| {
            format!(
                "Failed to move staged package install from '{}' to '{}': {}",
                staging_root.display(),
                install_root.display(),
                error
            )
        })
    });
    if let Err(error) = replacement_result {
        return Err(staged_install_failure_with_recovery(
            alias,
            error,
            &install_root,
            &backup_root,
            &staging_root,
            preserved_data_moved,
        ));
    }

    if let Err(error) = fs::remove_dir_all(&backup_root) {
        eprintln!(
            "Warning: package alias `{alias}` was replaced, but the prior package backup '{}' could not be removed: {error}",
            backup_root.display()
        );
    }
    Ok(())
}

fn staged_install_failure_with_recovery(
    alias: &str,
    failure: String,
    install_root: &Path,
    backup_root: &Path,
    staging_root: &Path,
    preserved_data_moved: bool,
) -> String {
    match restore_previous_install(
        install_root,
        backup_root,
        staging_root,
        preserved_data_moved,
    ) {
        Ok(()) => format!("{failure} Previous package alias `{alias}` was restored."),
        Err(recovery_error) => format!(
            "{failure} Automatic recovery for package alias `{alias}` failed: {recovery_error} Backup retained at '{}'; staged replacement retained at '{}'.",
            backup_root.display(),
            staging_root.display()
        ),
    }
}

fn restore_previous_install(
    install_root: &Path,
    backup_root: &Path,
    staging_root: &Path,
    preserved_data_moved: bool,
) -> Result<(), String> {
    if preserved_data_moved {
        maybe_fail_staged_install(StagedInstallFailurePoint::RestoreData)?;
        let staged_data_root = staging_root.join(INSTALLED_PACKAGE_DATA_DIR_NAME);
        let backup_data_root = backup_root.join(INSTALLED_PACKAGE_DATA_DIR_NAME);
        fs::rename(&staged_data_root, &backup_data_root).map_err(|error| {
            format!(
                "failed to restore package data from '{}' to '{}': {}",
                staged_data_root.display(),
                backup_data_root.display(),
                error
            )
        })?;
    }

    maybe_fail_staged_install(StagedInstallFailurePoint::RestoreAlias)?;
    fs::rename(backup_root, install_root).map_err(|error| {
        format!(
            "failed to restore package alias from '{}' to '{}': {}",
            backup_root.display(),
            install_root.display(),
            error
        )
    })?;
    let _ = fs::remove_dir_all(staging_root);
    Ok(())
}

fn maybe_fail_staged_install(point: StagedInstallFailurePoint) -> Result<(), String> {
    #[cfg(test)]
    {
        let should_fail = TEST_STAGED_INSTALL_FAILURES.with(|failures| {
            let mut failures = failures.borrow_mut();
            if failures.first().copied() == Some(point) {
                failures.remove(0);
                true
            } else {
                false
            }
        });
        if should_fail {
            return Err(format!(
                "Injected staged install failure at `{point:?}` for testing."
            ));
        }
    }
    #[cfg(not(test))]
    let _ = point;
    Ok(())
}

#[cfg(test)]
fn set_test_staged_install_failures(points: &[StagedInstallFailurePoint]) {
    TEST_STAGED_INSTALL_FAILURES.with(|failures| {
        failures.replace(points.to_vec());
    });
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

fn installed_package_data_root(alias: &str) -> PathBuf {
    installed_package_root(alias).join(INSTALLED_PACKAGE_DATA_DIR_NAME)
}

#[cfg(test)]
mod tests {
    use super::{
        build_entrypoints, determine_install_action, install_local_package, load_installed_package,
        resolve_entrypoint_reference, resolve_package_data_path, uninstall_package, InstallAction,
        InstallRequest, InstalledPackageRuntimeContext, PackageManifestDocument,
        PackagePermissionProfileDocument, StagedInstallFailurePoint,
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
            permissions: PackagePermissionProfileDocument::default(),
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
            super::TEST_STAGED_INSTALL_FAILURES.with(|failures| {
                failures.replace(Vec::new());
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

    fn local_install_request(package_root: &Path) -> InstallRequest {
        InstallRequest {
            source: Some(package_root.to_string_lossy().to_string()),
            alias: Some("data_integration".to_string()),
            profile: "default".to_string(),
            replace: false,
            downgrade: false,
        }
    }

    fn prepare_package_upgrade(stem: &str) -> (PackagesRootGuard, PathBuf, InstallRequest) {
        let store = PackagesRootGuard::new(stem);
        let package_root = temp_package_root(stem);
        let request = local_install_request(&package_root);
        let action = install_local_package(&request).expect("first install should succeed");
        assert!(matches!(action, InstallAction::New));

        let data_root = super::installed_package_data_root("data_integration");
        std::fs::write(data_root.join("state.json"), r#"{"kept":true}"#)
            .expect("data file should be writable");

        let manifest_path = package_root.join("cargo-ai-package.toml");
        let manifest = std::fs::read_to_string(&manifest_path).expect("manifest should read");
        std::fs::write(
            &manifest_path,
            manifest.replace(
                r#"project_version = "1.0.0""#,
                r#"project_version = "1.1.0""#,
            ),
        )
        .expect("manifest should update");
        std::fs::write(
            package_root.join("agents/lookup_account.json"),
            r#"{"version":2}"#,
        )
        .expect("updated definition should be writable");

        (store, package_root, request)
    }

    fn assert_failed_upgrade_restores_previous_install(
        point: StagedInstallFailurePoint,
        stem: &str,
    ) {
        let (_store, package_root, request) = prepare_package_upgrade(stem);
        super::set_test_staged_install_failures(&[point]);

        let error = install_local_package(&request).expect_err("upgrade should fail");

        assert!(error.contains("Previous package alias `data_integration` was restored"));
        let installed =
            load_installed_package("data_integration").expect("previous metadata should load");
        assert_eq!(installed.package_version, "1.0.0");
        assert_eq!(
            std::fs::read_to_string(
                super::installed_package_data_root("data_integration").join("state.json")
            )
            .expect("previous data should be restored"),
            r#"{"kept":true}"#
        );
        assert_eq!(
            std::fs::read_to_string(
                super::installed_package_root("data_integration")
                    .join("package/agents/lookup_account.json")
            )
            .expect("previous package payload should be restored"),
            "{}"
        );
        assert_eq!(
            std::fs::read_dir(super::packages_staging_root())
                .expect("staging root should remain readable")
                .count(),
            0,
            "recoverable failure should clean transaction artifacts"
        );

        remove_temp_dir_if_present(package_root.as_path());
    }

    fn runtime_context(package_data_root: PathBuf) -> InstalledPackageRuntimeContext {
        InstalledPackageRuntimeContext {
            alias: "data_integration".to_string(),
            source_kind: "hosted".to_string(),
            package_data_root,
            permissions: PackagePermissionProfileDocument::default(),
        }
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
                account_selector: None,
                requested_owner_handle: None,
                hosted_source_id: None,
                hosted_version_id: None,
                owner_account_id: None,
                owner_handle: None,
            },
            installed_at: "2026-06-22T00:00:00Z".to_string(),
            permissions: PackagePermissionProfileDocument::default(),
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

    #[test]
    fn package_upgrade_preserves_alias_data_directory() {
        let _store = PackagesRootGuard::new("preserve-data");
        let package_root = temp_package_root("preserve-data");
        let request = local_install_request(&package_root);
        let action = install_local_package(&request).expect("first install should succeed");
        assert!(matches!(action, InstallAction::New));

        let data_root = super::installed_package_data_root("data_integration");
        std::fs::create_dir_all(&data_root).expect("data root should exist");
        std::fs::write(data_root.join("state.json"), r#"{"kept":true}"#)
            .expect("data file should be writable");

        let manifest_path = package_root.join("cargo-ai-package.toml");
        let manifest = std::fs::read_to_string(&manifest_path).expect("manifest should read");
        std::fs::write(
            &manifest_path,
            manifest.replace(
                r#"project_version = "1.0.0""#,
                r#"project_version = "1.1.0""#,
            ),
        )
        .expect("manifest should update");

        let action = install_local_package(&request).expect("upgrade should succeed");
        assert!(matches!(action, InstallAction::Upgrade));
        assert_eq!(
            std::fs::read_to_string(data_root.join("state.json"))
                .expect("data file should be preserved"),
            r#"{"kept":true}"#
        );

        remove_temp_dir_if_present(package_root.as_path());
    }

    #[test]
    fn package_upgrade_restores_alias_after_failure_following_backup() {
        assert_failed_upgrade_restores_previous_install(
            StagedInstallFailurePoint::AfterBackup,
            "failure-after-backup",
        );
    }

    #[test]
    fn package_upgrade_restores_alias_after_failure_following_data_transfer() {
        assert_failed_upgrade_restores_previous_install(
            StagedInstallFailurePoint::AfterDataTransfer,
            "failure-after-data-transfer",
        );
    }

    #[test]
    fn package_upgrade_restores_alias_after_final_replacement_failure() {
        assert_failed_upgrade_restores_previous_install(
            StagedInstallFailurePoint::FinalAliasReplacement,
            "failure-final-replacement",
        );
    }

    #[test]
    fn package_upgrade_retains_recovery_paths_when_alias_restore_fails() {
        let (_store, package_root, request) = prepare_package_upgrade("failure-restore-alias");
        super::set_test_staged_install_failures(&[
            StagedInstallFailurePoint::FinalAliasReplacement,
            StagedInstallFailurePoint::RestoreAlias,
        ]);

        let error = install_local_package(&request).expect_err("upgrade should fail");
        assert!(error.contains("Automatic recovery for package alias `data_integration` failed"));

        let mut transaction_paths = std::fs::read_dir(super::packages_staging_root())
            .expect("staging root should remain readable")
            .map(|entry| entry.expect("staging entry should be readable").path())
            .collect::<Vec<_>>();
        transaction_paths.sort();
        assert_eq!(transaction_paths.len(), 2);
        let backup_root = transaction_paths
            .iter()
            .find(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .map(|name| name.ends_with("-backup"))
                    .unwrap_or(false)
            })
            .expect("backup should be retained");
        let replacement_root = transaction_paths
            .iter()
            .find(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .map(|name| name.ends_with("-replacement"))
                    .unwrap_or(false)
            })
            .expect("replacement should be retained");
        assert!(error.contains(backup_root.to_string_lossy().as_ref()));
        assert!(error.contains(replacement_root.to_string_lossy().as_ref()));
        assert_eq!(
            std::fs::read_to_string(backup_root.join("data/state.json"))
                .expect("backup should retain package data"),
            r#"{"kept":true}"#
        );
        assert!(backup_root.join("install.toml").is_file());
        assert!(replacement_root.join("install.toml").is_file());
        assert!(!super::installed_package_root("data_integration").exists());

        remove_temp_dir_if_present(package_root.as_path());
    }

    #[cfg(unix)]
    #[test]
    fn package_upgrade_rejects_symlinked_installed_data_root() {
        use std::os::unix::fs::symlink;

        let (store, package_root, request) = prepare_package_upgrade("upgrade-data-root-symlink");
        let data_root = super::installed_package_data_root("data_integration");
        let external_data_root = store.path.join("external-data");
        std::fs::remove_dir_all(&data_root).expect("installed data root should be removable");
        std::fs::create_dir_all(&external_data_root)
            .expect("external data root should be writable");
        std::fs::write(external_data_root.join("state.json"), "outside")
            .expect("external data should be writable");
        symlink(&external_data_root, &data_root).expect("data root symlink should be created");

        let error = install_local_package(&request).expect_err("upgrade should reject symlink");

        assert!(error.contains("has an unsafe data root"));
        assert_eq!(
            load_installed_package("data_integration")
                .expect("previous install should remain")
                .package_version,
            "1.0.0"
        );
        assert_eq!(
            std::fs::read_to_string(external_data_root.join("state.json"))
                .expect("external data should remain untouched"),
            "outside"
        );

        remove_temp_dir_if_present(package_root.as_path());
    }

    #[test]
    fn package_data_path_allows_missing_trailing_components() {
        let store = PackagesRootGuard::new("data-path-missing");
        let data_root = store.path.join("data");
        std::fs::create_dir_all(&data_root).expect("data root should be writable");
        let context = runtime_context(data_root.clone());

        let resolved = resolve_package_data_path(&context, Path::new("usage/2026/07/events.jsonl"))
            .expect("missing trailing components should be allowed");

        assert_eq!(resolved, data_root.join("usage/2026/07/events.jsonl"));
    }

    #[test]
    fn package_data_path_allows_missing_data_root() {
        let store = PackagesRootGuard::new("data-root-missing");
        let data_root = store.path.join("data");
        let context = runtime_context(data_root.clone());

        let resolved = resolve_package_data_path(&context, Path::new("usage/events.jsonl"))
            .expect("missing data root should be creatable by the caller");

        assert_eq!(resolved, data_root.join("usage/events.jsonl"));
    }

    #[cfg(unix)]
    #[test]
    fn package_data_path_rejects_symlinked_data_root() {
        use std::os::unix::fs::symlink;

        let store = PackagesRootGuard::new("data-path-root-symlink");
        let target_root = store.path.join("target");
        let data_root = store.path.join("data");
        std::fs::create_dir_all(&target_root).expect("target should be writable");
        symlink(&target_root, &data_root).expect("data root symlink should be created");
        let context = runtime_context(data_root);

        let error = resolve_package_data_path(&context, Path::new("events.jsonl"))
            .expect_err("symlinked data root should be rejected");

        assert!(error.contains("must be a real directory and not a symbolic link"));
    }

    #[cfg(unix)]
    #[test]
    fn package_data_path_rejects_nested_symlink() {
        use std::os::unix::fs::symlink;

        let store = PackagesRootGuard::new("data-path-nested-symlink");
        let data_root = store.path.join("data");
        let target_root = store.path.join("target");
        std::fs::create_dir_all(&data_root).expect("data root should be writable");
        std::fs::create_dir_all(&target_root).expect("target should be writable");
        symlink(&target_root, data_root.join("external"))
            .expect("nested symlink should be created");
        let context = runtime_context(data_root);

        let error = resolve_package_data_path(&context, Path::new("external/events.jsonl"))
            .expect_err("nested symlink should be rejected");

        assert!(error.contains("must not traverse symbolic link"));
    }

    #[cfg(unix)]
    #[test]
    fn package_data_path_rejects_symlink_target_leaf() {
        use std::os::unix::fs::symlink;

        let store = PackagesRootGuard::new("data-path-leaf-symlink");
        let data_root = store.path.join("data");
        let target_file = store.path.join("outside.jsonl");
        std::fs::create_dir_all(&data_root).expect("data root should be writable");
        std::fs::write(&target_file, "outside").expect("target should be writable");
        symlink(&target_file, data_root.join("events.jsonl"))
            .expect("leaf symlink should be created");
        let context = runtime_context(data_root);

        let error = resolve_package_data_path(&context, Path::new("events.jsonl"))
            .expect_err("symlink target leaf should be rejected");

        assert!(error.contains("must not traverse symbolic link"));
    }
}
